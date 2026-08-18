use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::migrate::SCHEMA_VERSION;
use super::{Paths, StoreError};
use crate::domain::{
    CheckResult, CompactSample, ErrorKind, MachineStatus, OutcomeClass, RuntimeState, ServiceStatus,
};

const SAMPLE_RETENTION_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_SAMPLES_PER_SERVICE: i64 = 2000;

/// SQLite history: runtime_state, last_results, check_samples.
/// Library API only — the scheduler (next PR) calls these methods.
#[derive(Debug)]
pub struct History {
    conn: Connection,
}

impl History {
    pub fn open_in(paths: &Paths) -> Result<Self, StoreError> {
        paths.ensure_dir()?;
        Self::open(paths.history_file())
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        configure(&conn)?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Missing row reconstructs as Pending. A persisted pending row is returned as-is.
    pub fn load_runtime(&self, service_id: &str) -> Result<RuntimeState, StoreError> {
        Ok(self
            .get_runtime(service_id)?
            .unwrap_or_else(pending_runtime))
    }

    pub fn get_runtime(&self, service_id: &str) -> Result<Option<RuntimeState>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT consecutive_hard_fails, status, down_since_ms, degraded_since_ms,
                    down_clock_adjust_ms, last_check_at_ms, snooze_until_ms,
                    paused_at_ms, slept_at_ms
             FROM runtime_state WHERE service_id = ?1",
        )?;
        stmt.query_row(params![service_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })
        .optional()?
        .map(
            |(
                fails,
                status,
                down_since,
                degraded_since,
                adjust,
                last_check,
                snooze,
                paused,
                slept,
            )| {
                Ok(RuntimeState {
                    consecutive_hard_fails: u32_from_sql(fails, "consecutive_hard_fails")?,
                    status: parse_machine_status(&status)?,
                    down_since: opt_ms_to_dt(down_since)?,
                    degraded_since: opt_ms_to_dt(degraded_since)?,
                    down_clock_adjust_ms: u64_from_sql(adjust, "down_clock_adjust_ms")?,
                    last_check_at: opt_ms_to_dt(last_check)?,
                    snooze_until: opt_ms_to_dt(snooze)?,
                    paused_at: opt_ms_to_dt(paused)?,
                    slept_at: opt_ms_to_dt(slept)?,
                })
            },
        )
        .transpose()
    }

    pub fn put_runtime(&self, service_id: &str, state: &RuntimeState) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO runtime_state (
                service_id, consecutive_hard_fails, status, down_since_ms,
                degraded_since_ms, down_clock_adjust_ms, last_check_at_ms,
                snooze_until_ms, paused_at_ms, slept_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(service_id) DO UPDATE SET
                consecutive_hard_fails = excluded.consecutive_hard_fails,
                status = excluded.status,
                down_since_ms = excluded.down_since_ms,
                degraded_since_ms = excluded.degraded_since_ms,
                down_clock_adjust_ms = excluded.down_clock_adjust_ms,
                last_check_at_ms = excluded.last_check_at_ms,
                snooze_until_ms = excluded.snooze_until_ms,
                paused_at_ms = excluded.paused_at_ms,
                slept_at_ms = excluded.slept_at_ms",
            params![
                service_id,
                i64::from(state.consecutive_hard_fails),
                machine_status_sql(state.status),
                state.down_since.map(dt_to_ms),
                state.degraded_since.map(dt_to_ms),
                i64_from_u64(state.down_clock_adjust_ms, "down_clock_adjust_ms")?,
                state.last_check_at.map(dt_to_ms),
                state.snooze_until.map(dt_to_ms),
                state.paused_at.map(dt_to_ms),
                state.slept_at.map(dt_to_ms),
            ],
        )?;
        Ok(())
    }

    /// On pause while down: record paused_at. Do not fold the clock yet.
    pub fn apply_pause(&self, service_id: &str, now: DateTime<Utc>) -> Result<(), StoreError> {
        let Some(mut state) = self.get_runtime(service_id)? else {
            return Ok(());
        };
        if state.status == MachineStatus::Down && state.paused_at.is_none() {
            state.paused_at = Some(now);
            self.put_runtime(service_id, &state)?;
        }
        Ok(())
    }

    /// On unpause: if still down, add (now - paused_at) to down_clock_adjust_ms.
    pub fn apply_unpause(&self, service_id: &str, now: DateTime<Utc>) -> Result<(), StoreError> {
        let Some(mut state) = self.get_runtime(service_id)? else {
            return Ok(());
        };
        let paused_at = state.paused_at;
        fold_clock_gap(&mut state, paused_at, now);
        state.paused_at = None;
        self.put_runtime(service_id, &state)?;
        Ok(())
    }

    /// On OS sleep while down: record slept_at. Do not fold the clock yet.
    /// No-op when already paused — the pause interval covers laptop sleep.
    /// A leftover `slept_at` (missed wake) is folded before the new stamp.
    pub fn apply_sleep(&self, service_id: &str, now: DateTime<Utc>) -> Result<(), StoreError> {
        let Some(mut state) = self.get_runtime(service_id)? else {
            return Ok(());
        };
        if state.paused_at.is_some() {
            return Ok(());
        }
        if state.status != MachineStatus::Down {
            return Ok(());
        }
        if let Some(old) = state.slept_at {
            fold_clock_gap(&mut state, Some(old), now);
        }
        state.slept_at = Some(now);
        self.put_runtime(service_id, &state)?;
        Ok(())
    }

    /// SQLite only. `None` clears snooze. Does not create a runtime row for unknown ids.
    pub fn set_snooze(
        &self,
        service_id: &str,
        until: Option<DateTime<Utc>>,
    ) -> Result<(), StoreError> {
        let mut state = self.load_runtime(service_id)?;
        state.snooze_until = until;
        self.put_runtime(service_id, &state)
    }

    /// On wake: if still down, add (now - slept_at) to down_clock_adjust_ms.
    /// While paused, only clear slept_at so the overlapping window is not added twice.
    pub fn apply_wake(&self, service_id: &str, now: DateTime<Utc>) -> Result<(), StoreError> {
        let Some(mut state) = self.get_runtime(service_id)? else {
            return Ok(());
        };
        if state.paused_at.is_some() {
            if state.slept_at.is_some() {
                state.slept_at = None;
                self.put_runtime(service_id, &state)?;
            }
            return Ok(());
        }
        let slept_at = state.slept_at;
        fold_clock_gap(&mut state, slept_at, now);
        state.slept_at = None;
        self.put_runtime(service_id, &state)?;
        Ok(())
    }

    pub fn last_result(&self, service_id: &str) -> Result<Option<CheckResult>, StoreError> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT payload_json FROM last_results WHERE service_id = ?1",
                params![service_id],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(payload) => Ok(Some(serde_json::from_str(&payload)?)),
            None => Ok(None),
        }
    }

    /// Full CheckResult JSON (body preview + assertion diffs). Never request headers.
    /// Canceled and offline-frozen probes are not written.
    pub fn put_last_result(
        &self,
        service_id: &str,
        result: &CheckResult,
    ) -> Result<(), StoreError> {
        if skip_probe(result.evidence.error_kind) {
            return Ok(());
        }
        let payload = serde_json::to_string(result)?;
        self.conn.execute(
            "INSERT INTO last_results (service_id, payload_json) VALUES (?1, ?2)
             ON CONFLICT(service_id) DO UPDATE SET payload_json = excluded.payload_json",
            params![service_id, payload],
        )?;
        Ok(())
    }

    /// Compact sample. Canceled and offline-frozen probes are not inserted.
    pub fn insert_sample(
        &self,
        service_id: &str,
        sample: &CompactSample,
    ) -> Result<(), StoreError> {
        if skip_probe(sample.error_kind) {
            return Ok(());
        }
        let error_kind = match sample.error_kind {
            Some(kind) => Some(serde_json::to_value(kind)?),
            None => None,
        };
        let error_kind = match error_kind {
            Some(serde_json::Value::String(s)) => Some(s),
            Some(_) => {
                return Err(StoreError::Corrupt(
                    "error_kind did not serialize as a string".into(),
                ))
            }
            None => None,
        };
        self.conn.execute(
            "INSERT INTO check_samples (
                service_id, at_ms, state, outcome, http_status, latency_ms, error_kind
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(service_id, at_ms) DO UPDATE SET
                state = excluded.state,
                outcome = excluded.outcome,
                http_status = excluded.http_status,
                latency_ms = excluded.latency_ms,
                error_kind = excluded.error_kind",
            params![
                service_id,
                dt_to_ms(sample.at),
                service_status_sql(sample.state),
                outcome_sql(sample.outcome),
                sample.http_status.map(i64::from),
                sample.latency_ms.map(i64::from),
                error_kind,
            ],
        )?;
        Ok(())
    }

    pub fn samples(
        &self,
        service_id: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<CompactSample>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT at_ms, state, outcome, http_status, latency_ms, error_kind
             FROM check_samples
             WHERE service_id = ?1 AND at_ms >= ?2
             ORDER BY at_ms ASC",
        )?;
        let rows = stmt.query_map(params![service_id, dt_to_ms(since)], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut samples = Vec::new();
        for row in rows {
            let (at_ms, state, outcome, http_status, latency_ms, error_kind) = row?;
            samples.push(CompactSample {
                at: ms_to_dt(at_ms)?,
                state: parse_service_status(&state)?,
                outcome: parse_outcome(&outcome)?,
                http_status: opt_u16(http_status, "http_status")?,
                latency_ms: opt_u32(latency_ms, "latency_ms")?,
                error_kind: match error_kind {
                    Some(kind) => Some(serde_json::from_value(serde_json::Value::String(kind))?),
                    None => None,
                },
            });
        }
        Ok(samples)
    }

    pub fn samples_24h(
        &self,
        service_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<CompactSample>, StoreError> {
        let since = now - chrono::Duration::milliseconds(SAMPLE_RETENTION_MS);
        self.samples(service_id, since)
    }

    /// Time prune (24h) then cap 2 000 rows/service. Caller invokes every ~10 minutes.
    pub fn prune(&self) -> Result<(), StoreError> {
        self.prune_at(Utc::now())
    }

    pub fn prune_at(&self, now: DateTime<Utc>) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let cutoff = dt_to_ms(now) - SAMPLE_RETENTION_MS;
        tx.execute(
            "DELETE FROM check_samples WHERE at_ms < ?1",
            params![cutoff],
        )?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT DISTINCT service_id FROM check_samples")?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for id in ids {
            tx.execute(
                "DELETE FROM check_samples WHERE rowid IN (
                    SELECT rowid FROM check_samples
                    WHERE service_id = ?1
                    ORDER BY at_ms DESC
                    LIMIT -1 OFFSET ?2
                 )",
                params![id, MAX_SAMPLES_PER_SERVICE],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Wipe runtime_state, last_results, and check_samples for one service.
    pub fn delete_service(&self, service_id: &str) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM runtime_state WHERE service_id = ?1",
            params![service_id],
        )?;
        tx.execute(
            "DELETE FROM last_results WHERE service_id = ?1",
            params![service_id],
        )?;
        tx.execute(
            "DELETE FROM check_samples WHERE service_id = ?1",
            params![service_id],
        )?;
        tx.commit()?;
        Ok(())
    }
}

