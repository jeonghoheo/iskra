use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::brain::Message;
use crate::logging;
use crate::memory;

pub const SESSION_FILE: &str = "session.json";
const VERSION: u32 = 1;
const MAX_STORED_WORK: usize = 50;
const MAX_CONTEXT_WORK: usize = 12;
const MAX_CONTEXT_BYTES: usize = 24 * 1024;
const MAX_TEXT_CHARS: usize = 8_000;
const TRUNCATION_MARKER: &str = "…[truncated]";

#[derive(Clone)]
pub struct Session {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    data: Result<SessionData, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionData {
    version: u32,
    next_id: u64,
    work: Vec<WorkRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkRecord {
    id: u64,
    status: WorkStatus,
    user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<FailureKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Running,
    Completed,
    Failed,
    Stopped,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Timeout,
    Connection,
    Auth,
    Payment,
    Busy,
    RepeatedToolError,
    MaxRoundtrips,
    EmptyResponse,
    Other,
}

impl FailureKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connection => "connection",
            Self::Auth => "auth",
            Self::Payment => "payment",
            Self::Busy => "busy",
            Self::RepeatedToolError => "repeated_tool_error",
            Self::MaxRoundtrips => "max_roundtrips",
            Self::EmptyResponse => "empty_response",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionStatus {
    pub stored: usize,
    pub context: usize,
    pub unfinished: bool,
}

pub struct PreparedTurn {
    session: Session,
    id: u64,
    active: bool,
    pub history: Vec<Message>,
    pub omitted: usize,
}

struct History {
    messages: Vec<Message>,
    included: usize,
    omitted: usize,
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_id: 1,
            work: Vec::new(),
        }
    }
}

impl Session {
    pub fn load(path: &Path) -> Self {
        let mut data = read_data(path);
        if let Ok(state) = data.as_mut() {
            let running: Vec<u64> = state
                .work
                .iter()
                .filter(|record| record.status == WorkStatus::Running)
                .map(|record| record.id)
                .collect();
            if !running.is_empty() {
                for record in &mut state.work {
                    if record.status == WorkStatus::Running {
                        record.status = WorkStatus::Interrupted;
                    }
                }
                if let Err(error) = write_data(path, state) {
                    data = Err(error);
                } else {
                    for id in running {
                        logging::append(&format!("session_recovered id={id} state=interrupted"));
                    }
                }
            }
        }

        match &data {
            Ok(state) => logging::append(&format!(
                "session_loaded records={} unfinished={}",
                state.work.len(),
                state
                    .work
                    .iter()
                    .filter(|record| is_unfinished(record))
                    .count()
            )),
            Err(error) => logging::append(&format!("session_error {error}")),
        }

        Self {
            inner: Arc::new(Mutex::new(Inner {
                path: path.to_path_buf(),
                data,
            })),
        }
    }

