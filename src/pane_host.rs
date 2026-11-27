//! Core-owned pane host: PTY lifecycle and the persisted pane-slot state.
//!
//! The host owns at most [`MAX_PANES`] panes for the `main` Agent's workspace.
//! Each pane is a fixed `/bin/bash` shell behind a pseudo-terminal, plus a
//! `PaneGrid` view. Slot state lives in `memory/panes.json` as a strict,
//! versioned, atomically written projection. A restart never replays a pane:
//! `spawning` and `live` become `interrupted` at open, with zero spawns.
//!
//! This module deliberately adds no command or surface. The pane-surface card
//! wires the accessor; keep the allowance below module-wide until then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::logging;
use crate::memory;
use crate::pane_grid::{self, OscEvent, PaneGrid, Snapshot};
use crate::pty::{self, PtyProcess, StopError};

/// File name of the pane-slot store inside the main memory directory.
pub const PANE_STORE_FILE: &str = "panes.json";
const VERSION: u32 = 1;
/// Hard pane limit.
pub const MAX_PANES: usize = 8;
/// Largest accepted row or column count.
pub const MAX_DIMENSION: u16 = 200;
/// Default window size for a new pane.
pub const DEFAULT_ROWS: u16 = 24;
pub const DEFAULT_COLS: u16 = 80;
/// Bound on explicit teardown.
pub const TEARDOWN_DEADLINE: Duration = Duration::from_secs(2);
/// Fixed shell program and argv; no shell string is ever interpreted.
pub const SHELL_PROGRAM: &str = "/bin/bash";
pub const SHELL_ARGS: [&str; 2] = ["--norc", "--noprofile"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneState {
    Spawning,
    Live,
    Interrupted,
    Finished,
}

impl PaneState {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Interrupted | Self::Finished)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneSlot {
    pub id: u64,
    pub state: PaneState,
    pub rows: u16,
    pub cols: u16,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneRecord {
    id: u64,
    state: PaneState,
    rows: u16,
    cols: u16,
    #[serde(default)]
    exit_code: Option<i32>,
}

impl PaneRecord {
    fn slot(&self) -> PaneSlot {
        PaneSlot {
            id: self.id,
            state: self.state,
            rows: self.rows,
            cols: self.cols,
            exit_code: self.exit_code,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneData {
    version: u32,
    next_pane_id: u64,
    panes: Vec<PaneRecord>,
}

impl Default for PaneData {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_pane_id: 1,
            panes: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum PaneError {
    /// The pane store is unreadable, invalid, or poisoned.
    Store(String),
    /// Eight panes are already live.
    Capacity,
    /// A row or column count is zero or above [`MAX_DIMENSION`].
    InvalidDimensions { rows: u16, cols: u16 },
    /// The requested screen alone exceeds the modeled 2 MiB pane budget.
    ModelLimit { rows: u16, cols: u16 },
    /// No slot or live pane has this id.
    UnknownPane(u64),
    /// A transition the state machine forbids.
    InvalidTransition {
        id: u64,
        from: PaneState,
        to: PaneState,
    },
    /// The PTY could not be created.
    Spawn(String),
    /// PTY I/O failed.
    Io(String),
    /// Teardown exceeded its deadline; the slot was persisted as interrupted.
    StopTimeout,
}

impl std::fmt::Display for PaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(detail) => write!(f, "Pane store error: {detail}"),
            Self::Capacity => write!(f, "Pane limit reached ({MAX_PANES})"),
            Self::InvalidDimensions { rows, cols } => {
                write!(f, "Invalid pane size {rows}x{cols}")
            }
            Self::ModelLimit { rows, cols } => {
                write!(f, "Pane size {rows}x{cols} exceeds the modeled memory cap")
            }
            Self::UnknownPane(id) => write!(f, "Unknown pane: {id}"),
            Self::InvalidTransition { id, from, to } => {
                write!(f, "Invalid pane transition {id}: {from:?} -> {to:?}")
            }
            Self::Spawn(detail) => write!(f, "Pane spawn failed: {detail}"),
            Self::Io(detail) => write!(f, "Pane I/O failed: {detail}"),
            Self::StopTimeout => f.write_str("Pane teardown timed out"),
        }
    }
}

/// One non-blocking pump result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pump {
    pub bytes: usize,
    pub exit_code: Option<i32>,
}

/// Core-owned pane host. All operations are serialized by one lock so admission,
/// spawn, and slot writes are atomic with respect to each other.
pub struct PaneHost {
    inner: Mutex<HostInner>,
    workspace: PathBuf,
}

struct HostInner {
    path: PathBuf,
    data: Result<PaneData, String>,
    live: HashMap<u64, LivePane>,
}

struct LivePane {
    pty: PtyProcess,
    grid: Box<dyn PaneGrid>,
    rows: u16,
    cols: u16,
    exit_code: Option<i32>,
}

impl PaneHost {
    /// Loads the slot store and performs restart recovery. A missing file is an
    /// empty host and creates nothing. Any read/validation error preserves the
    /// original bytes and poisons pane operations only; it never blocks startup.
    pub fn open(store_path: &Path, workspace: &Path) -> Self {
        let workspace = match fs::canonicalize(workspace) {
            Ok(path) => path,
            Err(error) => {
                logging::append(&format!(
                    "pane_workspace_error path={} err={error}",
                    workspace.display()
                ));
                workspace.to_path_buf()
            }
        };
        let inner = load_inner(store_path);
        Self {
            inner: Mutex::new(inner),
            workspace,
        }
    }

    /// Store error, if any. CLI/Telegram startup is unaffected.
    pub fn error(&self) -> Option<String> {
        let inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return Some("Pane store lock is unavailable".to_string()),
        };
        inner.data.as_ref().err().cloned()
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Explicitly starts a pane in the lowest reusable terminal slot, or a new
    /// slot when none is free. `spawning` is persisted before any PTY exists.
    pub fn start(&self) -> Result<u64, PaneError> {
        self.start_sized(DEFAULT_ROWS, DEFAULT_COLS)
    }

    pub fn start_sized(&self, rows: u16, cols: u16) -> Result<u64, PaneError> {
        validate_size(rows, cols)?;

        let mut inner = self.lock()?;
        let HostInner { path, data, live } = &mut *inner;
        {
            let state = data
                .as_ref()
                .map_err(|error| PaneError::Store(error.clone()))?;
            if live.len() >= MAX_PANES {
                return Err(PaneError::Capacity);
            }
            // An observed exit keeps its final screen readable, but the slot is
            // reusable only by this explicit start call.
            let terminal_ids: Vec<u64> = state
                .panes
                .iter()
                .filter(|record| record.state.is_terminal())
                .map(|record| record.id)
                .collect();
            for id in terminal_ids {
                if live.get(&id).is_some_and(|pane| pane.exit_code.is_some()) {
                    live.remove(&id);
                }
            }
        }

        let (id, previous) = {
            let state = data
                .as_mut()
                .map_err(|error| PaneError::Store(error.clone()))?;
            let reusable = state
                .panes
                .iter()
                .find(|record| record.state.is_terminal() && !live.contains_key(&record.id))
                .map(|record| record.id);
            match reusable {
                Some(id) => {
                    let record = find_mut(state, id)?;
                    let previous = (record.state, record.exit_code, record.rows, record.cols);
                    record.state = PaneState::Spawning;
                    record.exit_code = None;
                    record.rows = rows;
                    record.cols = cols;
                    (id, Some(previous))
                }
                None => {
                    if state.panes.len() >= MAX_PANES {
                        return Err(PaneError::Capacity);
                    }
                    let id = state.next_pane_id;
                    state.next_pane_id = state.next_pane_id.checked_add(1).ok_or_else(|| {
                        PaneError::Store("Pane id space is exhausted".to_string())
                    })?;
                    state.panes.push(PaneRecord {
                        id,
                        state: PaneState::Spawning,
                        rows,
                        cols,
                        exit_code: None,
                    });
                    (id, None)
                }
            }
        };

        // The initial write is the admission point: a failure spawns nothing.
        persist(path, data)?;

        let mut pty = match PtyProcess::spawn(
            Path::new(SHELL_PROGRAM),
            &SHELL_ARGS,
            &self.workspace,
            &pty::pane_env(),
            rows,
            cols,
        ) {
            Ok(pty) => pty,
            Err(error) => {
                logging::append(&format!("pane_spawn_error id={id} err={error}"));
                restore_failed_start(path, data, id, previous);
                return Err(PaneError::Spawn(error.to_string()));
            }
        };

        let scrollback = pane_grid::scrollback_rows(rows, cols);
        let grid = pane_grid::new_grid(rows, cols, scrollback);

        {
            let state = data
                .as_mut()
                .map_err(|error| PaneError::Store(error.clone()))?;
            let record = find_mut(state, id)?;
            check_transition(record.state, PaneState::Live, id)?;
            record.state = PaneState::Live;
            record.exit_code = None;
        }
        if let Err(error) = persist(path, data) {
            // Never leave a live process behind a failed transition record.
            let _ = pty.stop(TEARDOWN_DEADLINE);
            return Err(error);
        }

        live.insert(
            id,
            LivePane {
                pty,
                grid,
                rows,
                cols,
                exit_code: None,
            },
        );
        logging::append(&format!("pane_spawn id={id} rows={rows} cols={cols}"));
        Ok(id)
    }

    /// Reads whatever the pane has produced, feeds the grid, and folds an
    /// observed child exit into `finished`.
    pub fn pump(&self, id: u64) -> Result<Pump, PaneError> {
        self.pump_with_limit(id, 8192)
    }

    /// Test seam: limits one read call so split UTF-8 can be forced.
    pub fn pump_with_limit(&self, id: u64, limit: usize) -> Result<Pump, PaneError> {
        let mut inner = self.lock()?;
        pump_locked(&mut inner, id, limit)
    }

    pub fn snapshot(&self, id: u64) -> Result<Snapshot, PaneError> {
        let mut inner = self.lock()?;
        pump_locked(&mut inner, id, 8192)?;
        let pane = inner.live.get(&id).ok_or(PaneError::UnknownPane(id))?;
        Ok(pane.grid.snapshot())
    }

    pub fn osc_events(&self, id: u64) -> Result<Vec<OscEvent>, PaneError> {
        let inner = self.lock()?;
        let pane = inner.live.get(&id).ok_or(PaneError::UnknownPane(id))?;
        Ok(pane.grid.osc_events())
    }

    pub fn scrollback(&self, id: u64, rows: usize) -> Result<Vec<String>, PaneError> {
        let mut inner = self.lock()?;
        let pane = inner.live.get_mut(&id).ok_or(PaneError::UnknownPane(id))?;
        Ok(pane.grid.scrollback_view(rows))
    }

    pub fn write(&self, id: u64, bytes: &[u8]) -> Result<usize, PaneError> {
        let mut inner = self.lock()?;
        let pane = inner.live.get_mut(&id).ok_or(PaneError::UnknownPane(id))?;
        pane.pty
            .write(bytes, pty::WRITE_TIMEOUT)
            .map_err(|error| PaneError::Io(error.to_string()))
    }

    /// Resizes both the kernel window and the grid. A size that would exceed the
    /// modeled 2 MiB pane budget is refused with the current size unchanged.
    pub fn resize(&self, id: u64, rows: u16, cols: u16) -> Result<(), PaneError> {
        if !pane_grid::model_fits(rows, cols) {
            return Err(PaneError::ModelLimit { rows, cols });
        }
        validate_size(rows, cols)?;

        let mut inner = self.lock()?;
        let HostInner { path, data, live } = &mut *inner;
        {
            let state = data
                .as_ref()
                .map_err(|error| PaneError::Store(error.clone()))?;
            let record = find(state, id)?;
            if record.state != PaneState::Live {
                return Err(PaneError::InvalidTransition {
                    id,
                    from: record.state,
                    to: PaneState::Live,
                });
            }
        }
        let pane = live.get_mut(&id).ok_or(PaneError::UnknownPane(id))?;
        pane.pty
            .resize(rows, cols)
            .map_err(|error| PaneError::Io(error.to_string()))?;
        pane.grid.resize(rows, cols);
        pane.rows = rows;
        pane.cols = cols;

        {
            let state = data
                .as_mut()
                .map_err(|error| PaneError::Store(error.clone()))?;
            let record = find_mut(state, id)?;
            record.rows = rows;
            record.cols = cols;
        }
        if let Err(error) = persist(path, data) {
            // A later state write failure terminates the affected pane so no
            // unrecorded live process survives the poisoned store.
            if let Some(mut pane) = live.remove(&id) {
                let _ = pane.pty.stop(TEARDOWN_DEADLINE);
            }
            return Err(error);
        }
        logging::append(&format!("pane_resize id={id} rows={rows} cols={cols}"));
        Ok(())
    }

    /// Explicit stop. Drains, closes, kills, and reaps within the deadline. A
    /// timeout persists `interrupted`; a reaped child persists `finished`.
    pub fn stop(&self, id: u64) -> Result<(), PaneError> {
        self.stop_with_deadline(id, TEARDOWN_DEADLINE)
    }

    fn stop_with_deadline(&self, id: u64, deadline: Duration) -> Result<(), PaneError> {
        let mut inner = self.lock()?;
        let HostInner { path, data, live } = &mut *inner;
        let mut pane = live.remove(&id).ok_or(PaneError::UnknownPane(id))?;
        let result = match pane.exit_code {
            Some(code) => Ok(code),
            None => pane.pty.stop(deadline),
        };

        match result {
            Ok(code) => {
                {
                    let state = data
                        .as_mut()
                        .map_err(|error| PaneError::Store(error.clone()))?;
                    let record = find_mut(state, id)?;
                    if record.state != PaneState::Live && record.state != PaneState::Finished {
                        return Err(PaneError::InvalidTransition {
                            id,
                            from: record.state,
                            to: PaneState::Finished,
                        });
                    }
                    record.state = PaneState::Finished;
                    record.exit_code = Some(code);
                }
                persist(path, data)?;
                logging::append(&format!("pane_stop id={id} code={code}"));
                Ok(())
            }
            Err(StopError::Timeout) => {
                {
                    let state = data
                        .as_mut()
                        .map_err(|error| PaneError::Store(error.clone()))?;
                    let record = find_mut(state, id)?;
                    record.state = PaneState::Interrupted;
                    record.exit_code = None;
                }
                persist(path, data)?;
                logging::append(&format!("pane_stop_timeout id={id}"));
                Err(PaneError::StopTimeout)
            }
            Err(StopError::Io(detail)) => {
                {
                    let state = data
                        .as_mut()
                        .map_err(|error| PaneError::Store(error.clone()))?;
                    let record = find_mut(state, id)?;
                    record.state = PaneState::Interrupted;
                    record.exit_code = None;
                }
                persist(path, data)?;
                logging::append(&format!("pane_stop_error id={id}"));
                Err(PaneError::Io(detail))
            }
        }
    }

    /// Deterministic state-level timeout injection for tests. No real hang.
    #[cfg(test)]
    fn simulate_stop_timeout(&self, id: u64) -> Result<(), PaneError> {
        let mut inner = self.lock()?;
        let HostInner { path, data, live } = &mut *inner;
        let mut pane = live.remove(&id).ok_or(PaneError::UnknownPane(id))?;
        let _ = pane.pty.stop(Duration::from_millis(250));
        {
            let state = data
                .as_mut()
                .map_err(|error| PaneError::Store(error.clone()))?;
            let record = find_mut(state, id)?;
            record.state = PaneState::Interrupted;
            record.exit_code = None;
        }
        persist(path, data)?;
        logging::append(&format!("pane_stop_timeout id={id}"));
        Ok(())
    }

    pub fn slot(&self, id: u64) -> Result<PaneSlot, PaneError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| PaneError::Store(error.clone()))?;
        Ok(find(state, id)?.slot())
    }

    pub fn slots(&self) -> Result<Vec<PaneSlot>, PaneError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| PaneError::Store(error.clone()))?;
        Ok(state.panes.iter().map(PaneRecord::slot).collect())
    }

    #[cfg(test)]
    pub fn live_count(&self) -> usize {
        self.inner.lock().map(|inner| inner.live.len()).unwrap_or(0)
    }

    fn lock(&self) -> Result<MutexGuard<'_, HostInner>, PaneError> {
        self.inner
            .lock()
            .map_err(|_| PaneError::Store("Pane store lock is unavailable".to_string()))
    }
}

