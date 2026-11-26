use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::logging;
use crate::memory;

/// Core-owned dedupe index for `headless/v1` submit. It is a side store: the
/// `runs.json` execution authority and the `session/v1` projection are
/// unchanged. Every entry maps one client idempotency key to one Run id.
pub const IDEMPOTENCY_STORE_FILE: &str = "idempotency.json";
/// Maximum accepted idempotency key length. The HTTP boundary enforces it and
/// the store rejects persisted keys beyond it.
pub const MAX_KEY_CHARS: usize = 256;
/// Bound on retained keys. Oldest Run ids are dropped first; lookups also
/// ignore an entry whose Run left the bounded Run store.
pub const MAX_ENTRIES: usize = 256;
const VERSION: u32 = 1;

#[derive(Clone)]
pub struct IdempotencyStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    data: Result<IdempotencyData, String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdempotencyData {
    version: u32,
    entries: Vec<IdempotencyEntry>,
}

impl Default for IdempotencyData {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdempotencyEntry {
    key: String,
    run_id: u64,
}

impl IdempotencyStore {
    /// Loads the dedupe index. A missing file is empty. An unreadable or
    /// invalid file poisons dedupe only: ordinary CLI and Telegram work does not
    /// depend on it, while a submit that needs a durable idempotency decision
    /// fails closed instead of silently creating a duplicate Run.
    pub(crate) fn open(path: &Path) -> Self {
        let data = read_data(path);
        match &data {
            Ok(state) => logging::append(&format!(
                "idempotency_store_loaded keys={}",
                state.entries.len()
            )),
            Err(error) => logging::append(&format!("idempotency_store_error {error}")),
        }
        Self {
            inner: Arc::new(Mutex::new(Inner {
                path: path.to_path_buf(),
                data,
            })),
        }
    }

    /// Read-only key lookup. A poisoned store is a storage error, never a miss.
    pub(crate) fn lookup(&self, key: &str) -> Result<Option<u64>, String> {
        let inner = self.lock()?;
        let state = inner.data.as_ref().map_err(|error| error.clone())?;
        Ok(state
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.run_id))
    }

    /// Persists one key -> Run mapping before the Run is created, so a Run can
    /// never exist without its dedupe entry. Under the Core admission lock this
    /// is the single winning persist for a key. A write failure poisons the
    /// store and reports the error; the caller creates nothing.
    pub(crate) fn insert(&self, key: &str, run_id: u64) -> Result<(), String> {
        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let mut state = inner.data.as_ref().map_err(|error| error.clone())?.clone();
        match state.entries.iter_mut().find(|entry| entry.key == key) {
            Some(entry) => entry.run_id = run_id,
            None => state.entries.push(IdempotencyEntry {
                key: key.to_string(),
                run_id,
            }),
        }
        state.entries.sort_by_key(|entry| entry.run_id);
        if state.entries.len() > MAX_ENTRIES {
            let excess = state.entries.len() - MAX_ENTRIES;
            state.entries.drain(..excess);
        }
        write_state(&mut inner, &path, &state)?;
        inner.data = Ok(state);
        Ok(())
    }

    /// Removes one key. Used to release a reservation when the Run store write
    /// refuses the Run; it never creates or renumbers a Run.
    pub(crate) fn remove(&self, key: &str) -> Result<(), String> {
        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let mut state = inner.data.as_ref().map_err(|error| error.clone())?.clone();
        let before = state.entries.len();
        state.entries.retain(|entry| entry.key != key);
        if state.entries.len() == before {
            return Ok(());
        }
        write_state(&mut inner, &path, &state)?;
        inner.data = Ok(state);
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, String> {
        self.inner
            .lock()
            .map_err(|_| "Idempotency store lock is unavailable".to_string())
    }
}

fn write_state(inner: &mut Inner, path: &Path, state: &IdempotencyData) -> Result<(), String> {
    if let Err(error) = write_data(path, state) {
        inner.data = Err(error.clone());
        logging::append(&format!("idempotency_store_error {error}"));
        return Err(error);
    }
    Ok(())
}

fn read_data(path: &Path) -> Result<IdempotencyData, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IdempotencyData::default());
        }
        Err(error) => return Err(format!("Cannot read idempotency store: {error}")),
    };
    let data: IdempotencyData = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse idempotency store: {error}"))?;
    validate(&data)?;
    Ok(data)
}

fn validate(data: &IdempotencyData) -> Result<(), String> {
    if data.version != VERSION {
        return Err(format!(
            "Unsupported idempotency store version: {}",
            data.version
        ));
    }
    if data.entries.len() > MAX_ENTRIES {
        return Err("Idempotency store contains too many keys".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for entry in &data.entries {
        if entry.run_id == 0 {
            return Err("Idempotency entry has an invalid Run id".to_string());
        }
        if entry.key.trim().is_empty() || entry.key.chars().count() > MAX_KEY_CHARS {
            return Err("Idempotency entry has an invalid key".to_string());
        }
        if !seen.insert(entry.key.as_str()) {
            return Err("Idempotency store repeats a key".to_string());
        }
    }
    Ok(())
}

fn write_data(path: &Path, data: &IdempotencyData) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid idempotency store path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create idempotency store directory: {error}"))?;
    let mut bytes = serde_json::to_vec_pretty(data)
        .map_err(|error| format!("Cannot encode idempotency store: {error}"))?;
    bytes.push(b'\n');
    memory::atomic_write(path, &bytes)
        .map_err(|error| format!("Cannot write idempotency store: {error}"))
}