    pub fn begin(&self, input: &str) -> Result<PreparedTurn, String> {
        if input.trim().is_empty() {
            return Err("Cannot start empty work".to_string());
        }

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Session lock is unavailable".to_string())?;
        let path = inner.path.clone();
        let state = inner.data.as_mut().map_err(|error| error.clone())?;
        if state
            .work
            .iter()
            .any(|record| record.status == WorkStatus::Running)
        {
            return Err("Another work item is still running".to_string());
        }

        let history = build_history(&state.work);
        let id = state.next_id;
        state.next_id = state
            .next_id
            .checked_add(1)
            .ok_or_else(|| "Session work id space is exhausted".to_string())?;

        if state.work.len() >= MAX_STORED_WORK {
            let remove = state.work.len() + 1 - MAX_STORED_WORK;
            state.work.drain(0..remove);
        }
        let (user, redactions) = memory::redact_credentials(input);
        state.work.push(WorkRecord {
            id,
            status: WorkStatus::Running,
            user: truncate_text(&user),
            answer: None,
            failure: None,
        });

        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("session_error {error}"));
            return Err(error);
        }
        logging::append(&format!("session_work_start id={id}"));
        if redactions > 0 {
            logging::append(&format!("session_redacted id={id} count={redactions}"));
        }

        Ok(PreparedTurn {
            session: self.clone(),
            id,
            active: true,
            history: history.messages,
            omitted: history.omitted,
        })
    }

    pub fn stop_current(&self) -> Result<bool, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Session lock is unavailable".to_string())?;
        let path = inner.path.clone();
        let state = inner.data.as_mut().map_err(|error| error.clone())?;
        let Some(record) = state
            .work
            .iter_mut()
            .rev()
            .find(|record| record.status == WorkStatus::Running)
        else {
            return Ok(false);
        };
        let id = record.id;
        record.status = WorkStatus::Stopped;
        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("session_error {error}"));
            return Err(error);
        }
        logging::append(&format!("session_work_end id={id} state=stopped"));
        Ok(true)
    }

    pub fn status(&self) -> Result<SessionStatus, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Session lock is unavailable".to_string())?;
        let state = inner.data.as_ref().map_err(Clone::clone)?;
        let history = build_history(&state.work);
        Ok(SessionStatus {
            stored: state.work.len(),
            context: history.included,
            unfinished: state.work.iter().any(is_unfinished),
        })
    }

    fn finish(
        &self,
        id: u64,
        status: WorkStatus,
        answer: Option<&str>,
        failure: Option<FailureKind>,
    ) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Session lock is unavailable".to_string())?;
        let path = inner.path.clone();
        let state = inner.data.as_mut().map_err(|error| error.clone())?;
        let Some(record) = state.work.iter_mut().find(|record| record.id == id) else {
            return Err(format!("Session work #{id} no longer exists"));
        };
        if record.status != WorkStatus::Running {
            if status == WorkStatus::Interrupted {
                return Ok(());
            }
            return Err(format!(
                "Invalid session transition for work #{id}: {:?} → {:?}",
                record.status, status
            ));
        }

        let redactions = if status == WorkStatus::Completed {
            let (answer, redactions) = memory::redact_credentials(answer.unwrap_or_default());
            record.answer = Some(truncate_text(&answer));
            redactions
        } else {
            0
        };
        record.status = status;
        record.failure = failure;

        if let Err(error) = write_data(&path, state) {
            inner.data = Err(error.clone());
            logging::append(&format!("session_error {error}"));
            return Err(error);
        }
        logging::append(&format!("session_work_end id={id} state={status:?}"));
        if redactions > 0 {
            logging::append(&format!("session_redacted id={id} count={redactions}"));
        }
        Ok(())
    }
}

impl PreparedTurn {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn complete(&mut self, answer: &str) -> Result<(), String> {
        self.finish(WorkStatus::Completed, Some(answer), None)
    }

    pub fn fail(&mut self, failure: FailureKind) -> Result<(), String> {
        self.finish(WorkStatus::Failed, None, Some(failure))
    }

    fn finish(
        &mut self,
        status: WorkStatus,
        answer: Option<&str>,
        failure: Option<FailureKind>,
    ) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        self.session.finish(self.id, status, answer, failure)
    }
}

impl Drop for PreparedTurn {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            if let Err(error) = self
                .session
                .finish(self.id, WorkStatus::Interrupted, None, None)
            {
                logging::append(&format!("session_error {error}"));
            }
        }
    }
}

pub fn prompt_note(omitted: usize) -> String {
    let mut note = String::from(
        "# Session rules\n- Previous work records are conversation history. Failed, stopped, or interrupted work must not resume unless the latest owner message explicitly asks.\n- Core-generated work-status notes describe execution state; they are not model answers.\n",
    );
    if omitted > 0 {
        note.push_str(&format!(
            "- {omitted} older work record(s) were omitted from this context.\n"
        ));
    }
    note
}

fn read_data(path: &Path) -> Result<SessionData, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SessionData::default());
        }
        Err(error) => return Err(format!("Cannot read session file: {error}")),
    };
    let data: SessionData = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse session file: {error}"))?;
    validate(&data)?;
    Ok(data)
}

