use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::delivery::{
    DELIVERY_STORE_FILE, DeliveryChannel, DeliveryError, DeliveryStore, DeliveryTicket,
};
use crate::idempotency::{IDEMPOTENCY_STORE_FILE, IdempotencyStore};
use crate::logging;
use crate::loop_;
use crate::memory;
use crate::prompt;
use crate::run_lock::DispatcherLock;
use crate::runtime::RuntimeRegistry;
use crate::session::FailureKind;
use crate::supervisor::RunSupervisor;

pub const RUN_STORE_FILE: &str = "runs.json";
const VERSION: u32 = 1;
pub const MAX_STORED_RUNS: usize = 100;
pub const MAX_QUEUED_PER_AGENT: usize = 20;
pub const MAX_INPUT_CHARS: usize = 8_000;
pub const MAX_ACTIVE_RUNS: usize = 2;
const EVENT_BUFFER: usize = 128;

/// How often a channel re-checks the durable Run state while waiting for a
/// RuntimeEvent. RuntimeEvents are hints; this bound keeps a missing, lagged, or
/// closed event from leaving a watcher or typing indicator waiting forever.
pub const TERMINAL_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    QueuedPaused,
    Preparing,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl RunState {
    fn is_active(self) -> bool {
        matches!(self, Self::Preparing | Self::Running)
    }

    pub(crate) fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRecord {
    id: u64,
    agent_id: String,
    input: String,
    state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<FailureKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_work_id: Option<u64>,
    revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunData {
    version: u32,
    next_run_id: u64,
    runs: Vec<RunRecord>,
}

impl Default for RunData {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_run_id: 1,
            runs: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunTicket {
    pub id: u64,
    pub revision: u64,
}

/// The result of one idempotent admission: the Run that owns the client key and
/// the persisted state observed for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SubmitOutcome {
    pub run_id: u64,
    pub state: RunState,
    pub deduplicated: bool,
}

/// Read-only Run reference used to resolve delivery targets and stored answers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunTarget {
    pub id: u64,
    pub agent_id: String,
    pub state: RunState,
    pub failure: Option<FailureKind>,
    pub session_work_id: Option<u64>,
    pub revision: u64,
}

/// A terminal outcome projected from durable Run and Session records.
///
/// Channels use it only when a RuntimeEvent was missing, lagged, or lost. It
/// never creates work: the answer comes from the stored Session projection, and
/// failure detail stays out of the durable Run log.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalOutcome {
    Completed { answer: Option<String> },
    Failed { category: FailureKind },
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug)]
pub struct Claim {
    pub id: u64,
    pub agent_id: String,
    pub revision: u64,
    input: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelledRun {
    pub id: u64,
    pub revision: u64,
    pub was_active: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunQueueStatus {
    pub active: usize,
    pub queued: usize,
    pub paused: usize,
    pub interrupted: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CancelReport {
    pub cancelled: usize,
    pub active: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunError {
    Store(String),
    EmptyInput,
    InputTooLong,
    QueueFull,
    UnknownAgent(String),
    AgentUnavailable(String),
    UnknownRun(u64),
    Stale(u64),
    InvalidTransition(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(detail) => write!(f, "Run store error: {detail}"),
            Self::EmptyInput => f.write_str("Run input is empty"),
            Self::InputTooLong => {
                write!(f, "Run input exceeds the {MAX_INPUT_CHARS}-character limit")
            }
            Self::QueueFull => write!(
                f,
                "The Agent queue already holds {MAX_QUEUED_PER_AGENT} unfinished runs"
            ),
            Self::UnknownAgent(agent) => write!(f, "Unknown Agent: {agent}"),
            Self::AgentUnavailable(agent) => write!(f, "Agent is unavailable: {agent}"),
            Self::UnknownRun(id) => write!(f, "Unknown run: {id}"),
            Self::Stale(id) => write!(f, "Stale or late event for run {id}"),
            Self::InvalidTransition(detail) => write!(f, "Invalid run transition: {detail}"),
        }
    }
}

#[derive(Clone)]
pub struct RunStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    data: Result<RunData, String>,
}

impl RunStore {
    /// Loads the execution authority and performs startup recovery:
    /// `queued -> queued_paused`, `preparing/running -> interrupted`.
    ///
    /// No execution is started here. A read or recovery write failure leaves the
    /// original file untouched and poisons the store until the process restarts.
    #[cfg(test)]
    pub fn load(path: &Path) -> Self {
        Self::load_with(path, None)
    }

    /// Validates Agent references before startup recovery so an unknown Agent
    /// leaves the original file untouched and stops runtime construction.
    pub(crate) fn load_checked(path: &Path, known_agents: &HashSet<String>) -> Self {
        Self::load_with(path, Some(known_agents))
    }

    fn load_with(path: &Path, known_agents: Option<&HashSet<String>>) -> Self {
        let mut data = read_data(path);
        if let (Ok(state), Some(known)) = (&data, known_agents)
            && let Err(error) = validate_agents(state, known)
        {
            data = Err(error);
        }
        let mut paused = 0usize;
        let mut interrupted = 0usize;

        if let Ok(state) = data.as_mut() {
            for record in &mut state.runs {
                match record.state {
                    RunState::Queued => {
                        record.state = RunState::QueuedPaused;
                        record.revision += 1;
                        paused += 1;
                    }
                    RunState::Preparing | RunState::Running => {
                        record.state = RunState::Interrupted;
                        record.revision += 1;
                        interrupted += 1;
                    }
                    _ => {}
                }
            }
            if paused + interrupted > 0
                && let Err(error) = write_data(path, state)
            {
                data = Err(error);
            }
        }

        match &data {
            Ok(state) => logging::append(&format!(
                "run_store_loaded runs={} paused={paused} interrupted={interrupted}",
                state.runs.len()
            )),
            Err(error) => logging::append(&format!("run_store_error {error}")),
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
            Err(_) => return Some("Run store lock is unavailable".to_string()),
        };
        inner.data.as_ref().err().cloned()
    }

    pub fn enqueue(&self, agent_id: &str, input: &str) -> Result<RunTicket, RunError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(RunError::EmptyInput);
        }
        if input.chars().count() > MAX_INPUT_CHARS {
            return Err(RunError::InputTooLong);
        }
        let (canonical, redactions) = memory::redact_credentials(input);
        if canonical.trim().is_empty() {
            return Err(RunError::EmptyInput);
        }

        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let state = inner
            .data
            .as_mut()
            .map_err(|error| RunError::Store(error.clone()))?;

        let queued = state
            .runs
            .iter()
            .filter(|record| {
                record.agent_id == agent_id
                    && matches!(record.state, RunState::Queued | RunState::QueuedPaused)
            })
            .count();
        if queued >= MAX_QUEUED_PER_AGENT {
            return Err(RunError::QueueFull);
        }

        let id = state.next_run_id;
        let next_run_id = id
            .checked_add(1)
            .ok_or_else(|| RunError::Store("Run id space is exhausted".to_string()))?;
        prune_terminal(state)?;
        state.next_run_id = next_run_id;
        state.runs.push(RunRecord {
            id,
            agent_id: agent_id.to_string(),
            input: canonical,
            state: RunState::Queued,
            failure: None,
            session_work_id: None,
            revision: 1,
        });

        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("run_store_error {error}"));
            return Err(RunError::Store(error));
        }
        logging::append(&format!("run_enqueued id={id} agent={agent_id}"));
        if redactions > 0 {
            logging::append(&format!("run_redacted id={id} count={redactions}"));
        }
        Ok(RunTicket { id, revision: 1 })
    }

    /// The id the next accepted enqueue will receive. Read-only; the Core
    /// admission lock keeps it stable between a dedupe reservation and the
    /// matching enqueue.
    pub(crate) fn next_id(&self) -> Result<u64, RunError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| RunError::Store(error.clone()))?;
        Ok(state.next_run_id)
    }

    /// Claims the oldest eligible Agent head with the lowest global Run id.
    ///
    /// A later Run for the same Agent cannot overtake its head. A paused head,
    /// or any active Run for that Agent, blocks later work for the Agent.
    pub fn claim(
        &self,
        eligible_agents: &HashSet<String>,
        global_limit: usize,
    ) -> Result<Option<Claim>, RunError> {
        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let state = inner
            .data
            .as_mut()
            .map_err(|error| RunError::Store(error.clone()))?;

        let active_global = state
            .runs
            .iter()
            .filter(|record| record.state.is_active())
            .count();
        if active_global >= global_limit {
            return Ok(None);
        }

        let mut active_agents = HashSet::new();
        for record in &state.runs {
            if record.state.is_active() {
                active_agents.insert(record.agent_id.clone());
            }
        }

        let mut heads: HashMap<String, (u64, RunState)> = HashMap::new();
        for record in &state.runs {
            if matches!(record.state, RunState::Queued | RunState::QueuedPaused) {
                heads
                    .entry(record.agent_id.clone())
                    .or_insert((record.id, record.state));
            }
        }

        let candidate = heads
            .into_iter()
            .filter(|(agent, (_, head_state))| {
                eligible_agents.contains(agent)
                    && !active_agents.contains(agent)
                    && *head_state == RunState::Queued
            })
            .map(|(_, (id, _))| id)
            .min();
        let Some(id) = candidate else {
            return Ok(None);
        };

        let record = state
            .runs
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(RunError::UnknownRun(id))?;
        let agent_id = record.agent_id.clone();
        record.state = RunState::Preparing;
        record.revision += 1;
        let revision = record.revision;
        let input = record.input.clone();

        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("run_store_error {error}"));
            return Err(RunError::Store(error));
        }
        logging::append(&format!("run_claimed id={id} agent={agent_id}"));
        Ok(Some(Claim {
            id,
            agent_id,
            revision,
            input,
        }))
    }

    fn validate_claim(&self, id: u64, revision: u64) -> Result<(), RunError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| RunError::Store(error.clone()))?;
        let record = state
            .runs
            .iter()
            .find(|record| record.id == id)
            .ok_or(RunError::UnknownRun(id))?;
        if record.revision != revision || record.state != RunState::Preparing {
            return Err(RunError::Stale(id));
        }
        Ok(())
    }

    pub fn started(&self, id: u64, revision: u64, session_work_id: u64) -> Result<u64, RunError> {
        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let state = inner
            .data
            .as_mut()
            .map_err(|error| RunError::Store(error.clone()))?;
        let record = state
            .runs
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(RunError::UnknownRun(id))?;
        if record.revision != revision || record.state != RunState::Preparing {
            return Err(RunError::Stale(id));
        }
        record.state = RunState::Running;
        record.session_work_id = Some(session_work_id);
        record.revision += 1;
        let revision = record.revision;
        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("run_store_error {error}"));
            return Err(RunError::Store(error));
        }
        Ok(revision)
    }

    pub fn settle(
        &self,
        id: u64,
        revision: u64,
        state_after: RunState,
        failure: Option<FailureKind>,
    ) -> Result<u64, RunError> {
        if !matches!(
            state_after,
            RunState::Completed | RunState::Failed | RunState::Cancelled
        ) {
            return Err(RunError::InvalidTransition(format!(
                "settle cannot write {state_after:?}"
            )));
        }
        if (state_after == RunState::Failed) != failure.is_some() {
            return Err(RunError::InvalidTransition(
                "failure must be present exactly for failed runs".to_string(),
            ));
        }

        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let state = inner
            .data
            .as_mut()
            .map_err(|error| RunError::Store(error.clone()))?;
        let record = state
            .runs
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(RunError::UnknownRun(id))?;
        if record.revision != revision || !record.state.is_active() {
            return Err(RunError::Stale(id));
        }
        if state_after == RunState::Completed && record.state != RunState::Running {
            return Err(RunError::InvalidTransition(
                "only a running run can complete".to_string(),
            ));
        }
        record.state = state_after;
        record.failure = failure;
        record.revision += 1;
        let revision = record.revision;

        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("run_store_error {error}"));
            return Err(RunError::Store(error));
        }
        match state_after {
            RunState::Completed => logging::append(&format!("run_completed id={id}")),
            RunState::Failed => logging::append(&format!(
                "run_failed id={id} category={}",
                failure.map(FailureKind::as_str).unwrap_or("other")
            )),
            RunState::Cancelled => logging::append(&format!("run_cancelled id={id}")),
            _ => {}
        }
        Ok(revision)
    }

    /// Cancels the Agent's active, queued, and paused work in one write.
    pub fn cancel_agent(&self, agent_id: &str) -> Result<Vec<CancelledRun>, RunError> {
        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let state = inner
            .data
            .as_mut()
            .map_err(|error| RunError::Store(error.clone()))?;

        let mut cancelled = Vec::new();
        for record in &mut state.runs {
            if record.agent_id != agent_id {
                continue;
            }
            if !matches!(
                record.state,
                RunState::Queued | RunState::QueuedPaused | RunState::Preparing | RunState::Running
            ) {
                continue;
            }
            let was_active = record.state.is_active();
            record.state = RunState::Cancelled;
            record.failure = None;
            record.revision += 1;
            cancelled.push(CancelledRun {
                id: record.id,
                revision: record.revision,
                was_active,
            });
        }
        if cancelled.is_empty() {
            return Ok(cancelled);
        }

        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("run_store_error {error}"));
            return Err(RunError::Store(error));
        }
        for run in &cancelled {
            logging::append(&format!("run_cancelled id={} agent={agent_id}", run.id));
        }
        Ok(cancelled)
    }

    fn unfinished_for_agent(&self, agent_id: &str) -> Result<Vec<RunTicket>, RunError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| RunError::Store(error.clone()))?;
        Ok(state
            .runs
            .iter()
            .filter(|record| {
                record.agent_id == agent_id
                    && matches!(
                        record.state,
                        RunState::Queued
                            | RunState::QueuedPaused
                            | RunState::Preparing
                            | RunState::Running
                    )
            })
            .map(|record| RunTicket {
                id: record.id,
                revision: record.revision,
            })
            .collect())
    }

    /// Restores paused work to the queue in its original order. Dispatch is the
    /// caller's responsibility and never happens implicitly.
    pub fn resume_agent(&self, agent_id: &str) -> Result<Vec<RunTicket>, RunError> {
        let mut inner = self.lock()?;
        let path = inner.path.clone();
        let state = inner
            .data
            .as_mut()
            .map_err(|error| RunError::Store(error.clone()))?;

        let mut resumed = Vec::new();
        for record in &mut state.runs {
            if record.agent_id == agent_id && record.state == RunState::QueuedPaused {
                record.state = RunState::Queued;
                record.revision += 1;
                resumed.push(RunTicket {
                    id: record.id,
                    revision: record.revision,
                });
            }
        }
        if resumed.is_empty() {
            return Ok(resumed);
        }
        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("run_store_error {error}"));
            return Err(RunError::Store(error));
        }
        logging::append(&format!(
            "run_resumed agent={agent_id} count={}",
            resumed.len()
        ));
        Ok(resumed)
    }

    pub fn run_state(&self, id: u64) -> Result<Option<RunState>, RunError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| RunError::Store(error.clone()))?;
        Ok(state
            .runs
            .iter()
            .find(|record| record.id == id)
            .map(|record| record.state))
    }

    /// Read-only lookup of the Run's Agent and Session work reference.
    pub fn run_target(&self, id: u64) -> Result<Option<RunTarget>, RunError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| RunError::Store(error.clone()))?;
        Ok(state
            .runs
            .iter()
            .find(|record| record.id == id)
            .map(|record| RunTarget {
                id: record.id,
                agent_id: record.agent_id.clone(),
                state: record.state,
                failure: record.failure,
                session_work_id: record.session_work_id,
                revision: record.revision,
            }))
    }

    pub fn queue_status(&self, agent_id: &str) -> Result<RunQueueStatus, RunError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| RunError::Store(error.clone()))?;
        let mut status = RunQueueStatus::default();
        for record in state
            .runs
            .iter()
            .filter(|record| record.agent_id == agent_id)
        {
            match record.state {
                RunState::Preparing | RunState::Running => status.active += 1,
                RunState::Queued => status.queued += 1,
                RunState::QueuedPaused => status.paused += 1,
                RunState::Interrupted => status.interrupted += 1,
                _ => {}
            }
        }
        Ok(status)
    }

    #[cfg(test)]
    fn record(&self, id: u64) -> Result<RunRecord, RunError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| RunError::Store(error.clone()))?;
        state
            .runs
            .iter()
            .find(|record| record.id == id)
            .cloned()
            .ok_or(RunError::UnknownRun(id))
    }

    #[cfg(test)]
    fn records(&self) -> Result<Vec<RunRecord>, RunError> {
        let inner = self.lock()?;
        let state = inner
            .data
            .as_ref()
            .map_err(|error| RunError::Store(error.clone()))?;
        Ok(state.runs.clone())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, RunError> {
        self.inner
            .lock()
            .map_err(|_| RunError::Store("Run store lock is unavailable".to_string()))
    }
}

