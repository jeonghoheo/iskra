use crate::brain::Brain;
use crate::logging;
use crate::loop_;
use crate::stickies;
use crate::tools::Tools;
use crate::usage;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use teloxide::dptree;
use teloxide::prelude::*;
use teloxide::types::{BotCommand, ChatAction, MessageReactionUpdated, ParseMode};
use tokio::task::JoinHandle;

const MAX_MESSAGE_CHARS: usize = 4096;
const TYPING_REFRESH_SECS: u64 = 4;
const COMMANDS: [(&str, &str); 5] = [
    ("start", "시작 인사 보기"),
    ("status", "오늘 사용량과 남은 금액 보기"),
    ("stickies", "스티키 보드 보기"),
    ("stop", "하던 일 멈추기"),
    ("help", "명령어 목록 보기"),
];

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
    match bot.set_my_commands(commands()).await {
        Ok(_) => logging::append("telegram_commands_registered"),
        Err(e) => logging::append(&format!("telegram_set_commands_error {e}")),
    }

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

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handle_message))
        .branch(Update::filter_message_reaction_updated().endpoint(handle_reaction));
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

    let chat_id = msg.chat.id;
    let user_message = if let Some(text) = msg.text() {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        text.to_string()
    } else if let Some(sticker) = msg.sticker() {
        let emoji = sticker.emoji.as_deref();
        logging::append(&format!(
            "telegram_sticker emoji={}",
            emoji.unwrap_or("none")
        ));
        normalize_sticker(emoji, sticker.set_name.as_deref())
    } else {
        logging::append("telegram_ignore non_text");
        return Ok(());
    };

    if let Some(reply) = msg.reply_to_message() {
        if is_board_reply(reply) {
            if let Ok(number) = user_message.trim().parse::<u64>() {
                match stickies::remove(number) {
                    Ok(board) => {
                        logging::append(&format!("telegram_sticky_remove number={number}"));
                        if let Err(e) = send_board(&bot, chat_id, &board).await {
                            logging::append(&format!("telegram_board_send_error {e}"));
                        }
                    }
                    Err(e) => {
                        logging::append(&format!(
                            "telegram_sticky_remove_error number={number} err={e}"
                        ));
                        bot.send_message(chat_id, format!("스티키를 삭제하지 못했습니다: {e}"))
                            .await?;
                    }
                }
                return Ok(());
            }
        }
    }

    match command_name(&user_message).as_deref() {
        Some("/start") => {
            bot.send_message(chat_id, start_text()).await?;
        }
        Some("/help") => {
            bot.send_message(chat_id, help_text()).await?;
        }
        Some("/stickies") => send_stickies(&state, chat_id).await?,
        Some("/stop") => stop_turn(&state, chat_id).await?,
        Some("/status") => send_status(&state, chat_id).await?,
        _ => start_turn(&state, chat_id, &user_message),
    }

    Ok(())
}

fn is_board_reply(reply: &Message) -> bool {
    reply.from.as_ref().is_some_and(|user| user.is_bot)
        && reply.text().is_some_and(stickies::is_board_text)
}

fn commands() -> Vec<BotCommand> {
    COMMANDS
        .iter()
        .map(|(command, description)| BotCommand::new(*command, *description))
        .collect()
}

fn help_text() -> String {
    let mut text = String::from("사용할 수 있는 명령어:");
    for (command, description) in COMMANDS {
        text.push_str(&format!("\n/{command} — {description}"));
    }
    text
}

fn start_text() -> String {
    format!(
        "이스크라입니다. 메시지를 보내면 대화합니다.\n\n{}",
        help_text()
    )
}

fn normalize_sticker(emoji: Option<&str>, set_name: Option<&str>) -> String {
    match emoji {
        Some(emoji) => match set_name {
            Some(set_name) => format!("[스티커 {emoji} ({set_name})]"),
            None => format!("[스티커 {emoji}]"),
        },
        None => "[스티커]".to_string(),
    }
}

