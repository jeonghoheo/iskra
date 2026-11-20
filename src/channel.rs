use crate::brain::{BalanceResponse, Brain};
use crate::locale::{self, Key, Locale};
use crate::logging;
use crate::loop_;
use crate::memory;
use crate::prompt;
use crate::stickies;
use crate::tools::Tools;
use crate::usage;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use teloxide::dptree;
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, Chat, ChatAction, MediaKind, MessageId, MessageKind, MessageReactionUpdated,
    ParseMode,
};
use teloxide::{ApiError, RequestError};
use tokio::task::JoinHandle;

const MAX_MESSAGE_CHARS: usize = 4096;
const TYPING_REFRESH_SECS: u64 = 4;
const COMMAND_KEYS: [(&str, Key); 5] = [
    ("start", Key::StartDescription),
    ("status", Key::StatusDescription),
    ("stickies", Key::StickiesDescription),
    ("stop", Key::StopDescription),
    ("help", Key::HelpDescription),
];

type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct TelegramConfig {
    token: String,
    allowed_user_id: u64,
}

impl TelegramConfig {
    fn from_env() -> Result<Self, String> {
        let token = env_non_empty("TELEGRAM_BOT_TOKEN")
            .ok_or_else(|| "TELEGRAM_BOT_TOKEN is missing from .env".to_string())?;
        let allowed_user_id = env_non_empty("ALLOWED_USER_ID")
            .ok_or_else(|| "ALLOWED_USER_ID is missing from .env".to_string())?
            .parse::<u64>()
            .map_err(|_| "ALLOWED_USER_ID must be a number".to_string())?;
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
    identity: prompt::Identity,
    allowed_user_id: u64,
    current_turn: Mutex<Option<JoinHandle<()>>>,
}

pub async fn run(
    brain: Brain,
    tools: Tools,
    identity: prompt::Identity,
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
    register_commands(&bot).await;

    let state = Arc::new(ChannelState {
        bot: bot.clone(),
        brain,
        tools,
        identity,
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

    let lang = Locale::from_language_code(from.language_code.as_deref());
    locale::set_latest(lang);

    let chat_id = msg.chat.id;
    if !msg.chat.is_private() {
        leave_non_private(&bot, &msg.chat, lang).await;
        return Ok(());
    }

    let forwarded = msg.forward_origin().is_some();
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
        if matches!(&msg.kind, MessageKind::Common(_)) {
            let kind = non_text_kind(&msg);
            logging::append(&format!("telegram_non_text_notice kind={kind}"));
            bot.send_message(chat_id, locale::text(lang, Key::NonTextNotice))
                .await?;
        } else {
            logging::append("telegram_ignore non_text");
        }
        return Ok(());
    };

    if forwarded {
        let wrapped = prompt::wrap_forwarded(&user_message);
        start_turn(&state, chat_id, &wrapped, lang);
        return Ok(());
    }

    if let Some(reply) = msg.reply_to_message() {
        if is_board_reply(reply) {
            if let Ok(number) = user_message.trim().parse::<u64>() {
                match stickies::remove(number) {
                    Ok(_) => {
                        logging::append(&format!("telegram_sticky_remove number={number}"));
                        let board = match stickies::board(lang) {
                            Ok(board) => board,
                            Err(e) => {
                                logging::append(&format!("sticky_board_error {e}"));
                                bot.send_message(
                                    chat_id,
                                    locale::with(
                                        lang,
                                        Key::StickyRemovedButBoardReadFailed,
                                        number,
                                    ),
                                )
                                .await?;
                                return Ok(());
                            }
                        };
                        match sync_board(&bot, chat_id, &board, BoardMode::Update).await {
                            Ok(BoardOutcome::EmptyNoBoard) => {
                                bot.send_message(chat_id, board).await?;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                logging::append(&format!("telegram_board_sync_error {e}"));
                                bot.send_message(
                                    chat_id,
                                    locale::text(lang, Key::StickyBoardSyncFailed),
                                )
                                .await?;
                            }
                        }
                    }
                    Err(e) => {
                        logging::append(&format!(
                            "telegram_sticky_remove_error number={number} err={e}"
                        ));
                        bot.send_message(chat_id, locale::with(lang, Key::StickyRemoveFailed, e))
                            .await?;
                    }
                }
                return Ok(());
            }
        }
    }

    match command_name(&user_message).as_deref() {
        Some("/start") => {
            bot.send_message(chat_id, start_text(lang)).await?;
        }
        Some("/help") => {
            bot.send_message(chat_id, help_text(lang)).await?;
        }
        Some("/stickies") => handle_stickies_command(&state, chat_id, &user_message, lang).await?,
        Some("/stop") => stop_turn(&state, chat_id, lang).await?,
        Some("/status") => send_status(&state, chat_id, lang).await?,
        _ => start_turn(&state, chat_id, &user_message, lang),
    }

    Ok(())
}

fn is_board_reply(reply: &Message) -> bool {
    reply.from.as_ref().is_some_and(|user| user.is_bot)
        && reply.text().is_some_and(stickies::is_board_text)
}

fn chat_type(chat: &Chat) -> &'static str {
    if chat.is_group() {
        "group"
    } else if chat.is_supergroup() {
        "supergroup"
    } else if chat.is_channel() {
        "channel"
    } else {
        "private"
    }
}

async fn leave_non_private(bot: &Bot, chat: &Chat, lang: Locale) {
    let kind = chat_type(chat);
    if let Err(e) = bot
        .send_message(chat.id, locale::text(lang, Key::PrivateOnly))
        .await
    {
        logging::append(&format!("telegram_send_failed {e}"));
    }
    let left = match bot.leave_chat(chat.id).await {
        Ok(_) => true,
        Err(e) => {
            logging::append(&format!("telegram_leave_error {e}"));
            false
        }
    };
    logging::append(&format!(
        "telegram_non_private chat_type={kind} left={left}"
    ));
}

fn non_text_kind(msg: &Message) -> &'static str {
    let MessageKind::Common(common) = &msg.kind else {
        return "other";
    };
    match &common.media_kind {
        MediaKind::Animation(_) => "animation",
        MediaKind::Audio(_) => "audio",
        MediaKind::Contact(_) => "contact",
        MediaKind::Document(_) => "document",
        MediaKind::Game(_) => "game",
        MediaKind::Location(_) => "location",
        MediaKind::PaidMedia(_) => "paid_media",
        MediaKind::Photo(_) => "photo",
        MediaKind::Poll(_) => "poll",
        MediaKind::Checklist(_) => "checklist",
        MediaKind::Sticker(_) => "sticker",
        MediaKind::Story(_) => "story",
        MediaKind::Text(_) => "text",
        MediaKind::Venue(_) => "venue",
        MediaKind::Video(_) => "video",
        MediaKind::VideoNote(_) => "video_note",
        MediaKind::Voice(_) => "voice",
        MediaKind::Migration(_) => "migration",
    }
}