fn validate_size(rows: u16, cols: u16) -> Result<(), PaneError> {
    if rows == 0 || cols == 0 || rows > MAX_DIMENSION || cols > MAX_DIMENSION {
        return Err(PaneError::InvalidDimensions { rows, cols });
    }
    Ok(())
}

fn load_inner(path: &Path) -> HostInner {
    let mut data = read_data(path);
    let mut recovered = 0usize;
    if let Ok(state) = data.as_mut() {
        for record in &mut state.panes {
            if matches!(record.state, PaneState::Spawning | PaneState::Live) {
                logging::append(&format!("pane_interrupted id={} reason=restart", record.id));
                record.state = PaneState::Interrupted;
                record.exit_code = None;
                recovered += 1;
            }
        }
        if recovered > 0
            && let Err(error) = write_data(path, state)
        {
            data = Err(error);
        }
    }

    match &data {
        Ok(state) => logging::append(&format!(
            "pane_store_loaded panes={} interrupted={recovered}",
            state.panes.len()
        )),
        Err(error) => logging::append(&format!("pane_store_error {error}")),
    }

    HostInner {
        path: path.to_path_buf(),
        data,
        live: HashMap::new(),
    }
}

fn restore_failed_start(
    path: &Path,
    data: &mut Result<PaneData, String>,
    id: u64,
    previous: Option<(PaneState, Option<i32>, u16, u16)>,
) {
    let Ok(state) = data.as_mut() else {
        return;
    };
    match previous {
        Some((state_before, exit_code, rows, cols)) => {
            if let Some(record) = state.panes.iter_mut().find(|record| record.id == id) {
                record.state = state_before;
                record.exit_code = exit_code;
                record.rows = rows;
                record.cols = cols;
            }
        }
        None => state.panes.retain(|record| record.id != id),
    }
    if let Err(error) = persist(path, data) {
        logging::append(&format!("pane_store_error restore id={id} err={error}"));
    }
}