async fn handle_reaction(
    state: Arc<ChannelState>,
    reaction: MessageReactionUpdated,
) -> HandlerResult {
    let Some(user) = reaction.user() else {
        logging::append("telegram_reaction_ignore no_user");
        return Ok(());
    };
    if user.id.0 != state.allowed_user_id {
        logging::append(&format!("telegram_ignore user_id={}", user.id.0));
        return Ok(());
    }

    let emoji = reaction
        .new_reaction
        .iter()
        .filter_map(|reaction| reaction.emoji())
        .map(|emoji| emoji.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let emoji = if emoji.is_empty() {
        "none".to_string()
    } else {
        emoji
    };
    logging::append(&format!(
        "telegram_reaction emoji={emoji} message_id={}",
        reaction.message_id.0
    ));
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
        let sticky_changed = task_state.tools.take_sticky_changed();

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

        if sticky_changed {
            match stickies::board() {
                Ok(board) => {
                    if let Err(e) = send_board(&task_state.bot, chat_id, &board).await {
                        logging::append(&format!("telegram_board_send_error {e}"));
                    }
                }
                Err(e) => logging::append(&format!("sticky_board_error {e}")),
            }
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
    let balance = match state.brain.balance().await {
        Ok(balance) => balance.display(),
        Err(_) => "unavailable".to_string(),
    };
    let text = format!(
        "모델: {}\n기억: {}\n오늘 토큰: {}\n잔액: {}",
        state.brain.model(),
        state.memory_status,
        usage::today().total,
        balance
    );
    state.bot.send_message(chat_id, text).await?;
    Ok(())
}

async fn send_stickies(state: &Arc<ChannelState>, chat_id: ChatId) -> HandlerResult {
    match stickies::board() {
        Ok(board) => {
            if let Err(e) = send_board(&state.bot, chat_id, &board).await {
                logging::append(&format!("telegram_board_send_error {e}"));
                state
                    .bot
                    .send_message(chat_id, "스티키 보드를 전송하지 못했습니다.")
                    .await?;
            }
        }
        Err(e) => {
            logging::append(&format!("sticky_board_error {e}"));
            state
                .bot
                .send_message(chat_id, "스티키 보드를 읽을 수 없습니다.")
                .await?;
        }
    }
    Ok(())
}

async fn send_board(bot: &Bot, chat_id: ChatId, board: &str) -> Result<(), String> {
    let sent = bot
        .send_message(chat_id, board)
        .await
        .map_err(|e| e.to_string())?;
    logging::append("telegram_board_sent");

    match bot.pin_chat_message(chat_id, sent.id).await {
        Ok(_) => logging::append("telegram_board_pinned"),
        Err(e) => logging::append(&format!("telegram_pin_error {e}")),
    }
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
    fn normalizes_stickers() {
        assert_eq!(
            normalize_sticker(Some("😀"), Some("Cats")),
            "[스티커 😀 (Cats)]"
        );
        assert_eq!(normalize_sticker(Some("😀"), None), "[스티커 😀]");
        assert_eq!(normalize_sticker(None, Some("Cats")), "[스티커]");
        assert_eq!(normalize_sticker(None, None), "[스티커]");
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
    fn registers_expected_commands_and_help() {
        let registered = commands();
        let names: Vec<&str> = registered
            .iter()
            .map(|command| command.command.as_str())
            .collect();
        assert_eq!(names, ["start", "status", "stickies", "stop", "help"]);
        assert!(
            registered
                .iter()
                .all(|command| !command.description.trim().is_empty())
        );
        assert!(
            registered
                .iter()
                .all(|command| command.description.chars().count() <= 256)
        );

        let help = help_text();
        for name in &names {
            assert!(help.contains(&format!("/{name}")));
        }
        assert!(start_text().ends_with(&help));
    }
}