async fn register_commands(bot: &Bot) {
    for lang in Locale::ALL {
        let request = bot.set_my_commands(commands(lang));
        let request = match lang {
            Locale::En => request,
            Locale::Ko | Locale::Ru => request.language_code(lang.code()),
        };
        match request.await {
            Ok(_) => logging::append(&format!(
                "telegram_commands_registered locale={}",
                lang.code()
            )),
            Err(e) => logging::append(&format!(
                "telegram_set_commands_error locale={} err={e}",
                lang.code()
            )),
        }
    }
}

fn commands(lang: Locale) -> Vec<BotCommand> {
    COMMAND_KEYS
        .iter()
        .map(|(command, key)| BotCommand::new(*command, locale::text(lang, *key)))
        .collect()
}

fn help_text(lang: Locale) -> String {
    let mut text = String::from(locale::text(lang, Key::HelpHeader));
    for (command, key) in COMMAND_KEYS {
        text.push_str(&format!("\n/{command} — {}", locale::text(lang, key)));
    }
    text.push_str("\n\n");
    text.push_str(locale::text(lang, Key::BoardFooter));
    text
}

fn start_text(lang: Locale) -> String {
    locale::text(lang, Key::StartGreeting).to_string()
}

fn normalize_sticker(emoji: Option<&str>, set_name: Option<&str>) -> String {
    match emoji {
        Some(emoji) => match set_name {
            Some(set_name) => format!("[sticker {emoji} ({set_name})]"),
            None => format!("[sticker {emoji}]"),
        },
        None => "[sticker]".to_string(),
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

    let lang = Locale::from_language_code(user.language_code.as_deref());
    locale::set_latest(lang);

    if !reaction.chat.is_private() {
        leave_non_private(&state.bot, &reaction.chat, lang).await;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StickiesCommand {
    Show,
    Remove(u64),
    Usage,
}

fn parse_stickies_command(text: &str) -> StickiesCommand {
    let mut parts = text.split_whitespace();
    let Some(command) = parts.next() else {
        return StickiesCommand::Usage;
    };
    if command.split('@').next().unwrap_or(command) != "/stickies" {
        return StickiesCommand::Usage;
    }
    match parts.collect::<Vec<_>>().as_slice() {
        [] => StickiesCommand::Show,
        [number] => number
            .parse()
            .map(StickiesCommand::Remove)
            .unwrap_or(StickiesCommand::Usage),
        [subcommand, number] if matches!(*subcommand, "remove" | "delete" | "del") => number
            .parse()
            .map(StickiesCommand::Remove)
            .unwrap_or(StickiesCommand::Usage),
        _ => StickiesCommand::Usage,
    }
}

async fn handle_stickies_command(
    state: &Arc<ChannelState>,
    chat_id: ChatId,
    text: &str,
    lang: Locale,
) -> HandlerResult {
    match parse_stickies_command(text) {
        StickiesCommand::Show => send_stickies(state, chat_id, lang).await,
        StickiesCommand::Remove(number) => {
            match stickies::remove(number) {
                Ok(_) => {
                    logging::append(&format!("telegram_sticky_remove number={number}"));
                    let board = match stickies::board(lang) {
                        Ok(board) => board,
                        Err(e) => {
                            logging::append(&format!("sticky_board_error {e}"));
                            state
                                .bot
                                .send_message(
                                    chat_id,
                                    locale::with(
                                        lang,
                                        Key::StickyRemovedButBoardReadFailed,
                                        number,
                                    ),
                                )
                                .await?;
                            return Ok(());
                        }
                    };
                    match sync_board(&state.bot, chat_id, &board, BoardMode::Update).await {
                        Ok(_) => {
                            state
                                .bot
                                .send_message(
                                    chat_id,
                                    locale::with(lang, Key::StickyRemoved, number),
                                )
                                .await?;
                        }
                        Err(e) => {
                            logging::append(&format!("telegram_board_sync_error {e}"));
                            state
                                .bot
                                .send_message(
                                    chat_id,
                                    locale::with(
                                        lang,
                                        Key::StickyRemovedButBoardUpdateFailed,
                                        number,
                                    ),
                                )
                                .await?;
                        }
                    }
                }
                Err(e) => {
                    logging::append(&format!(
                        "telegram_sticky_remove_error number={number} err={e}"
                    ));
                    state
                        .bot
                        .send_message(chat_id, locale::with(lang, Key::StickyRemoveFailed, e))
                        .await?;
                }
            }
            Ok(())
        }
        StickiesCommand::Usage => {
            state
                .bot
                .send_message(chat_id, locale::text(lang, Key::StickiesUsage))
                .await?;
            Ok(())
        }
    }
}

fn turn_error_text(lang: Locale, error: &loop_::TurnError) -> String {
    match error {
        loop_::TurnError::Timeout(_) | loop_::TurnError::Connection(_) => {
            locale::text(lang, Key::ErrorTimeout).to_string()
        }
        loop_::TurnError::Auth(_) => locale::text(lang, Key::ErrorAuth).to_string(),
        loop_::TurnError::Payment(_) => locale::text(lang, Key::ErrorPayment).to_string(),
        loop_::TurnError::Busy(_) => locale::text(lang, Key::ErrorBusy).to_string(),
        loop_::TurnError::RepeatedToolError(detail) => locale::with(
            lang,
            Key::ErrorRepeatedTool,
            locale::text(lang, repeated_reason_key(detail)),
        ),
        loop_::TurnError::MaxRoundtrips(_) => {
            locale::text(lang, Key::ErrorMaxRoundtrips).to_string()
        }
        loop_::TurnError::EmptyResponse(_) => locale::text(lang, Key::ErrorEmpty).to_string(),
        loop_::TurnError::Other(_) => locale::text(lang, Key::ErrorOther).to_string(),
    }
}

fn repeated_reason_key(detail: &str) -> Key {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("not found")
        || lower.contains("no such file")
        || lower.contains("does not exist")
    {
        Key::ReasonNotFound
    } else if lower.contains("refus")
        || lower.contains("denied")
        || lower.contains("not allowed")
        || lower.contains("outside workspace")
    {
        Key::ReasonAccessRefused
    } else if lower.contains("timed out") || lower.contains("timeout") {
        Key::ReasonTooLong
    } else if lower.contains("parse") || lower.contains("json") || lower.contains("invalid") {
        Key::ReasonInvalid
    } else {
        Key::ReasonGeneric
    }
}

fn start_turn(state: &Arc<ChannelState>, chat_id: ChatId, text: &str, lang: Locale) {
    let busy = {
        let slot = state.current_turn.lock().unwrap();
        slot.as_ref().is_some_and(|handle| !handle.is_finished())
    };
    if busy {
        logging::append("telegram_busy");
        let bot = state.bot.clone();
        tokio::spawn(async move {
            let _ = bot
                .send_message(chat_id, locale::text(lang, Key::BusyNotice))
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

        let (system_prompt, _) =
            prompt::turn_prompt(&task_state.identity, Path::new(memory::MEMORY_DIR));
        let reply =
            loop_::run_turn(&task_state.brain, &task_state.tools, &system_prompt, &input).await;
        drop(typing_guard);
        let sticky_changed = task_state.tools.take_sticky_changed();

        let answer = match reply {
            Ok(answer) => answer,
            Err(e) => {
                logging::append(&format!("telegram_turn_error {e}"));
                turn_error_text(lang, &e)
            }
        };
        if let Err(e) = send_reply(&task_state.bot, chat_id, &answer).await {
            logging::append(&format!("telegram_send_failed {e}"));
        }

        if sticky_changed {
            match stickies::board(lang) {
                Ok(board) => {
                    if let Err(e) =
                        sync_board(&task_state.bot, chat_id, &board, BoardMode::Update).await
                    {
                        logging::append(&format!("telegram_board_sync_error {e}"));
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

async fn stop_turn(state: &Arc<ChannelState>, chat_id: ChatId, lang: Locale) -> HandlerResult {
    let handle = { state.current_turn.lock().unwrap().take() };
    let message = match handle {
        Some(handle) if !handle.is_finished() => {
            handle.abort();
            logging::append("telegram_stop aborted");
            locale::text(lang, Key::StopStopped)
        }
        _ => {
            logging::append("telegram_stop idle");
            locale::text(lang, Key::StopNoActive)
        }
    };
    state.bot.send_message(chat_id, message).await?;
    Ok(())
}

async fn send_status(state: &Arc<ChannelState>, chat_id: ChatId, lang: Locale) -> HandlerResult {
    let balance = match state.brain.balance().await {
        Ok(balance) => balance_display(lang, &balance),
        Err(_) => locale::text(lang, Key::Unavailable).to_string(),
    };
    let loaded = memory::load_from(Path::new(memory::MEMORY_DIR));
    let memory = locale::memory_status(lang, loaded.truncated, loaded.warnings.len());
    let usage = usage::today();
    let cost = match usage::estimate_cost(state.brain.model(), &usage) {
        Some(cost) => locale::with(lang, Key::CostEstimate, format!("${cost:.6}")),
        None => locale::text(lang, Key::CostUnknown).to_string(),
    };
    let text = status_text(
        lang,
        state.brain.model(),
        &memory,
        usage.total,
        &cost,
        &balance,
    );
    state.bot.send_message(chat_id, text).await?;
    Ok(())
}

fn status_text(
    lang: Locale,
    model: &str,
    memory: &str,
    tokens: u64,
    cost: &str,
    balance: &str,
) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        locale::with(lang, Key::StatusCost, cost),
        locale::with(lang, Key::StatusBalance, balance),
        locale::with(lang, Key::StatusModel, model),
        locale::with(lang, Key::StatusMemory, memory),
        locale::with(lang, Key::StatusTokens, tokens)
    )
}

fn balance_display(lang: Locale, balance: &BalanceResponse) -> String {
    if balance.balance_infos.is_empty() {
        return locale::text(lang, Key::Unavailable).to_string();
    }
    let mut text = balance
        .balance_infos
        .iter()
        .map(|info| format!("{} {}", info.total_balance, info.currency))
        .collect::<Vec<_>>()
        .join(", ");
    if !balance.is_available {
        text.push_str(locale::text(lang, Key::BalanceInsufficient));
    }
    text
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoardPlan {
    Send,
    Edit(MessageId),
    Teardown(MessageId),
    EmptyNoBoard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoardOutcome {
    Updated,
    Unchanged,
    TornDown,
    EmptyNoBoard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoardMode {
    Update,
    Show,
}

fn plan_board(empty: bool, state: Option<stickies::BoardState>, chat_id: i64) -> BoardPlan {
    let live = state.filter(|state| state.chat_id == chat_id);
    if empty {
        match live {
            Some(state) => BoardPlan::Teardown(MessageId(state.message_id)),
            None => BoardPlan::EmptyNoBoard,
        }
    } else {
        match live {
            Some(state) => BoardPlan::Edit(MessageId(state.message_id)),
            None => BoardPlan::Send,
        }
    }
}

fn fallback_after_edit_failure(plan: BoardPlan) -> BoardPlan {
    match plan {
        BoardPlan::Edit(_) => BoardPlan::Send,
        other => other,
    }
}

async fn send_stickies(state: &Arc<ChannelState>, chat_id: ChatId, lang: Locale) -> HandlerResult {
    match stickies::board(lang) {
        Ok(board) => {
            if let Err(e) = sync_board(&state.bot, chat_id, &board, BoardMode::Show).await {
                logging::append(&format!("telegram_board_sync_error {e}"));
                state
                    .bot
                    .send_message(chat_id, locale::text(lang, Key::StickyBoardDisplayFailed))
                    .await?;
            }
        }
        Err(e) => {
            logging::append(&format!("sticky_board_error {e}"));
            state
                .bot
                .send_message(chat_id, locale::text(lang, Key::StickyBoardReadFailed))
                .await?;
        }
    }
    Ok(())
}

async fn remove_old_board(bot: &Bot, chat_id: ChatId, message_id: MessageId) {
    if let Err(e) = bot.unpin_chat_message(chat_id).message_id(message_id).await {
        logging::append(&format!("telegram_unpin_error {e}"));
    }
    if let Err(e) = bot.delete_message(chat_id, message_id).await {
        logging::append(&format!("telegram_delete_error {e}"));
    }
}

async fn show_board(bot: &Bot, chat_id: ChatId, board: &str) -> Result<BoardOutcome, String> {
    let state = match stickies::load_board_state() {
        Ok(state) => state,
        Err(e) => {
            logging::append(&format!("sticky_board_state_error {e}"));
            None
        }
    };
    let live = state.filter(|state| state.chat_id == chat_id.0);

    if stickies::is_empty_board(board) {
        if let Some(state) = live {
            remove_old_board(bot, chat_id, MessageId(state.message_id)).await;
            if let Err(e) = stickies::clear_board_state() {
                logging::append(&format!("sticky_board_state_clear_error {e}"));
            }
        }
        bot.send_message(chat_id, board)
            .await
            .map_err(|e| e.to_string())?;
        logging::append("telegram_board_shown_empty");
        return Ok(BoardOutcome::EmptyNoBoard);
    }

    if live.is_none() {
        let sent = bot
            .send_message(chat_id, board)
            .await
            .map_err(|e| e.to_string())?;
        logging::append("telegram_board_sent");
        match bot.pin_chat_message(chat_id, sent.id).await {
            Ok(_) => logging::append("telegram_board_pinned"),
            Err(e) => logging::append(&format!("telegram_pin_error {e}")),
        }
        let state = stickies::BoardState {
            chat_id: chat_id.0,
            message_id: sent.id.0,
        };
        if let Err(e) = stickies::save_board_state(&state) {
            logging::append(&format!("sticky_board_state_save_error {e}"));
        }
        return Ok(BoardOutcome::Updated);
    }

    bot.send_message(chat_id, board)
        .await
        .map_err(|e| e.to_string())?;
    logging::append("telegram_board_view_sent");
    Ok(BoardOutcome::Updated)
}

async fn sync_board(
    bot: &Bot,
    chat_id: ChatId,
    board: &str,
    mode: BoardMode,
) -> Result<BoardOutcome, String> {
    if mode == BoardMode::Show {
        return show_board(bot, chat_id, board).await;
    }

    let empty = stickies::is_empty_board(board);
    let state = match stickies::load_board_state() {
        Ok(state) => state,
        Err(e) => {
            logging::append(&format!("sticky_board_state_error {e}"));
            None
        }
    };
    let mut plan = plan_board(empty, state, chat_id.0);

    match plan {
        BoardPlan::Teardown(message_id) => {
            remove_old_board(bot, chat_id, message_id).await;
            if let Err(e) = stickies::clear_board_state() {
                logging::append(&format!("sticky_board_state_clear_error {e}"));
            }
            logging::append("telegram_board_torn_down");
            return Ok(BoardOutcome::TornDown);
        }
        BoardPlan::EmptyNoBoard => return Ok(BoardOutcome::EmptyNoBoard),
        BoardPlan::Edit(_) | BoardPlan::Send => {}
    }

    let mut refreshed_fallback = false;
    if let BoardPlan::Edit(message_id) = plan {
        match bot.edit_message_text(chat_id, message_id, board).await {
            Ok(_) => {
                logging::append("telegram_board_edited");
                return Ok(BoardOutcome::Updated);
            }
            Err(RequestError::Api(ApiError::MessageNotModified)) => {
                logging::append("telegram_board_unchanged");
                return Ok(BoardOutcome::Unchanged);
            }
            Err(e) => {
                logging::append(&format!("telegram_board_edit_error {e}"));
                plan = fallback_after_edit_failure(plan);
                refreshed_fallback = matches!(plan, BoardPlan::Send);
            }
        }
    }

    let sent = bot
        .send_message(chat_id, board)
        .await
        .map_err(|e| e.to_string())?;
    logging::append("telegram_board_sent");
    match bot.pin_chat_message(chat_id, sent.id).await {
        Ok(_) => logging::append("telegram_board_pinned"),
        Err(e) => logging::append(&format!("telegram_pin_error {e}")),
    }
    let state = stickies::BoardState {
        chat_id: chat_id.0,
        message_id: sent.id.0,
    };
    if let Err(e) = stickies::save_board_state(&state) {
        logging::append(&format!("sticky_board_state_save_error {e}"));
    }
    if refreshed_fallback {
        logging::append("telegram_board_refreshed_fallback");
    }
    Ok(BoardOutcome::Updated)
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
        let text = "a".repeat(MAX_MESSAGE_CHARS * 2 + 5);
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
        let chunks = split_message("short reply");
        assert_eq!(chunks, vec!["short reply"]);
    }

    #[test]
    fn normalizes_stickers() {
        assert_eq!(
            normalize_sticker(Some("😀"), Some("Cats")),
            "[sticker 😀 (Cats)]"
        );
        assert_eq!(normalize_sticker(Some("😀"), None), "[sticker 😀]");
        assert_eq!(normalize_sticker(None, Some("Cats")), "[sticker]");
        assert_eq!(normalize_sticker(None, None), "[sticker]");
    }

    #[test]
    fn parses_only_known_commands() {
        assert_eq!(command_name("/start").as_deref(), Some("/start"));
        assert_eq!(
            command_name("/status@iskra_bot").as_deref(),
            Some("/status")
        );
        assert_eq!(command_name("/unknown").as_deref(), Some("/unknown"));
        assert_eq!(command_name("hello").as_deref(), None);
    }

    #[test]
    fn registers_expected_commands_and_help() {
        for lang in Locale::ALL {
            let registered = commands(lang);
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

            let help = help_text(lang);
            for name in &names {
                assert!(help.contains(&format!("/{name}")));
            }
            assert!(help.contains("/stickies remove N"));
            assert_eq!(start_text(lang), locale::text(lang, Key::StartGreeting));
            assert!(start_text(lang).contains("/status"));
            assert!(start_text(lang).contains("/stickies"));
            assert!(start_text(lang).contains("/stop"));
            assert!(start_text(lang).contains("/help"));
        }
    }

    #[test]
    fn status_lines_follow_the_approved_order() {
        let text = status_text(Locale::En, "model-x", "ok", 12345, "~$0.040000", "4.20 USD");
        let cost = text.find("Spent today:").unwrap();
        let balance = text.find("Balance:").unwrap();
        let model = text.find("Model:").unwrap();
        let memory = text.find("Memory:").unwrap();
        let tokens = text.find("Tokens today:").unwrap();
        assert!(cost < balance && balance < model && model < memory && memory < tokens);
    }

    #[test]
    fn maps_every_error_category_to_plain_user_text() {
        for lang in Locale::ALL {
            assert_eq!(
                turn_error_text(lang, &loop_::TurnError::Timeout("raw timeout".into())),
                locale::text(lang, Key::ErrorTimeout)
            );
            assert_eq!(
                turn_error_text(lang, &loop_::TurnError::Connection("raw conn".into())),
                locale::text(lang, Key::ErrorTimeout)
            );
            assert_eq!(
                turn_error_text(lang, &loop_::TurnError::Auth("raw 401".into())),
                locale::text(lang, Key::ErrorAuth)
            );
            assert_eq!(
                turn_error_text(lang, &loop_::TurnError::Payment("raw 402".into())),
                locale::text(lang, Key::ErrorPayment)
            );
            assert_eq!(
                turn_error_text(lang, &loop_::TurnError::Busy("raw 429".into())),
                locale::text(lang, Key::ErrorBusy)
            );
            assert_eq!(
                turn_error_text(lang, &loop_::TurnError::MaxRoundtrips("raw".into())),
                locale::text(lang, Key::ErrorMaxRoundtrips)
            );
            assert_eq!(
                turn_error_text(lang, &loop_::TurnError::EmptyResponse("raw".into())),
                locale::text(lang, Key::ErrorEmpty)
            );
            assert_eq!(
                turn_error_text(lang, &loop_::TurnError::Other("raw secret".into())),
                locale::text(lang, Key::ErrorOther)
            );
            assert!(
                !turn_error_text(lang, &loop_::TurnError::Other("raw secret".into()))
                    .contains("raw secret")
            );

            let repeated = turn_error_text(
                lang,
                &loop_::TurnError::RepeatedToolError("read_file: No such file or directory".into()),
            );
            assert_eq!(
                repeated,
                locale::with(
                    lang,
                    Key::ErrorRepeatedTool,
                    locale::text(lang, Key::ReasonNotFound)
                )
            );
            assert!(!repeated.contains("read_file"));
        }
    }

    #[test]
    fn labels_non_text_media_kinds() {
        let photo: Message = serde_json::from_str(
            r#"{"message_id":1,"date":0,"chat":{"id":1,"type":"private","first_name":"A"},"photo":[{"file_id":"f","file_unique_id":"u","width":1,"height":1}]}"#,
        )
        .unwrap();
        assert!(matches!(&photo.kind, MessageKind::Common(_)));
        assert_eq!(non_text_kind(&photo), "photo");

        let voice: Message = serde_json::from_str(
            r#"{"message_id":1,"date":0,"chat":{"id":1,"type":"private","first_name":"A"},"voice":{"file_id":"f","file_unique_id":"u","duration":3,"mime_type":"audio/ogg"}}"#,
        )
        .unwrap();
        assert_eq!(non_text_kind(&voice), "voice");
    }

    #[test]
    fn labels_chat_types_for_logging() {
        let group: Chat = serde_json::from_str(r#"{"id":1,"type":"group","title":"G"}"#).unwrap();
        let supergroup: Chat =
            serde_json::from_str(r#"{"id":2,"type":"supergroup","title":"S"}"#).unwrap();
        let channel: Chat =
            serde_json::from_str(r#"{"id":3,"type":"channel","title":"C"}"#).unwrap();
        let private: Chat =
            serde_json::from_str(r#"{"id":4,"type":"private","first_name":"A"}"#).unwrap();
        assert_eq!(chat_type(&group), "group");
        assert_eq!(chat_type(&supergroup), "supergroup");
        assert_eq!(chat_type(&channel), "channel");
        assert_eq!(chat_type(&private), "private");
    }

    #[test]
    fn parses_stickies_commands() {
        assert_eq!(parse_stickies_command("/stickies"), StickiesCommand::Show);
        assert_eq!(
            parse_stickies_command("/stickies@iskra_bot"),
            StickiesCommand::Show
        );
        assert_eq!(
            parse_stickies_command("/stickies remove 3"),
            StickiesCommand::Remove(3)
        );
        assert_eq!(
            parse_stickies_command("/stickies delete 4"),
            StickiesCommand::Remove(4)
        );
        assert_eq!(
            parse_stickies_command("/stickies 5"),
            StickiesCommand::Remove(5)
        );
        assert_eq!(
            parse_stickies_command("/stickies remove"),
            StickiesCommand::Usage
        );
        assert_eq!(
            parse_stickies_command("/stickies remove x"),
            StickiesCommand::Usage
        );
        assert_eq!(
            parse_stickies_command("/stickies remove 1 extra"),
            StickiesCommand::Usage
        );
    }

    #[test]
    fn plans_board_state_transitions() {
        let state = stickies::BoardState {
            chat_id: 10,
            message_id: 5,
        };

        assert_eq!(plan_board(false, None, 10), BoardPlan::Send);
        assert_eq!(
            plan_board(false, Some(state), 10),
            BoardPlan::Edit(MessageId(5))
        );
        assert_eq!(
            plan_board(true, Some(state), 10),
            BoardPlan::Teardown(MessageId(5))
        );
        assert_eq!(plan_board(true, None, 10), BoardPlan::EmptyNoBoard);

        assert_eq!(plan_board(false, Some(state), 11), BoardPlan::Send);
        assert_eq!(plan_board(true, Some(state), 11), BoardPlan::EmptyNoBoard);
    }

    #[test]
    fn falls_back_to_send_after_edit_failure() {
        assert_eq!(
            fallback_after_edit_failure(BoardPlan::Edit(MessageId(5))),
            BoardPlan::Send
        );
        assert_eq!(
            fallback_after_edit_failure(BoardPlan::Send),
            BoardPlan::Send
        );
        assert_eq!(
            fallback_after_edit_failure(BoardPlan::EmptyNoBoard),
            BoardPlan::EmptyNoBoard
        );
    }
}
