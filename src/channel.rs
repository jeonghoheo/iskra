use crate::brain::{BalanceResponse, Brain};
use crate::health::{self, Health};
use crate::instance::InstancePaths;
use crate::locale::{self, Key, Locale};
use crate::logging;
use crate::loop_;
use crate::memory;
use crate::prompt;
use crate::session::Session;
use crate::stickies;
use crate::supervisor::RunSupervisor;
use crate::tools::Tools;
use crate::usage;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use teloxide::dptree;
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, Chat, ChatAction, MediaKind, MessageId, MessageKind, MessageReactionUpdated,
    ParseMode,
};
use teloxide::update_listeners::polling_default;
use teloxide::{ApiError, RequestError};

const MAX_MESSAGE_CHARS: usize = 4096;
const TYPING_REFRESH_SECS: u64 = 4;
const STARTUP_WINDOW_SECS: u64 = 20;
const STARTUP_ATTEMPT_TIMEOUT_SECS: u64 = 5;
const HEALTH_INTERVAL_SECS: u64 = 30;
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
    session: Session,
    paths: InstancePaths,
    allowed_user_id: u64,
    supervisor: RunSupervisor,
}

pub async fn run(
    brain: Brain,
    tools: Tools,
    identity: prompt::Identity,
    session: Session,
    paths: InstancePaths,
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

    let health = Arc::new(Health::new(paths.memory.join(health::FILE_NAME)));
    let now = health::now_unix();
    if let Err(e) = health.write_force(now) {
        logging::append(&format!("health_write_error {e}"));
    }

    if let Err(reason) = wait_for_telegram(&bot, &health).await {
        logging::append(&format!("telegram_fatal reason={reason}"));
        return Err(format!("Telegram startup failed: {reason}").into());
    }

    logging::append("telegram_start");
    register_commands(&bot).await;

    let state = Arc::new(ChannelState {
        bot: bot.clone(),
        brain,
        tools,
        identity,
        session,
        paths,
        allowed_user_id,
        supervisor: RunSupervisor::default(),
    });

    if let Err(error) = serve(
        bot,
        state,
        health.clone(),
        Duration::from_secs(HEALTH_INTERVAL_SECS),
    )
    .await
    {
        let kind = request_error_kind(&error);
        logging::append(&format!("telegram_fatal reason=dispatch kind={kind}"));
        let now = health::now_unix();
        let _ = health.record_api_error(now, kind);
        if let Err(e) = health.write_force(now) {
            logging::append(&format!("health_write_error {e}"));
        }
        return Err(format!("Telegram dispatch failed: {kind}").into());
    }

    Ok(())
}

async fn serve(
    bot: Bot,
    state: Arc<ChannelState>,
    health: Arc<Health>,
    watchdog_interval: Duration,
) -> Result<(), RequestError> {
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handle_message))
        .branch(Update::filter_message_reaction_updated().endpoint(handle_reaction));
    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .dependencies(dptree::deps![state])
        .build();

    let listener = polling_default(bot.clone()).await;
    let watchdog = spawn_health_watchdog(bot, health.clone(), watchdog_interval);

    let error_health = health.clone();
    let error_handler = Arc::new(move |error: RequestError| {
        let health = error_health.clone();
        async move {
            let now = health::now_unix();
            let kind = request_error_kind(&error);
            let update = health.record_poll_error(now, kind);
            if update.state_changed {
                logging::append(&format!("telegram_health state={}", update.state.as_str()));
            }
            if update.failures <= 3 || update.failures.is_multiple_of(10) {
                logging::append(&format!(
                    "telegram_poll_error attempt={} kind={kind} state={}",
                    update.failures,
                    update.state.as_str()
                ));
            }
            if let Err(e) = health.write_if_due(now) {
                logging::append(&format!("health_write_error {e}"));
            }
        }
    });

    let result = dispatcher
        .try_dispatch_with_listener(listener, error_handler)
        .await;
    watchdog.abort();
    result
}

