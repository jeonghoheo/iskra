use crate::brain::{Brain, Message};
use crate::tools::Tools;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_TOOL_ROUNDTRIPS: usize = 10;
pub const MAX_CONSECUTIVE_SAME_ERROR: usize = 2;

const LOG_PATH: &str = "logs/agent.log";
const SYSTEM_PROMPT: &str = "너는 터미널에서 동작하는 파일 작업 도우미 'iskra'다. \
파일을 읽거나 쓸 때는 반드시 read_file/write_file 도구를 사용하고, \
모든 경로는 workspace 폴더 기준 상대 경로만 사용한다. \
도구 없이 파일에 접근할 수 있다고 가정하지 말고, 필요한 확인은 도구로 한 뒤 답한다.";

pub async fn run_turn(brain: &Brain, tools: &Tools, user_input: &str) -> Result<(), String> {
    let mut messages = vec![Message::system(SYSTEM_PROMPT), Message::user(user_input)];
    let mut roundtrips = 0usize;
    let mut last_error: Option<(String, String)> = None;
    let mut same_error_count = 0usize;
    let mut empty_retried = false;

    append_log("turn_start");

    loop {
        let reply = brain.chat(&messages).await.map_err(|e| {
            append_log(&format!("model_error err={e}"));
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
        append_log(&format!("model_call tools=[{tool_names}] usage={usage}"));

        let has_tools = reply
            .message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty());
        let content = reply.message.content.clone().unwrap_or_default();

        if !has_tools {
            if content.trim().is_empty() {
                if empty_retried {
                    append_log("stop empty_response");
                    return Err("모델이 빈 응답을 두 번 보냈습니다".to_string());
                }
                empty_retried = true;
                append_log("empty_response_retry");
                continue;
            }

            println!("{content}");
            append_log("turn_end answer");
            return Ok(());
        }

        roundtrips += 1;
        if roundtrips > MAX_TOOL_ROUNDTRIPS {
            append_log("stop max_roundtrips");
            return Err(format!(
                "도구 왕복이 최대 {MAX_TOOL_ROUNDTRIPS}회를 넘어 중단했습니다"
            ));
        }

        messages.push(reply.message.clone());
        let calls = reply.message.tool_calls.clone().unwrap_or_default();

        for call in calls {
            let name = call.function.name.clone();
            let outcome = match serde_json::from_str::<serde_json::Value>(&call.function.arguments)
            {
                Ok(args) => tools.execute(&name, &args),
                Err(e) => Err(format!("arguments JSON 파싱 실패: {e}")),
            };

            match outcome {
                Ok(output) => {
                    last_error = None;
                    same_error_count = 0;
                    append_log(&format!("tool_ok name={name}"));
                    messages.push(Message::tool(&call.id, &output));
                }
                Err(err) => {
                    append_log(&format!("tool_error name={name} err={err}"));
                    let stop =
                        register_error(&mut last_error, &mut same_error_count, &name, &err);
                    if stop {
                        append_log("stop repeated_tool_error");
                        return Err(format!(
                            "같은 도구 오류가 {MAX_CONSECUTIVE_SAME_ERROR}회 연속 발생해 중단했습니다: {name} ({err})"
                        ));
                    }
                    messages.push(Message::tool(&call.id, &format!("도구 오류: {err}")));
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

fn append_log(line: &str) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = format!("ts={ts} {line}\n");
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH)
        .and_then(|mut file| file.write_all(entry.as_bytes()));
    if let Err(e) = result {
        eprintln!("로그 기록 실패: {e}");
    }
}
