use crate::brain::{ChatError, ChatErrorKind, Message};
use crate::logging;
use crate::run_context::RunContext;
use crate::runtime::AgentRuntime;
use crate::session::FailureKind;

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
            | Self::Other(detail) => detail,
        };
        f.write_str(detail)
    }
}

impl TurnError {
    pub(crate) fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Timeout(_) => FailureKind::Timeout,
            Self::Connection(_) => FailureKind::Connection,
            Self::Auth(_) => FailureKind::Auth,
            Self::Payment(_) => FailureKind::Payment,
            Self::Busy(_) => FailureKind::Busy,
            Self::RepeatedToolError(_) => FailureKind::RepeatedToolError,
            Self::MaxRoundtrips(_) => FailureKind::MaxRoundtrips,
            Self::EmptyResponse(_) => FailureKind::EmptyResponse,
            Self::Other(_) => FailureKind::Other,
        }
    }
}

fn from_chat_error(error: ChatError) -> TurnError {
    let kind = error.kind();
    let detail = error.into_detail();
    match kind {
        ChatErrorKind::Timeout => TurnError::Timeout(detail),
        ChatErrorKind::Connection => TurnError::Connection(detail),
        ChatErrorKind::Auth => TurnError::Auth(detail),
        ChatErrorKind::Payment => TurnError::Payment(detail),
        ChatErrorKind::Busy => TurnError::Busy(detail),
        ChatErrorKind::Other => TurnError::Other(detail),
    }
}

/// Runs one model/tool turn that the caller already owns.
///
/// Session begin, Session completion, and Run-store settlement stay with the
/// caller so the core can persist each authority in the required order.
pub async fn run_work(
    runtime: &AgentRuntime,
    run_id: u64,
    history: Vec<Message>,
    omitted: usize,
    system_prompt: &str,
    user_input: &str,
) -> Result<String, TurnError> {
    let context = RunContext::new(
        run_id,
        &runtime.agent_id,
        &runtime.engine_id,
        &runtime.tools,
    );
    run_active_turn(
        runtime,
        &context,
        system_prompt,
        user_input,
        history,
        omitted,
    )
    .await
}

async fn run_active_turn(
    runtime: &AgentRuntime,
    context: &RunContext<'_>,
    system_prompt: &str,
    user_input: &str,
    history: Vec<Message>,
    omitted: usize,
) -> Result<String, TurnError> {
    logging::append(&format!(
        "turn_start run_id={} agent={} engine={}",
        context.run_id(),
        context.agent_id(),
        context.engine_id()
    ));

    let daily_usage = crate::usage::today_for_agent(context.agent_id());
    let balance = match runtime.brain.balance_cached().await {
        Ok(balance) => balance.display(),
        Err(_) => "unavailable".to_string(),
    };
    let self_state = crate::usage::self_state_block(
        runtime.brain.model(),
        runtime.brain.base_url(),
        &daily_usage,
        &balance,
    );
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
        let reply = runtime.brain.chat(context, &messages).await.map_err(|e| {
            logging::append(&format!(
                "model_error agent={} run_id={} engine={} model={} category={} status={}",
                context.agent_id(),
                context.run_id(),
                context.engine_id(),
                runtime.brain.model(),
                e.category(),
                e.status()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ));
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
            "model_call agent={} run_id={} engine={} model={} tools=[{tool_names}] usage={usage}",
            context.agent_id(),
            context.run_id(),
            context.engine_id(),
            runtime.brain.model()
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
                Ok(args) => context.tools().execute(&name, &args).await,
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
                    logging::append(&format!(
                        "tool_error agent={} run_id={} engine={} name={name}",
                        context.agent_id(),
                        context.run_id(),
                        context.engine_id()
                    ));
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