async fn wait_for_telegram(bot: &Bot, health: &Health) -> Result<(), &'static str> {
    let deadline = std::time::Instant::now() + Duration::from_secs(STARTUP_WINDOW_SECS);
    let mut delay = Duration::ZERO;
    let mut attempt = 0u32;
    let mut last_kind = "startup_exhausted";

    loop {
        if !delay.is_zero() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            tokio::time::sleep(delay.min(remaining)).await;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }

        attempt = attempt.saturating_add(1);
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let attempt_timeout = remaining.min(Duration::from_secs(STARTUP_ATTEMPT_TIMEOUT_SECS));
        let started = health::now_unix();

        match tokio::time::timeout(attempt_timeout, bot.get_me().send()).await {
            Ok(Ok(_)) => {
                let now = health::now_unix();
                health.record_api_ok(now);
                if let Err(e) = health.write_if_due(now) {
                    logging::append(&format!("health_write_error {e}"));
                }
                return Ok(());
            }
            Ok(Err(error)) => {
                last_kind = request_error_kind(&error);
            }
            Err(_) => {
                last_kind = "timeout";
            }
        }

        let update = health.record_api_error(started, last_kind);
        logging::append(&format!(
            "telegram_start_retry attempt={attempt} delay_s={} kind={last_kind} state={}",
            delay.as_secs(),
            update.state.as_str()
        ));
        if let Err(e) = health.write_if_due(started) {
            logging::append(&format!("health_write_error {e}"));
        }
        if last_kind == "auth" {
            break;
        }
        delay = next_startup_delay(delay);
    }

    let now = health::now_unix();
    health.record_api_error(now, last_kind);
    if let Err(e) = health.write_force(now) {
        logging::append(&format!("health_write_error {e}"));
    }
    Err(last_kind)
}

fn next_startup_delay(current: Duration) -> Duration {
    if current.is_zero() {
        Duration::from_secs(1)
    } else {
        (current * 2).min(Duration::from_secs(8))
    }
}

fn spawn_health_watchdog(
    bot: Bot,
    health: Arc<Health>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            let now = health::now_unix();
            let update = match bot.get_me().send().await {
                Ok(_) => health.record_api_ok(now),
                Err(error) => {
                    let kind = request_error_kind(&error);
                    health.record_api_error(now, kind)
                }
            };

            if update.state_changed {
                logging::append(&format!("telegram_health state={}", update.state.as_str()));
            }
            if let Err(e) = health.write_if_due(now) {
                logging::append(&format!("health_write_error {e}"));
            }
        }
    })
}

