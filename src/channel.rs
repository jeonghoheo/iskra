use crate::brain::Brain;
use crate::logging;
use crate::loop_;
use crate::tools::Tools;

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use teloxide::dptree;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, ParseMode};
use tokio::task::JoinHandle;

const MAX_MESSAGE_CHARS: usize = 4096;
const TYPING_REFRESH_SECS: u64 = 4;
const START_TEXT: &str = "이스크라입니다. 메시지를 보내면 대화합니다. /status로 상태, /stop으로 현재 턴을 중단할 수 있습니다.";

type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct TelegramConfig {
    token: String,
    allowed_user_id: u64,
}

impl TelegramConfig {
    fn from_env() -> Result<Self, String> {
        let token = env_non_empty("TELEGRAM_BOT_TOKEN")
            .ok_or_else(|| "TELEGRAM_BOT_TOKEN이 .env에 없습니다".to_string())?;
        let allowed_user_id = env_non_empty("ALLOWED_USER_ID")
            .ok_or_else(|| "ALLOWED_USER_ID가 .env에 없습니다".to_string())?
            .parse::<u64>()
            .map_err(|_| "ALLOWED_USER_ID는 숫자여야 합니다".to_string())?;
        Ok(Self {
            token,
            allowed_user_id,
        })
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

struct ChannelState {
    bot: Bot,
    brain: Brain,
    tools: Tools,
    system_prompt: String,
    memory_status: String,
    allowed_user_id: u64,
    current_turn: Mutex<Option<JoinHandle<()>>>,
}

pub async fn run(
    brain: Brain,
    tools: Tools,
    system_prompt: String,
    memory_status: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = match TelegramConfig::from_env() {
        Ok(config) => config,
        Err(e) => {
            logging::append(&format!("telegram_config_error reason={e}"));
            return Err(e.into());
        }
    };

    let TelegramConfig {
        token,
        allowed_user_id,
    } = config;
    let bot = Bot::new(token);

    let state = Arc::new(ChannelState {
        bot: bot.clone(),
        brain,
        tools,
        system_prompt,
        memory_status,
        allowed_user_id,
        current_turn: Mutex::new(None),
    });

    logging::append("telegram_start");

    let handler = Update::filter_message().endpoint(handle_message);
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn handle_message(bot: Bot, msg: Message, state: Arc<ChannelState>) -> HandlerResult {
    let Some(from) = msg.from.as_ref() else {
        logging::append("telegram_ignore no_sender");
        return Ok(());
    };
    if from.id.0 != state.allowed_user_id {
        logging::append(&format!("telegram_ignore user_id={}", from.id.0));
        return Ok(());
    }

    let Some(text) = msg.text() else {
        logging::append("telegram_ignore non_text");
        return Ok(());
    };
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }

    let chat_id = msg.chat.id;
    match command_name(text).as_deref() {
        Some("/start") => {
            bot.send_message(chat_id, START_TEXT).await?;
        }
        Some("/stop") => stop_turn(&state, chat_id).await?,
        Some("/status") => send_status(&state, chat_id).await?,
        _ => start_turn(&state, chat_id, text),
    }

    Ok(())
}

fn command_name(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?;
    if !first.starts_with('/') {
        return None;
    }
    let name = first.split('@').next().unwrap_or(first);
    Some(name.to_string())
}

fn start_turn(state: &Arc<ChannelState>, chat_id: ChatId, text: &str) {
    let busy = {
        let slot = state.current_turn.lock().unwrap();
        slot.as_ref().is_some_and(|handle| !handle.is_finished())
    };
    if busy {
        logging::append("telegram_busy");
        let bot = state.bot.clone();
        tokio::spawn(async move {
            let _ = bot
                .send_message(
                    chat_id,
                    "이전 턴이 진행 중입니다. /stop으로 중단할 수 있습니다.",
                )
                .await;
        });
        return;
    }

    let task_state = Arc::clone(state);
    let input = text.to_string();
    let handle = tokio::spawn(async move {
        let (typing_stop, typing_rx) = tokio::sync::watch::channel(false);
        let typing_guard = TypingGuard(typing_stop);
        tokio::spawn(typing_loop(task_state.bot.clone(), chat_id, typing_rx));

        let reply = loop_::run_turn(
            &task_state.brain,
            &task_state.tools,
            &task_state.system_prompt,
            &input,
        )
        .await;
        drop(typing_guard);

        let answer = match reply {
            Ok(answer) => answer,
            Err(e) => {
                logging::append(&format!("telegram_turn_error {e}"));
                format!("중단: {e}")
            }
        };
        if let Err(e) = send_reply(&task_state.bot, chat_id, &answer).await {
            logging::append(&format!("telegram_send_failed {e}"));
        }
    });
    *state.current_turn.lock().unwrap() = Some(handle);
}

struct TypingGuard(tokio::sync::watch::Sender<bool>);

impl Drop for TypingGuard {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

async fn typing_loop(bot: Bot, chat_id: ChatId, mut stop: tokio::sync::watch::Receiver<bool>) {
    logging::append("telegram_typing_start");
    loop {
        if *stop.borrow() {
            break;
        }
        match bot.send_chat_action(chat_id, ChatAction::Typing).await {
            Ok(_) => logging::append("telegram_typing_sent"),
            Err(e) => logging::append(&format!("telegram_typing_error {e}")),
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(TYPING_REFRESH_SECS)) => {}
            _ = stop.changed() => break,
        }
    }
    logging::append("telegram_typing_stop");
}

async fn stop_turn(state: &Arc<ChannelState>, chat_id: ChatId) -> HandlerResult {
    let handle = { state.current_turn.lock().unwrap().take() };
    let message = match handle {
        Some(handle) if !handle.is_finished() => {
            handle.abort();
            logging::append("telegram_stop aborted");
            "현재 턴을 중단했습니다."
        }
        _ => {
            logging::append("telegram_stop idle");
            "진행 중인 턴이 없습니다."
        }
    };
    state.bot.send_message(chat_id, message).await?;
    Ok(())
}

async fn send_status(state: &Arc<ChannelState>, chat_id: ChatId) -> HandlerResult {
    let text = format!(
        "모델: {}\n기억: {}\n오늘 토큰: {}",
        state.brain.model(),
        state.memory_status,
        today_token_usage()
    );
    state.bot.send_message(chat_id, text).await?;
    Ok(())
}

async fn send_reply(bot: &Bot, chat_id: ChatId, text: &str) -> Result<(), String> {
    let chunks = split_message(text);
    for chunk in &chunks {
        send_chunk(bot, chat_id, chunk).await?;
    }
    logging::append(&format!("telegram_reply_sent chunks={}", chunks.len()));
    Ok(())
}

#[allow(deprecated)]
async fn send_chunk(bot: &Bot, chat_id: ChatId, text: &str) -> Result<(), String> {
    match bot
        .send_message(chat_id, text)
        .parse_mode(ParseMode::Markdown)
        .await
    {
        Ok(_) => Ok(()),
        Err(first) => {
            logging::append(&format!("telegram_send_retry first_err={first}"));
            match bot.send_message(chat_id, text).await {
                Ok(_) => Ok(()),
                Err(second) => {
                    logging::append(&format!("telegram_send_error err={second}"));
                    Err(second.to_string())
                }
            }
        }
    }
}

fn split_message(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut count = 0usize;

    for (idx, _) in text.char_indices() {
        if count == MAX_MESSAGE_CHARS {
            chunks.push(&text[start..idx]);
            start = idx;
            count = 0;
        }
        count += 1;
    }

    if start < text.len() {
        chunks.push(&text[start..]);
    }

    chunks
}

fn today_token_usage() -> u64 {
    let Ok(log) = std::fs::read_to_string(logging::LOG_PATH) else {
        return 0;
    };
    let since = local_midnight_secs().unwrap_or_else(utc_midnight_secs);
    sum_tokens_since(&log, since)
}

fn sum_tokens_since(log: &str, since: i64) -> u64 {
    let mut total = 0u64;
    for line in log.lines() {
        let Some(ts) = line
            .strip_prefix("ts=")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<i64>().ok())
        else {
            continue;
        };
        if ts < since || !line.contains(" model_call ") {
            continue;
        }
        let Some((_, tail)) = line.rsplit_once("total=") else {
            continue;
        };
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(value) = digits.parse::<u64>() {
            total += value;
        }
    }
    total
}

#[cfg(unix)]
fn local_midnight_secs() -> Option<i64> {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        if now == -1 {
            return None;
        }
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        let midnight = libc::mktime(&mut tm);
        if midnight == -1 {
            None
        } else {
            Some(midnight as i64)
        }
    }
}

