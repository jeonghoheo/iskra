use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::logging;
use crate::memory;
use crate::run::valid_agent_id;

pub const DELIVERY_STORE_FILE: &str = "deliveries.json";
const VERSION: u32 = 1;
pub const MAX_STORED_DELIVERIES: usize = 200;
pub const MAX_CHUNKS_PER_DELIVERY: u32 = 256;
pub const MAX_ATTEMPTS_PER_CHUNK: u32 = 10;
const MAX_TARGET_CHARS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryChannel {
    Telegram,
}

impl DeliveryChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Sent,
    Failed,
    Unknown,
}

impl DeliveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryChunk {
    pub index: u32,
    pub state: DeliveryState,
    pub attempts: u32,
    pub fallback_used: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub id: u64,
    pub run_id: u64,
    pub agent_id: String,
    pub channel: DeliveryChannel,
    pub target: String,
    pub chunk_count: u32,
    pub chunks: Vec<DeliveryChunk>,
    pub revision: u64,
}

impl Delivery {
    fn is_resolved(&self) -> bool {
        !self.chunks.is_empty()
            && self
                .chunks
                .iter()
                .all(|chunk| chunk.state == DeliveryState::Sent)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryData {
    version: u32,
    next_delivery_id: u64,
    deliveries: Vec<Delivery>,
}

impl Default for DeliveryData {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_delivery_id: 1,
            deliveries: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryTicket {
    pub id: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryOutcome {
    Sent { message_id: i64 },
    Failed,
    Unknown,
}

/// Channel-proven evidence for an `unknown` chunk. The approved `delivery/v1`
/// contract requires this transition even though no user command wires it in
/// Stage 2.5-A; it stays available and test-covered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum Reconciliation {
    Sent { message_id: i64 },
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeliveryCounts {
    pub records: usize,
    pub unresolved_records: usize,
    pub chunks: usize,
    pub pending: usize,
    pub sent: usize,
    pub failed: usize,
    pub unknown: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryError {
    Store(String),
    UnknownRun(u64),
    UnknownDelivery(u64),
    Stale(u64),
    UnexpectedChunk(u32),
    DuplicateChunk(u32),
    InvalidTransition(String),
    InvalidReference(String),
    InvalidTarget,
    InvalidChunkCount(u32),
    RetentionFull,
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(detail) => write!(f, "Delivery store error: {detail}"),
            Self::UnknownRun(id) => write!(f, "Unknown run: {id}"),
            Self::UnknownDelivery(id) => write!(f, "Unknown delivery: {id}"),
            Self::Stale(id) => write!(f, "Stale or late event for delivery {id}"),
            Self::UnexpectedChunk(index) => write!(f, "Unexpected delivery chunk: {index}"),
            Self::DuplicateChunk(index) => write!(f, "Duplicate delivery chunk: {index}"),
            Self::InvalidTransition(detail) => write!(f, "Invalid delivery transition: {detail}"),
            Self::InvalidReference(detail) => write!(f, "Invalid delivery reference: {detail}"),
            Self::InvalidTarget => f.write_str("Invalid delivery target"),
            Self::InvalidChunkCount(count) => write!(f, "Invalid delivery chunk count: {count}"),
            Self::RetentionFull => f.write_str("Delivery store is full of unresolved records"),
        }
    }
}

#[derive(Clone)]
pub struct DeliveryStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    data: Result<DeliveryData, String>,
}

impl DeliveryStore {
    /// Loads `delivery/v1` and maps every unresolved `pending` chunk to
    /// `unknown`. No send, settlement, or Run work is started here.
    ///
    /// Missing data is an empty store and creates no file. Invalid data leaves
    /// the original file untouched and poisons the store until reload.
    pub fn open(path: &Path) -> Self {
        let mut data = read_data(path);
        let mut unknown = 0usize;

        if let Ok(state) = data.as_mut() {
            for record in &mut state.deliveries {
                let mut changed = false;
                for chunk in &mut record.chunks {
                    if chunk.state == DeliveryState::Pending {
                        chunk.state = DeliveryState::Unknown;
                        unknown += 1;
                        changed = true;
                    }
                }
                if changed {
                    record.revision += 1;
                }
            }
            if unknown > 0
                && let Err(error) = write_data(path, state)
            {
                data = Err(error);
            }
        }

        match &data {
            Ok(state) => logging::append(&format!(
                "delivery_store_loaded deliveries={} unknown={unknown}",
                state.deliveries.len()
            )),
            Err(error) => logging::append(&format!("delivery_store_error {error}")),
        }

        Self {
            inner: Arc::new(Mutex::new(Inner {
                path: path.to_path_buf(),
                data,
            })),
        }
    }

    pub fn error(&self) -> Option<String> {
        let inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return Some("Delivery store lock is unavailable".to_string()),
        };
        inner.data.as_ref().err().cloned()
    }

    /// Creates the delivery identity for one logical answer. Chunk attempts are
    /// added separately, immediately before each external send.
    pub fn create(
        &self,
        run_id: u64,
        agent_id: &str,
        channel: DeliveryChannel,
        target: &str,
        chunk_count: u32,
    ) -> Result<DeliveryTicket, DeliveryError> {
        if run_id == 0 {
            return Err(DeliveryError::InvalidReference(
                "run id must be positive".to_string(),
            ));
        }
        if !valid_agent_id(agent_id) {
            return Err(DeliveryError::InvalidReference(
                "invalid Agent id".to_string(),
            ));
        }
        if !valid_target(target) {
            return Err(DeliveryError::InvalidTarget);
        }
        if chunk_count == 0 || chunk_count > MAX_CHUNKS_PER_DELIVERY {
            return Err(DeliveryError::InvalidChunkCount(chunk_count));
        }

        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let state = inner
            .data
            .as_mut()
            .map_err(|error| DeliveryError::Store(error.clone()))?;
        prune_resolved(state)?;

        let id = state.next_delivery_id;
        let next = id
            .checked_add(1)
            .ok_or_else(|| DeliveryError::Store("Delivery id space is exhausted".to_string()))?;
        state.next_delivery_id = next;
        state.deliveries.push(Delivery {
            id,
            run_id,
            agent_id: agent_id.to_string(),
            channel,
            target: target.to_string(),
            chunk_count,
            chunks: Vec::new(),
            revision: 1,
        });

        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("delivery_store_error {error}"));
            return Err(DeliveryError::Store(error));
        }
        logging::append(&format!(
            "delivery_created id={id} run={run_id} agent={agent_id} channel={} chunks={chunk_count}",
            channel.as_str()
        ));
        Ok(DeliveryTicket { id, revision: 1 })
    }

    /// Persists the `pending` state and first attempt for one chunk. This write
    /// must succeed before the external send is attempted.
    pub fn begin_chunk(&self, id: u64, revision: u64, index: u32) -> Result<u64, DeliveryError> {
        self.update(id, revision, |record| {
            if index >= record.chunk_count {
                return Err(DeliveryError::UnexpectedChunk(index));
            }
            if record.chunks.iter().any(|chunk| chunk.index == index) {
                return Err(DeliveryError::DuplicateChunk(index));
            }
            if index != record.chunks.len() as u32 {
                return Err(DeliveryError::UnexpectedChunk(index));
            }
            record.chunks.push(DeliveryChunk {
                index,
                state: DeliveryState::Pending,
                attempts: 1,
                fallback_used: false,
                message_id: None,
            });
            Ok(format!("delivery_pending chunk={index} attempts=1"))
        })
    }

    /// Records the single plain-text fallback attempt for a chunk that received
    /// a definitive Markdown rejection. Any other retry is not allowed.
    pub fn begin_fallback(&self, id: u64, revision: u64, index: u32) -> Result<u64, DeliveryError> {
        self.update(id, revision, |record| {
            let chunk = find_chunk(record, index)?;
            if chunk.state != DeliveryState::Pending {
                return Err(DeliveryError::InvalidTransition(format!(
                    "fallback requires a pending chunk, found {}",
                    chunk.state.as_str()
                )));
            }
            if chunk.fallback_used {
                return Err(DeliveryError::InvalidTransition(
                    "the plain-text fallback was already used".to_string(),
                ));
            }
            if chunk.attempts >= MAX_ATTEMPTS_PER_CHUNK {
                return Err(DeliveryError::InvalidTransition(
                    "the attempt limit was reached".to_string(),
                ));
            }
            chunk.fallback_used = true;
            chunk.attempts += 1;
            Ok(format!(
                "delivery_fallback chunk={index} attempts={}",
                chunk.attempts
            ))
        })
    }

    pub fn settle_chunk(
        &self,
        id: u64,
        revision: u64,
        index: u32,
        outcome: DeliveryOutcome,
    ) -> Result<u64, DeliveryError> {
        self.update(id, revision, |record| {
            let chunk = find_chunk(record, index)?;
            if chunk.state != DeliveryState::Pending {
                return Err(DeliveryError::InvalidTransition(format!(
                    "cannot settle a {} chunk",
                    chunk.state.as_str()
                )));
            }
            match outcome {
                DeliveryOutcome::Sent { message_id } => {
                    if message_id <= 0 {
                        return Err(DeliveryError::InvalidTransition(
                            "a sent chunk requires a positive message id".to_string(),
                        ));
                    }
                    chunk.state = DeliveryState::Sent;
                    chunk.message_id = Some(message_id);
                }
                DeliveryOutcome::Failed => {
                    chunk.state = DeliveryState::Failed;
                    chunk.message_id = None;
                }
                DeliveryOutcome::Unknown => {
                    chunk.state = DeliveryState::Unknown;
                    chunk.message_id = None;
                }
            }
            Ok(format!(
                "delivery_settled chunk={index} state={}",
                chunk.state.as_str()
            ))
        })
    }

    /// Applies channel-specific evidence to an `unknown` chunk. This is the only
    /// path from `unknown`; ambiguous outcomes are never retried blindly.
    ///
    /// Required by the `delivery/v1` contract; no Stage 2.5-A command wires it.
    #[allow(dead_code)]
    pub fn reconcile_chunk(
        &self,
        id: u64,
        revision: u64,
        index: u32,
        outcome: Reconciliation,
    ) -> Result<u64, DeliveryError> {
        self.update(id, revision, |record| {
            let chunk = find_chunk(record, index)?;
            if chunk.state != DeliveryState::Unknown {
                return Err(DeliveryError::InvalidTransition(format!(
                    "reconciliation requires an unknown chunk, found {}",
                    chunk.state.as_str()
                )));
            }
            match outcome {
                Reconciliation::Sent { message_id } => {
                    if message_id <= 0 {
                        return Err(DeliveryError::InvalidTransition(
                            "a sent chunk requires a positive message id".to_string(),
                        ));
                    }
                    chunk.state = DeliveryState::Sent;
                    chunk.message_id = Some(message_id);
                }
                Reconciliation::Failed => {
                    chunk.state = DeliveryState::Failed;
                    chunk.message_id = None;
                }
            }
            Ok(format!(
                "delivery_reconciled chunk={index} state={}",
                chunk.state.as_str()
            ))
        })
    }

    /// Re-opens a definitively failed chunk for a new attempt. Only an explicit
    /// owner action calls this; no automatic retry exists.
    ///
    /// Required by the `delivery/v1` contract; no Stage 2.5-A command wires it.
    #[allow(dead_code)]
    pub fn resend_chunk(&self, id: u64, revision: u64, index: u32) -> Result<u64, DeliveryError> {
        self.update(id, revision, |record| {
            let chunk = find_chunk(record, index)?;
            if chunk.state != DeliveryState::Failed {
                return Err(DeliveryError::InvalidTransition(format!(
                    "resend requires a failed chunk, found {}",
                    chunk.state.as_str()
                )));
            }
            if chunk.attempts >= MAX_ATTEMPTS_PER_CHUNK {
                return Err(DeliveryError::InvalidTransition(
                    "the attempt limit was reached".to_string(),
                ));
            }
            chunk.state = DeliveryState::Pending;
            chunk.message_id = None;
            chunk.fallback_used = false;
            chunk.attempts += 1;
            Ok(format!(
                "delivery_resend chunk={index} attempts={}",
                chunk.attempts
            ))
        })
    }

    pub fn get(&self, id: u64) -> Result<Option<Delivery>, DeliveryError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| DeliveryError::Store(error.clone()))?;
        Ok(state
            .deliveries
            .iter()
            .find(|record| record.id == id)
            .cloned())
    }

    /// True when this Run already has a delivery record for the same channel and
    /// target. Terminal recovery uses it so an event-backed send and a durable
    /// recovery cannot both deliver the same answer. An unresolved record still
    /// counts as an existing attempt: it is never retried automatically.
    pub fn existing_attempt(
        &self,
        run_id: u64,
        channel: DeliveryChannel,
        target: &str,
    ) -> Result<bool, DeliveryError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| DeliveryError::Store(error.clone()))?;
        Ok(state.deliveries.iter().any(|record| {
            record.run_id == run_id && record.channel == channel && record.target == target
        }))
    }

    pub fn counts(&self) -> Result<DeliveryCounts, DeliveryError> {
        self.counts_for(|_| true)
    }

    pub fn counts_for_agent(&self, agent_id: &str) -> Result<DeliveryCounts, DeliveryError> {
        self.counts_for(|record| record.agent_id == agent_id)
    }

    fn counts_for<F>(&self, include: F) -> Result<DeliveryCounts, DeliveryError>
    where
        F: Fn(&Delivery) -> bool,
    {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| DeliveryError::Store(error.clone()))?;
        let mut counts = DeliveryCounts::default();
        for record in state.deliveries.iter().filter(|record| include(record)) {
            counts.records += 1;
            if !record.is_resolved() {
                counts.unresolved_records += 1;
            }
            for chunk in &record.chunks {
                counts.chunks += 1;
                match chunk.state {
                    DeliveryState::Pending => counts.pending += 1,
                    DeliveryState::Sent => counts.sent += 1,
                    DeliveryState::Failed => counts.failed += 1,
                    DeliveryState::Unknown => counts.unknown += 1,
                }
            }
        }
        Ok(counts)
    }

    fn update<F>(&self, id: u64, revision: u64, apply: F) -> Result<u64, DeliveryError>
    where
        F: FnOnce(&mut Delivery) -> Result<String, DeliveryError>,
    {
        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let state = inner
            .data
            .as_mut()
            .map_err(|error| DeliveryError::Store(error.clone()))?;
        let record = state
            .deliveries
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(DeliveryError::UnknownDelivery(id))?;
        if record.revision != revision {
            return Err(DeliveryError::Stale(id));
        }
        let detail = apply(record)?;
        record.revision += 1;
        let revision = record.revision;

        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("delivery_store_error {error}"));
            return Err(DeliveryError::Store(error));
        }
        logging::append(&format!("{detail} id={id} revision={revision}"));
        Ok(revision)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, DeliveryError> {
        self.inner
            .lock()
            .map_err(|_| DeliveryError::Store("Delivery store lock is unavailable".to_string()))
    }
}

