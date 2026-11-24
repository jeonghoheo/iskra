use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "telegram_health.json";
const FILE_VERSION: u32 = 1;
const DEFAULT_API_STALE_SECS: u64 = 90;
const DEFAULT_POLL_RECENT_SECS: u64 = 60;
const DEFAULT_WRITE_INTERVAL_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    Starting,
    Ok,
    Degraded,
    Unreachable,
}

impl HealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ok => "ok",
            Self::Degraded => "degraded",
            Self::Unreachable => "unreachable",
        }
    }
}

impl std::fmt::Display for HealthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthFile {
    pub version: u32,
    pub state: HealthState,
    pub last_api_ok_unix: Option<u64>,
    pub last_api_error_unix: Option<u64>,
    pub last_poll_error_unix: Option<u64>,
    pub consecutive_poll_failures: u64,
    pub last_error_kind: Option<String>,
    pub updated_unix: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthUpdate {
    pub state: HealthState,
    pub state_changed: bool,
    pub failures: u64,
}

#[derive(Debug, Clone, Copy)]
struct Windows {
    api_stale_secs: u64,
    poll_recent_secs: u64,
    write_interval_secs: u64,
}

impl Default for Windows {
    fn default() -> Self {
        Self {
            api_stale_secs: DEFAULT_API_STALE_SECS,
            poll_recent_secs: DEFAULT_POLL_RECENT_SECS,
            write_interval_secs: DEFAULT_WRITE_INTERVAL_SECS,
        }
    }
}

#[derive(Debug)]
struct Inner {
    last_api_ok_unix: Option<u64>,
    last_api_error_unix: Option<u64>,
    last_poll_error_unix: Option<u64>,
    consecutive_poll_failures: u64,
    last_error_kind: Option<String>,
    last_written_unix: Option<u64>,
    last_written_state: Option<HealthState>,
    last_logged_state: Option<HealthState>,
}

/// Telegram reachability state shared by the polling error handler and the
/// API watchdog.
///
/// The file is a small, atomic JSON snapshot. It is ephemeral state, not user
/// memory: it contains timestamps, counters, and a short error category only.
pub struct Health {
    path: PathBuf,
    inner: Mutex<Inner>,
    windows: Windows,
}

impl Health {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_windows(path, Windows::default())
    }

    fn with_windows(path: impl Into<PathBuf>, windows: Windows) -> Self {
        Self {
            path: path.into(),
            inner: Mutex::new(Inner {
                last_api_ok_unix: None,
                last_api_error_unix: None,
                last_poll_error_unix: None,
                consecutive_poll_failures: 0,
                last_error_kind: None,
                last_written_unix: None,
                last_written_state: None,
                last_logged_state: Some(HealthState::Starting),
            }),
            windows,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_windows(
        path: impl Into<PathBuf>,
        api_stale_secs: u64,
        poll_recent_secs: u64,
        write_interval_secs: u64,
    ) -> Self {
        Self::with_windows(
            path,
            Windows {
                api_stale_secs,
                poll_recent_secs,
                write_interval_secs,
            },
        )
    }

    #[cfg(test)]
    pub fn state(&self, now: u64) -> HealthState {
        let inner = self.inner.lock().unwrap();
        compute_state(&inner, now, &self.windows)
    }

    pub fn record_api_ok(&self, now: u64) -> HealthUpdate {
        let mut inner = self.inner.lock().unwrap();
        inner.last_api_ok_unix = Some(now);
        if inner
            .last_poll_error_unix
            .is_none_or(|at| now.saturating_sub(at) >= self.windows.poll_recent_secs)
        {
            inner.consecutive_poll_failures = 0;
        }
        finalize(&mut inner, now, &self.windows)
    }

    pub fn record_api_error(&self, now: u64, kind: &str) -> HealthUpdate {
        let mut inner = self.inner.lock().unwrap();
        inner.last_api_error_unix = Some(now);
        inner.last_error_kind = Some(kind.to_string());
        finalize(&mut inner, now, &self.windows)
    }

    pub fn record_poll_error(&self, now: u64, kind: &str) -> HealthUpdate {
        let mut inner = self.inner.lock().unwrap();
        inner.consecutive_poll_failures = inner.consecutive_poll_failures.saturating_add(1);
        inner.last_poll_error_unix = Some(now);
        inner.last_error_kind = Some(kind.to_string());
        finalize(&mut inner, now, &self.windows)
    }

    pub fn write_if_due(&self, now: u64) -> Result<bool, String> {
        let mut inner = self.inner.lock().unwrap();
        let state = compute_state(&inner, now, &self.windows);
        let due = inner.last_written_state != Some(state)
            || inner
                .last_written_unix
                .is_none_or(|at| now.saturating_sub(at) >= self.windows.write_interval_secs);
        if !due {
            return Ok(false);
        }
        self.write_locked(&mut inner, now, state)?;
        Ok(true)
    }

    pub fn write_force(&self, now: u64) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = compute_state(&inner, now, &self.windows);
        self.write_locked(&mut inner, now, state)
    }

    fn write_locked(&self, inner: &mut Inner, now: u64, state: HealthState) -> Result<(), String> {
        let file = HealthFile {
            version: FILE_VERSION,
            state,
            last_api_ok_unix: inner.last_api_ok_unix,
            last_api_error_unix: inner.last_api_error_unix,
            last_poll_error_unix: inner.last_poll_error_unix,
            consecutive_poll_failures: inner.consecutive_poll_failures,
            last_error_kind: inner.last_error_kind.clone(),
            updated_unix: now,
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|e| format!("Failed to serialize Telegram health: {e}"))?;
        write_atomic(&self.path, &bytes)?;
        inner.last_written_unix = Some(now);
        inner.last_written_state = Some(state);
        Ok(())
    }
}

fn finalize(inner: &mut Inner, now: u64, windows: &Windows) -> HealthUpdate {
    let state = compute_state(inner, now, windows);
    let state_changed = inner.last_logged_state != Some(state);
    if state_changed {
        inner.last_logged_state = Some(state);
    }
    HealthUpdate {
        state,
        state_changed,
        failures: inner.consecutive_poll_failures,
    }
}

fn compute_state(inner: &Inner, now: u64, windows: &Windows) -> HealthState {
    let Some(ok_at) = inner.last_api_ok_unix else {
        return if inner.last_api_error_unix.is_none() && inner.last_poll_error_unix.is_none() {
            HealthState::Starting
        } else {
            HealthState::Unreachable
        };
    };

    if now.saturating_sub(ok_at) > windows.api_stale_secs {
        return HealthState::Unreachable;
    }

    if inner
        .last_poll_error_unix
        .is_some_and(|at| now.saturating_sub(at) < windows.poll_recent_secs)
    {
        HealthState::Degraded
    } else {
        HealthState::Ok
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Telegram health path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create Telegram health directory: {e}"))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Telegram health path has no file name".to_string())?;
    let temp = parent.join(format!(".{file_name}.tmp"));

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&temp)
        .map_err(|e| format!("Failed to open Telegram health temp file: {e}"))?;
    file.write_all(bytes)
        .map_err(|e| format!("Failed to write Telegram health temp file: {e}"))?;
    file.flush()
        .map_err(|e| format!("Failed to flush Telegram health temp file: {e}"))?;
    drop(file);