fn request_error_kind(error: &RequestError) -> &'static str {
    match error {
        RequestError::Api(ApiError::InvalidToken) => "auth",
        RequestError::Api(_) => "api",
        RequestError::Network(_) => "network",
        RequestError::InvalidJson { .. } => "protocol",
        RequestError::Io(_) => "io",
        RequestError::RetryAfter(_) => "rate_limit",
        RequestError::MigrateToChatId(_) => "chat_migrated",
    }
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
                match stickies::remove_at(&state.paths.memory, number, lang) {
                    Ok(_) => {
                        logging::append(&format!("telegram_sticky_remove number={number}"));
                        let board = match stickies::board_at(&state.paths.memory, lang) {
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
                        match sync_board(
                            &bot,
                            chat_id,
                            &board,
                            BoardMode::Update,
                            &state.paths.memory,
                        )
                        .await
                        {
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
            match stickies::remove_at(&state.paths.memory, number, lang) {
                Ok(_) => {
                    logging::append(&format!("telegram_sticky_remove number={number}"));
                    let board = match stickies::board_at(&state.paths.memory, lang) {
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
                    match sync_board(
                        &state.bot,
                        chat_id,
                        &board,
                        BoardMode::Update,
                        &state.paths.memory,
                    )
                    .await
                    {
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
        loop_::TurnError::Session(_) => locale::text(lang, Key::ErrorSession).to_string(),
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
    let task_state = Arc::clone(state);
    let input = text.to_string();
    let started = state.supervisor.start(async move {
        let (typing_stop, typing_rx) = tokio::sync::watch::channel(false);
        let typing_guard = TypingGuard(typing_stop);
        tokio::spawn(typing_loop(task_state.bot.clone(), chat_id, typing_rx));

        let (system_prompt, _) =
            prompt::turn_prompt(&task_state.identity, &task_state.paths.memory);
        let reply = loop_::run_turn(
            &task_state.brain,
            &task_state.tools,
            &task_state.session,
            &system_prompt,
            &input,
        )
        .await;
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
            match stickies::board_at(&task_state.paths.memory, lang) {
                Ok(board) => {
                    if let Err(e) = sync_board(
                        &task_state.bot,
                        chat_id,
                        &board,
                        BoardMode::Update,
                        &task_state.paths.memory,
                    )
                    .await
                    {
                        logging::append(&format!("telegram_board_sync_error {e}"));
                    }
                }
                Err(e) => logging::append(&format!("sticky_board_error {e}")),
            }
        }
    });
    if started.is_err() {
        logging::append("telegram_busy");
        let bot = state.bot.clone();
        tokio::spawn(async move {
            let _ = bot
                .send_message(chat_id, locale::text(lang, Key::BusyNotice))
                .await;
        });
    }
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
    let handle = state.supervisor.take_running();
    let message = match handle {
        Some(handle) => {
            let saved = state.session.stop_current();
            handle.abort();
            logging::append("telegram_stop aborted");
            match saved {
                Ok(_) => locale::text(lang, Key::StopStopped),
                Err(error) => {
                    logging::append(&format!("session_error {error}"));
                    locale::text(lang, Key::ErrorSession)
                }
            }
        }
        None => {
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
    let loaded = memory::load_from(&state.paths.memory);
    let memory = locale::memory_status(lang, loaded.truncated, loaded.warnings.len());
    let usage = usage::today();
    let cost = match usage::estimate_cost(state.brain.model(), &usage) {
        Some(cost) => locale::with(lang, Key::CostEstimate, format!("${cost:.6}")),
        None => locale::text(lang, Key::CostUnknown).to_string(),
    };
    let session = locale::session_status(lang, state.session.status());
    let text = status_text(
        lang,
        state.brain.model(),
        &memory,
        &session,
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
    session: &str,
    tokens: u64,
    cost: &str,
    balance: &str,
) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        locale::with(lang, Key::StatusCost, cost),
        locale::with(lang, Key::StatusBalance, balance),
        locale::with(lang, Key::StatusModel, model),
        locale::with(lang, Key::StatusMemory, memory),
        locale::with(lang, Key::StatusSession, session),
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
    match stickies::board_at(&state.paths.memory, lang) {
        Ok(board) => {
            if let Err(e) = sync_board(
                &state.bot,
                chat_id,
                &board,
                BoardMode::Show,
                &state.paths.memory,
            )
            .await
            {
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

async fn show_board(
    bot: &Bot,
    chat_id: ChatId,
    board: &str,
    memory_dir: &Path,
) -> Result<BoardOutcome, String> {
    let state = match stickies::load_board_state_at(memory_dir) {
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
            if let Err(e) = stickies::clear_board_state_at(memory_dir) {
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
        if let Err(e) = stickies::save_board_state_at(memory_dir, &state) {
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
    memory_dir: &Path,
) -> Result<BoardOutcome, String> {
    if mode == BoardMode::Show {
        return show_board(bot, chat_id, board, memory_dir).await;
    }

    let empty = stickies::is_empty_board(board);
    let state = match stickies::load_board_state_at(memory_dir) {
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
            if let Err(e) = stickies::clear_board_state_at(memory_dir) {
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
    if let Err(e) = stickies::save_board_state_at(memory_dir, &state) {
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
    use std::sync::Mutex;

    struct MockBotApi {
        address: String,
        messages: Arc<Mutex<Vec<serde_json::Value>>>,
        stop: std::sync::mpsc::Sender<()>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl MockBotApi {
        fn start() -> Self {
            use std::io::{BufRead, BufReader, Read, Write};
            use std::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let messages = Arc::new(Mutex::new(Vec::new()));
            let server_messages = Arc::clone(&messages);
            let (stop, stopped) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                while stopped.try_recv().is_err() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            let mut writer = stream.try_clone().unwrap();
                            let mut reader = BufReader::new(stream);
                            let mut request_line = String::new();
                            reader.read_line(&mut request_line).unwrap();
                            let mut content_length = 0usize;
                            loop {
                                let mut header = String::new();
                                reader.read_line(&mut header).unwrap();
                                if header == "\r\n" || header.is_empty() {
                                    break;
                                }
                                if let Some((name, value)) = header.split_once(':')
                                    && name.eq_ignore_ascii_case("content-length")
                                {
                                    content_length = value.trim().parse().unwrap();
                                }
                            }
                            let mut body = vec![0; content_length];
                            reader.read_exact(&mut body).unwrap();
                            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                            server_messages.lock().unwrap().push(body);
                            let response = serde_json::json!({
                                "ok": true,
                                "result": {
                                    "message_id": 1,
                                    "date": 0,
                                    "chat": {"id": 42, "type": "private"},
                                    "text": "ok"
                                }
                            });
                            let response = serde_json::to_vec(&response).unwrap();
                            write!(
                                writer,
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                response.len()
                            )
                            .unwrap();
                            writer.write_all(&response).unwrap();
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("mock Bot API accept failed: {error}"),
                    }
                }
            });
            Self {
                address,
                messages,
                stop,
                handle: Some(handle),
            }
        }

        fn url(&self) -> reqwest::Url {
            reqwest::Url::parse(&format!("http://{}", self.address)).unwrap()
        }
    }

    impl Drop for MockBotApi {
        fn drop(&mut self) {
            let _ = self.stop.send(());
            let _ = std::net::TcpStream::connect(&self.address);
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    #[tokio::test]
    async fn telegram_stop_persists_stopped_before_task_drop() {
        let dir = std::env::temp_dir().join(format!(
            "iskra-channel-stop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let session_path = dir.join("session.json");
        let session = Session::load(&session_path);
        let work = session.begin("stop this work").unwrap();
        let bot_api = MockBotApi::start();
        let supervisor = RunSupervisor::default();
        supervisor.start(std::future::pending::<()>()).unwrap();
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            brain: Brain::new(crate::brain::Config {
                api_key: "dummy".to_string(),
                base_url: "http://127.0.0.1:9/chat/completions".to_string(),
                model: "test-model".to_string(),
            }),
            tools: Tools::new_in(&dir.join("workspace"), &dir.join("memory")).unwrap(),
            identity: prompt::Identity {
                soul: prompt::DEFAULT_SOUL.to_string(),
                user: prompt::DEFAULT_USER.to_string(),
                warnings: Vec::new(),
                soul_source: prompt::IdentitySource::Default,
                user_source: prompt::IdentitySource::Default,
            },
            session: session.clone(),
            paths: InstancePaths {
                workspace: dir.join("workspace"),
                memory: dir.join("memory"),
                session: session_path.clone(),
            },
            allowed_user_id: 1,
            supervisor,
        });

        stop_turn(&state, ChatId(42), Locale::Ko).await.unwrap();

        let messages = bot_api.messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["chat_id"], 42);
        assert_eq!(
            messages[0]["text"].as_str(),
            Some(locale::text(Locale::Ko, Key::StopStopped))
        );
        drop(messages);
        drop(work);
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&session_path).unwrap()).unwrap();
        assert_eq!(stored["work"][0]["status"], "stopped");
        assert!(Session::load(&session_path).status().unwrap().unfinished);
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
        let text = status_text(
            Locale::En,
            "model-x",
            "ok",
            "2 saved / 2 in context",
            12345,
            "~$0.040000",
            "4.20 USD",
        );
        let cost = text.find("Spent today:").unwrap();
        let balance = text.find("Balance:").unwrap();
        let model = text.find("Model:").unwrap();
        let memory = text.find("Memory:").unwrap();
        let session = text.find("Workspace:").unwrap();
        let tokens = text.find("Tokens today:").unwrap();
        assert!(
            cost < balance
                && balance < model
                && model < memory
                && memory < session
                && session < tokens
        );
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
                turn_error_text(lang, &loop_::TurnError::Session("raw path".into())),
                locale::text(lang, Key::ErrorSession)
            );
            assert!(
                !turn_error_text(lang, &loop_::TurnError::Session("raw path".into()))
                    .contains("raw path")
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

    #[test]
    fn startup_backoff_is_bounded() {
        let mut delay = Duration::ZERO;
        let mut delays = Vec::new();
        for _ in 0..6 {
            delays.push(delay.as_secs());
            delay = next_startup_delay(delay);
        }
        assert_eq!(delays, [0, 1, 2, 4, 8, 8]);
    }

    #[test]
    fn classifies_invalid_token_as_auth_for_health() {
        let error = RequestError::Api(ApiError::InvalidToken);
        assert_eq!(request_error_kind(&error), "auth");
    }

    struct MockPollBotApi {
        address: String,
        ok: Arc<std::sync::atomic::AtomicBool>,
        stop: std::sync::mpsc::Sender<()>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl MockPollBotApi {
        fn start() -> Self {
            use std::io::{BufRead, BufReader, Read, Write};
            use std::net::TcpListener;
            use std::sync::atomic::Ordering;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let ok = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let server_ok = Arc::clone(&ok);
            let (stop, stopped) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                while stopped.try_recv().is_err() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            let mut writer = stream.try_clone().unwrap();
                            let mut reader = BufReader::new(stream);
                            let mut request_line = String::new();
                            if reader.read_line(&mut request_line).is_err() {
                                continue;
                            }
                            let mut content_length = 0usize;
                            loop {
                                let mut header = String::new();
                                if reader.read_line(&mut header).is_err() {
                                    break;
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

                            let path = request_line
                                .split_whitespace()
                                .nth(1)
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            if !server_ok.load(Ordering::Relaxed)
                                && (path.contains("getupdates") || path.contains("getme"))
                            {
                                continue;
                            }

                            let response = if path.contains("getme") {
                                serde_json::json!({
                                    "ok": true,
                                    "result": {
                                        "id": 1,
                                        "is_bot": true,
                                        "first_name": "Probe",
                                        "username": "probe_bot",
                                        "can_join_groups": false,
                                        "can_read_all_group_messages": false,
                                        "supports_inline_queries": false,
                                        "has_main_web_app": false
                                    }
                                })
                            } else if path.contains("getwebhookinfo") {
                                serde_json::json!({
                                    "ok": true,
                                    "result": {
                                        "url": "",
                                        "has_custom_certificate": false,
                                        "pending_update_count": 0
                                    }
                                })
                            } else if path.contains("getupdates") {
                                std::thread::sleep(Duration::from_millis(100));
                                serde_json::json!({"ok": true, "result": []})
                            } else {
                                serde_json::json!({"ok": true, "result": true})
                            };

                            let bytes = serde_json::to_vec(&response).unwrap();
                            let _ = write!(
                                writer,
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                bytes.len()
                            );
                            let _ = writer.write_all(&bytes);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("mock poll Bot API accept failed: {error}"),
                    }
                }
            });
            Self {
                address,
                ok,
                stop,
                handle: Some(handle),
            }
        }

        fn url(&self) -> reqwest::Url {
            reqwest::Url::parse(&format!("http://{}", self.address)).unwrap()
        }

        fn set_ok(&self, ok: bool) {
            self.ok.store(ok, std::sync::atomic::Ordering::Relaxed);
        }
    }

    impl Drop for MockPollBotApi {
        fn drop(&mut self) {
            let _ = self.stop.send(());
            let _ = std::net::TcpStream::connect(&self.address);
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    async fn wait_for_health(
        health: &Health,
        expected: health::HealthState,
        timeout: Duration,
    ) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if health.state(health::now_unix()) == expected {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_polling_failure_updates_health_and_recovers() {
        let dir = std::env::temp_dir().join(format!(
            "iskra-channel-health-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("workspace")).unwrap();
        std::fs::create_dir_all(dir.join("memory")).unwrap();

        let bot_api = MockPollBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let health_path = dir.join("memory").join(health::FILE_NAME);
        let health = Arc::new(Health::with_test_windows(&health_path, 2, 1, 0));
        let state = Arc::new(ChannelState {
            bot: bot.clone(),
            brain: Brain::new(crate::brain::Config {
                api_key: "dummy".to_string(),
                base_url: "http://127.0.0.1:9/chat/completions".to_string(),
                model: "test-model".to_string(),
            }),
            tools: Tools::new_in(&dir.join("workspace"), &dir.join("memory")).unwrap(),
            identity: prompt::Identity {
                soul: prompt::DEFAULT_SOUL.to_string(),
                user: prompt::DEFAULT_USER.to_string(),
                warnings: Vec::new(),
                soul_source: prompt::IdentitySource::Default,
                user_source: prompt::IdentitySource::Default,
            },
            session: Session::load(&dir.join("session.json")),
            paths: InstancePaths {
                workspace: dir.join("workspace"),
                memory: dir.join("memory"),
                session: dir.join("session.json"),
            },
            allowed_user_id: 1,
            supervisor: RunSupervisor::default(),
        });

        let task = tokio::spawn(serve(
            bot,
            state,
            health.clone(),
            Duration::from_millis(200),
        ));

        assert!(
            wait_for_health(&health, health::HealthState::Ok, Duration::from_secs(5)).await,
            "health never reached ok"
        );

        bot_api.set_ok(false);
        assert!(
            wait_for_health(
                &health,
                health::HealthState::Degraded,
                Duration::from_secs(5)
            )
            .await,
            "health never reached degraded"
        );
        assert!(
            wait_for_health(
                &health,
                health::HealthState::Unreachable,
                Duration::from_secs(8)
            )
            .await,
            "health never reached unreachable"
        );

        bot_api.set_ok(true);
        assert!(
            wait_for_health(&health, health::HealthState::Ok, Duration::from_secs(8)).await,
            "health never recovered to ok"
        );

        task.abort();
        let _ = task.await;
        drop(bot_api);

        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&health_path).unwrap()).unwrap();
        assert_eq!(stored["state"], "ok");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