fn find_chunk(record: &mut Delivery, index: u32) -> Result<&mut DeliveryChunk, DeliveryError> {
    record
        .chunks
        .iter_mut()
        .find(|chunk| chunk.index == index)
        .ok_or(DeliveryError::UnexpectedChunk(index))
}

fn prune_resolved(state: &mut DeliveryData) -> Result<(), DeliveryError> {
    while state.deliveries.len() >= MAX_STORED_DELIVERIES {
        let Some(index) = state.deliveries.iter().position(Delivery::is_resolved) else {
            return Err(DeliveryError::RetentionFull);
        };
        state.deliveries.remove(index);
    }
    Ok(())
}

fn valid_target(target: &str) -> bool {
    if target.is_empty() || target.chars().count() > MAX_TARGET_CHARS {
        return false;
    }
    let mut chars = target.chars();
    match chars.next() {
        Some('-') => {
            let rest = chars.as_str();
            !rest.is_empty() && rest.chars().all(|character| character.is_ascii_digit())
        }
        Some(first) => first.is_ascii_digit() && chars.all(|character| character.is_ascii_digit()),
        None => false,
    }
}

fn read_data(path: &Path) -> Result<DeliveryData, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DeliveryData::default());
        }
        Err(error) => return Err(format!("Cannot read delivery store: {error}")),
    };
    let data: DeliveryData = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse delivery store: {error}"))?;
    validate(&data)?;
    Ok(data)
}