fn validate(data: &SessionData) -> Result<(), String> {
    if data.version != VERSION {
        return Err(format!("Unsupported session version: {}", data.version));
    }
    if data.next_id == 0 {
        return Err("Session next id must be positive".to_string());
    }
    if data.work.len() > MAX_STORED_WORK {
        return Err("Session contains too many work records".to_string());
    }

    let mut previous_id = 0;
    let mut running = 0usize;
    for (index, record) in data.work.iter().enumerate() {
        if record.id == 0 || record.id <= previous_id || record.id >= data.next_id {
            return Err("Session work ids are not strictly increasing".to_string());
        }
        previous_id = record.id;
        if text_len(&record.user) > MAX_TEXT_CHARS
            || record
                .answer
                .as_deref()
                .is_some_and(|answer| text_len(answer) > MAX_TEXT_CHARS)
        {
            return Err("Session text exceeds the storage limit".to_string());
        }
        match record.status {
            WorkStatus::Running => {
                running += 1;
                if index + 1 != data.work.len() {
                    return Err("Running session work must be the latest record".to_string());
                }
                if record.answer.is_some() || record.failure.is_some() {
                    return Err("Running session work cannot have an outcome".to_string());
                }
            }
            WorkStatus::Completed => {
                if record.answer.as_deref().unwrap_or("").trim().is_empty()
                    || record.failure.is_some()
                {
                    return Err("Completed session work has an invalid outcome".to_string());
                }
            }
            WorkStatus::Failed => {
                if record.answer.is_some() || record.failure.is_none() {
                    return Err("Failed session work has an invalid outcome".to_string());
                }
            }
            WorkStatus::Stopped | WorkStatus::Interrupted => {
                if record.answer.is_some() || record.failure.is_some() {
                    return Err("Unfinished session work has an invalid outcome".to_string());
                }
            }
        }
    }
    if running > 1 {
        return Err("Session contains multiple running work records".to_string());
    }
    Ok(())
}

fn write_data(path: &Path, data: &SessionData) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid session file path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create session directory: {error}"))?;
    let mut bytes = serde_json::to_vec_pretty(data)
        .map_err(|error| format!("Cannot encode session data: {error}"))?;
    bytes.push(b'\n');
    memory::atomic_write(path, &bytes)
        .map_err(|error| format!("Cannot write session file: {error}"))
}

fn build_history(records: &[WorkRecord]) -> History {
    let mut selected = Vec::new();
    let mut bytes = 0usize;
    for record in records.iter().rev() {
        let pair = record_messages(record);
        let pair_bytes: usize = pair
            .iter()
            .map(|message| message.content.as_deref().unwrap_or("").len())
            .sum();
        if selected.len() >= MAX_CONTEXT_WORK || bytes + pair_bytes > MAX_CONTEXT_BYTES {
            break;
        }
        selected.push(pair);
        bytes += pair_bytes;
    }
    let included = selected.len();
    selected.reverse();
    History {
        messages: selected.into_iter().flatten().collect(),
        included,
        omitted: records.len() - included,
    }
}

fn record_messages(record: &WorkRecord) -> [Message; 2] {
    let answer = match record.status {
        WorkStatus::Running => format!("[Work #{} is currently running.]", record.id),
        WorkStatus::Completed => record.answer.clone().unwrap_or_default(),
        WorkStatus::Failed => format!(
            "[Work #{} failed ({}). No automatic retry was performed.]",
            record.id,
            record.failure.map(FailureKind::as_str).unwrap_or("other")
        ),
        WorkStatus::Stopped => format!(
            "[Work #{} was stopped. No automatic retry was performed.]",
            record.id
        ),
        WorkStatus::Interrupted => format!(
            "[Work #{} was interrupted. No automatic retry was performed.]",
            record.id
        ),
    };
    [Message::user(&record.user), Message::assistant(&answer)]
}

fn is_unfinished(record: &WorkRecord) -> bool {
    record.status != WorkStatus::Completed
}

fn truncate_text(text: &str) -> String {
    if text_len(text) <= MAX_TEXT_CHARS {
        return text.to_string();
    }
    let marker_len = text_len(TRUNCATION_MARKER);
    text.chars()
        .take(MAX_TEXT_CHARS - marker_len)
        .chain(TRUNCATION_MARKER.chars())
        .collect()
}