    std::fs::rename(&temp, path)
        .map_err(|e| format!("Failed to replace Telegram health file: {e}"))?;
    Ok(())
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iskra-health-{}-{}-{}",
            name,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(FILE_NAME)
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn starts_in_starting_state_without_errors() {
        let path = temp_path("starting");
        let health = Health::new(&path);
        assert_eq!(health.state(100), HealthState::Starting);
        cleanup(&path);
    }

    #[test]
    fn api_success_is_ok_and_only_logs_the_change() {
        let path = temp_path("ok");
        let health = Health::new(&path);
        let first = health.record_api_ok(100);
        assert_eq!(first.state, HealthState::Ok);
        assert!(first.state_changed);
        let second = health.record_api_ok(101);
        assert_eq!(second.state, HealthState::Ok);
        assert!(!second.state_changed);
        cleanup(&path);
    }

    #[test]
    fn poll_error_degrades_until_the_window_passes() {
        let path = temp_path("degraded");
        let health = Health::with_test_windows(&path, 60, 5, 0);
        health.record_api_ok(100);
        let update = health.record_poll_error(101, "network");
        assert_eq!(update.state, HealthState::Degraded);
        assert!(update.state_changed);
        assert_eq!(update.failures, 1);

        let recovered = health.record_api_ok(104);
        assert_eq!(recovered.state, HealthState::Degraded);
        let still_degraded = health.state(105);
        assert_eq!(still_degraded, HealthState::Degraded);
        let recovered = health.record_api_ok(107);
        assert_eq!(recovered.state, HealthState::Ok);
        assert_eq!(recovered.failures, 0);
        cleanup(&path);
    }

    #[test]
    fn stale_api_state_is_unreachable() {
        let path = temp_path("stale");
        let health = Health::with_test_windows(&path, 10, 5, 0);
        health.record_api_ok(100);
        assert_eq!(health.state(110), HealthState::Ok);
        assert_eq!(health.state(111), HealthState::Unreachable);
        cleanup(&path);
    }

    #[test]
    fn error_before_first_success_is_unreachable() {
        let path = temp_path("never_ok");
        let health = Health::new(&path);
        let update = health.record_api_error(100, "auth");
        assert_eq!(update.state, HealthState::Unreachable);
        assert!(update.state_changed);
        cleanup(&path);
    }

    #[test]
    fn writes_a_valid_snapshot_with_private_permissions() {
        let path = temp_path("snapshot");
        let health = Health::new(&path);
        health.record_api_ok(100);
        health.record_poll_error(101, "network");
        health.write_force(102).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let file: HealthFile = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(file.version, FILE_VERSION);
        assert_eq!(file.state, HealthState::Degraded);
        assert_eq!(file.last_api_ok_unix, Some(100));
        assert_eq!(file.last_poll_error_unix, Some(101));
        assert_eq!(file.consecutive_poll_failures, 1);
        assert_eq!(file.last_error_kind.as_deref(), Some("network"));
        assert_eq!(file.updated_unix, 102);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("token"));
        assert!(!text.contains("http"));
        cleanup(&path);
    }

    #[test]
    fn write_is_throttled_but_state_changes_force_a_write() {
        let path = temp_path("throttle");
        let health = Health::with_test_windows(&path, 60, 5, 10);
        health.write_force(100).unwrap();

        health.record_api_ok(101);
        assert!(health.write_if_due(101).unwrap());
        assert!(!health.write_if_due(102).unwrap());
        assert!(health.write_if_due(112).unwrap());

        health.record_poll_error(113, "network");
        assert!(health.write_if_due(113).unwrap());

        cleanup(&path);
    }

    #[test]
    fn state_is_computed_at_read_time() {
        let path = temp_path("computed");
        let health = Health::with_test_windows(&path, 10, 5, 0);
        health.record_api_ok(100);
        assert_eq!(health.state(105), HealthState::Ok);
        assert_eq!(health.state(120), HealthState::Unreachable);
        cleanup(&path);
    }
}