fn pump_locked(inner: &mut HostInner, id: u64, limit: usize) -> Result<Pump, PaneError> {
    let HostInner { path, data, live } = inner;
    let pane = live.get_mut(&id).ok_or(PaneError::UnknownPane(id))?;
    let limit = limit.clamp(1, 65_536);
    let mut buffer = vec![0u8; limit];
    let mut total = 0usize;
    loop {
        match pane.pty.read(&mut buffer) {
            Ok(Some(0)) => break,
            Ok(Some(read)) => {
                pane.grid.process(&buffer[..read]);
                total += read;
                if read < buffer.len() {
                    continue;
                }
            }
            Ok(None) => break,
            Err(error) => return Err(PaneError::Io(error.to_string())),
        }
    }

    if pane.exit_code.is_none()
        && let Some(code) = pane
            .pty
            .poll_exit()
            .map_err(|error| PaneError::Io(error.to_string()))?
    {
        pane.exit_code = Some(code);
        {
            let state = data
                .as_mut()
                .map_err(|error| PaneError::Store(error.clone()))?;
            let record = find_mut(state, id)?;
            if record.state == PaneState::Live {
                record.state = PaneState::Finished;
                record.exit_code = Some(code);
            }
        }
        persist(path, data)?;
        logging::append(&format!("pane_exit id={id} code={code}"));
    }

    Ok(Pump {
        bytes: total,
        exit_code: pane.exit_code,
    })
}