fn validate(data: &DeliveryData) -> Result<(), String> {
    if data.version != VERSION {
        return Err(format!(
            "Unsupported delivery store version: {}",
            data.version
        ));
    }
    if data.next_delivery_id == 0 {
        return Err("Delivery store next id must be positive".to_string());
    }
    if data.deliveries.len() > MAX_STORED_DELIVERIES {
        return Err("Delivery store contains too many records".to_string());
    }

    let mut previous_id = 0;
    for record in &data.deliveries {
        if record.id == 0 || record.id <= previous_id || record.id >= data.next_delivery_id {
            return Err("Delivery ids are not strictly increasing".to_string());
        }
        previous_id = record.id;
        if record.run_id == 0 {
            return Err("Delivery record has an invalid Run reference".to_string());
        }
        if !valid_agent_id(&record.agent_id) {
            return Err("Delivery record has an invalid Agent id".to_string());
        }
        if !valid_target(&record.target) {
            return Err("Delivery record has an invalid target".to_string());
        }
        if record.chunk_count == 0 || record.chunk_count > MAX_CHUNKS_PER_DELIVERY {
            return Err("Delivery record has an invalid chunk count".to_string());
        }
        if record.revision == 0 {
            return Err("Delivery revision must be positive".to_string());
        }
        if record.chunks.len() > record.chunk_count as usize {
            return Err("Delivery record has too many chunks".to_string());
        }
        for (position, chunk) in record.chunks.iter().enumerate() {
            if chunk.index != position as u32 {
                return Err("Delivery chunks are not in send order".to_string());
            }
            if chunk.attempts == 0 || chunk.attempts > MAX_ATTEMPTS_PER_CHUNK {
                return Err("Delivery chunk has an invalid attempt count".to_string());
            }
            if chunk.fallback_used && chunk.attempts < 2 {
                return Err("Delivery chunk records a fallback without an attempt".to_string());
            }
            match chunk.state {
                DeliveryState::Sent => {
                    if chunk.message_id.is_none_or(|id| id <= 0) {
                        return Err("Sent delivery chunk requires a message id".to_string());
                    }
                }
                DeliveryState::Pending | DeliveryState::Failed | DeliveryState::Unknown => {
                    if chunk.message_id.is_some() {
                        return Err("Unsent delivery chunk cannot have a message id".to_string());
                    }
                }
            }
        }
    }
    Ok(())
}