fn configure(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (
            version INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS runtime_state (
            service_id TEXT PRIMARY KEY,
            consecutive_hard_fails INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL,
            down_since_ms INTEGER,
            degraded_since_ms INTEGER,
            down_clock_adjust_ms INTEGER NOT NULL DEFAULT 0,
            last_check_at_ms INTEGER,
            snooze_until_ms INTEGER,
            paused_at_ms INTEGER,
            slept_at_ms INTEGER
         );
         CREATE TABLE IF NOT EXISTS last_results (
            service_id TEXT PRIMARY KEY,
            payload_json TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS check_samples (
            service_id TEXT NOT NULL,
            at_ms INTEGER NOT NULL,
            state TEXT NOT NULL,
            outcome TEXT NOT NULL,
            http_status INTEGER,
            latency_ms INTEGER,
            error_kind TEXT,
            PRIMARY KEY (service_id, at_ms)
         );
         CREATE INDEX IF NOT EXISTS idx_samples_at ON check_samples(at_ms);",
    )?;
    ensure_column(conn, "runtime_state", "degraded_since_ms", "INTEGER")?;

    let version: Option<u32> = conn
        .query_row("SELECT version FROM schema_meta LIMIT 1", [], |row| {
            row.get(0)
        })
        .optional()?;
    match version {
        Some(found) if found > SCHEMA_VERSION => Err(StoreError::SchemaTooNew { found }),
        Some(found) if found == SCHEMA_VERSION => Ok(()),
        Some(_) => {
            conn.execute(
                "UPDATE schema_meta SET version = ?1",
                params![SCHEMA_VERSION],
            )?;
            Ok(())
        }
        None => {
            conn.execute(
                "INSERT INTO schema_meta (version) VALUES (?1)",
                params![SCHEMA_VERSION],
            )?;
            Ok(())
        }
    }
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<(), StoreError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|name| name.as_deref() == Ok(column));
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
            [],
        )?;
    }
    Ok(())
}

