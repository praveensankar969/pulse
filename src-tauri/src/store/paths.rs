use std::path::{Path, PathBuf};

use super::StoreError;

/// Thin wrapper around Tauri `app_config_dir()` so tests can inject a temp dir.
/// Files sit in that directory (no extra `config` leaf).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn from_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<Self, StoreError> {
        use tauri::Manager;
        let root = app
            .path()
            .app_config_dir()
            .map_err(|error| StoreError::Path(error.to_string()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn services_file(&self) -> PathBuf {
        self.root.join("services.json")
    }

    pub fn history_file(&self) -> PathBuf {
        self.root.join("history.sqlite3")
    }

    pub fn ensure_dir(&self) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.root)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Paths;
    use std::path::PathBuf;

    #[test]
    fn files_sit_directly_in_app_config_dir() {
        let paths = Paths::new("/tmp/dev.pulsebar.app");
        assert_eq!(
            paths.config_file(),
            PathBuf::from("/tmp/dev.pulsebar.app/config.json")
        );
        assert_eq!(
            paths.services_file(),
            PathBuf::from("/tmp/dev.pulsebar.app/services.json")
        );
        assert_eq!(
            paths.history_file(),
            PathBuf::from("/tmp/dev.pulsebar.app/history.sqlite3")
        );
    }
}