fn write_data(path: &Path, data: &DeliveryData) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid delivery store path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create delivery store directory: {error}"))?;
    let mut bytes = serde_json::to_vec_pretty(data)
        .map_err(|error| format!("Cannot encode delivery store: {error}"))?;
    bytes.push(b'\n');
    memory::atomic_write(path, &bytes)
        .map_err(|error| format!("Cannot write delivery store: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "iskra-delivery-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_path(dir: &Path) -> PathBuf {
        dir.join("memory").join(DELIVERY_STORE_FILE)
    }

    fn new_store(name: &str) -> (PathBuf, PathBuf, DeliveryStore) {
        let dir = temp_dir(name);
        let path = store_path(&dir);
        let store = DeliveryStore::open(&path);
        (dir, path, store)
    }

    fn create(store: &DeliveryStore, chunks: u32) -> DeliveryTicket {
        store
            .create(1, "main", DeliveryChannel::Telegram, "12345", chunks)
            .unwrap()
    }

    fn begin(store: &DeliveryStore, ticket: &DeliveryTicket, index: u32) -> u64 {
        store
            .begin_chunk(ticket.id, ticket.revision, index)
            .unwrap()
    }

    fn settle(
        store: &DeliveryStore,
        id: u64,
        revision: u64,
        index: u32,
        outcome: DeliveryOutcome,
    ) -> u64 {
        store.settle_chunk(id, revision, index, outcome).unwrap()
    }

    #[test]
    fn missing_store_is_empty_and_creates_no_file() {
        let (_dir, path, store) = new_store("missing");
        assert!(store.error().is_none());
        assert_eq!(store.counts().unwrap(), DeliveryCounts::default());
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn records_store_no_answer_prompt_tool_or_error_text() {
        let (_dir, path, store) = new_store("no-text");
        let ticket = create(&store, 2);
        let revision = begin(&store, &ticket, 0);
        settle(
            &store,
            ticket.id,
            revision,
            0,
            DeliveryOutcome::Sent { message_id: 41 },
        );

        let text = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        let record = &value["deliveries"][0];
        let mut keys: Vec<&str> = record
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "agent_id",
                "channel",
                "chunk_count",
                "chunks",
                "id",
                "revision",
                "run_id",
                "target"
            ]
        );
        for forbidden in ["answer", "prompt", "tool", "error", "content"] {
            assert!(!text.contains(forbidden), "store leaked {forbidden}");
        }
    }

    #[test]
    fn create_validates_references_targets_and_chunk_bounds() {
        let (_dir, path, store) = new_store("create-validation");
        let too_long = "9".repeat(65);
        for target in ["", "abc", "12a", "-", "-12a", too_long.as_str()] {
            assert_eq!(
                store.create(1, "main", DeliveryChannel::Telegram, target, 1),
                Err(DeliveryError::InvalidTarget),
                "{target}"
            );
        }
        for agent in ["", "Main", "a b", "agent_1"] {
            assert!(matches!(
                store.create(1, agent, DeliveryChannel::Telegram, "1", 1),
                Err(DeliveryError::InvalidReference(_))
            ));
        }
        assert!(matches!(
            store.create(0, "main", DeliveryChannel::Telegram, "1", 1),
            Err(DeliveryError::InvalidReference(_))
        ));
        for chunks in [0, MAX_CHUNKS_PER_DELIVERY + 1] {
            assert_eq!(
                store.create(1, "main", DeliveryChannel::Telegram, "1", chunks),
                Err(DeliveryError::InvalidChunkCount(chunks))
            );
        }
        assert_eq!(store.counts().unwrap(), DeliveryCounts::default());
        assert!(!path.exists());
    }

    #[test]
    fn chunk_attempts_start_pending_and_are_in_send_order() {
        let (_dir, path, store) = new_store("pending-order");
        let ticket = create(&store, 2);
        assert_eq!(
            store.begin_chunk(ticket.id, ticket.revision, 1),
            Err(DeliveryError::UnexpectedChunk(1))
        );

        let revision = begin(&store, &ticket, 0);
        let delivery = store.get(ticket.id).unwrap().unwrap();
        assert_eq!(delivery.chunks.len(), 1);
        assert_eq!(delivery.chunks[0].state, DeliveryState::Pending);
        assert_eq!(delivery.chunks[0].attempts, 1);
        assert_eq!(delivery.chunks[0].message_id, None);
        assert_eq!(
            store.begin_chunk(ticket.id, revision, 0),
            Err(DeliveryError::DuplicateChunk(0))
        );
        assert_eq!(
            store.begin_chunk(ticket.id, revision, 2),
            Err(DeliveryError::UnexpectedChunk(2))
        );
        assert!(path.exists());
    }

    #[test]
    fn sent_requires_a_positive_message_id_and_persists_it() {
        let (_dir, path, store) = new_store("sent-id");
        let ticket = create(&store, 1);
        let revision = begin(&store, &ticket, 0);
        assert!(matches!(
            store.settle_chunk(
                ticket.id,
                revision,
                0,
                DeliveryOutcome::Sent { message_id: 0 }
            ),
            Err(DeliveryError::InvalidTransition(_))
        ));
        assert!(matches!(
            store.settle_chunk(
                ticket.id,
                revision,
                0,
                DeliveryOutcome::Sent { message_id: -5 }
            ),
            Err(DeliveryError::InvalidTransition(_))
        ));

        let revision = settle(
            &store,
            ticket.id,
            revision,
            0,
            DeliveryOutcome::Sent { message_id: 42 },
        );
        let delivery = store.get(ticket.id).unwrap().unwrap();
        assert_eq!(delivery.revision, revision);
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);
        assert_eq!(delivery.chunks[0].message_id, Some(42));

        let reloaded = DeliveryStore::open(&path);
        let delivery = reloaded.get(ticket.id).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);
        assert_eq!(delivery.chunks[0].message_id, Some(42));
    }

    #[test]
    fn forbidden_transitions_are_rejected_without_writes() {
        let (_dir, path, store) = new_store("forbidden");
        let first = create(&store, 2);
        let revision = begin(&store, &first, 0);
        let revision = settle(
            &store,
            first.id,
            revision,
            0,
            DeliveryOutcome::Sent { message_id: 1 },
        );
        let before = fs::read(&path).unwrap();

        assert!(matches!(
            store.settle_chunk(first.id, revision, 0, DeliveryOutcome::Failed),
            Err(DeliveryError::InvalidTransition(_))
        ));
        assert!(matches!(
            store.resend_chunk(first.id, revision, 0),
            Err(DeliveryError::InvalidTransition(_))
        ));
        assert!(matches!(
            store.begin_fallback(first.id, revision, 0),
            Err(DeliveryError::InvalidTransition(_))
        ));
        assert!(matches!(
            store.reconcile_chunk(first.id, revision, 0, Reconciliation::Failed),
            Err(DeliveryError::InvalidTransition(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), before);

        let second = create(&store, 1);
        let revision = begin(&store, &second, 0);
        let revision = settle(&store, second.id, revision, 0, DeliveryOutcome::Failed);
        assert!(matches!(
            store.settle_chunk(
                second.id,
                revision,
                0,
                DeliveryOutcome::Sent { message_id: 2 }
            ),
            Err(DeliveryError::InvalidTransition(_))
        ));
        assert!(matches!(
            store.reconcile_chunk(
                second.id,
                revision,
                0,
                Reconciliation::Sent { message_id: 2 }
            ),
            Err(DeliveryError::InvalidTransition(_))
        ));
    }

    #[test]
    fn explicit_resend_restores_one_fallback_cycle_and_is_bounded() {
        let (_dir, _path, store) = new_store("resend");
        let ticket = create(&store, 1);
        let revision = begin(&store, &ticket, 0);
        let revision = store.begin_fallback(ticket.id, revision, 0).unwrap();
        assert!(matches!(
            store.begin_fallback(ticket.id, revision, 0),
            Err(DeliveryError::InvalidTransition(_))
        ));
        let revision = settle(&store, ticket.id, revision, 0, DeliveryOutcome::Failed);
        let revision = store.resend_chunk(ticket.id, revision, 0).unwrap();

        let delivery = store.get(ticket.id).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Pending);
        assert_eq!(delivery.chunks[0].attempts, 3);
        assert!(!delivery.chunks[0].fallback_used);

        let revision = store.begin_fallback(ticket.id, revision, 0).unwrap();
        let revision = settle(&store, ticket.id, revision, 0, DeliveryOutcome::Failed);
        let mut revision = revision;
        while store.get(ticket.id).unwrap().unwrap().chunks[0].attempts < MAX_ATTEMPTS_PER_CHUNK {
            revision = store.resend_chunk(ticket.id, revision, 0).unwrap();
            revision = settle(&store, ticket.id, revision, 0, DeliveryOutcome::Failed);
        }
        assert_eq!(
            store.resend_chunk(ticket.id, revision, 0),
            Err(DeliveryError::InvalidTransition(
                "the attempt limit was reached".to_string()
            ))
        );
    }

    #[test]
    fn unknown_is_reached_by_ambiguity_and_settled_only_by_reconciliation() {
        let (_dir, _path, store) = new_store("unknown");
        let ticket = create(&store, 1);
        let revision = begin(&store, &ticket, 0);
        let revision = settle(&store, ticket.id, revision, 0, DeliveryOutcome::Unknown);
        assert_eq!(
            store.resend_chunk(ticket.id, revision, 0),
            Err(DeliveryError::InvalidTransition(
                "resend requires a failed chunk, found unknown".to_string()
            ))
        );
        assert!(matches!(
            store.settle_chunk(
                ticket.id,
                revision,
                0,
                DeliveryOutcome::Sent { message_id: 9 }
            ),
            Err(DeliveryError::InvalidTransition(_))
        ));

        let revision = store
            .reconcile_chunk(
                ticket.id,
                revision,
                0,
                Reconciliation::Sent { message_id: 9 },
            )
            .unwrap();
        let delivery = store.get(ticket.id).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);
        assert_eq!(delivery.chunks[0].message_id, Some(9));
        assert_eq!(
            store.reconcile_chunk(ticket.id, revision, 0, Reconciliation::Failed),
            Err(DeliveryError::InvalidTransition(
                "reconciliation requires an unknown chunk, found sent".to_string()
            ))
        );
    }

    #[test]
    fn restart_maps_pending_to_unknown_without_sending_or_rerunning() {
        let (_dir, path, store) = new_store("restart");
        let ticket = create(&store, 2);
        let revision = begin(&store, &ticket, 0);
        assert_eq!(
            store.get(ticket.id).unwrap().unwrap().chunks[0].state,
            DeliveryState::Pending
        );

        let reloaded = DeliveryStore::open(&path);
        assert!(reloaded.error().is_none());
        let delivery = reloaded.get(ticket.id).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Unknown);
        assert_eq!(delivery.chunks[0].message_id, None);
        assert_eq!(delivery.revision, revision + 1);

        let again = DeliveryStore::open(&path);
        let delivery = again.get(ticket.id).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Unknown);
        assert_eq!(delivery.revision, revision + 1);
        assert_eq!(again.counts().unwrap().pending, 0);
        assert_eq!(again.counts().unwrap().unknown, 1);
        assert_eq!(again.counts().unwrap().sent, 0);
    }

    #[test]
    fn restart_leaves_resolved_failed_and_unknown_state_untouched() {
        let (_dir, path, store) = new_store("restart-stable");
        let sent = create(&store, 1);
        let revision = begin(&store, &sent, 0);
        settle(
            &store,
            sent.id,
            revision,
            0,
            DeliveryOutcome::Sent { message_id: 5 },
        );

        let failed = store
            .create(2, "main", DeliveryChannel::Telegram, "12345", 1)
            .unwrap();
        let revision = begin(&store, &failed, 0);
        settle(&store, failed.id, revision, 0, DeliveryOutcome::Failed);

        let unknown = store
            .create(3, "main", DeliveryChannel::Telegram, "12345", 1)
            .unwrap();
        let revision = begin(&store, &unknown, 0);
        settle(&store, unknown.id, revision, 0, DeliveryOutcome::Unknown);

        let before = fs::read(&path).unwrap();
        let reloaded = DeliveryStore::open(&path);
        assert!(reloaded.error().is_none());
        assert_eq!(fs::read(&path).unwrap(), before);
        let counts = reloaded.counts().unwrap();
        assert_eq!((counts.sent, counts.failed, counts.unknown), (1, 1, 1));
    }

    #[test]
    fn invalid_data_fails_closed_and_preserves_the_original_file() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("corrupt-json", b"{not-json".to_vec()),
            ("invalid-utf8", vec![0xff, 0xfe, 0xfd]),
            (
                "unknown-version",
                br#"{"version":2,"next_delivery_id":1,"deliveries":[]}"#.to_vec(),
            ),
            (
                "unknown-top-field",
                br#"{"version":1,"next_delivery_id":1,"deliveries":[],"extra":true}"#.to_vec(),
            ),
            (
                "unknown-record-field",
                br#"{"version":1,"next_delivery_id":2,"deliveries":[{"id":1,"run_id":1,"agent_id":"main","channel":"telegram","target":"1","chunk_count":1,"chunks":[],"revision":1,"extra":true}]}"#.to_vec(),
            ),
            (
                "sent-without-message-id",
                br#"{"version":1,"next_delivery_id":2,"deliveries":[{"id":1,"run_id":1,"agent_id":"main","channel":"telegram","target":"1","chunk_count":1,"chunks":[{"index":0,"state":"sent","attempts":1,"fallback_used":false}],"revision":1}]}"#.to_vec(),
            ),
            (
                "out-of-order-chunks",
                br#"{"version":1,"next_delivery_id":2,"deliveries":[{"id":1,"run_id":1,"agent_id":"main","channel":"telegram","target":"1","chunk_count":2,"chunks":[{"index":1,"state":"pending","attempts":1,"fallback_used":false}],"revision":1}]}"#.to_vec(),
            ),
        ];

        for (name, bytes) in cases {
            let dir = temp_dir(name);
            let path = store_path(&dir);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &bytes).unwrap();

            let store = DeliveryStore::open(&path);
            assert!(store.error().is_some(), "{name} was accepted");
            assert_eq!(fs::read(&path).unwrap(), bytes, "{name} was rewritten");
            assert!(matches!(
                store.create(1, "main", DeliveryChannel::Telegram, "1", 1),
                Err(DeliveryError::Store(_))
            ));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn write_failure_poisons_the_store_until_reload() {
        let dir = temp_dir("poison");
        let blocked = dir.join("memory");
        let path = blocked.join(DELIVERY_STORE_FILE);
        let store = DeliveryStore::open(&path);
        assert!(store.error().is_none());
        fs::write(&blocked, b"not a directory").unwrap();

        assert!(matches!(
            store.create(1, "main", DeliveryChannel::Telegram, "1", 1),
            Err(DeliveryError::Store(_))
        ));
        assert!(store.error().is_some());
        assert!(matches!(store.counts(), Err(DeliveryError::Store(_))));

        fs::remove_file(&blocked).unwrap();
        let reloaded = DeliveryStore::open(&path);
        assert!(reloaded.error().is_none());
        assert_eq!(reloaded.counts().unwrap(), DeliveryCounts::default());
        assert!(
            reloaded
                .create(1, "main", DeliveryChannel::Telegram, "1", 1)
                .is_ok()
        );
    }

    #[test]
    fn retention_prunes_only_resolved_records_and_refuses_when_unresolved() {
        let (_dir, _path, store) = new_store("retention");
        let mut last_id = 0;
        for _ in 0..MAX_STORED_DELIVERIES {
            let ticket = create(&store, 1);
            let revision = begin(&store, &ticket, 0);
            settle(
                &store,
                ticket.id,
                revision,
                0,
                DeliveryOutcome::Sent { message_id: 1 },
            );
            last_id = ticket.id;
        }
        assert_eq!(store.counts().unwrap().records, MAX_STORED_DELIVERIES);

        let newest = create(&store, 1);
        assert_eq!(newest.id, last_id + 1);
        let counts = store.counts().unwrap();
        assert_eq!(counts.records, MAX_STORED_DELIVERIES);
        assert!(store.get(1).unwrap().is_none());
        assert!(store.get(newest.id).unwrap().is_some());

        let dir = temp_dir("retention-full");
        let path = store_path(&dir);
        let store = DeliveryStore::open(&path);
        for _ in 0..MAX_STORED_DELIVERIES {
            let ticket = create(&store, 1);
            begin(&store, &ticket, 0);
        }
        let before = fs::read(&path).unwrap();
        assert_eq!(
            store.create(99, "main", DeliveryChannel::Telegram, "1", 1),
            Err(DeliveryError::RetentionFull)
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(store.counts().unwrap().records, MAX_STORED_DELIVERIES);
        assert_eq!(store.counts().unwrap().pending, MAX_STORED_DELIVERIES);
    }

    #[test]
    fn delivery_file_is_private_and_leaves_no_temporary_files() {
        let (_dir, path, store) = new_store("permissions");
        let ticket = create(&store, 1);
        begin(&store, &ticket, 0);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn counts_separate_states_and_support_agent_filtering() {
        let (_dir, _path, store) = new_store("counts");
        let main = create(&store, 2);
        let revision = begin(&store, &main, 0);
        settle(
            &store,
            main.id,
            revision,
            0,
            DeliveryOutcome::Sent { message_id: 1 },
        );

        let helper = store
            .create(2, "helper", DeliveryChannel::Telegram, "12345", 2)
            .unwrap();
        let revision = begin(&store, &helper, 0);
        let revision = store.begin_chunk(helper.id, revision, 1).unwrap();
        store.begin_fallback(helper.id, revision, 1).unwrap();

        let counts = store.counts().unwrap();
        assert_eq!(counts.records, 2);
        assert_eq!(counts.unresolved_records, 1);
        assert_eq!(counts.chunks, 3);
        assert_eq!(counts.sent, 1);
        assert_eq!(counts.pending, 2);
        assert_eq!(counts.failed, 0);
        assert_eq!(counts.unknown, 0);

        let helper_counts = store.counts_for_agent("helper").unwrap();
        assert_eq!(helper_counts.records, 1);
        assert_eq!(helper_counts.pending, 2);
        assert_eq!(helper_counts.sent, 0);
        assert_eq!(
            store.counts_for_agent("absent").unwrap(),
            DeliveryCounts::default()
        );
    }

    #[test]
    fn stale_revisions_and_unknown_ids_are_rejected() {
        let (_dir, _path, store) = new_store("stale");
        let ticket = create(&store, 1);
        let revision = begin(&store, &ticket, 0);
        assert_eq!(
            store.settle_chunk(
                ticket.id,
                ticket.revision,
                0,
                DeliveryOutcome::Sent { message_id: 1 }
            ),
            Err(DeliveryError::Stale(ticket.id))
        );
        assert_eq!(store.get(99).unwrap(), None);
        assert_eq!(
            store.begin_chunk(99, revision, 0),
            Err(DeliveryError::UnknownDelivery(99))
        );
        assert_eq!(store.get(ticket.id).unwrap().unwrap().revision, revision);
    }

    #[test]
    fn an_existing_attempt_is_scoped_to_run_channel_and_target() {
        let (_dir, _path, store) = new_store("existing");
        let ticket = create(&store, 1);
        assert!(
            store
                .existing_attempt(1, DeliveryChannel::Telegram, "12345")
                .unwrap()
        );
        assert!(
            !store
                .existing_attempt(1, DeliveryChannel::Telegram, "999")
                .unwrap()
        );
        assert!(
            !store
                .existing_attempt(2, DeliveryChannel::Telegram, "12345")
                .unwrap()
        );

        // An unresolved attempt still counts: recovery must not resend it
        // automatically just because no chunk reached Telegram.
        begin(&store, &ticket, 0);
        assert!(
            store
                .existing_attempt(1, DeliveryChannel::Telegram, "12345")
                .unwrap()
        );
    }

    #[test]
    fn store_never_accepts_answer_like_payloads() {
        let (_dir, path, store) = new_store("payload");
        let ticket = create(&store, 1);
        let revision = begin(&store, &ticket, 0);
        let answer = "the final answer must stay in session only";
        assert!(!answer.is_empty());
        settle(
            &store,
            ticket.id,
            revision,
            0,
            DeliveryOutcome::Sent { message_id: 7 },
        );

        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(value["deliveries"][0].get("answer").is_none());
        assert!(!fs::read_to_string(&path).unwrap().contains(answer));
        assert_eq!(
            store.get(ticket.id).unwrap().unwrap(),
            Delivery {
                id: ticket.id,
                run_id: 1,
                agent_id: "main".to_string(),
                channel: DeliveryChannel::Telegram,
                target: "12345".to_string(),
                chunk_count: 1,
                chunks: vec![DeliveryChunk {
                    index: 0,
                    state: DeliveryState::Sent,
                    attempts: 1,
                    fallback_used: false,
                    message_id: Some(7),
                }],
                revision: revision + 1,
            }
        );
    }
}