fn pending_runtime() -> RuntimeState {
    RuntimeState {
        consecutive_hard_fails: 0,
        status: MachineStatus::Pending,
        down_since: None,
        degraded_since: None,
        down_clock_adjust_ms: 0,
        last_check_at: None,
        snooze_until: None,
        paused_at: None,
        slept_at: None,
    }
}

fn skip_probe(error_kind: Option<ErrorKind>) -> bool {
    matches!(error_kind, Some(ErrorKind::Canceled | ErrorKind::Offline))
}

fn fold_clock_gap(state: &mut RuntimeState, started: Option<DateTime<Utc>>, now: DateTime<Utc>) {
    if state.status != MachineStatus::Down {
        return;
    }
    if let Some(started) = started {
        let elapsed = (now - started).num_milliseconds().max(0) as u64;
        state.down_clock_adjust_ms = state.down_clock_adjust_ms.saturating_add(elapsed);
    }
}

fn dt_to_ms(dt: DateTime<Utc>) -> i64 {
    dt.timestamp_millis()
}

fn ms_to_dt(ms: i64) -> Result<DateTime<Utc>, StoreError> {
    DateTime::from_timestamp_millis(ms)
        .ok_or_else(|| StoreError::Corrupt(format!("invalid timestamp {ms}")))
}