fn prune_terminal(state: &mut RunData) -> Result<(), RunError> {
    while state.runs.len() >= MAX_STORED_RUNS {
        let Some(index) = state
            .runs
            .iter()
            .position(|record| record.state.is_terminal())
        else {
            return Err(RunError::Store(
                "Run store is full of unfinished records".to_string(),
            ));
        };
        state.runs.remove(index);
    }
    Ok(())
}

fn read_data(path: &Path) -> Result<RunData, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RunData::default());
        }
        Err(error) => return Err(format!("Cannot read run store: {error}")),
    };
    let data: RunData = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse run store: {error}"))?;
    validate(&data)?;
    Ok(data)
}

fn validate_agents(data: &RunData, known: &HashSet<String>) -> Result<(), String> {
    for record in &data.runs {
        if !known.contains(&record.agent_id) {
            return Err(format!(
                "Run store references Agent '{}', which is not in the registry",
                record.agent_id
            ));
        }
    }
    Ok(())
}

fn validate(data: &RunData) -> Result<(), String> {
    if data.version != VERSION {
        return Err(format!("Unsupported run store version: {}", data.version));
    }
    if data.next_run_id == 0 {
        return Err("Run store next id must be positive".to_string());
    }
    if data.runs.len() > MAX_STORED_RUNS {
        return Err("Run store contains too many records".to_string());
    }

    let mut previous_id = 0;
    let mut active_global = 0usize;
    let mut active_agents: HashMap<&str, usize> = HashMap::new();
    let mut queued_agents: HashMap<&str, usize> = HashMap::new();

    for record in &data.runs {
        if record.id == 0 || record.id <= previous_id || record.id >= data.next_run_id {
            return Err("Run ids are not strictly increasing".to_string());
        }
        previous_id = record.id;
        if !valid_agent_id(&record.agent_id) {
            return Err("Run record has an invalid Agent id".to_string());
        }
        if record.input.trim().is_empty() || record.input.chars().count() > MAX_INPUT_CHARS {
            return Err("Run record input is empty or too long".to_string());
        }
        if record.revision == 0 {
            return Err("Run revision must be positive".to_string());
        }
        match record.state {
            RunState::Queued | RunState::QueuedPaused => {
                if record.session_work_id.is_some() {
                    return Err("Queued run cannot have Session work".to_string());
                }
            }
            RunState::Preparing => {
                if record.session_work_id.is_some() {
                    return Err("Preparing run cannot have Session work".to_string());
                }
            }
            RunState::Running => {
                if record.session_work_id.is_none() {
                    return Err("Running run must have Session work".to_string());
                }
            }
            RunState::Completed => {
                if record.session_work_id.is_none() {
                    return Err("Completed run must have Session work".to_string());
                }
            }
            RunState::Failed => {
                if record.failure.is_none() {
                    return Err("Failed run must have a failure category".to_string());
                }
            }
            RunState::Cancelled | RunState::Interrupted => {}
        }
        if record.state != RunState::Failed && record.failure.is_some() {
            return Err("Only failed runs can have a failure category".to_string());
        }
        if record.state.is_active() {
            active_global += 1;
            *active_agents.entry(record.agent_id.as_str()).or_default() += 1;
        }
        if matches!(record.state, RunState::Queued | RunState::QueuedPaused) {
            *queued_agents.entry(record.agent_id.as_str()).or_default() += 1;
        }
    }

    if active_global > MAX_ACTIVE_RUNS {
        return Err("Run store exceeds the global active limit".to_string());
    }
    if active_agents.values().any(|count| *count > 1) {
        return Err("Run store exceeds the per-Agent active limit".to_string());
    }
    if queued_agents
        .values()
        .any(|count| *count > MAX_QUEUED_PER_AGENT)
    {
        return Err("Run store exceeds the per-Agent queue limit".to_string());
    }
    Ok(())
}