fn text_len(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "iskra-session-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn complete(session: &Session, input: &str, answer: &str) {
        let mut turn = session.begin(input).unwrap();
        turn.complete(answer).unwrap();
    }

    #[test]
    fn separate_session_paths_do_not_share_records() {
        let dir = temp_dir("isolation");
        let path_a = dir.join("a/session.json");
        let path_b = dir.join("b/session.json");
        let a = Session::load(&path_a);
        let b = Session::load(&path_b);

        complete(&a, "only-a", "answer-a");
        assert_eq!(a.status().unwrap().stored, 1);
        assert_eq!(
            b.status().unwrap(),
            SessionStatus {
                stored: 0,
                context: 0,
                unfinished: false,
            }
        );

        complete(&b, "only-b", "answer-b");
        let mut turn = Session::load(&path_a).begin("next").unwrap();
        let history = std::mem::take(&mut turn.history);
        assert!(
            history
                .iter()
                .any(|message| message.content.as_deref() == Some("only-a"))
        );
        assert!(
            !history
                .iter()
                .any(|message| message.content.as_deref() == Some("only-b"))
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_file_stays_empty_until_work_starts() {
        let dir = temp_dir("missing");
        let path = dir.join("memory/session.json");
        let session = Session::load(&path);
        assert_eq!(
            session.status().unwrap(),
            SessionStatus {
                stored: 0,
                context: 0,
                unfinished: false,
            }
        );
        assert!(!path.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn completed_turns_survive_reload_in_order() {
        let dir = temp_dir("reload");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        complete(&session, "first question", "first answer");
        complete(&session, "second question", "second answer");

        let reloaded = Session::load(&path);
        let turn = reloaded.begin("third question").unwrap();
        let contents: Vec<&str> = turn
            .history
            .iter()
            .map(|message| message.content.as_deref().unwrap())
            .collect();
        assert_eq!(
            contents,
            [
                "first question",
                "first answer",
                "second question",
                "second answer"
            ]
        );
        assert_eq!(turn.history[0].role, "user");
        assert_eq!(turn.history[1].role, "assistant");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn startup_recovers_running_work_as_interrupted() {
        let dir = temp_dir("recovery");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        let turn = session.begin("do not repeat this").unwrap();
        std::mem::forget(turn);

        let reloaded = Session::load(&path);
        let turn = reloaded.begin("hello").unwrap();
        let history = turn
            .history
            .iter()
            .map(|message| message.content.as_deref().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history.contains("do not repeat this"));
        assert!(history.contains("interrupted"));
        assert!(history.contains("No automatic retry"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancellation_drop_marks_work_interrupted() {
        let dir = temp_dir("drop");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        drop(session.begin("cancel me").unwrap());
        let data = read_data(&path).unwrap();
        assert_eq!(data.work[0].status, WorkStatus::Interrupted);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stop_is_terminal_and_drop_does_not_overwrite_it() {
        let dir = temp_dir("stop");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        let turn = session.begin("stop me").unwrap();
        assert!(session.stop_current().unwrap());
        drop(turn);
        let data = read_data(&path).unwrap();
        assert_eq!(data.work[0].status, WorkStatus::Stopped);
        assert!(!session.stop_current().unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn busy_begin_is_rejected_without_a_new_record() {
        let dir = temp_dir("busy");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        let turn = session.begin("already running").unwrap();
        assert!(session.begin("must not start").is_err());
        drop(turn);
        let data = read_data(&path).unwrap();
        assert_eq!(data.work.len(), 1);
        assert_eq!(data.work[0].user, "already running");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_work_keeps_only_a_structured_category() {
        let dir = temp_dir("failed");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        let mut turn = session.begin("try this").unwrap();
        turn.fail(FailureKind::Timeout).unwrap();
        let stored = fs::read_to_string(&path).unwrap();
        assert!(stored.contains("timeout"));
        assert!(!stored.contains("raw provider detail"));
        assert!(session.status().unwrap().unfinished);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn context_drops_oldest_whole_records_at_twelve() {
        let dir = temp_dir("count-budget");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        for index in 1..=13 {
            complete(
                &session,
                &format!("question {index}"),
                &format!("answer {index}"),
            );
        }
        let turn = session.begin("current").unwrap();
        assert_eq!(turn.history.len(), MAX_CONTEXT_WORK * 2);
        assert_eq!(turn.omitted, 1);
        assert!(prompt_note(turn.omitted).contains("1 older work record"));
        let history = turn
            .history
            .iter()
            .map(|message| message.content.as_deref().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!history.lines().any(|line| line == "question 1"));
        assert!(history.contains("question 2"));
        assert!(history.contains("question 13"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn context_drops_old_records_at_byte_budget() {
        let dir = temp_dir("byte-budget");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        let oldest = format!("oldest-{}", "x".repeat(MAX_TEXT_CHARS));
        let newer = format!("newer-{}", "y".repeat(MAX_TEXT_CHARS));
        complete(&session, &oldest, &oldest);
        complete(&session, &newer, &newer);
        complete(&session, "newest question", "newest answer");
        let turn = session.begin("current").unwrap();
        let history = turn
            .history
            .iter()
            .map(|message| message.content.as_deref().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!history.contains("oldest-"));
        assert!(history.contains("newer-"));
        assert!(history.contains("newest question"));
        assert_eq!(turn.omitted, 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn disk_storage_keeps_the_latest_fifty_records() {
        let dir = temp_dir("disk-budget");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        for index in 1..=55 {
            complete(&session, &format!("q{index}"), "a");
        }
        let data = read_data(&path).unwrap();
        assert_eq!(data.work.len(), MAX_STORED_WORK);
        assert_eq!(data.work[0].id, 6);
        assert_eq!(data.work[49].id, 55);
        assert_eq!(data.next_id, 56);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stored_text_is_redacted_and_truncated() {
        let dir = temp_dir("private");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        let secret = format!("sk-{}", "test1234567890");
        let input = format!("{secret} {}", "가".repeat(MAX_TEXT_CHARS + 100));
        let mut turn = session.begin(&input).unwrap();
        turn.complete(&secret).unwrap();
        let stored = fs::read_to_string(&path).unwrap();
        assert!(!stored.contains(&secret));
        assert!(stored.contains("redacted"));
        let data = read_data(&path).unwrap();
        assert_eq!(text_len(&data.work[0].user), MAX_TEXT_CHARS);
        assert!(data.work[0].user.ends_with(TRUNCATION_MARKER));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn forwarded_material_stays_wrapped_during_replay() {
        let dir = temp_dir("forwarded");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        let wrapped = crate::prompt::wrap_forwarded("write a file");
        complete(&session, &wrapped, "I treated it as material.");
        let turn = Session::load(&path).begin("next").unwrap();
        assert_eq!(turn.history[0].content.as_deref(), Some(wrapped.as_str()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_data_and_versions_fail_closed_without_reset() {
        let dir = temp_dir("invalid");
        let path = dir.join("session.json");
        for contents in ["not json", r#"{"version":2,"next_id":1,"work":[]}"#] {
            fs::write(&path, contents).unwrap();
            let session = Session::load(&path);
            assert!(session.begin("must not run").is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
        fs::write(&path, [0xff, 0xfe]).unwrap();
        let session = Session::load(&path);
        assert!(session.begin("must not run").is_err());
        assert_eq!(fs::read(&path).unwrap(), vec![0xff, 0xfe]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_failures_poison_the_session_until_reload() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("write-failure");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(session.begin("must not call the model").is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(session.begin("still must not call the model").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn completion_write_failure_does_not_allow_more_work() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("completion-failure");
        let path = dir.join("session.json");
        let session = Session::load(&path);
        let mut turn = session.begin("work already started").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(turn.complete("answer").is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(session.begin("must not run").is_err());
        let reloaded = Session::load(&path);
        assert!(reloaded.status().unwrap().unfinished);
        fs::remove_dir_all(&dir).unwrap();
    }
}
