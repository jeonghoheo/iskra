use crate::brain::{Brain, Message};
use crate::logging;
use crate::tools::Tools;

pub const MAX_TOOL_ROUNDTRIPS: usize = 10;
pub const MAX_CONSECUTIVE_SAME_ERROR: usize = 2;

pub async fn run_turn(
    brain: &Brain,
    tools: &Tools,
    system_prompt: &str,
    user_input: &str,
) -> Result<String, String> {
    logging::append("turn_start");

    let daily_usage = crate::usage::today();
    let balance = match brain.balance_cached().await {
        Ok(balance) => balance.display(),
        Err(_) => "unavailable".to_string(),
    };
    let self_state =
        crate::usage::self_state_block(brain.model(), brain.base_url(), &daily_usage, &balance);
    let system_prompt = format!("{system_prompt}\n\n{self_state}");
    let mut messages = vec![Message::system(&system_prompt), Message::user(user_input)];
    let mut roundtrips = 0usize;
    let mut last_error: Option<(String, String)> = None;
    let mut same_error_count = 0usize;
    let mut empty_retried = false;

    loop {
        let reply = brain.chat(&messages).await.map_err(|e| {
            logging::append(&format!("model_error err={e}"));
            e
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
                    return Err("The model returned an empty response twice".to_string());
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
            return Err(format!(
                "Stopped after exceeding the maximum of {MAX_TOOL_ROUNDTRIPS} tool round-trips"
            ));
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
                        return Err(format!(
                            "Stopped after the same tool error occurred {MAX_CONSECUTIVE_SAME_ERROR} times consecutively: {name} ({err})"
                        ));
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