pub(crate) fn valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn write_data(path: &Path, data: &RunData) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid run store path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create run store directory: {error}"))?;
    let mut bytes = serde_json::to_vec_pretty(data)
        .map_err(|error| format!("Cannot encode run store: {error}"))?;
    bytes.push(b'\n');
    memory::atomic_write(path, &bytes).map_err(|error| format!("Cannot write run store: {error}"))
}

/// One in-memory projection of a Run transition. The answer exists only here,
/// never in `runs.json`.
#[derive(Clone, Debug)]
pub struct RuntimeEvent {
    pub run_id: u64,
    pub agent_id: String,
    pub revision: u64,
    pub seq: u64,
    pub kind: RuntimeEventKind,
}

#[derive(Clone, Debug)]
pub enum RuntimeEventKind {
    Started,
    Completed {
        answer: String,
    },
    Failed {
        category: FailureKind,
        detail: String,
    },
    Cancelled,
    StorageFailed,
}

/// The core-owned Run/Queue authority.
///
/// Admission, claim, dispatch, settlement, recovery, and cancellation all pass
/// through this type. Channels only project `RuntimeEvent` values.
pub struct Core {
    store: RunStore,
    idempotency: IdempotencyStore,
    delivery: DeliveryStore,
    registry: Arc<RuntimeRegistry>,
    supervisor: RunSupervisor,
    events: broadcast::Sender<RuntimeEvent>,
    admission: Mutex<Admission>,
    /// Present when this process owns the data directory. Held until the Core
    /// and every run handle holding an `Arc<Core>` are dropped.
    _dispatcher: Option<DispatcherLock>,
    #[cfg(test)]
    preparation_gate: Option<Arc<PreparationGate>>,
}

#[cfg(test)]
#[derive(Default)]
struct PreparationGate {
    arrived: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[derive(Default)]
struct Admission {
    sequences: HashMap<u64, EventSequence>,
    poisoned: Option<String>,
}

struct EventSequence {
    value: u64,
    terminal: bool,
}

impl Core {
    /// Production constructor. The dispatcher lock must already be held, so
    /// this constructor is the only path that performs RunStore recovery for a
    /// live data directory.
    pub fn open(
        registry: RuntimeRegistry,
        dispatcher: DispatcherLock,
    ) -> Result<Arc<Self>, String> {
        Self::build(registry, Some(dispatcher))
    }

    #[cfg(test)]
    pub fn new(registry: RuntimeRegistry) -> Result<Arc<Self>, String> {
        Self::build(registry, None)
    }

    fn build(
        registry: RuntimeRegistry,
        dispatcher: Option<DispatcherLock>,
    ) -> Result<Arc<Self>, String> {
        let store_path = registry.main().paths.memory.join(RUN_STORE_FILE);
        let known: HashSet<String> = registry
            .slots()
            .iter()
            .map(|slot| slot.id.clone())
            .collect();
        let store = RunStore::load_checked(&store_path, &known);
        if let Some(error) = store.error() {
            return Err(error);
        }
        // A missing or invalid dedupe index poisons headless submit only. A
        // stored client key is never spent on a Run that was not created.
        let idempotency =
            IdempotencyStore::open(&registry.main().paths.memory.join(IDEMPOTENCY_STORE_FILE));
        // A missing delivery store is empty and creates nothing. An invalid
        // store poisons delivery only; Run and conversation startup continue.
        let delivery = DeliveryStore::open(&registry.main().paths.memory.join(DELIVERY_STORE_FILE));
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        logging::append(&format!("core_ready agents={}", known.len()));
        Ok(Arc::new(Self {
            store,
            idempotency,
            delivery,
            registry: Arc::new(registry),
            supervisor: RunSupervisor::default(),
            events,
            admission: Mutex::new(Admission::default()),
            _dispatcher: dispatcher,
            #[cfg(test)]
            preparation_gate: None,
        }))
    }

    pub fn registry(&self) -> &RuntimeRegistry {
        &self.registry
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }

    pub fn submit(self: &Arc<Self>, agent_id: &str, input: &str) -> Result<RunTicket, RunError> {
        self.enqueue_and_dispatch(agent_id, input)
    }