fn find(state: &PaneData, id: u64) -> Result<&PaneRecord, PaneError> {
    state
        .panes
        .iter()
        .find(|record| record.id == id)
        .ok_or(PaneError::UnknownPane(id))
}

fn find_mut(state: &mut PaneData, id: u64) -> Result<&mut PaneRecord, PaneError> {
    state
        .panes
        .iter_mut()
        .find(|record| record.id == id)
        .ok_or(PaneError::UnknownPane(id))
}

fn allowed_transition(from: PaneState, to: PaneState) -> bool {
    matches!(
        (from, to),
        (PaneState::Spawning, PaneState::Live)
            | (PaneState::Spawning, PaneState::Interrupted)
            | (PaneState::Spawning, PaneState::Finished)
            | (PaneState::Live, PaneState::Finished)
            | (PaneState::Live, PaneState::Interrupted)
            | (PaneState::Interrupted, PaneState::Spawning)
            | (PaneState::Finished, PaneState::Spawning)
    )
}

fn check_transition(from: PaneState, to: PaneState, id: u64) -> Result<(), PaneError> {
    if allowed_transition(from, to) {
        Ok(())
    } else {
        Err(PaneError::InvalidTransition { id, from, to })
    }
}

fn persist(path: &Path, data: &mut Result<PaneData, String>) -> Result<(), PaneError> {
    let result = match &*data {
        Ok(state) => write_data(path, state),
        Err(error) => return Err(PaneError::Store(error.clone())),
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            logging::append(&format!("pane_store_error {error}"));
            *data = Err(error.clone());
            Err(PaneError::Store(error))
        }
    }
}

