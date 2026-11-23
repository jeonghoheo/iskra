use crate::brain::{Brain, ChatError, Message};
use crate::logging;
use crate::session::{FailureKind, Session};
use crate::tools::Tools;

pub const MAX_TOOL_ROUNDTRIPS: usize = 10;
pub const MAX_CONSECUTIVE_SAME_ERROR: usize = 2;

#[derive(Debug)]
pub enum TurnError {
    Timeout(String),
    Connection(String),
    Auth(String),
    Payment(String),
    Busy(String),
    RepeatedToolError(String),
    MaxRoundtrips(String),
    EmptyResponse(String),
    Session(String),
    Other(String),
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::Timeout(detail)
            | Self::Connection(detail)
            | Self::Auth(detail)
            | Self::Payment(detail)
            | Self::Busy(detail)
            | Self::RepeatedToolError(detail)
            | Self::MaxRoundtrips(detail)
            | Self::EmptyResponse(detail)
            | Self::Session(detail)
            | Self::Other(detail) => detail,
        };
        f.write_str(detail)
    }
}

impl TurnError {
    fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Timeout(_) => FailureKind::Timeout,
            Self::Connection(_) => FailureKind::Connection,
            Self::Auth(_) => FailureKind::Auth,
            Self::Payment(_) => FailureKind::Payment,
            Self::Busy(_) => FailureKind::Busy,
            Self::RepeatedToolError(_) => FailureKind::RepeatedToolError,
            Self::MaxRoundtrips(_) => FailureKind::MaxRoundtrips,
            Self::EmptyResponse(_) => FailureKind::EmptyResponse,
            Self::Session(_) | Self::Other(_) => FailureKind::Other,
        }
    }
}

fn from_chat_error(error: ChatError) -> TurnError {
    match error {
        ChatError::Timeout(detail) => TurnError::Timeout(detail),
        ChatError::Connection(detail) => TurnError::Connection(detail),
        ChatError::Auth(detail) => TurnError::Auth(detail),
        ChatError::Payment(detail) => TurnError::Payment(detail),
        ChatError::Busy(detail) => TurnError::Busy(detail),
        ChatError::Other(detail) => TurnError::Other(detail),
    }
}

pub async fn run_turn(
    brain: &Brain,
    tools: &Tools,
    session: &Session,
    system_prompt: &str,
    user_input: &str,
) -> Result<String, TurnError> {
    let mut work = session.begin(user_input).map_err(|error| {
        logging::append(&format!("session_error {error}"));
        TurnError::Session(error)
    })?;
    let history = std::mem::take(&mut work.history);
    let omitted = work.omitted;
    let result = run_active_turn(brain, tools, system_prompt, user_input, history, omitted).await;

    match result {
        Ok(answer) => {
            if let Err(error) = work.complete(&answer) {
                logging::append(&format!("session_error {error}"));
                return Err(TurnError::Session(error));
            }
            Ok(answer)
        }
        Err(error) => {
            if let Err(session_error) = work.fail(error.failure_kind()) {
                logging::append(&format!("session_error {session_error}"));
                return Err(TurnError::Session(session_error));
            }
            Err(error)
        }
    }
}

async fn run_active_turn(
    brain: &Brain,
    tools: &Tools,
    system_prompt: &str,
    user_input: &str,
    history: Vec<Message>,
    omitted: usize,
) -> Result<String, TurnError> {
    logging::append("turn_start");

    let daily_usage = crate::usage::today();
    let balance = match brain.balance_cached().await {
        Ok(balance) => balance.display(),
        Err(_) => "unavailable".to_string(),
    };
    let self_state =
        crate::usage::self_state_block(brain.model(), brain.base_url(), &daily_usage, &balance);
    let system_prompt = format!(
        "{system_prompt}\n\n{}\n\n{self_state}",
        crate::session::prompt_note(omitted)
    );
    let mut messages = Vec::with_capacity(history.len() + 2);
    messages.push(Message::system(&system_prompt));
    messages.extend(history);
    messages.push(Message::user(user_input));
    let mut roundtrips = 0usize;
    let mut last_error: Option<(String, String)> = None;
    let mut same_error_count = 0usize;
    let mut empty_retried = false;

    loop {
        let reply = brain.chat(&messages).await.map_err(|e| {
            logging::append(&format!("model_error err={e}"));
            from_chat_error(e)
        })?;

        let usage = match reply.usage.as_ref() {
            Some(u) => format!(
                "prompt={} completion={} total={}",
                u.prompt_tokens, u.completion_tokens, u.total_tokens
            ),
            None => "unknown".to_string(),
        };
        let tool_names = match reply.message.tool_calls.as_ref() {
            Some(calls) if !calls.is_empty() => calls
                .iter()
                .map(|c| c.function.name.clone())
                .collect::<Vec<_>>()
                .join(","),
            _ => "none".to_string(),
        };
        logging::append(&format!(
            "model_call model={} tools=[{tool_names}] usage={usage}",
            brain.model()
        ));

        let has_tools = reply
            .message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty());
        let content = reply.message.content.clone().unwrap_or_default();

        if !has_tools {
            if content.trim().is_empty() {
                if empty_retried {
                    logging::append("stop empty_response");
                    return Err(TurnError::EmptyResponse(
                        "The model returned an empty response twice".to_string(),
                    ));
                }
                empty_retried = true;
                logging::append("empty_response_retry");
                continue;
            }

            logging::append("turn_end answer");
            return Ok(content);
        }

        roundtrips += 1;
        if roundtrips > MAX_TOOL_ROUNDTRIPS {
            logging::append("stop max_roundtrips");
            return Err(TurnError::MaxRoundtrips(format!(
                "Stopped after exceeding the maximum of {MAX_TOOL_ROUNDTRIPS} tool round-trips"
            )));
        }

        messages.push(reply.message.clone());
        let calls = reply.message.tool_calls.clone().unwrap_or_default();

        for call in calls {
            let name = call.function.name.clone();
            let outcome = match serde_json::from_str::<serde_json::Value>(&call.function.arguments)
            {
                Ok(args) => tools.execute(&name, &args).await,
                Err(e) => Err(format!("Failed to parse arguments JSON: {e}")),
            };

            match outcome {
                Ok(output) => {
                    last_error = None;
                    same_error_count = 0;
                    logging::append(&format!("tool_ok name={name}"));
                    messages.push(Message::tool(&call.id, &output));
                }
                Err(err) => {
                    logging::append(&format!("tool_error name={name} err={err}"));
                    let stop = register_error(&mut last_error, &mut same_error_count, &name, &err);
                    if stop {
                        logging::append("stop repeated_tool_error");
                        return Err(TurnError::RepeatedToolError(format!(
                            "Stopped after the same tool error occurred {MAX_CONSECUTIVE_SAME_ERROR} times consecutively: {name} ({err})"
                        )));
                    }
                    messages.push(Message::tool(&call.id, &format!("Tool error: {err}")));
                }
            }
        }
    }
}

fn register_error(
    last_error: &mut Option<(String, String)>,
    count: &mut usize,
    name: &str,
    err: &str,
) -> bool {
    let current = (name.to_string(), err.to_string());
    if last_error.as_ref() == Some(&current) {
        *count += 1;
    } else {
        *last_error = Some(current);
        *count = 1;
    }
    *count >= MAX_CONSECUTIVE_SAME_ERROR
}
