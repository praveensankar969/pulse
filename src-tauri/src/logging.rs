use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

use crate::domain::{is_redacted_header, SECRET_MASK};

pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_LOG_FILES: u32 = 3;

/// `PULSE_LOG=debug` (or a full env-filter). Default `info`.
pub fn pulse_filter() -> EnvFilter {
    let raw = std::env::var("PULSE_LOG").unwrap_or_else(|_| "info".into());
    EnvFilter::try_new(raw).unwrap_or_else(|_| EnvFilter::new("info"))
}

pub fn pulse_log_level() -> log::LevelFilter {
    match std::env::var("PULSE_LOG")
        .unwrap_or_else(|_| "info".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "trace" => log::LevelFilter::Trace,
        "debug" => log::LevelFilter::Debug,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        "off" => log::LevelFilter::Off,
        _ => log::LevelFilter::Info,
    }
}

pub fn pulse_tracing_level() -> LevelFilter {
    match pulse_log_level() {
        log::LevelFilter::Off => LevelFilter::OFF,
        log::LevelFilter::Error => LevelFilter::ERROR,
        log::LevelFilter::Warn => LevelFilter::WARN,
        log::LevelFilter::Info => LevelFilter::INFO,
        log::LevelFilter::Debug => LevelFilter::DEBUG,
        log::LevelFilter::Trace => LevelFilter::TRACE,
    }
}

/// Denylist redact: `Bearer `, JWT `eyJ`, and known header names.
pub fn redact(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let lower = input.to_ascii_lowercase();
    let mut i = 0;
    while i < input.len() {
        if let Some((skip, keep)) = secret_prefix(&input[i..], &lower[i..]) {
            out.push_str(&input[i..i + keep]);
            i += skip;
            i += scan_token(&input[i..]);
            out.push_str("***");
            continue;
        }
        if let Some((_name_len, value_at)) = header_assignment(&lower[i..]) {
            out.push_str(&input[i..i + value_at]);
            i += value_at;
            i += scan_header_value(&input[i..]);
            out.push_str(SECRET_MASK);
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn secret_prefix(original: &str, lower: &str) -> Option<(usize, usize)> {
    if lower.starts_with("bearer ") {
        return Some(("bearer ".len(), "Bearer ".len().min(original.len())));
    }
    if original.starts_with("eyJ") || lower.starts_with("eyj") {
        return Some((3, 3));
    }
    None
}

fn header_assignment(lower: &str) -> Option<(usize, usize)> {
    for name in [
        "authorization",
        "proxy-authorization",
        "cookie",
        "set-cookie",
        "x-api-key",
        "x-auth-token",
    ] {
        if let Some(rest) = lower.strip_prefix(name) {
            let ws = rest.chars().take_while(|c| *c == ' ' || *c == '\t').count();
            let after = &rest[ws..];
            if after.starts_with(':') || after.starts_with('=') {
                let mut value_at = name.len() + ws + 1;
                let tail = &lower[value_at..];
                value_at += tail.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                return Some((name.len(), value_at));
            }
        }
    }
    None
}

fn scan_token(input: &str) -> usize {
    input
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != ',')
        .map(|c| c.len_utf8())
        .sum()
}

fn scan_header_value(input: &str) -> usize {
    let first = scan_token(input);
    let rest = &input[first..];
    let pad = rest.len() - rest.trim_start().len();
    let next = rest.trim_start();
    if input
        .get(..first)
        .is_some_and(|token| token.eq_ignore_ascii_case("bearer"))
        && !next.is_empty()
    {
        first + pad + scan_token(next)
    } else {
        first
    }
}

/// Debug wrapper so a `HeaderMap` never prints secret / denylist values.
pub struct RedactingHeaderMap<'a>(pub &'a reqwest::header::HeaderMap);

impl std::fmt::Debug for RedactingHeaderMap<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut map = f.debug_map();
        for (name, value) in self.0 {
            if is_redacted_header(name.as_str()) || value.is_sensitive() {
                map.entry(&name.as_str(), &SECRET_MASK);
            } else {
                match value.to_str() {
                    Ok(text) => map.entry(&name.as_str(), &redact(text)),
                    Err(_) => map.entry(&name.as_str(), &SECRET_MASK),
                };
            }
        }
        map.finish()
    }
}