fn read_data(path: &Path) -> Result<PaneData, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PaneData::default());
        }
        Err(error) => return Err(format!("Cannot read pane store: {error}")),
    };
    let data: PaneData = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse pane store: {error}"))?;
    validate(&data)?;
    Ok(data)
}

fn validate(data: &PaneData) -> Result<(), String> {
    if data.version != VERSION {
        return Err(format!("Unsupported pane store version: {}", data.version));
    }
    if data.next_pane_id == 0 {
        return Err("Pane store next id must be positive".to_string());
    }
    if data.panes.len() > MAX_PANES {
        return Err("Pane store contains too many slots".to_string());
    }
    let mut previous = 0u64;
    for record in &data.panes {
        if record.id == 0 || record.id <= previous || record.id >= data.next_pane_id {
            return Err("Pane ids are not strictly increasing".to_string());
        }
        previous = record.id;
        if record.rows == 0
            || record.cols == 0
            || record.rows > MAX_DIMENSION
            || record.cols > MAX_DIMENSION
        {
            return Err("Pane slot dimensions are out of range".to_string());
        }
        if record.exit_code.is_some() && record.state != PaneState::Finished {
            return Err("Only a finished pane can carry an exit code".to_string());
        }
    }
    Ok(())
}

fn write_data(path: &Path, data: &PaneData) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid pane store path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create pane store directory: {error}"))?;
    let mut bytes = serde_json::to_vec_pretty(data)
        .map_err(|error| format!("Cannot encode pane store: {error}"))?;
    bytes.push(b'\n');
    memory::atomic_write(path, &bytes).map_err(|error| format!("Cannot write pane store: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("iskra-pane-{name}-{}-{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("workspace")).unwrap();
        dir
    }

    fn host(dir: &Path) -> PaneHost {
        PaneHost::open(&dir.join(PANE_STORE_FILE), &dir.join("workspace"))
    }

    fn write(host: &PaneHost, id: u64, text: &str) {
        host.write(id, text.as_bytes()).unwrap();
    }

    fn pump_until(host: &PaneHost, id: u64, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let _ = host.pump(id);
            let text = host
                .snapshot(id)
                .map(|snap| snap.text())
                .unwrap_or_default();
            if text.contains(needle) {
                return text;
            }
            if Instant::now() >= deadline {
                return text;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_exit(host: &PaneHost, id: u64, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(pump) = host.pump(id)
                && pump.exit_code.is_some()
            {
                return pump.exit_code;
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn seed(dir: &Path, value: serde_json::Value) -> PathBuf {
        let path = dir.join(PANE_STORE_FILE);
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        path
    }

    #[test]
    fn pane_store_transitions_allowed_and_forbidden() {
        use PaneState::*;
        for (from, to) in [
            (Spawning, Live),
            (Spawning, Interrupted),
            (Spawning, Finished),
            (Live, Finished),
            (Live, Interrupted),
            (Interrupted, Spawning),
            (Finished, Spawning),
        ] {
            assert!(
                allowed_transition(from, to),
                "{from:?} -> {to:?} must be allowed"
            );
        }
        for (from, to) in [
            (Interrupted, Live),
            (Finished, Live),
            (Finished, Finished),
            (Live, Spawning),
            (Interrupted, Finished),
        ] {
            assert!(
                !allowed_transition(from, to),
                "{from:?} -> {to:?} must be refused"
            );
            assert!(check_transition(from, to, 1).is_err());
        }
    }

    #[test]
    fn pane_store_initial_write_failure_spawns_nothing() {
        let dir = temp_dir("initial-write");
        // A regular file where the store's parent directory should be makes the
        // initial `spawning` write fail.
        let broken_root = dir.join("broken");
        fs::write(&broken_root, b"not a directory").unwrap();
        let store = broken_root.join(PANE_STORE_FILE);
        let host = PaneHost::open(&store, &dir.join("workspace"));

        let attempts = pty::spawn_attempts();
        let error = host.start().expect_err("initial write must fail");
        assert!(matches!(error, PaneError::Store(_)), "{error}");
        assert_eq!(pty::spawn_attempts(), attempts, "no PTY may be created");
        assert_eq!(host.live_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_store_missing_file_creates_nothing() {
        let dir = temp_dir("missing");
        let path = dir.join(PANE_STORE_FILE);
        let host = host(&dir);
        assert!(host.error().is_none());
        assert!(host.slots().unwrap().is_empty());
        assert!(!path.exists(), "opening must not create the store");
        assert_eq!(host.live_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_store_recovery_maps_spawning_and_live_to_interrupted() {
        let dir = temp_dir("recovery");
        let path = seed(
            &dir,
            serde_json::json!({
                "version": 1,
                "next_pane_id": 3,
                "panes": [
                    {"id": 1, "state": "spawning", "rows": 24, "cols": 80, "exit_code": null},
                    {"id": 2, "state": "live", "rows": 30, "cols": 100, "exit_code": null}
                ]
            }),
        );
        let attempts = pty::spawn_attempts();
        let host = host(&dir);
        assert_eq!(pty::spawn_attempts(), attempts, "recovery must not spawn");
        let slots = host.slots().unwrap();
        assert_eq!(slots.len(), 2);
        assert!(
            slots
                .iter()
                .all(|slot| slot.state == PaneState::Interrupted)
        );
        assert!(slots.iter().all(|slot| slot.exit_code.is_none()));
        let written: PaneData = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            written
                .panes
                .iter()
                .all(|record| record.state == PaneState::Interrupted)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_store_invalid_data_is_preserved_and_poisoned() {
        let cases = [
            "not json".to_string(),
            serde_json::json!({"version": 2, "next_pane_id": 1, "panes": []}).to_string(),
            serde_json::json!({"version": 1, "next_pane_id": 1, "panes": [], "extra": true})
                .to_string(),
            serde_json::json!({
                "version": 1,
                "next_pane_id": 3,
                "panes": [
                    {"id": 1, "state": "finished", "rows": 24, "cols": 80, "exit_code": 0},
                    {"id": 1, "state": "finished", "rows": 24, "cols": 80, "exit_code": 0}
                ]
            })
            .to_string(),
            serde_json::json!({
                "version": 1,
                "next_pane_id": 2,
                "panes": [{"id": 2, "state": "finished", "rows": 24, "cols": 80, "exit_code": 0}]
            })
            .to_string(),
            serde_json::json!({
                "version": 1,
                "next_pane_id": 2,
                "panes": [{"id": 1, "state": "finished", "rows": 0, "cols": 80, "exit_code": 0}]
            })
            .to_string(),
            serde_json::json!({
                "version": 1,
                "next_pane_id": 2,
                "panes": [{"id": 1, "state": "live", "rows": 24, "cols": 80, "exit_code": 3}]
            })
            .to_string(),
            serde_json::json!({
                "version": 1,
                "next_pane_id": 10,
                "panes": (1..=9).map(|id| serde_json::json!({
                    "id": id, "state": "finished", "rows": 24, "cols": 80, "exit_code": 0
                })).collect::<Vec<_>>()
            })
            .to_string(),
        ];

        for (index, content) in cases.iter().enumerate() {
            let dir = temp_dir(&format!("invalid-{index}"));
            let path = dir.join(PANE_STORE_FILE);
            fs::write(&path, content.as_bytes()).unwrap();
            let host = host(&dir);
            assert!(
                host.error().is_some(),
                "case {index} must poison: {content}"
            );
            let error = host.start().expect_err("poisoned host must refuse");
            assert!(
                matches!(error, PaneError::Store(_)),
                "case {index}: {error}"
            );
            assert_eq!(
                fs::read(&path).unwrap(),
                content.as_bytes(),
                "case {index} must preserve the original bytes"
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn pane_store_invalid_utf8_is_preserved() {
        let dir = temp_dir("invalid-utf8");
        let path = dir.join(PANE_STORE_FILE);
        let bytes = [0xffu8, 0xfe, 0x00, 0x7b];
        fs::write(&path, bytes).unwrap();
        let host = host(&dir);
        assert!(host.error().is_some());
        assert!(matches!(host.start(), Err(PaneError::Store(_))));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_store_later_write_failure_poisons_further_starts() {
        let dir = temp_dir("later-write");
        let path = dir.join(PANE_STORE_FILE);
        let host = host(&dir);
        let id = host.start().unwrap();
        let before = fs::read(&path).unwrap();

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let error = host.stop(id).expect_err("the finished write must fail");
        assert!(matches!(error, PaneError::Store(_)), "{error}");
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "the last correct state must survive"
        );

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let error = host
            .start()
            .expect_err("a poisoned host must refuse further starts");
        assert!(matches!(error, PaneError::Store(_)), "{error}");
        assert_eq!(host.live_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_store_file_is_private() {
        let dir = temp_dir("mode");
        let path = dir.join(PANE_STORE_FILE);
        let host = host(&dir);
        let id = host.start().unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = host.stop(id);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_start_reports_live_and_shows_output() {
        let dir = temp_dir("start");
        let host = host(&dir);
        let id = host.start().unwrap();
        assert_eq!(host.slot(id).unwrap().state, PaneState::Live);
        write(&host, id, "printf 'PANE_READY\\n'\r");
        let text = pump_until(&host, id, "PANE_READY", Duration::from_secs(5));
        assert!(text.contains("PANE_READY"), "{text:?}");
        let _ = host.stop(id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_resize_reports_stty_and_snapshot() {
        let dir = temp_dir("resize");
        let host = host(&dir);
        let id = host.start().unwrap();
        host.resize(id, 30, 100).unwrap();
        write(&host, id, "stty size\r");
        let text = pump_until(&host, id, "30 100", Duration::from_secs(5));
        assert!(text.contains("30 100"), "{text:?}");
        let snapshot = host.snapshot(id).unwrap();
        assert_eq!((snapshot.rows, snapshot.cols), (30, 100));
        assert_eq!(host.slot(id).unwrap().rows, 30);
        assert_eq!(host.slot(id).unwrap().cols, 100);
        let _ = host.stop(id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_resize_beyond_model_cap_is_refused() {
        let dir = temp_dir("resize-cap");
        let host = host(&dir);
        let id = host.start().unwrap();
        let error = host.resize(id, 400, 200).expect_err("model cap");
        assert!(matches!(error, PaneError::ModelLimit { .. }), "{error}");
        let snapshot = host.snapshot(id).unwrap();
        assert_eq!((snapshot.rows, snapshot.cols), (DEFAULT_ROWS, DEFAULT_COLS));
        assert_eq!(host.slot(id).unwrap().rows, DEFAULT_ROWS);
        let _ = host.stop(id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_exit_is_finished_with_code_and_requires_explicit_start() {
        let dir = temp_dir("exit");
        let host = host(&dir);
        let first = host.start().unwrap();
        write(&host, first, "exit 7\r");
        assert_eq!(wait_exit(&host, first, Duration::from_secs(5)), Some(7));
        let slot = host.slot(first).unwrap();
        assert_eq!(slot.state, PaneState::Finished);
        assert_eq!(slot.exit_code, Some(7));

        // No implicit restart: the slot stays finished until this call.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(host.slot(first).unwrap().state, PaneState::Finished);

        let second = host.start().unwrap();
        assert_eq!(second, first, "the terminal slot is reused on start");
        assert_eq!(host.slot(second).unwrap().state, PaneState::Live);
        let _ = host.stop(second);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_stop_reaps_and_persists_finished() {
        let dir = temp_dir("stop");
        let host = host(&dir);
        let id = host.start().unwrap();
        host.stop(id).unwrap();
        let slot = host.slot(id).unwrap();
        assert_eq!(slot.state, PaneState::Finished);
        assert!(slot.exit_code.is_some());
        assert_eq!(host.live_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_stop_timeout_is_interrupted() {
        let dir = temp_dir("stop-timeout");
        let host = host(&dir);
        let id = host.start().unwrap();
        let error = host
            .stop_with_deadline(id, Duration::ZERO)
            .expect_err("zero deadline must time out");
        assert!(matches!(error, PaneError::StopTimeout), "{error}");
        let slot = host.slot(id).unwrap();
        assert_eq!(slot.state, PaneState::Interrupted);
        assert_eq!(slot.exit_code, None);
        assert_eq!(host.live_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_environment_is_allowlisted_and_cwd_is_workspace() {
        let dir = temp_dir("env");
        unsafe {
            std::env::set_var("ISKRA_PANE_CANARY", "leaked");
        }
        let host = host(&dir);
        let id = host.start().unwrap();
        write(
            &host,
            id,
            "printf 'CANARY=[%s]\\n' \"$ISKRA_PANE_CANARY\"; pwd\r",
        );
        let text = pump_until(&host, id, "CANARY=[]", Duration::from_secs(5));
        assert!(text.contains("CANARY=[]"), "{text:?}");
        let canonical = fs::canonicalize(dir.join("workspace")).unwrap();
        // A narrow pane wraps a long path across rows; joining rows restores it.
        let joined = text.replace('\n', "");
        assert!(
            joined.contains(canonical.to_str().unwrap()),
            "pwd must equal the canonical workspace: {text:?}"
        );
        unsafe {
            std::env::remove_var("ISKRA_PANE_CANARY");
        }
        let _ = host.stop(id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_korean_text_keeps_wide_cells() {
        let dir = temp_dir("korean");
        let host = host(&dir);
        let id = host.start().unwrap();
        write(&host, id, "printf 'a한b'\r");
        let text = pump_until(&host, id, "a한b", Duration::from_secs(5));
        assert!(text.contains("a한b"), "{text:?}");

        let snapshot = host.snapshot(id).unwrap();
        let row = snapshot
            .lines
            .iter()
            .position(|line| line.contains("a한b"))
            .expect("line with Korean text");
        let line = &snapshot.lines[row];
        let byte_index = line.find("a한b").expect("target occurrence");
        let column = line[..byte_index].chars().count() as u16;
        let first = snapshot_cell(&host, id, row as u16, column);
        assert_eq!(first.text, "a");
        let wide = snapshot_cell(&host, id, row as u16, column + 1);
        assert_eq!(wide.text, "한");
        assert!(wide.wide);
        let continuation = snapshot_cell(&host, id, row as u16, column + 2);
        assert!(continuation.wide_cont);
        let last = snapshot_cell(&host, id, row as u16, column + 3);
        assert_eq!(last.text, "b");
        let _ = host.stop(id);
        let _ = fs::remove_dir_all(&dir);
    }

    fn snapshot_cell(host: &PaneHost, id: u64, row: u16, col: u16) -> pane_grid::CellInfo {
        let mut inner = host.inner.lock().unwrap();
        pump_locked(&mut inner, id, 8192).unwrap();
        inner.live[&id].grid.cell(row, col).expect("cell")
    }

    #[test]
    fn pane_split_output_is_held_across_pumps() {
        let dir = temp_dir("split");
        let host = host(&dir);
        let id = host.start().unwrap();
        write(&host, id, "printf '\\352\\260\\200'\r");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut text = String::new();
        while Instant::now() < deadline {
            let _ = host.pump_with_limit(id, 1);
            text = host.snapshot(id).unwrap().text();
            if text.contains('가') {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(text.contains('가'), "{text:?}");
        assert!(!text.contains('\u{fffd}'), "a split byte must be held");
        let _ = host.stop(id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_ninth_start_is_refused() {
        let dir = temp_dir("nine");
        let host = host(&dir);
        let mut ids = Vec::new();
        for _ in 0..MAX_PANES {
            ids.push(host.start().unwrap());
        }
        let error = host.start().expect_err("the ninth pane must be refused");
        assert!(matches!(error, PaneError::Capacity), "{error}");
        assert_eq!(host.live_count(), MAX_PANES);
        for id in ids {
            let _ = host.stop(id);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_model_dimensions_match_scrollback_rows() {
        let dir = temp_dir("model");
        let host = host(&dir);
        for (rows, cols) in [(24u16, 80u16), (200, 200)] {
            let id = host.start_sized(rows, cols).unwrap();
            let snapshot = host.snapshot(id).unwrap();
            assert_eq!((snapshot.rows, snapshot.cols), (rows, cols));
            assert_eq!(
                snapshot.scrollback_capacity,
                pane_grid::scrollback_rows(rows, cols)
            );
            let modeled = pane_grid::model_screen_bytes(rows, cols)
                + snapshot.scrollback_capacity * cols as usize * pane_grid::BYTES_PER_CELL;
            assert!(modeled <= pane_grid::PANE_MODEL_BUDGET, "{rows}x{cols}");
            let _ = host.stop(id);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn simulate_stop_timeout_state_is_interrupted() {
        let dir = temp_dir("simulate-timeout");
        let host = host(&dir);
        let id = host.start().unwrap();
        host.simulate_stop_timeout(id).unwrap();
        assert_eq!(host.slot(id).unwrap().state, PaneState::Interrupted);
        let _ = fs::remove_dir_all(&dir);
    }
}