#[cfg(not(unix))]
fn local_midnight_secs() -> Option<i64> {
    None
}

fn utc_midnight_secs() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    now - now.rem_euclid(86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_long_messages_into_bounded_chunks() {
        let text = "가".repeat(MAX_MESSAGE_CHARS * 2 + 5);
        let chunks = split_message(&text);
        assert_eq!(chunks.len(), 3);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= MAX_MESSAGE_CHARS)
        );
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn keeps_short_message_in_one_chunk() {
        let chunks = split_message("짧은 답변");
        assert_eq!(chunks, vec!["짧은 답변"]);
    }

    #[test]
    fn parses_only_known_commands() {
        assert_eq!(command_name("/start").as_deref(), Some("/start"));
        assert_eq!(
            command_name("/status@iskra_bot").as_deref(),
            Some("/status")
        );
        assert_eq!(command_name("/unknown").as_deref(), Some("/unknown"));
        assert_eq!(command_name("안녕").as_deref(), None);
    }

    #[test]
    fn sums_only_model_calls_since_midnight() {
        let log = "\
ts=100 model_call tools=[none] usage=prompt=1 completion=2 total=3
ts=200 model_call tools=[read_file] usage=prompt=4 completion=5 total=9
ts=50 model_call tools=[none] usage=prompt=7 completion=8 total=15
ts=300 turn_end answer
ts=400 model_error err=timeout
";
        assert_eq!(sum_tokens_since(log, 100), 12);
    }
}