fn opt_ms_to_dt(ms: Option<i64>) -> Result<Option<DateTime<Utc>>, StoreError> {
    ms.map(ms_to_dt).transpose()
}

fn machine_status_sql(status: MachineStatus) -> &'static str {
    match status {
        MachineStatus::Pending => "pending",
        MachineStatus::Healthy => "healthy",
        MachineStatus::Degraded => "degraded",
        MachineStatus::Down => "down",
    }
}

fn parse_machine_status(value: &str) -> Result<MachineStatus, StoreError> {
    match value {
        "pending" => Ok(MachineStatus::Pending),
        "healthy" => Ok(MachineStatus::Healthy),
        "degraded" => Ok(MachineStatus::Degraded),
        "down" => Ok(MachineStatus::Down),
        other => Err(StoreError::Corrupt(format!("unknown status {other}"))),
    }
}

fn service_status_sql(status: ServiceStatus) -> &'static str {
    match status {
        ServiceStatus::Healthy => "healthy",
        ServiceStatus::Degraded => "degraded",
        ServiceStatus::Down => "down",
    }
}

fn parse_service_status(value: &str) -> Result<ServiceStatus, StoreError> {
    match value {
        "healthy" => Ok(ServiceStatus::Healthy),
        "degraded" => Ok(ServiceStatus::Degraded),
        "down" => Ok(ServiceStatus::Down),
        other => Err(StoreError::Corrupt(format!("unknown sample state {other}"))),
    }
}

fn outcome_sql(outcome: OutcomeClass) -> &'static str {
    match outcome {
        OutcomeClass::Ok => "ok",
        OutcomeClass::Soft => "soft",
        OutcomeClass::Hard => "hard",
    }
}

fn parse_outcome(value: &str) -> Result<OutcomeClass, StoreError> {
    match value {
        "ok" => Ok(OutcomeClass::Ok),
        "soft" => Ok(OutcomeClass::Soft),
        "hard" => Ok(OutcomeClass::Hard),
        other => Err(StoreError::Corrupt(format!("unknown outcome {other}"))),
    }
}

fn u32_from_sql(value: i64, field: &str) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| StoreError::Corrupt(format!("{field} out of range: {value}")))
}

fn u64_from_sql(value: i64, field: &str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt(format!("{field} out of range: {value}")))
}