    /// One idempotent admission for the headless submit surface. It reuses the
    /// same Core queue and dispatch path as `submit`; only the acknowledgement
    /// differs (a post-admission dispatch failure does not unmake the accepted
    /// `queued` Run).
    ///
    /// Dedupe state is Core-owned, persisted before the Run, and bounded. The
    /// admission lock makes exactly one concurrent writer win for one key; every
    /// other caller observes the already persisted Run id.
    pub(crate) fn admit_idempotent(
        self: &Arc<Self>,
        agent_id: &str,
        input: &str,
        idempotency_key: &str,
    ) -> Result<SubmitOutcome, RunError> {
        if self.registry.slot(agent_id).is_none() {
            return Err(RunError::UnknownAgent(agent_id.to_string()));
        }
        if self.registry.runtime(agent_id).is_none() {
            return Err(RunError::AgentUnavailable(agent_id.to_string()));
        }

        let mut admission = self.lock_admission()?;
        Self::ensure_admission_open(&admission)?;

        match self.idempotency.lookup(idempotency_key) {
            Ok(Some(run_id)) => {
                match self.store.run_target(run_id) {
                    Ok(Some(target)) => {
                        logging::append(&format!(
                            "headless_submit id={run_id} deduplicated=true state={:?}",
                            target.state
                        ));
                        return Ok(SubmitOutcome {
                            run_id,
                            state: target.state,
                            deduplicated: true,
                        });
                    }
                    // The Run left the bounded Run store: the key is free again.
                    Ok(None) => {
                        if let Err(error) = self.idempotency.remove(idempotency_key) {
                            logging::append(&format!("idempotency_store_error {error}"));
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(None) => {}
            Err(error) => return Err(RunError::Store(error)),
        }

        // Reserve the next Run id in the durable dedupe index before the Run is
        // written. A crash between the two writes leaves a key that points to no
        // Run; the next submit treats it as a miss and reuses the id.
        let reserved_id = self.store.next_id()?;
        if let Err(error) = self.idempotency.insert(idempotency_key, reserved_id) {
            return Err(RunError::Store(error));
        }

        let ticket = match self.store.enqueue(agent_id, input) {
            Ok(ticket) => ticket,
            Err(error) => {
                // No Run was persisted, so release the reservation instead of
                // spending the client key on nothing.
                if let Err(remove_error) = self.idempotency.remove(idempotency_key) {
                    logging::append(&format!("idempotency_store_error {remove_error}"));
                }
                return Err(error);
            }
        };
        if ticket.id != reserved_id {
            // Unreachable while every enqueue holds the admission lock; keep the
            // mapping on the Run that actually exists if it ever changes.
            logging::append(&format!(
                "idempotency_id_mismatch reserved={reserved_id} actual={}",
                ticket.id
            ));
            if let Err(error) = self.idempotency.insert(idempotency_key, ticket.id) {
                logging::append(&format!("idempotency_store_error {error}"));
            }
        }
        logging::append(&format!(
            "headless_submit id={} deduplicated=false state=Queued",
            ticket.id
        ));
        if let Err(error) = self.dispatch_locked(&mut admission) {
            logging::append(&format!("run_dispatch_error id={} err={error}", ticket.id));
        }
        Ok(SubmitOutcome {
            run_id: ticket.id,
            state: RunState::Queued,
            deduplicated: false,
        })
    }

    fn enqueue_and_dispatch(
        self: &Arc<Self>,
        agent_id: &str,
        input: &str,
    ) -> Result<RunTicket, RunError> {
        if self.registry.slot(agent_id).is_none() {
            return Err(RunError::UnknownAgent(agent_id.to_string()));
        }
        if self.registry.runtime(agent_id).is_none() {
            return Err(RunError::AgentUnavailable(agent_id.to_string()));
        }

        let mut admission = self.lock_admission()?;
        Self::ensure_admission_open(&admission)?;
        let ticket = self.store.enqueue(agent_id, input)?;
        if let Err(error) = self.dispatch_locked(&mut admission) {
            logging::append(&format!("run_dispatch_error id={} err={error}", ticket.id));
            return Err(error);
        }
        Ok(ticket)
    }

    pub fn cancel_agent(self: &Arc<Self>, agent_id: &str) -> Result<CancelReport, RunError> {
        if self.registry.slot(agent_id).is_none() {
            return Err(RunError::UnknownAgent(agent_id.to_string()));
        }

        // Hold admission across the store write, Session stop, abort, and event
        // emission so no finishing task can interleave a late terminal event.
        let mut admission = match self.lock_admission() {
            Ok(admission) => admission,
            Err(error) => {
                let _ = self.stop_agent_execution(agent_id);
                return Err(error);
            }
        };
        let affected = match self.store.unfinished_for_agent(agent_id) {
            Ok(affected) => affected,
            Err(error) => {
                let _ = self.stop_agent_execution(agent_id);
                admission.poisoned = Some(error.to_string());
                return Err(error);
            }
        };
        let cancelled = match self.store.cancel_agent(agent_id) {
            Ok(cancelled) => cancelled,
            Err(error) => {
                logging::append(&format!(
                    "run_store_error cancel agent={agent_id} err={error}"
                ));
                let _ = self.stop_agent_execution(agent_id);
                admission.poisoned = Some(error.to_string());
                for run in affected {
                    self.emit_locked(
                        &mut admission,
                        run.id,
                        agent_id,
                        run.revision,
                        RuntimeEventKind::StorageFailed,
                    );
                }
                return Err(error);
            }
        };

        if let Err(error) = self.stop_agent_execution(agent_id) {
            admission.poisoned = Some(error.to_string());
            for run in cancelled {
                self.emit_locked(
                    &mut admission,
                    run.id,
                    agent_id,
                    run.revision,
                    RuntimeEventKind::StorageFailed,
                );
            }
            return Err(error);
        }

        let report = CancelReport {
            cancelled: cancelled.len(),
            active: cancelled.iter().filter(|run| run.was_active).count(),
        };
        for run in &cancelled {
            self.emit_locked(
                &mut admission,
                run.id,
                agent_id,
                run.revision,
                RuntimeEventKind::Cancelled,
            );
        }
        if let Err(error) = self.dispatch_locked(&mut admission) {
            logging::append(&format!("run_dispatch_error err={error}"));
        }
        Ok(report)
    }

    pub fn resume_agent(self: &Arc<Self>, agent_id: &str) -> Result<Vec<RunTicket>, RunError> {
        if self.registry.slot(agent_id).is_none() {
            return Err(RunError::UnknownAgent(agent_id.to_string()));
        }
        let mut admission = self.lock_admission()?;
        Self::ensure_admission_open(&admission)?;
        let resumed = self.store.resume_agent(agent_id)?;
        if let Err(error) = self.dispatch_locked(&mut admission) {
            logging::append(&format!("run_dispatch_error err={error}"));
            return Err(error);
        }
        Ok(resumed)
    }

    pub fn queue_status(&self, agent_id: &str) -> Result<RunQueueStatus, RunError> {
        self.store.queue_status(agent_id)
    }

    pub fn run_state(&self, id: u64) -> Result<Option<RunState>, RunError> {
        self.store.run_state(id)
    }

    /// Read-only Run identity lookup for delivery targets.
    pub fn run_target(&self, id: u64) -> Result<Option<RunTarget>, RunError> {
        self.store.run_target(id)
    }

    /// Core-owned delivery authority. Channels use it, but do not own it.
    pub fn delivery(&self) -> &DeliveryStore {
        &self.delivery
    }

    /// Creates the delivery identity for a terminal Run. The Agent reference is
    /// taken from the Run record, never from the caller.
    pub fn create_delivery(
        &self,
        run_id: u64,
        channel: DeliveryChannel,
        target: &str,
        chunk_count: u32,
    ) -> Result<DeliveryTicket, DeliveryError> {
        let run = self
            .store
            .run_target(run_id)
            .map_err(|error| DeliveryError::Store(error.to_string()))?
            .ok_or(DeliveryError::UnknownRun(run_id))?;
        if !run.state.is_terminal() {
            return Err(DeliveryError::InvalidTransition(format!(
                "Run {run_id} is not terminal"
            )));
        }
        self.delivery
            .create(run_id, &run.agent_id, channel, target, chunk_count)
    }

    /// Read-only durable terminal projection for channel recovery.
    ///
    /// Returns `None` while the Run is unknown or not terminal. A completed Run
    /// yields its stored redacted Session answer (`None` when that projection is
    /// missing); failed Runs yield only the structured category. This performs
    /// no Run, model, or tool work.
    pub fn terminal_outcome(&self, run_id: u64) -> Result<Option<TerminalOutcome>, String> {
        let Some(run) = self
            .store
            .run_target(run_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        match run.state {
            RunState::Completed => {
                let answer = self.completed_answer(run_id)?;
                Ok(Some(TerminalOutcome::Completed { answer }))
            }
            RunState::Failed => Ok(Some(TerminalOutcome::Failed {
                category: run.failure.unwrap_or(FailureKind::Other),
            })),
            RunState::Cancelled => Ok(Some(TerminalOutcome::Cancelled)),
            RunState::Interrupted => Ok(Some(TerminalOutcome::Interrupted)),
            _ => Ok(None),
        }
    }

    /// Returns a completed Run's stored redacted answer. Failed, stopped,
    /// interrupted, active, and unknown Runs return `None`; no model or tool
    /// call is involved.
    pub fn completed_answer(&self, run_id: u64) -> Result<Option<String>, String> {
        let Some(run) = self
            .store
            .run_target(run_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        if run.state != RunState::Completed {
            return Ok(None);
        }
        let Some(work_id) = run.session_work_id else {
            return Ok(None);
        };
        let runtime = self
            .registry
            .runtime(&run.agent_id)
            .ok_or_else(|| format!("Agent '{}' is unavailable", run.agent_id))?;
        runtime.session.completed_answer(work_id)
    }

    #[cfg(test)]
    pub fn active_handles(&self) -> usize {
        self.supervisor.active_count()
    }

    /// Test-only equivalent of an external process kill: abort every in-flight
    /// execution without settling a Run, leaving the durable state to be
    /// recovered by a fresh store load.
    #[cfg(test)]
    pub fn abandon_active_runs_for_test(&self) {
        for handle in self.supervisor.take_all() {
            handle.abort();
        }
    }

    fn lock_admission(&self) -> Result<std::sync::MutexGuard<'_, Admission>, RunError> {
        self.admission
            .lock()
            .map_err(|_| RunError::Store("Core admission lock is unavailable".to_string()))
    }

    fn ensure_admission_open(admission: &Admission) -> Result<(), RunError> {
        match &admission.poisoned {
            Some(error) => Err(RunError::Store(error.clone())),
            None => Ok(()),
        }
    }

    fn dispatch_locked(self: &Arc<Self>, admission: &mut Admission) -> Result<(), RunError> {
        Self::ensure_admission_open(admission)?;
        let eligible: HashSet<String> = self
            .registry
            .slots()
            .iter()
            .filter(|slot| slot.is_available())
            .map(|slot| slot.id.clone())
            .collect();

        loop {
            let claim = self.store.claim(&eligible, MAX_ACTIVE_RUNS)?;
            let Some(claim) = claim else {
                return Ok(());
            };
            let run_id = claim.id;
            let agent_id = claim.agent_id.clone();
            let core = Arc::clone(self);
            let handle = tokio::spawn(async move {
                core.execute(claim).await;
            });
            self.supervisor.insert(run_id, &agent_id, handle);
        }
    }

    async fn execute(self: Arc<Self>, claim: Claim) {
        let run_id = claim.id;
        let agent_id = claim.agent_id.clone();
        let Some(runtime) = self.registry.runtime(&agent_id) else {
            self.settle_failure(
                run_id,
                &agent_id,
                claim.revision,
                FailureKind::Other,
                "Agent runtime is unavailable",
            );
            return;
        };

        let (system_prompt, _) = prompt::turn_prompt(&runtime.identity, &runtime.paths.memory);
        #[cfg(test)]
        if let Some(gate) = &self.preparation_gate {
            gate.arrived.notify_one();
            gate.resume.notified().await;
        }
        let (revision, mut work, history, omitted) = {
            let mut admission = match self.lock_admission() {
                Ok(admission) => admission,
                Err(error) => {
                    logging::append(&format!("run_admission_error id={run_id} err={error}"));
                    return;
                }
            };
            if let Err(error) = self.store.validate_claim(run_id, claim.revision) {
                if !matches!(error, RunError::Stale(_)) {
                    admission.poisoned = Some(error.to_string());
                    self.emit_locked(
                        &mut admission,
                        run_id,
                        &agent_id,
                        claim.revision,
                        RuntimeEventKind::StorageFailed,
                    );
                }
                self.supervisor.remove(run_id);
                return;
            }
            let mut work = match runtime.session.begin(&claim.input) {
                Ok(work) => work,
                Err(error) => {
                    logging::append(&format!("run_session_begin_error id={run_id} err={error}"));
                    drop(admission);
                    self.settle_failure(
                        run_id,
                        &agent_id,
                        claim.revision,
                        FailureKind::Other,
                        &error,
                    );
                    return;
                }
            };
            let session_work_id = work.id();
            let history = std::mem::take(&mut work.history);
            let omitted = work.omitted;

            let revision = match self.store.started(run_id, claim.revision, session_work_id) {
                Ok(revision) => {
                    logging::append(&format!(
                        "run_started id={run_id} agent={agent_id} session_work={session_work_id}"
                    ));
                    self.emit_locked(
                        &mut admission,
                        run_id,
                        &agent_id,
                        revision,
                        RuntimeEventKind::Started,
                    );
                    revision
                }
                Err(error) => {
                    logging::append(&format!("run_store_error id={run_id} err={error}"));
                    if !matches!(error, RunError::Stale(_)) {
                        admission.poisoned = Some(error.to_string());
                        self.emit_locked(
                            &mut admission,
                            run_id,
                            &agent_id,
                            claim.revision,
                            RuntimeEventKind::StorageFailed,
                        );
                    }
                    drop(work);
                    self.supervisor.remove(run_id);
                    return;
                }
            };
            (revision, work, history, omitted)
        };

        let result = loop_::run_work(
            runtime,
            run_id,
            history,
            omitted,
            &system_prompt,
            &claim.input,
        )
        .await;

        let (state_after, failure, answer, detail) = match result {
            Ok(answer) => match work.complete(&answer) {
                Ok(()) => (RunState::Completed, None, Some(answer), String::new()),
                Err(error) => {
                    logging::append(&format!("session_error {error}"));
                    (RunState::Failed, Some(FailureKind::Other), None, error)
                }
            },
            Err(error) => {
                let category = error.failure_kind();
                if let Err(session_error) = work.fail(category) {
                    logging::append(&format!("session_error {session_error}"));
                }
                (RunState::Failed, Some(category), None, error.to_string())
            }
        };

        let mut admission = match self.lock_admission() {
            Ok(admission) => admission,
            Err(error) => {
                logging::append(&format!("run_admission_error id={run_id} err={error}"));
                self.supervisor.remove(run_id);
                return;
            }
        };
        let event = match self.store.settle(run_id, revision, state_after, failure) {
            Ok(revision) => match (state_after, answer, failure) {
                (RunState::Completed, Some(answer), _) => {
                    Some((revision, RuntimeEventKind::Completed { answer }))
                }
                (RunState::Failed, _, Some(category)) => {
                    Some((revision, RuntimeEventKind::Failed { category, detail }))
                }
                _ => None,
            },
            Err(RunError::Stale(_)) => {
                logging::append(&format!("run_late_event id={run_id}"));
                None
            }
            Err(error) => {
                logging::append(&format!("run_store_error id={run_id} err={error}"));
                admission.poisoned = Some(error.to_string());
                Some((revision, RuntimeEventKind::StorageFailed))
            }
        };
        if let Some((revision, kind)) = event {
            self.emit_locked(&mut admission, run_id, &agent_id, revision, kind);
        }
        self.supervisor.remove(run_id);
        if let Err(error) = self.dispatch_locked(&mut admission) {
            logging::append(&format!("run_dispatch_error err={error}"));
        }
    }

    fn settle_failure(
        self: &Arc<Self>,
        run_id: u64,
        agent_id: &str,
        revision: u64,
        category: FailureKind,
        detail: &str,
    ) {
        let Ok(mut admission) = self.lock_admission() else {
            return;
        };
        let event = match self
            .store
            .settle(run_id, revision, RunState::Failed, Some(category))
        {
            Ok(revision) => Some((
                revision,
                RuntimeEventKind::Failed {
                    category,
                    detail: detail.to_string(),
                },
            )),
            Err(RunError::Stale(_)) => {
                logging::append(&format!("run_late_event id={run_id}"));
                None
            }
            Err(error) => {
                logging::append(&format!("run_store_error id={run_id} err={error}"));
                admission.poisoned = Some(error.to_string());
                Some((revision, RuntimeEventKind::StorageFailed))
            }
        };
        if let Some((revision, kind)) = event {
            self.emit_locked(&mut admission, run_id, agent_id, revision, kind);
        }
        self.supervisor.remove(run_id);
        if let Err(error) = self.dispatch_locked(&mut admission) {
            logging::append(&format!("run_dispatch_error err={error}"));
        }
    }

    fn stop_agent_execution(&self, agent_id: &str) -> Result<(), RunError> {
        let session_result = if let Some(runtime) = self.registry.runtime(agent_id) {
            runtime.session.stop_current().map(|_| ())
        } else {
            Ok(())
        };
        for handle in self.supervisor.take_agent(agent_id) {
            handle.abort();
        }
        logging::append(&format!("run_stop_agent agent={agent_id}"));
        session_result.map_err(|error| {
            logging::append(&format!("session_error {error}"));
            RunError::Store(format!("Session stop error: {error}"))
        })
    }

    fn emit_locked(
        &self,
        admission: &mut Admission,
        run_id: u64,
        agent_id: &str,
        revision: u64,
        kind: RuntimeEventKind,
    ) {
        let terminal = !matches!(kind, RuntimeEventKind::Started);
        let sequence = admission.sequences.entry(run_id).or_insert(EventSequence {
            value: 0,
            terminal: false,
        });
        sequence.value += 1;
        sequence.terminal = terminal;
        let event = RuntimeEvent {
            run_id,
            agent_id: agent_id.to_string(),
            revision,
            seq: sequence.value,
            kind,
        };
        let _ = self.events.send(event);

        if admission.sequences.len() > 256 {
            let mut ids: Vec<u64> = admission
                .sequences
                .iter()
                .filter(|(_, sequence)| sequence.terminal)
                .map(|(id, _)| *id)
                .collect();
            ids.sort_unstable();
            let drop_count = admission.sequences.len().saturating_sub(256);
            for id in ids.into_iter().take(drop_count) {
                admission.sequences.remove(&id);
            }
        }
    }

    #[cfg(test)]
    fn record(&self, id: u64) -> Result<RunRecord, RunError> {
        self.store.record(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Condvar;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tokio::sync::broadcast;

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("iskra-run-{name}-{}-{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn agents(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    fn enqueue(store: &RunStore, agent: &str, input: &str) -> RunTicket {
        store.enqueue(agent, input).unwrap()
    }

    fn claim(store: &RunStore, ids: &[&str]) -> Claim {
        store.claim(&agents(ids), MAX_ACTIVE_RUNS).unwrap().unwrap()
    }

    fn start_claim(store: &RunStore, claim: &Claim, session_work: u64) -> u64 {
        store
            .started(claim.id, claim.revision, session_work)
            .unwrap()
    }

    #[derive(Default)]
    struct ServerState {
        posts: Vec<String>,
        active: usize,
        max_active: usize,
    }

    struct MockModel {
        address: String,
        state: Arc<(Mutex<ServerState>, Condvar)>,
        stop: std::sync::mpsc::Sender<()>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl MockModel {
        fn start(answer: &str, delay: Duration) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let state = Arc::new((Mutex::new(ServerState::default()), Condvar::new()));
            let server_state = Arc::clone(&state);
            let answer = answer.to_string();
            let (stop, stopped) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                while stopped.try_recv().is_err() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let state = Arc::clone(&server_state);
                            let answer = answer.clone();
                            std::thread::spawn(move || {
                                handle_model_connection(stream, &answer, delay, &state);
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("mock model accept failed: {error}"),
                    }
                }
            });
            Self {
                address,
                state,
                stop,
                handle: Some(handle),
            }
        }

        fn url(&self) -> String {
            format!("http://{}/chat/completions", self.address)
        }

        fn posts(&self) -> Vec<String> {
            self.state.0.lock().unwrap().posts.clone()
        }

        fn wait_for_posts(&self, count: usize, timeout: Duration) -> bool {
            let deadline = Instant::now() + timeout;
            let mut state = self.state.0.lock().unwrap();
            while state.posts.len() < count {
                let now = Instant::now();
                if now >= deadline {
                    return false;
                }
                let (next, _) = self.state.1.wait_timeout(state, deadline - now).unwrap();
                state = next;
            }
            true
        }

        fn max_active(&self) -> usize {
            self.state.0.lock().unwrap().max_active
        }
    }

    impl Drop for MockModel {
        fn drop(&mut self) {
            let _ = self.stop.send(());
            let _ = std::net::TcpStream::connect(&self.address);
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    fn handle_model_connection(
        stream: std::net::TcpStream,
        answer: &str,
        delay: Duration,
        state: &Arc<(Mutex<ServerState>, Condvar)>,
    ) {
        stream.set_nonblocking(false).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        let mut content_length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).is_err() {
                return;
            }
            if header == "\r\n" || header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; content_length];
        let _ = reader.read_exact(&mut body);

        let response = if request_line.starts_with("POST ") {
            {
                let mut current = state.0.lock().unwrap();
                current
                    .posts
                    .push(String::from_utf8_lossy(&body).to_string());
                current.active += 1;
                current.max_active = current.max_active.max(current.active);
                state.1.notify_all();
            }
            std::thread::sleep(delay);
            let mut current = state.0.lock().unwrap();
            current.active -= 1;
            state.1.notify_all();
            json!({
                "choices": [{"message": {"role": "assistant", "content": answer}}],
                "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
            })
        } else {
            json!({"is_available": true, "balance_infos": []})
        };
        let response = serde_json::to_vec(&response).unwrap();
        let _ = write!(
            writer,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        );
        let _ = writer.write_all(&response);
    }

    fn main_registry(dir: &Path, url: &str) -> RuntimeRegistry {
        let url = url.to_string();
        RuntimeRegistry::load(dir, move |key| match key {
            "LLM_API_KEY" => Some("dummy-main-key".to_string()),
            "LLM_BASE_URL" => Some(url.clone()),
            "LLM_MODEL" => Some("main-test-model".to_string()),
            _ => None,
        })
        .unwrap()
    }

    fn two_agent_registry(dir: &Path, url: &str) -> RuntimeRegistry {
        let registry = json!({
            "version": 1,
            "engines": [
                {
                    "id": "main-engine",
                    "kind": "openai_compatible",
                    "base_url": url,
                    "model": "main-test-model",
                    "api_key_env": "MAIN_KEY"
                },
                {
                    "id": "helper-engine",
                    "kind": "openai_compatible",
                    "base_url": url,
                    "model": "helper-test-model",
                    "api_key_env": "HELPER_KEY"
                }
            ],
            "agents": [
                {
                    "id": "main",
                    "display_name": "Iskra",
                    "engine_id": "main-engine",
                    "role": "Personal assistant"
                },
                {
                    "id": "helper",
                    "display_name": "Helper",
                    "engine_id": "helper-engine",
                    "role": "Test helper"
                }
            ]
        });
        fs::write(
            dir.join(crate::registry::REGISTRY_FILE),
            registry.to_string(),
        )
        .unwrap();
        RuntimeRegistry::load(dir, |key| match key {
            "MAIN_KEY" => Some("dummy-main-key".to_string()),
            "HELPER_KEY" => Some("dummy-helper-key".to_string()),
            _ => None,
        })
        .unwrap()
    }

    fn write_store(dir: &Path, value: serde_json::Value) -> PathBuf {
        let path = dir.join("memory/runs.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        path
    }

    async fn await_terminal(
        events: &mut broadcast::Receiver<RuntimeEvent>,
        run_id: u64,
    ) -> RuntimeEventKind {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.expect("runtime event");
                if event.run_id != run_id {
                    continue;
                }
                match event.kind {
                    RuntimeEventKind::Started => {}
                    kind => break kind,
                }
            }
        })
        .await
        .expect("terminal event")
    }

    #[test]
    fn enqueue_redacts_and_persists_before_returning() {
        let dir = temp_dir("enqueue");
        let path = dir.join("memory/runs.json");
        let store = RunStore::load(&path);
        let secret = format!("sk-{}", "test1234567890");

        let ticket = enqueue(&store, "main", &format!("my key is {secret}"));
        assert_eq!(ticket.id, 1);
        assert!(path.exists());

        let stored = fs::read_to_string(&path).unwrap();
        assert!(!stored.contains(&secret));
        assert!(stored.contains("redacted"));
        let record = store.record(1).unwrap();
        assert_eq!(record.state, RunState::Queued);
        assert_eq!(record.agent_id, "main");
        assert_eq!(record.revision, 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_target_exposes_the_agent_and_session_reference() {
        let dir = temp_dir("run-target");
        let store = RunStore::load(&dir.join("memory/runs.json"));
        assert_eq!(store.run_target(99).unwrap(), None);

        let ticket = enqueue(&store, "main", "target me");
        assert_eq!(
            store.run_target(ticket.id).unwrap().unwrap(),
            RunTarget {
                id: ticket.id,
                agent_id: "main".to_string(),
                state: RunState::Queued,
                failure: None,
                session_work_id: None,
                revision: 1,
            }
        );

        let claim = claim(&store, &["main"]);
        let target = store.run_target(ticket.id).unwrap().unwrap();
        assert_eq!(target.state, RunState::Preparing);
        assert_eq!(target.revision, claim.revision);

        let revision = start_claim(&store, &claim, 7);
        let target = store.run_target(ticket.id).unwrap().unwrap();
        assert_eq!(target.state, RunState::Running);
        assert_eq!(target.session_work_id, Some(7));
        assert_eq!(target.revision, revision);

        let revision = store
            .settle(ticket.id, revision, RunState::Completed, None)
            .unwrap();
        let target = store.run_target(ticket.id).unwrap().unwrap();
        assert_eq!(target.state, RunState::Completed);
        assert_eq!(target.session_work_id, Some(7));
        assert_eq!(target.revision, revision);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_and_oversized_inputs_are_refused_without_records() {
        let dir = temp_dir("limits");
        let store = RunStore::load(&dir.join("memory/runs.json"));

        assert_eq!(store.enqueue("main", "   "), Err(RunError::EmptyInput));
        assert_eq!(
            store.enqueue("main", &"x".repeat(MAX_INPUT_CHARS + 1)),
            Err(RunError::InputTooLong)
        );
        assert!(store.records().unwrap().is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn per_agent_queue_is_bounded_at_twenty() {
        let dir = temp_dir("queue-bound");
        let store = RunStore::load(&dir.join("memory/runs.json"));
        for index in 0..MAX_QUEUED_PER_AGENT {
            enqueue(&store, "main", &format!("queued {index}"));
        }
        assert_eq!(
            store.enqueue("main", "one too many"),
            Err(RunError::QueueFull)
        );
        assert_eq!(store.records().unwrap().len(), MAX_QUEUED_PER_AGENT);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn claim_is_per_agent_fifo_and_global_two() {
        let dir = temp_dir("claim");
        let store = RunStore::load(&dir.join("memory/runs.json"));
        enqueue(&store, "main", "alpha");
        enqueue(&store, "main", "beta");
        enqueue(&store, "helper", "gamma");
        enqueue(&store, "helper", "delta");

        let first = claim(&store, &["main", "helper"]);
        assert_eq!(first.id, 1);
        let second = claim(&store, &["main", "helper"]);
        assert_eq!(second.id, 3);
        assert!(
            store
                .claim(&agents(&["main", "helper"]), MAX_ACTIVE_RUNS)
                .unwrap()
                .is_none()
        );

        let first_revision = start_claim(&store, &first, 1);
        let second_revision = start_claim(&store, &second, 2);
        store
            .settle(1, first_revision, RunState::Completed, None)
            .unwrap();

        let third = claim(&store, &["main", "helper"]);
        assert_eq!(third.id, 2, "the same Agent's later work must not overtake");
        assert!(
            store
                .claim(&agents(&["main", "helper"]), MAX_ACTIVE_RUNS)
                .unwrap()
                .is_none()
        );

        let third_revision = start_claim(&store, &third, 3);
        store
            .settle(3, second_revision, RunState::Completed, None)
            .unwrap();
        let fourth = claim(&store, &["main", "helper"]);
        assert_eq!(fourth.id, 4);

        store
            .settle(2, third_revision, RunState::Completed, None)
            .unwrap();
        let fourth_revision = start_claim(&store, &fourth, 4);
        store
            .settle(4, fourth_revision, RunState::Completed, None)
            .unwrap();

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restart_pauses_queued_work_and_requires_an_explicit_resume() {
        let dir = temp_dir("paused");
        let store = RunStore::load(&dir.join("memory/runs.json"));
        enqueue(&store, "main", "first");
        enqueue(&store, "main", "second");
        drop(store);

        let reloaded = RunStore::load(&dir.join("memory/runs.json"));
        assert_eq!(reloaded.queue_status("main").unwrap().paused, 2);
        assert!(
            reloaded
                .claim(&agents(&["main"]), MAX_ACTIVE_RUNS)
                .unwrap()
                .is_none(),
            "paused work must not dispatch automatically"
        );

        assert_eq!(reloaded.resume_agent("main").unwrap().len(), 2);
        let first = claim(&reloaded, &["main"]);
        assert_eq!(first.id, 1, "resume keeps the original order");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn startup_recovery_maps_queued_to_paused_and_active_to_interrupted() {
        let dir = temp_dir("recovery");
        let path = write_store(
            &dir,
            json!({
                "version": 1,
                "next_run_id": 4,
                "runs": [
                    {
                        "id": 1, "agent_id": "main", "input": "was running",
                        "state": "running", "session_work_id": 7, "revision": 2
                    },
                    {
                        "id": 2, "agent_id": "helper", "input": "was queued",
                        "state": "queued", "revision": 1
                    },
                    {
                        "id": 3, "agent_id": "main", "input": "also queued",
                        "state": "queued", "revision": 1
                    }
                ]
            }),
        );

        let store = RunStore::load(&path);
        assert!(store.error().is_none());
        assert_eq!(store.record(1).unwrap().state, RunState::Interrupted);
        assert_eq!(store.record(2).unwrap().state, RunState::QueuedPaused);
        assert_eq!(store.record(3).unwrap().state, RunState::QueuedPaused);
        assert_eq!(store.queue_status("main").unwrap().interrupted, 1);
        assert_eq!(store.queue_status("main").unwrap().paused, 1);
        assert_eq!(store.queue_status("helper").unwrap().paused, 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_covers_active_queued_and_paused_work_for_one_agent() {
        let dir = temp_dir("cancel");
        let path = dir.join("memory/runs.json");
        let store = RunStore::load(&path);
        enqueue(&store, "main", "active");
        enqueue(&store, "main", "queued");
        enqueue(&store, "helper", "other");
        let active = claim(&store, &["main", "helper"]);
        let active_revision = start_claim(&store, &active, 1);

        let cancelled = store.cancel_agent("main").unwrap();
        assert_eq!(cancelled.len(), 2);
        assert!(cancelled.iter().any(|run| run.was_active));
        assert_eq!(store.record(1).unwrap().state, RunState::Cancelled);
        assert_eq!(store.record(2).unwrap().state, RunState::Cancelled);
        assert_eq!(store.record(3).unwrap().state, RunState::Queued);
        assert_eq!(
            store.settle(1, active_revision, RunState::Completed, None),
            Err(RunError::Stale(1)),
            "a late completion cannot reopen a cancelled run"
        );

        let paused_store = RunStore::load(&path);
        assert_eq!(
            paused_store.record(3).unwrap().state,
            RunState::QueuedPaused
        );
        let cancelled = paused_store.cancel_agent("helper").unwrap();
        assert_eq!(cancelled.len(), 1);
        assert_eq!(paused_store.record(3).unwrap().state, RunState::Cancelled);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn concurrent_claims_never_duplicate_or_exceed_the_global_limit() {
        let dir = temp_dir("atomic-claim");
        let store = RunStore::load(&dir.join("memory/runs.json"));
        for index in 0..10 {
            let agent = if index % 2 == 0 { "main" } else { "helper" };
            enqueue(&store, agent, &format!("work {index}"));
        }

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .claim(&agents(&["main", "helper"]), MAX_ACTIVE_RUNS)
                        .unwrap()
                })
            })
            .collect();
        let claims: Vec<Claim> = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect();

        assert!(claims.len() <= MAX_ACTIVE_RUNS);
        let ids: HashSet<u64> = claims.iter().map(|claim| claim.id).collect();
        assert_eq!(ids.len(), claims.len(), "a run was claimed twice");
        let records = store.records().unwrap();
        let active = records
            .iter()
            .filter(|record| record.state.is_active())
            .count();
        assert_eq!(active, claims.len());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn forbidden_transitions_are_rejected() {
        let dir = temp_dir("transitions");
        let store = RunStore::load(&dir.join("memory/runs.json"));
        enqueue(&store, "main", "queued work");
        assert!(
            store.settle(1, 1, RunState::Completed, None).is_err(),
            "queued cannot complete"
        );
        let claim = claim(&store, &["main"]);
        assert!(
            store
                .settle(claim.id, claim.revision, RunState::Completed, None)
                .is_err(),
            "preparing cannot complete before running"
        );
        assert!(
            store.started(claim.id, claim.revision + 1, 1).is_err(),
            "a stale revision cannot start a run"
        );
        let revision = start_claim(&store, &claim, 1);
        assert!(
            store.started(claim.id, revision, 2).is_err(),
            "a running run cannot start twice"
        );
        store
            .settle(claim.id, revision, RunState::Completed, None)
            .unwrap();
        assert_eq!(store.record(claim.id).unwrap().state, RunState::Completed);
        assert!(
            store
                .settle(
                    claim.id,
                    revision + 1,
                    RunState::Failed,
                    Some(FailureKind::Other)
                )
                .is_err(),
            "a terminal run cannot change state"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn late_started_event_after_cancel_is_rejected() {
        let dir = temp_dir("late-started");
        let store = RunStore::load(&dir.join("memory/runs.json"));
        enqueue(&store, "main", "work");
        let claim = claim(&store, &["main"]);
        store.cancel_agent("main").unwrap();

        assert_eq!(
            store.started(claim.id, claim.revision, 9),
            Err(RunError::Stale(claim.id))
        );
        assert_eq!(store.record(claim.id).unwrap().state, RunState::Cancelled);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn storage_keeps_the_latest_hundred_records() {
        let dir = temp_dir("retention");
        let store = RunStore::load(&dir.join("memory/runs.json"));
        for index in 1..=105 {
            enqueue(&store, "main", &format!("q{index}"));
            let claim = claim(&store, &["main"]);
            let revision = start_claim(&store, &claim, index);
            store
                .settle(claim.id, revision, RunState::Completed, None)
                .unwrap();
        }
        let records = store.records().unwrap();
        assert_eq!(records.len(), MAX_STORED_RUNS);
        assert_eq!(records[0].id, 6);
        assert_eq!(records[99].id, 105);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_store_data_fails_closed_without_rewriting() {
        let dir = temp_dir("invalid");
        let path = dir.join("memory/runs.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let cases = [
            "not json".to_string(),
            json!({"version": 2, "next_run_id": 1, "runs": []}).to_string(),
            json!({"version": 1, "next_run_id": 1, "runs": [], "extra": 1}).to_string(),
            json!({
                "version": 1, "next_run_id": 2,
                "runs": [{
                    "id": 1, "agent_id": "main", "input": "x",
                    "state": "running", "revision": 1
                }]
            })
            .to_string(),
        ];
        for contents in cases {
            fs::write(&path, &contents).unwrap();
            let store = RunStore::load(&path);
            assert!(store.error().is_some());
            assert!(store.enqueue("main", "must not run").is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_failure_poisons_the_store_until_reload() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("poison");
        let memory = dir.join("memory");
        let path = memory.join("runs.json");
        let store = RunStore::load(&path);
        enqueue(&store, "main", "first");

        fs::set_permissions(&memory, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(matches!(
            store.enqueue("main", "second"),
            Err(RunError::Store(_))
        ));
        fs::set_permissions(&memory, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(store.enqueue("main", "third"), Err(RunError::Store(_))),
            "a poisoned store must refuse further writes"
        );

        let reloaded = RunStore::load(&path);
        assert!(reloaded.error().is_none());
        assert_eq!(reloaded.records().unwrap().len(), 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn core_rejects_records_for_an_unknown_agent() {
        let dir = temp_dir("unknown-agent");
        let path = write_store(
            &dir,
            json!({
                "version": 1, "next_run_id": 2,
                "runs": [{
                    "id": 1, "agent_id": "ghost", "input": "hello",
                    "state": "queued", "revision": 1
                }]
            }),
        );
        let original = fs::read_to_string(&path).unwrap();
        let registry = main_registry(&dir, "http://127.0.0.1:9/chat/completions");

        let error = Core::new(registry).err().expect("unknown agent must fail");
        assert!(error.contains("ghost"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn core_startup_never_calls_the_model() {
        let dir = temp_dir("startup");
        let model = MockModel::start("must not arrive", Duration::ZERO);
        write_store(
            &dir,
            json!({
                "version": 1, "next_run_id": 3,
                "runs": [
                    {
                        "id": 1, "agent_id": "main", "input": "was running",
                        "state": "running", "session_work_id": 1, "revision": 2
                    },
                    {
                        "id": 2, "agent_id": "main", "input": "was queued",
                        "state": "queued", "revision": 1
                    }
                ]
            }),
        );
        let registry = main_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(model.posts().len(), 0, "restart must not replay work");
        assert_eq!(core.active_handles(), 0);
        assert_eq!(core.queue_status("main").unwrap().interrupted, 1);
        assert_eq!(core.queue_status("main").unwrap().paused, 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn core_executes_a_queued_run_and_projects_the_answer() {
        let dir = temp_dir("execute");
        let model = MockModel::start("ok answer", Duration::ZERO);
        let registry = main_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();

        let ticket = core.submit("main", "do the work").unwrap();
        let mut seqs = Vec::new();
        let kind = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.unwrap();
                if event.run_id != ticket.id {
                    continue;
                }
                seqs.push(event.seq);
                match event.kind {
                    RuntimeEventKind::Started => {}
                    kind => break kind,
                }
            }
        })
        .await
        .expect("terminal event");
        assert_eq!(seqs, [1, 2], "per-Run sequence numbers must be monotonic");
        match kind {
            RuntimeEventKind::Completed { answer } => assert_eq!(answer, "ok answer"),
            other => panic!("unexpected terminal event: {other:?}"),
        }

        let record = core.record(ticket.id).unwrap();
        assert_eq!(record.state, RunState::Completed);
        assert!(record.session_work_id.is_some());
        assert_eq!(
            core.queue_status("main").unwrap(),
            RunQueueStatus::default()
        );
        assert_eq!(core.registry().main().session.status().unwrap().stored, 1);
        assert_eq!(model.posts().len(), 1);

        let target = core.run_target(ticket.id).unwrap().unwrap();
        assert_eq!(target.agent_id, "main");
        assert_eq!(target.state, RunState::Completed);
        let work_id = target.session_work_id.unwrap();
        assert_eq!(
            core.completed_answer(ticket.id).unwrap().as_deref(),
            Some("ok answer")
        );
        assert_eq!(
            core.registry()
                .main()
                .session
                .completed_answer(work_id)
                .unwrap()
                .as_deref(),
            Some("ok answer")
        );
        assert_eq!(core.completed_answer(999).unwrap(), None);

        let delivery = core
            .create_delivery(ticket.id, DeliveryChannel::Telegram, "12345", 2)
            .unwrap();
        assert_eq!(delivery.revision, 1);
        assert_eq!(
            core.delivery().get(delivery.id).unwrap().unwrap().run_id,
            ticket.id
        );
        assert_eq!(core.delivery().counts().unwrap().records, 1);
        assert_eq!(
            core.create_delivery(999, DeliveryChannel::Telegram, "1", 1),
            Err(DeliveryError::UnknownRun(999))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn terminal_outcome_reads_the_completed_answer_without_a_model_call() {
        let dir = temp_dir("terminal-completed");
        let model = MockModel::start("completed durably", Duration::ZERO);
        let registry = main_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();
        let ticket = core.submit("main", "finish this").unwrap();
        let kind = await_terminal(&mut events, ticket.id).await;
        assert!(matches!(kind, RuntimeEventKind::Completed { .. }));
        let posts = model.posts().len();

        assert_eq!(
            core.terminal_outcome(ticket.id).unwrap(),
            Some(TerminalOutcome::Completed {
                answer: Some("completed durably".to_string())
            })
        );
        assert_eq!(core.terminal_outcome(999).unwrap(), None);
        assert_eq!(
            model.posts().len(),
            posts,
            "reading a durable outcome must not call the model"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn terminal_outcome_projects_failed_cancelled_and_interrupted_records() {
        let dir = temp_dir("terminal-states");
        write_store(
            &dir,
            json!({
                "version": 1, "next_run_id": 5,
                "runs": [
                    {
                        "id": 1, "agent_id": "main", "input": "a",
                        "state": "failed", "failure": "timeout", "revision": 2
                    },
                    {
                        "id": 2, "agent_id": "main", "input": "b",
                        "state": "cancelled", "revision": 1
                    },
                    {
                        "id": 3, "agent_id": "main", "input": "c",
                        "state": "interrupted", "revision": 1
                    },
                    {
                        "id": 4, "agent_id": "main", "input": "d",
                        "state": "completed", "session_work_id": 9, "revision": 1
                    }
                ]
            }),
        );
        let registry = main_registry(&dir, "http://127.0.0.1:9/chat/completions");
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();

        assert_eq!(
            core.terminal_outcome(1).unwrap(),
            Some(TerminalOutcome::Failed {
                category: FailureKind::Timeout
            })
        );
        assert_eq!(
            core.terminal_outcome(2).unwrap(),
            Some(TerminalOutcome::Cancelled)
        );
        assert_eq!(
            core.terminal_outcome(3).unwrap(),
            Some(TerminalOutcome::Interrupted)
        );
        assert_eq!(
            core.terminal_outcome(4).unwrap(),
            Some(TerminalOutcome::Completed { answer: None })
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(200), events.recv())
                .await
                .is_err(),
            "restart must not publish terminal events for old Runs"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn terminal_outcome_is_none_while_a_run_is_still_active() {
        let dir = temp_dir("terminal-active");
        let model = MockModel::start("slow answer", Duration::from_millis(600));
        let registry = main_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();
        let ticket = core.submit("main", "take your time").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while core.run_state(ticket.id).unwrap() != Some(RunState::Running) {
            assert!(Instant::now() < deadline, "the run never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(core.terminal_outcome(ticket.id).unwrap(), None);

        assert!(matches!(
            await_terminal(&mut events, ticket.id).await,
            RuntimeEventKind::Completed { .. }
        ));
        assert_eq!(
            core.terminal_outcome(ticket.id).unwrap(),
            Some(TerminalOutcome::Completed {
                answer: Some("slow answer".to_string())
            })
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_before_begin_creates_no_session_work_or_model_call() {
        let dir = temp_dir("cancel-before-begin");
        let model = MockModel::start("must not arrive", Duration::ZERO);
        let registry = main_registry(&dir, &model.url());
        let mut core = Core::new(registry).unwrap();
        let gate = Arc::new(PreparationGate::default());
        Arc::get_mut(&mut core).unwrap().preparation_gate = Some(Arc::clone(&gate));
        let mut events = core.subscribe();
        let ticket = core.submit("main", "cancel before Session begin").unwrap();
        tokio::time::timeout(Duration::from_secs(5), gate.arrived.notified())
            .await
            .expect("execution never reached the preparation gate");

        let report = core.cancel_agent("main").unwrap();
        assert_eq!(report.cancelled, 1);
        gate.resume.notify_one();

        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("cancel event timed out")
            .unwrap();
        assert_eq!(event.run_id, ticket.id);
        assert_eq!(event.seq, 1);
        assert!(matches!(event.kind, RuntimeEventKind::Cancelled));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(events.try_recv().is_err(), "unexpected post-cancel event");
        assert_eq!(model.posts().len(), 0);
        assert_eq!(core.record(ticket.id).unwrap().state, RunState::Cancelled);
        assert!(!core.supervisor.contains(ticket.id));
        let session_path = dir.join("memory/session.json");
        if let Ok(bytes) = fs::read(session_path) {
            let session: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(session["work"].as_array().unwrap().len(), 0);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_after_begin_projects_stopped_without_late_terminal_events() {
        let dir = temp_dir("cancel-after-begin");
        let model = MockModel::start("must not arrive", Duration::from_millis(300));
        let registry = main_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();
        let ticket = core.submit("main", "cancel after Session begin").unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = events.recv().await.unwrap();
                if event.run_id == ticket.id && matches!(event.kind, RuntimeEventKind::Started) {
                    assert_eq!(event.seq, 1);
                    break;
                }
            }
        })
        .await
        .expect("run never started");
        assert!(model.wait_for_posts(1, Duration::from_secs(5)));

        core.cancel_agent("main").unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("cancel event timed out")
            .unwrap();
        assert_eq!(event.run_id, ticket.id);
        assert_eq!(event.seq, 2);
        assert!(matches!(event.kind, RuntimeEventKind::Cancelled));
        let session: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("memory/session.json")).unwrap()).unwrap();
        assert_eq!(session["work"].as_array().unwrap().len(), 1);
        assert_eq!(session["work"][0]["status"], "stopped");

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(events.try_recv().is_err(), "unexpected post-cancel event");
        assert_eq!(model.posts().len(), 1);
        assert_eq!(core.record(ticket.id).unwrap().state, RunState::Cancelled);
        assert!(!core.supervisor.contains(ticket.id));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn active_run_sequence_survives_terminal_sequence_pruning() {
        let dir = temp_dir("sequence-pruning");
        let registry = main_registry(&dir, "http://127.0.0.1:9/chat/completions");
        let core = Core::new(registry).unwrap();
        let mut admission = core.lock_admission().unwrap();
        core.emit_locked(&mut admission, 1, "main", 2, RuntimeEventKind::Started);
        for id in 2..=300 {
            core.emit_locked(
                &mut admission,
                id,
                "main",
                1,
                RuntimeEventKind::Completed {
                    answer: "done".to_string(),
                },
            );
        }
        assert_eq!(admission.sequences.get(&1).unwrap().value, 1);
        core.emit_locked(
            &mut admission,
            1,
            "main",
            3,
            RuntimeEventKind::Completed {
                answer: "done".to_string(),
            },
        );
        assert_eq!(admission.sequences.get(&1).unwrap().value, 2);
        drop(admission);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn core_resume_dispatches_paused_work_only_when_asked() {
        let dir = temp_dir("resume");
        let model = MockModel::start("resumed answer", Duration::ZERO);
        write_store(
            &dir,
            json!({
                "version": 1, "next_run_id": 2,
                "runs": [{
                    "id": 1, "agent_id": "main", "input": "resume me",
                    "state": "queued", "revision": 1
                }]
            }),
        );
        let registry = main_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(model.posts().len(), 0);
        let resumed = core.resume_agent("main").unwrap();
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].id, 1);

        let kind = await_terminal(&mut events, 1).await;
        assert!(matches!(kind, RuntimeEventKind::Completed { .. }));
        assert_eq!(core.record(1).unwrap().state, RunState::Completed);
        assert_eq!(model.posts().len(), 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_write_failure_stops_work_and_poisons_the_store() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("cancel-poison");
        let model = MockModel::start("must not arrive", Duration::from_secs(2));
        let registry = two_agent_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();
        let ticket = core.submit("main", "stop under failure").unwrap();
        core.submit("helper", "other active work").unwrap();
        assert!(
            model.wait_for_posts(2, Duration::from_secs(5)),
            "runs never reached the model"
        );
        let replacement = core.submit("helper", "replacement must not run").unwrap();

        let memory = dir.join("memory");
        fs::set_permissions(&memory, fs::Permissions::from_mode(0o500)).unwrap();
        let error = core.cancel_agent("main").unwrap_err();
        assert!(matches!(error, RunError::Store(_)));
        fs::set_permissions(&memory, fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            matches!(
                await_terminal(&mut events, ticket.id).await,
                RuntimeEventKind::StorageFailed
            ),
            "failed cancellation must not project a clean cancellation"
        );
        assert!(!core.supervisor.contains(ticket.id));
        assert!(core.queue_status("main").is_err());
        assert!(core.submit("main", "later admission").is_err());
        assert!(core.submit("helper", "later admission").is_err());
        tokio::time::sleep(Duration::from_millis(2200)).await;
        assert_eq!(model.posts().len(), 2, "replacement work was dispatched");

        let reloaded = RunStore::load(&dir.join("memory/runs.json"));
        assert_eq!(
            reloaded.record(ticket.id).unwrap().state,
            RunState::Interrupted
        );
        assert_eq!(
            reloaded.record(replacement.id).unwrap().state,
            RunState::QueuedPaused
        );
        assert!(
            reloaded
                .claim(&agents(&["main", "helper"]), MAX_ACTIVE_RUNS)
                .unwrap()
                .is_none(),
            "a failed cancellation must not replay work"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_stop_failure_aborts_without_clean_cancel_or_replacement() {
        let dir = temp_dir("session-stop-poison");
        let model = MockModel::start("must not replace", Duration::from_secs(2));
        let registry = two_agent_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();
        let ticket = core.submit("main", "Session stop must fail").unwrap();
        core.submit("helper", "other active work").unwrap();
        assert!(model.wait_for_posts(2, Duration::from_secs(5)));
        let replacement = core.submit("helper", "replacement must not run").unwrap();

        let session_path = dir.join("memory/session.json");
        fs::remove_file(&session_path).unwrap();
        fs::create_dir(&session_path).unwrap();
        let error = core.cancel_agent("main").unwrap_err();
        assert!(error.to_string().contains("Session stop error"));

        assert!(matches!(
            await_terminal(&mut events, ticket.id).await,
            RuntimeEventKind::StorageFailed
        ));
        assert!(!core.supervisor.contains(ticket.id));
        assert_eq!(core.record(ticket.id).unwrap().state, RunState::Cancelled);
        assert!(core.submit("main", "later admission").is_err());
        assert!(core.submit("helper", "later admission").is_err());
        tokio::time::sleep(Duration::from_millis(2200)).await;
        assert_eq!(model.posts().len(), 2, "replacement work was dispatched");
        assert_eq!(core.record(replacement.id).unwrap().state, RunState::Queued);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn core_keeps_per_agent_fifo_with_global_concurrency_two() {
        let dir = temp_dir("fifo");
        let model = MockModel::start("done", Duration::from_millis(300));
        let registry = two_agent_registry(&dir, &model.url());
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();

        let alpha = core.submit("main", "alpha work").unwrap();
        let gamma = core.submit("helper", "gamma work").unwrap();
        let beta = core.submit("main", "beta work").unwrap();
        assert!(
            model.wait_for_posts(3, Duration::from_secs(10)),
            "the mock model never received all three requests"
        );

        tokio::time::timeout(Duration::from_secs(10), async {
            let mut completed = 0usize;
            while completed < 3 {
                let event = events.recv().await.unwrap();
                if matches!(event.kind, RuntimeEventKind::Completed { .. }) {
                    completed += 1;
                }
            }
        })
        .await
        .expect("all runs should finish");

        assert_eq!(model.max_active(), 2, "global concurrency must reach two");
        let posts = model.posts();
        assert_eq!(posts.len(), 3);
        assert!(
            posts[0].contains("alpha work") || posts[0].contains("gamma work"),
            "the first two requests must be the two eligible Agent heads"
        );
        assert!(
            posts[1].contains("alpha work") || posts[1].contains("gamma work"),
            "the run behind its Agent head must wait"
        );
        assert!(
            posts[2].contains("beta work"),
            "the same-Agent follow-up must run after its head"
        );

        for ticket in [alpha, gamma, beta] {
            assert_eq!(core.record(ticket.id).unwrap().state, RunState::Completed);
        }
        assert_eq!(
            core.queue_status("main").unwrap(),
            RunQueueStatus::default()
        );
        assert_eq!(
            core.queue_status("helper").unwrap(),
            RunQueueStatus::default()
        );

        fs::remove_dir_all(&dir).unwrap();
    }
}