struct RotatingInner {
    path: PathBuf,
    file: File,
    written: u64,
}

impl RotatingInner {
    fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path,
            file,
            written,
        })
    }

    fn rotate_if_needed(&mut self, incoming: u64) -> io::Result<()> {
        if self.written.saturating_add(incoming) <= MAX_LOG_BYTES {
            return Ok(());
        }
        drop(std::mem::replace(
            &mut self.file,
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        ));
        for i in (1..MAX_LOG_FILES).rev() {
            let from = rotated_path(&self.path, i - 1);
            let to = rotated_path(&self.path, i);
            if from.exists() {
                let _ = fs::remove_file(&to);
                let _ = fs::rename(&from, &to);
            }
        }
        let _ = fs::remove_file(&self.path);
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.rotate_if_needed(buf.len() as u64)?;
        self.file.write_all(buf)?;
        self.written = self.written.saturating_add(buf.len() as u64);
        Ok(())
    }
}

fn rotated_path(path: &Path, index: u32) -> PathBuf {
    if index == 0 {
        path.to_path_buf()
    } else {
        path.with_file_name(format!(
            "{}.{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            index
        ))
    }
}

#[derive(Clone)]
struct RotatingFile {
    inner: Arc<Mutex<RotatingInner>>,
}

impl RotatingFile {
    fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(RotatingInner::open(path.into())?)),
        })
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let redacted = redact(&String::from_utf8_lossy(buf));
        self.inner
            .lock()
            .expect("log file lock")
            .write_all(redacted.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.lock().expect("log file lock").file.flush()
    }
}

impl<'a> MakeWriter<'a> for RotatingFile {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

pub fn init(log_file: &Path) -> io::Result<()> {
    if let Some(parent) = log_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = RotatingFile::open(log_file)?;
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(pulse_filter())
        .with_ansi(false)
        .with_writer(file)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    install_panic_hook(log_file.to_path_buf());
    Ok(())
}

fn install_panic_hook(log_file: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = redact(&info.to_string());
        let _ = append_redacted(&log_file, &format!("PANIC {msg}\n"));
        previous(info);
    }));
}

fn append_redacted(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(redact(line).as_bytes())
}

pub fn tauri_log_plugin() -> tauri_plugin_log::Builder {
    tauri_plugin_log::Builder::new().level(pulse_log_level())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

    #[test]
    fn redacts_bearer_jwt_and_header_names() {
        let raw = r#"Authorization: Bearer super-secret eyJabc.def cookie: sid=1"#;
        let cleaned = redact(raw);
        assert!(!cleaned.contains("super-secret"));
        assert!(!cleaned.contains("eyJabc.def"));
        assert!(!cleaned.contains("sid=1"));
        assert!(cleaned.contains(SECRET_MASK));
        assert!(cleaned.contains("***"));
    }

    #[test]
    fn redacting_header_map_hides_sensitive() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer tok"));
        headers.insert("x-api-key", HeaderValue::from_static("abc"));
        headers.insert("accept", HeaderValue::from_static("application/json"));
        let debug = format!("{:?}", RedactingHeaderMap(&headers));
        assert!(!debug.contains("Bearer tok"));
        assert!(!debug.contains("abc"));
        assert!(debug.contains("application/json"));
        assert!(debug.contains(SECRET_MASK));
    }

    #[test]
    fn pulse_filter_accepts_level() {
        let filter = EnvFilter::try_new("debug").unwrap();
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::DEBUG));
        let _ = pulse_tracing_level();
        let _ = pulse_log_level();
    }

    #[test]
    fn rotating_file_rolls_at_2mb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pulse.log");
        let mut file = RotatingFile::open(&path).unwrap();
        let chunk = vec![b'x'; 1024];
        for _ in 0..(MAX_LOG_BYTES as usize / chunk.len() + 4) {
            file.write_all(&chunk).unwrap();
        }
        assert!(path.exists());
        assert!(rotated_path(&path, 1).exists());
    }
}