fn i64_from_u64(value: u64, field: &str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt(format!("{field} out of range: {value}")))
}

fn opt_u16(value: Option<i64>, field: &str) -> Result<Option<u16>, StoreError> {
    value
        .map(|v| {
            u16::try_from(v).map_err(|_| StoreError::Corrupt(format!("{field} out of range: {v}")))
        })
        .transpose()
}

fn opt_u32(value: Option<i64>, field: &str) -> Result<Option<u32>, StoreError> {
    value.map(|v| u32_from_sql(v, field)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AssertOp, AssertionResult, CheckEvidence};
    use crate::store::StoreError;

    fn open_temp() -> (tempfile::TempDir, History) {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(dir.path().join("history.sqlite3")).unwrap();
        (dir, history)
    }

    fn at_ms(ms: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(ms).unwrap()
    }

    fn sample(
        at: DateTime<Utc>,
        state: ServiceStatus,
        outcome: OutcomeClass,
        error_kind: Option<ErrorKind>,
    ) -> CompactSample {
        CompactSample {
            at,
            state,
            outcome,
            http_status: Some(200),
            latency_ms: Some(12),
            error_kind,
        }
    }

    fn result_with(
        at: DateTime<Utc>,
        state: ServiceStatus,
        outcome: OutcomeClass,
        error_kind: Option<ErrorKind>,
    ) -> CheckResult {
        CheckResult {
            evidence: CheckEvidence {
                at,
                outcome,
                http_status: Some(200),
                latency_ms: Some(42),
                redirects: Some(0),
                headers_stripped_on_redirect: None,
                assertion_results: vec![AssertionResult {
                    path: "status".into(),
                    op: AssertOp::Equals,
                    ok: false,
                    expected: Some(serde_json::json!("ok")),
                    actual: Some(serde_json::json!("unhealthy")),
                    reason: Some("mismatch".into()),
                }],
                assertion_skipped: None,
                error_kind,
                error: error_kind.map(|_| "fail".into()),
                body_preview: Some(r#"{"status":"unhealthy"}"#.into()),
            },
            state,
        }
    }

    fn down_state(now: DateTime<Utc>) -> RuntimeState {
        RuntimeState {
            consecutive_hard_fails: 3,
            status: MachineStatus::Down,
            down_since: Some(now),
            degraded_since: None,
            down_clock_adjust_ms: 0,
            last_check_at: Some(now),
            snooze_until: None,
            paused_at: None,
            slept_at: None,
        }
    }

    #[test]
    fn opens_with_wal_and_foreign_keys() {
        let (_dir, history) = open_temp();
        let mode: String = history
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let fk: i64 = history
            .conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        let version: u32 = history
            .conn
            .query_row("SELECT version FROM schema_meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn missing_runtime_is_pending() {
        let (_dir, history) = open_temp();
        let state = history.load_runtime("svc-1").unwrap();
        assert_eq!(state, pending_runtime());
        assert!(history.get_runtime("svc-1").unwrap().is_none());
    }

    #[test]
    fn persisted_pending_is_not_reconstructed() {
        let (_dir, history) = open_temp();
        let pending = pending_runtime();
        history.put_runtime("svc-1", &pending).unwrap();
        let loaded = history.get_runtime("svc-1").unwrap().unwrap();
        assert_eq!(loaded.status, MachineStatus::Pending);
        assert!(loaded.last_check_at.is_none());
    }

    #[test]
    fn runtime_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        let now = at_ms(1_700_000_000_000);
        let state = RuntimeState {
            consecutive_hard_fails: 3,
            status: MachineStatus::Down,
            down_since: Some(now),
            degraded_since: None,
            down_clock_adjust_ms: 1_500,
            last_check_at: Some(now),
            snooze_until: Some(at_ms(1_700_000_900_000)),
            paused_at: Some(now),
            slept_at: None,
        };
        {
            let history = History::open(&path).unwrap();
            history.put_runtime("svc-1", &state).unwrap();
        }
        let history = History::open(&path).unwrap();
        assert_eq!(history.load_runtime("svc-1").unwrap(), state);
    }

    #[test]
    fn adds_degraded_since_column_to_existing_runtime_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_meta (version INTEGER NOT NULL);
                 INSERT INTO schema_meta (version) VALUES (1);
                 CREATE TABLE runtime_state (
                    service_id TEXT PRIMARY KEY,
                    consecutive_hard_fails INTEGER NOT NULL DEFAULT 0,
                    status TEXT NOT NULL,
                    down_since_ms INTEGER,
                    down_clock_adjust_ms INTEGER NOT NULL DEFAULT 0,
                    last_check_at_ms INTEGER,
                    snooze_until_ms INTEGER,
                    paused_at_ms INTEGER,
                    slept_at_ms INTEGER
                 );
                 INSERT INTO runtime_state (
                    service_id, consecutive_hard_fails, status
                 ) VALUES ('svc-1', 1, 'degraded');",
            )
            .unwrap();
        }
        let history = History::open(&path).unwrap();
        let state = history.load_runtime("svc-1").unwrap();
        assert_eq!(state.status, MachineStatus::Degraded);
        assert_eq!(state.degraded_since, None);

        let now = at_ms(1_700_000_000_000);
        let mut next = state;
        next.degraded_since = Some(now);
        history.put_runtime("svc-1", &next).unwrap();
        assert_eq!(
            history.load_runtime("svc-1").unwrap().degraded_since,
            Some(now)
        );
    }

    #[test]
    fn mid_pause_restart_still_subtracts_pause_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        let down_at = at_ms(1_700_000_000_000);
        let pause_at = at_ms(1_700_000_060_000);
        let unpause_at = at_ms(1_700_000_180_000);
        {
            let history = History::open(&path).unwrap();
            history.put_runtime("svc-1", &down_state(down_at)).unwrap();
            history.apply_pause("svc-1", pause_at).unwrap();
        }
        let history = History::open(&path).unwrap();
        let mid = history.load_runtime("svc-1").unwrap();
        assert_eq!(mid.status, MachineStatus::Down);
        assert_eq!(mid.paused_at, Some(pause_at));
        assert_eq!(mid.down_clock_adjust_ms, 0);

        history.apply_unpause("svc-1", unpause_at).unwrap();
        let after = history.load_runtime("svc-1").unwrap();
        assert_eq!(after.paused_at, None);
        assert_eq!(after.down_clock_adjust_ms, 120_000);
        assert_eq!(after.status, MachineStatus::Down);
    }

    #[test]
    fn pause_covers_overlapping_sleep() {
        let (_dir, history) = open_temp();
        let down_at = at_ms(1_700_000_000_000);
        let pause_at = at_ms(1_700_000_060_000);
        let sleep_at = at_ms(1_700_000_090_000);
        let wake_at = at_ms(1_700_000_150_000);
        let unpause_at = at_ms(1_700_000_180_000);
        history.put_runtime("svc-1", &down_state(down_at)).unwrap();
        history.apply_pause("svc-1", pause_at).unwrap();
        history.apply_sleep("svc-1", sleep_at).unwrap();
        let mid = history.load_runtime("svc-1").unwrap();
        assert_eq!(mid.paused_at, Some(pause_at));
        assert!(mid.slept_at.is_none());

        history.apply_wake("svc-1", wake_at).unwrap();
        history.apply_unpause("svc-1", unpause_at).unwrap();
        let after = history.load_runtime("svc-1").unwrap();
        assert_eq!(after.paused_at, None);
        assert_eq!(after.slept_at, None);
        assert_eq!(after.down_clock_adjust_ms, 120_000);
    }

    #[test]
    fn leftover_slept_at_is_folded_before_new_sleep() {
        let (_dir, history) = open_temp();
        let down_at = at_ms(1_700_000_000_000);
        history.put_runtime("svc-1", &down_state(down_at)).unwrap();
        history
            .apply_sleep("svc-1", at_ms(1_700_000_010_000))
            .unwrap();
        // Missed wake; a later sleep must not leave the old stamp hanging.
        history
            .apply_sleep("svc-1", at_ms(1_700_000_040_000))
            .unwrap();
        let mid = history.load_runtime("svc-1").unwrap();
        assert_eq!(mid.slept_at, Some(at_ms(1_700_000_040_000)));
        assert_eq!(mid.down_clock_adjust_ms, 30_000);

        history
            .apply_wake("svc-1", at_ms(1_700_000_050_000))
            .unwrap();
        let after = history.load_runtime("svc-1").unwrap();
        assert_eq!(after.slept_at, None);
        assert_eq!(after.down_clock_adjust_ms, 40_000);
    }

    #[test]
    fn sleep_wake_adjusts_down_clock() {
        let (_dir, history) = open_temp();
        let down_at = at_ms(1_700_000_000_000);
        history.put_runtime("svc-1", &down_state(down_at)).unwrap();
        history
            .apply_sleep("svc-1", at_ms(1_700_000_010_000))
            .unwrap();
        history
            .apply_wake("svc-1", at_ms(1_700_000_040_000))
            .unwrap();
        let state = history.load_runtime("svc-1").unwrap();
        assert_eq!(state.slept_at, None);
        assert_eq!(state.down_clock_adjust_ms, 30_000);
    }

    #[test]
    fn last_result_roundtrips_body_and_assertion_diffs() {
        let (_dir, history) = open_temp();
        let result = result_with(
            at_ms(1_700_000_000_000),
            ServiceStatus::Degraded,
            OutcomeClass::Hard,
            Some(ErrorKind::Assertion),
        );
        history.put_last_result("svc-1", &result).unwrap();
        let loaded = history.last_result("svc-1").unwrap().unwrap();
        assert_eq!(loaded, result);
        assert_eq!(
            loaded.evidence.body_preview.as_deref(),
            Some(r#"{"status":"unhealthy"}"#)
        );
        assert_eq!(
            loaded.evidence.assertion_results[0].expected,
            Some(serde_json::json!("ok"))
        );
        assert_eq!(
            loaded.evidence.assertion_results[0].actual,
            Some(serde_json::json!("unhealthy"))
        );

        let raw: String = history
            .conn
            .query_row(
                "SELECT payload_json FROM last_results WHERE service_id = 'svc-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(value.get("headers").is_none());
        assert!(value.get("requestHeaders").is_none());
        assert!(value.get("bodyPreview").is_some());
    }

    #[test]
    fn canceled_and_offline_are_not_written() {
        let (_dir, history) = open_temp();
        let at = at_ms(1_700_000_000_000);
        history
            .insert_sample(
                "svc-1",
                &sample(
                    at,
                    ServiceStatus::Healthy,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Canceled),
                ),
            )
            .unwrap();
        history
            .insert_sample(
                "svc-1",
                &sample(
                    at,
                    ServiceStatus::Healthy,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Offline),
                ),
            )
            .unwrap();
        history
            .put_last_result(
                "svc-1",
                &result_with(
                    at,
                    ServiceStatus::Down,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Canceled),
                ),
            )
            .unwrap();
        history
            .put_last_result(
                "svc-1",
                &result_with(
                    at,
                    ServiceStatus::Down,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Offline),
                ),
            )
            .unwrap();

        assert!(history.samples_24h("svc-1", at).unwrap().is_empty());
        assert!(history.last_result("svc-1").unwrap().is_none());
    }

    #[test]
    fn samples_store_state_and_outcome() {
        let (_dir, history) = open_temp();
        let t0 = at_ms(1_700_000_000_000);
        history
            .insert_sample(
                "svc-1",
                &sample(t0, ServiceStatus::Healthy, OutcomeClass::Ok, None),
            )
            .unwrap();
        history
            .insert_sample(
                "svc-1",
                &sample(
                    at_ms(1_700_000_060_000),
                    ServiceStatus::Degraded,
                    OutcomeClass::Soft,
                    Some(ErrorKind::Slow),
                ),
            )
            .unwrap();
        history
            .insert_sample(
                "svc-1",
                &sample(
                    at_ms(1_700_000_120_000),
                    ServiceStatus::Down,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Timeout),
                ),
            )
            .unwrap();

        let samples = history
            .samples_24h("svc-1", at_ms(1_700_000_120_000))
            .unwrap();
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].state, ServiceStatus::Healthy);
        assert_eq!(samples[0].outcome, OutcomeClass::Ok);
        assert_eq!(samples[1].outcome, OutcomeClass::Soft);
        assert_eq!(samples[2].state, ServiceStatus::Down);
        assert_eq!(samples[2].error_kind, Some(ErrorKind::Timeout));
    }

    #[test]
    fn prune_drops_older_than_24h_then_caps_per_service() {
        let (_dir, history) = open_temp();
        let now = at_ms(1_700_100_000_000);
        history
            .insert_sample(
                "svc-1",
                &sample(
                    now - chrono::Duration::hours(25),
                    ServiceStatus::Healthy,
                    OutcomeClass::Ok,
                    None,
                ),
            )
            .unwrap();
        history
            .insert_sample(
                "svc-1",
                &sample(
                    now - chrono::Duration::hours(1),
                    ServiceStatus::Healthy,
                    OutcomeClass::Ok,
                    None,
                ),
            )
            .unwrap();
        history.prune_at(now).unwrap();
        assert_eq!(history.samples_24h("svc-1", now).unwrap().len(), 1);

        for i in 0..2_010 {
            history
                .insert_sample(
                    "svc-1",
                    &sample(
                        at_ms(now.timestamp_millis() - i64::from(i) * 1_000),
                        ServiceStatus::Healthy,
                        OutcomeClass::Ok,
                        None,
                    ),
                )
                .unwrap();
            history
                .insert_sample(
                    "svc-2",
                    &sample(
                        at_ms(now.timestamp_millis() - i64::from(i) * 1_000),
                        ServiceStatus::Degraded,
                        OutcomeClass::Soft,
                        Some(ErrorKind::Slow),
                    ),
                )
                .unwrap();
        }
        history.prune_at(now).unwrap();
        assert_eq!(history.samples_24h("svc-1", now).unwrap().len(), 2000);
        assert_eq!(history.samples_24h("svc-2", now).unwrap().len(), 2000);

        let newest = history.samples_24h("svc-1", now).unwrap();
        assert_eq!(newest.last().unwrap().at, now);
        assert_eq!(
            newest.first().unwrap().at,
            at_ms(now.timestamp_millis() - 1_999_000)
        );
    }

    #[test]
    fn set_snooze_writes_and_clears() {
        let (_dir, history) = open_temp();
        let until = at_ms(1_700_000_900_000);
        history.set_snooze("svc-1", Some(until)).unwrap();
        assert_eq!(
            history.load_runtime("svc-1").unwrap().snooze_until,
            Some(until)
        );
        history.set_snooze("svc-1", None).unwrap();
        assert!(history
            .load_runtime("svc-1")
            .unwrap()
            .snooze_until
            .is_none());
    }

    #[test]
    fn delete_service_clears_all_three_tables() {
        let (_dir, history) = open_temp();
        let at = at_ms(1_700_000_000_000);
        history.put_runtime("keep", &down_state(at)).unwrap();
        history.put_runtime("gone", &down_state(at)).unwrap();
        history
            .put_last_result(
                "gone",
                &result_with(
                    at,
                    ServiceStatus::Down,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Timeout),
                ),
            )
            .unwrap();
        history
            .put_last_result(
                "keep",
                &result_with(
                    at,
                    ServiceStatus::Down,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Timeout),
                ),
            )
            .unwrap();
        history
            .insert_sample(
                "gone",
                &sample(
                    at,
                    ServiceStatus::Down,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Timeout),
                ),
            )
            .unwrap();
        history
            .insert_sample(
                "keep",
                &sample(
                    at,
                    ServiceStatus::Down,
                    OutcomeClass::Hard,
                    Some(ErrorKind::Timeout),
                ),
            )
            .unwrap();

        history.delete_service("gone").unwrap();
        assert!(history.get_runtime("gone").unwrap().is_none());
        assert!(history.last_result("gone").unwrap().is_none());
        assert!(history.samples_24h("gone", at).unwrap().is_empty());
        assert!(history.get_runtime("keep").unwrap().is_some());
        assert!(history.last_result("keep").unwrap().is_some());
        assert_eq!(history.samples_24h("keep", at).unwrap().len(), 1);
    }

    #[test]
    fn newer_history_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        {
            let history = History::open(&path).unwrap();
            history
                .conn
                .execute("UPDATE schema_meta SET version = 99", [])
                .unwrap();
        }
        let err = History::open(&path).unwrap_err();
        assert!(matches!(err, StoreError::SchemaTooNew { found: 99 }));
    }

    #[test]
    fn open_in_uses_paths_history_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let history = History::open_in(&paths).unwrap();
        history.put_runtime("svc-1", &pending_runtime()).unwrap();
        assert!(paths.history_file().exists());
    }
}
