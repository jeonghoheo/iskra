use crate::brain::BalanceResponse;
use crate::delivery::{
    DeliveryChannel, DeliveryError, DeliveryOutcome, DeliveryState, DeliveryStore, DeliveryTicket,
};
use crate::health::{self, Health};
use crate::locale::{self, Key, Locale};
use crate::logging;
use crate::memory;
use crate::prompt;
use crate::route::{self, RouteError};
use crate::run::{
    Core, RunError, RunQueueStatus, RunState, RunTicket, RuntimeEventKind, TERMINAL_POLL_INTERVAL,
    TerminalOutcome,
};
use crate::run_context::MAIN_AGENT_ID;
use crate::runtime::AgentRuntime;
use crate::session::FailureKind;
use crate::stickies;
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
const COMMAND_KEYS: [(&str, Key); 8] = [
    ("start", Key::StartDescription),
    ("status", Key::StatusDescription),
    ("agents", Key::AgentsDescription),
    ("stickies", Key::StickiesDescription),
    ("resume", Key::ResumeDescription),
    ("stop", Key::StopDescription),
    ("result", Key::ResultDescription),
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
    core: Arc<Core>,
    allowed_user_id: u64,
}

impl ChannelState {
    fn main(&self) -> &AgentRuntime {
        self.core.registry().main()
    }
}

pub async fn run(core: Arc<Core>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    let memory_dir = core.registry().main().paths.memory.clone();

    let health = Arc::new(Health::new(memory_dir.join(health::FILE_NAME)));
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
    match core.delivery().counts() {
        Ok(counts) => logging::append(&format!(
            "telegram_delivery_start deliveries={} unresolved={}",
            counts.records, counts.unresolved_records
        )),
        Err(error) => logging::append(&format!("telegram_delivery_start_error {error}")),
    }

    let state = Arc::new(ChannelState {
        bot: bot.clone(),
        core,
        allowed_user_id,
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
        start_turn(&state, chat_id, MAIN_AGENT_ID, &wrapped, lang);
        return Ok(());
    }

    if let Some(reply) = msg.reply_to_message() {
        if is_board_reply(reply) {
            if let Ok(number) = user_message.trim().parse::<u64>() {
                match stickies::remove_at(&state.main().paths.memory, number, lang) {
                    Ok(_) => {
                        logging::append(&format!("telegram_sticky_remove number={number}"));
                        let board = match stickies::board_at(&state.main().paths.memory, lang) {
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
                            &state.main().paths.memory,
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
            start_turn(&state, chat_id, MAIN_AGENT_ID, &user_message, lang);
            return Ok(());
        }
    }

    let (command, args) = command_parts(&user_message).unwrap_or_default();
    match command.as_str() {
        "/start" => {
            bot.send_message(chat_id, start_text(lang)).await?;
        }
        "/help" => {
            bot.send_message(chat_id, help_text(lang)).await?;
        }
        "/stickies" => handle_stickies_command(&state, chat_id, &user_message, lang).await?,
        "/stop" => stop_turn(&state, chat_id, &args, lang).await?,
        "/resume" => resume_turn(&state, chat_id, &args, lang).await?,
        "/status" => send_status(&state, chat_id, &args, lang).await?,
        "/result" => send_result(&state, chat_id, &args, lang).await?,
        "/agents" => send_agents(&state, chat_id, lang).await?,
        _ => match route::select(&user_message, state.core.registry()) {
            Ok(route) => start_turn(&state, chat_id, &route.agent_id, &route.input, lang),
            Err(error) => {
                logging::append(&format!("telegram_route_error err={error:?}"));
                bot.send_message(chat_id, route_error_text(lang, error))
                    .await?;
            }
        },
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

fn command_parts(text: &str) -> Option<(String, String)> {
    let (first, rest) = match text.split_once(char::is_whitespace) {
        Some((first, rest)) => (first, rest),
        None => (text, ""),
    };
    if !first.starts_with('/') {
        return None;
    }
    let name = first.split('@').next().unwrap_or(first);
    Some((name.to_string(), rest.trim().to_string()))
}

pub(crate) fn route_error_text(lang: Locale, error: RouteError) -> String {
    match error {
        RouteError::Empty => locale::text(lang, Key::RouteEmpty).to_string(),
        RouteError::Malformed => locale::text(lang, Key::RouteMalformed).to_string(),
        RouteError::UnknownAgent(agent) => locale::with(lang, Key::RouteUnknownAgent, agent),
        RouteError::Unavailable(agent) => locale::with(lang, Key::RouteUnavailable, agent),
    }
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
            match stickies::remove_at(&state.main().paths.memory, number, lang) {
                Ok(_) => {
                    logging::append(&format!("telegram_sticky_remove number={number}"));
                    let board = match stickies::board_at(&state.main().paths.memory, lang) {
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
                        &state.main().paths.memory,
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

pub(crate) fn failure_text(lang: Locale, category: FailureKind, detail: &str) -> String {
    match category {
        FailureKind::Timeout | FailureKind::Connection => {
            locale::text(lang, Key::ErrorTimeout).to_string()
        }
        FailureKind::Auth => locale::text(lang, Key::ErrorAuth).to_string(),
        FailureKind::Payment => locale::text(lang, Key::ErrorPayment).to_string(),
        FailureKind::Busy => locale::text(lang, Key::ErrorBusy).to_string(),
        FailureKind::RepeatedToolError => locale::with(
            lang,
            Key::ErrorRepeatedTool,
            locale::text(lang, repeated_reason_key(detail)),
        ),
        FailureKind::MaxRoundtrips => locale::text(lang, Key::ErrorMaxRoundtrips).to_string(),
        FailureKind::EmptyResponse => locale::text(lang, Key::ErrorEmpty).to_string(),
        FailureKind::Other => locale::text(lang, Key::ErrorOther).to_string(),
    }
}

pub(crate) fn submission_error_text(lang: Locale, error: &RunError) -> String {
    match error {
        RunError::Store(_) => locale::text(lang, Key::ErrorSession).to_string(),
        RunError::QueueFull => locale::text(lang, Key::BusyNotice).to_string(),
        RunError::UnknownAgent(agent) => locale::with(lang, Key::RouteUnknownAgent, agent),
        RunError::AgentUnavailable(agent) => locale::with(lang, Key::RouteUnavailable, agent),
        RunError::InputTooLong => locale::text(lang, Key::InputTooLong).to_string(),
        _ => locale::text(lang, Key::ErrorOther).to_string(),
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

fn start_turn(
    state: &Arc<ChannelState>,
    chat_id: ChatId,
    agent_id: &str,
    text: &str,
    lang: Locale,
) {
    let mut events = state.core.subscribe();
    match state.core.submit(agent_id, text) {
        Ok(ticket) => {
            logging::append(&format!(
                "telegram_run_submitted run_id={} agent={agent_id}",
                ticket.id
            ));
            let notice = queued_notice(state, agent_id, ticket.id, lang);
            let core = Arc::clone(&state.core);
            let bot = state.bot.clone();
            let agent_id = agent_id.to_string();
            tokio::spawn(async move {
                watch_turn(
                    core,
                    bot,
                    chat_id,
                    agent_id,
                    ticket,
                    lang,
                    &mut events,
                    notice,
                )
                .await;
            });
        }
        Err(error) => {
            logging::append(&format!("telegram_submit_error agent={agent_id} {error}"));
            let text = submission_error_text(lang, &error);
            let bot = state.bot.clone();
            tokio::spawn(async move {
                if let Err(e) = bot.send_message(chat_id, text).await {
                    logging::append(&format!("telegram_send_failed {e}"));
                }
            });
        }
    }
}

/// Result of one durable-state check made while waiting for a RuntimeEvent.
enum Recovery {
    /// The Run is still active or not yet recorded.
    Pending,
    /// A terminal outcome was handled; the watcher must stop.
    Handled,
    /// The durable stores could not be read. The watcher stops without a
    /// successful claim and without retrying the work.
    Unavailable,
}

#[allow(clippy::too_many_arguments)]
async fn watch_turn(
    core: Arc<Core>,
    bot: Bot,
    chat_id: ChatId,
    agent_id: String,
    ticket: RunTicket,
    lang: Locale,
    events: &mut tokio::sync::broadcast::Receiver<crate::run::RuntimeEvent>,
    notice: Option<(String, String)>,
) {
    if let Some((text, log)) = notice {
        logging::append(&log);
        if let Err(e) = bot.send_message(chat_id, text).await {
            logging::append(&format!("telegram_send_failed {e}"));
        }
    }
    let mut typing: Option<TypingGuard> = None;
    loop {
        // RuntimeEvents are hints. Each wait is bounded, and a timeout or a
        // lagged stream falls back to the durable Run/Session projection so a
        // missing event cannot hang the watcher or leave typing active.
        let received = tokio::time::timeout(TERMINAL_POLL_INTERVAL, events.recv()).await;
        match received {
            Ok(Ok(event)) if event.run_id == ticket.id => match event.kind {
                RuntimeEventKind::Started => {
                    logging::append(&format!(
                        "telegram_run_started run_id={} agent={} revision={} seq={}",
                        ticket.id, event.agent_id, event.revision, event.seq
                    ));
                    if typing.is_none() {
                        let (stop, stop_rx) = tokio::sync::watch::channel(false);
                        typing = Some(TypingGuard(stop));
                        tokio::spawn(typing_loop(bot.clone(), chat_id, stop_rx));
                    }
                }
                RuntimeEventKind::Completed { answer } => {
                    drop(typing.take());
                    send_run_reply(&core, &bot, chat_id, ticket.id, &agent_id, &answer, lang).await;
                    sync_sticky_after_turn(&core, &bot, chat_id, lang).await;
                    break;
                }
                RuntimeEventKind::Failed { category, detail } => {
                    drop(typing.take());
                    logging::append(&format!(
                        "telegram_turn_error run_id={} agent={agent_id} category={category:?}",
                        ticket.id
                    ));
                    let text = failure_text(lang, category, &detail);
                    send_run_reply(&core, &bot, chat_id, ticket.id, &agent_id, &text, lang).await;
                    sync_sticky_after_turn(&core, &bot, chat_id, lang).await;
                    break;
                }
                RuntimeEventKind::Cancelled => {
                    drop(typing.take());
                    logging::append(&format!("telegram_run_cancelled run_id={}", ticket.id));
                    break;
                }
                RuntimeEventKind::StorageFailed => {
                    drop(typing.take());
                    logging::append(&format!("telegram_storage_failed run_id={}", ticket.id));
                    // A storage failure may leave the Run non-terminal, so no valid
                    // delivery identity can be attached to this notice.
                    let text = attributed_reply(&agent_id, locale::text(lang, Key::ErrorSession));
                    send_notice(&bot, chat_id, &text).await;
                    break;
                }
            },
            Ok(Ok(_)) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped))) => {
                logging::append(&format!(
                    "telegram_event_lagged run_id={} skipped={skipped}",
                    ticket.id
                ));
                match recover_terminal(&core, &bot, chat_id, ticket.id, &agent_id, lang).await {
                    Recovery::Pending => {}
                    Recovery::Handled | Recovery::Unavailable => {
                        drop(typing.take());
                        break;
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                drop(typing.take());
                logging::append(&format!("telegram_event_closed run_id={}", ticket.id));
                if !matches!(
                    recover_terminal(&core, &bot, chat_id, ticket.id, &agent_id, lang).await,
                    Recovery::Handled
                ) {
                    logging::append(&format!(
                        "telegram_recovery_unavailable run_id={}",
                        ticket.id
                    ));
                }
                break;
            }
            Err(_elapsed) => {
                match recover_terminal(&core, &bot, chat_id, ticket.id, &agent_id, lang).await {
                    Recovery::Pending => {}
                    Recovery::Handled | Recovery::Unavailable => {
                        drop(typing.take());
                        break;
                    }
                }
            }
        }
    }
}

/// Recovers a terminal outcome from the durable Run and Session records.
///
/// It only reads. Telegram is contacted only when the Run is terminal, the
/// stored answer or category is available, and this Run has no delivery attempt
/// for the same channel and target. A missing or unreadable answer is reported
/// as a storage notice rather than a fabricated success.
async fn recover_terminal(
    core: &Core,
    bot: &Bot,
    chat_id: ChatId,
    run_id: u64,
    agent_id: &str,
    lang: Locale,
) -> Recovery {
    let outcome = match core.terminal_outcome(run_id) {
        Ok(Some(outcome)) => outcome,
        Ok(None) => return Recovery::Pending,
        Err(error) => {
            logging::append(&format!("telegram_recovery_error run_id={run_id} {error}"));
            return Recovery::Unavailable;
        }
    };
    let state = match &outcome {
        TerminalOutcome::Completed { .. } => "completed",
        TerminalOutcome::Failed { .. } => "failed",
        TerminalOutcome::Cancelled => "cancelled",
        TerminalOutcome::Interrupted => "interrupted",
    };
    logging::append(&format!("telegram_recovered run_id={run_id} state={state}"));

    match outcome {
        TerminalOutcome::Completed {
            answer: Some(answer),
        } => {
            if !recovered_delivery_allowed(core, bot, chat_id, run_id, lang).await {
                return Recovery::Handled;
            }
            send_run_reply(core, bot, chat_id, run_id, agent_id, &answer, lang).await;
            sync_sticky_after_turn(core, bot, chat_id, lang).await;
        }
        TerminalOutcome::Completed { answer: None } => {
            logging::append(&format!("telegram_recovery_answer_missing run_id={run_id}"));
            let text = attributed_reply(agent_id, locale::text(lang, Key::ErrorSession));
            send_notice(bot, chat_id, &text).await;
        }
        TerminalOutcome::Failed { category } => {
            if !recovered_delivery_allowed(core, bot, chat_id, run_id, lang).await {
                return Recovery::Handled;
            }
            logging::append(&format!(
                "telegram_turn_error run_id={run_id} agent={agent_id} category={category:?}"
            ));
            let text = failure_text(lang, category, "");
            send_run_reply(core, bot, chat_id, run_id, agent_id, &text, lang).await;
            sync_sticky_after_turn(core, bot, chat_id, lang).await;
        }
        TerminalOutcome::Cancelled | TerminalOutcome::Interrupted => {
            logging::append(&format!(
                "telegram_recovered_stop run_id={run_id} state={state}"
            ));
        }
    }
    Recovery::Handled
}

/// A recovery send is allowed only when no delivery attempt exists for this Run,
/// channel, and target. An unresolved attempt is not retried automatically.
async fn recovered_delivery_allowed(
    core: &Core,
    bot: &Bot,
    chat_id: ChatId,
    run_id: u64,
    lang: Locale,
) -> bool {
    let target = chat_id.0.to_string();
    match core
        .delivery()
        .existing_attempt(run_id, DeliveryChannel::Telegram, &target)
    {
        Ok(false) => true,
        Ok(true) => {
            logging::append(&format!(
                "telegram_recovery_delivery_exists run_id={run_id}"
            ));
            false
        }
        Err(error) => {
            logging::append(&format!(
                "telegram_recovery_delivery_error run_id={run_id} error={error}"
            ));
            send_notice(bot, chat_id, locale::text(lang, Key::ErrorDeliveryStorage)).await;
            false
        }
    }
}

fn queued_notice(
    state: &Arc<ChannelState>,
    agent_id: &str,
    run_id: u64,
    lang: Locale,
) -> Option<(String, String)> {
    if !matches!(state.core.run_state(run_id), Ok(Some(RunState::Queued))) {
        return None;
    }
    let waiting = state
        .core
        .queue_status(agent_id)
        .map(|status| status.queued.saturating_sub(1))
        .unwrap_or(0);
    let display = state
        .core
        .registry()
        .slot(agent_id)
        .map(|slot| slot.display_name.clone())
        .unwrap_or_else(|| agent_id.to_string());
    let text = locale::with_values(lang, Key::QueueAccepted, &[display, waiting.to_string()]);
    Some((
        text,
        format!("telegram_run_queued run_id={run_id} agent={agent_id} waiting={waiting}"),
    ))
}

async fn sync_sticky_after_turn(core: &Core, bot: &Bot, chat_id: ChatId, lang: Locale) {
    let main = core.registry().main();
    if !main.tools.take_sticky_changed() {
        return;
    }
    match stickies::board_at(&main.paths.memory, lang) {
        Ok(board) => {
            if let Err(e) =
                sync_board(bot, chat_id, &board, BoardMode::Update, &main.paths.memory).await
            {
                logging::append(&format!("telegram_board_sync_error {e}"));
            }
        }
        Err(e) => logging::append(&format!("sticky_board_error {e}")),
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

async fn stop_turn(
    state: &Arc<ChannelState>,
    chat_id: ChatId,
    args: &str,
    lang: Locale,
) -> HandlerResult {
    let agent_id = match route::target(args, state.core.registry()) {
        Ok(agent_id) => agent_id,
        Err(error) => {
            logging::append(&format!("telegram_stop_target_error err={error:?}"));
            state
                .bot
                .send_message(chat_id, route_error_text(lang, error))
                .await?;
            return Ok(());
        }
    };

    let message = match state.core.cancel_agent(&agent_id) {
        Ok(report) if report.cancelled == 0 => {
            logging::append(&format!("telegram_stop idle agent={agent_id}"));
            locale::text(lang, Key::StopNoActive)
        }
        Ok(report) => {
            logging::append(&format!(
                "telegram_stop aborted agent={agent_id} cancelled={} active={}",
                report.cancelled, report.active
            ));
            locale::text(lang, Key::StopStopped)
        }
        Err(error) => {
            logging::append(&format!("telegram_stop_error agent={agent_id} {error}"));
            locale::text(lang, Key::ErrorSession)
        }
    };
    state.bot.send_message(chat_id, message).await?;
    Ok(())
}

async fn resume_turn(
    state: &Arc<ChannelState>,
    chat_id: ChatId,
    args: &str,
    lang: Locale,
) -> HandlerResult {
    let agent_id = match route::target(args, state.core.registry()) {
        Ok(agent_id) => agent_id,
        Err(error) => {
            logging::append(&format!("telegram_resume_target_error err={error:?}"));
            state
                .bot
                .send_message(chat_id, route_error_text(lang, error))
                .await?;
            return Ok(());
        }
    };

    let display = display_name(state, &agent_id);
    let mut events = state.core.subscribe();
    match state.core.resume_agent(&agent_id) {
        Ok(resumed) if resumed.is_empty() => {
            logging::append(&format!("telegram_resume none agent={agent_id}"));
            state
                .bot
                .send_message(chat_id, locale::with(lang, Key::ResumeNone, display))
                .await?;
        }
        Ok(resumed) => {
            logging::append(&format!(
                "telegram_resume agent={agent_id} resumed={}",
                resumed.len()
            ));
            state
                .bot
                .send_message(
                    chat_id,
                    locale::with_values(
                        lang,
                        Key::ResumeDone,
                        &[resumed.len().to_string(), display],
                    ),
                )
                .await?;
            let core = Arc::clone(&state.core);
            let bot = state.bot.clone();
            tokio::spawn(async move {
                for ticket in resumed {
                    watch_turn(
                        Arc::clone(&core),
                        bot.clone(),
                        chat_id,
                        agent_id.clone(),
                        ticket,
                        lang,
                        &mut events,
                        None,
                    )
                    .await;
                }
            });
        }
        Err(error) => {
            logging::append(&format!("telegram_resume_error agent={agent_id} {error}"));
            state
                .bot
                .send_message(chat_id, locale::text(lang, Key::ErrorSession))
                .await?;
        }
    }
    Ok(())
}

fn display_name(state: &Arc<ChannelState>, agent_id: &str) -> String {
    state
        .core
        .registry()
        .slot(agent_id)
        .map(|slot| slot.display_name.clone())
        .unwrap_or_else(|| agent_id.to_string())
}

async fn send_agents(state: &Arc<ChannelState>, chat_id: ChatId, lang: Locale) -> HandlerResult {
    state
        .bot
        .send_message(chat_id, agents_text(&state.core, lang))
        .await?;
    Ok(())
}

pub(crate) fn agents_text(core: &Core, lang: Locale) -> String {
    let mut lines = vec![locale::text(lang, Key::AgentsHeader).to_string()];
    for slot in core.registry().slots() {
        let model = slot
            .runtime()
            .map(|runtime| runtime.brain.model().to_string())
            .unwrap_or_else(|| slot.model.clone());
        let status = locale::text(
            lang,
            if slot.is_available() {
                Key::Available
            } else {
                Key::Unavailable
            },
        );
        let mut values = vec![
            slot.id.clone(),
            slot.display_name.clone(),
            slot.engine_id.clone(),
            model,
            status.to_string(),
        ];
        let key = match core.queue_status(&slot.id) {
            Ok(queue) => {
                values.extend([
                    queue.active.to_string(),
                    queue.queued.to_string(),
                    queue.paused.to_string(),
                ]);
                Key::AgentLine
            }
            Err(_) => Key::AgentQueueUnavailable,
        };
        lines.push(locale::with_values(lang, key, &values));
    }
    lines.join("\n")
}

async fn send_status(
    state: &Arc<ChannelState>,
    chat_id: ChatId,
    args: &str,
    lang: Locale,
) -> HandlerResult {
    let agent_id = match route::target(args, state.core.registry()) {
        Ok(agent_id) => agent_id,
        Err(error) => {
            logging::append(&format!("telegram_status_target_error err={error:?}"));
            state
                .bot
                .send_message(chat_id, route_error_text(lang, error))
                .await?;
            return Ok(());
        }
    };

    let Some(text) = agent_status_text(&state.core, lang, &agent_id).await else {
        state
            .bot
            .send_message(
                chat_id,
                route_error_text(lang, RouteError::UnknownAgent(agent_id)),
            )
            .await?;
        return Ok(());
    };
    state.bot.send_message(chat_id, text).await?;
    Ok(())
}

/// Parses `/result`'s single work-number argument. Only one positive integer is
/// accepted; anything else is a localized usage error at the call site.
pub(crate) fn parse_run_id(args: &str) -> Option<u64> {
    let args = args.trim();
    if args.is_empty() || args.split_whitespace().count() != 1 {
        return None;
    }
    args.parse::<u64>().ok().filter(|id| *id > 0)
}

/// Sends a completed Run's stored redacted answer as an explicit, tracked
/// delivery. This reads Session without a model/tool call and never retries
/// automatically; every invocation is a new owner action.
async fn send_result(
    state: &Arc<ChannelState>,
    chat_id: ChatId,
    args: &str,
    lang: Locale,
) -> HandlerResult {
    let Some(run_id) = parse_run_id(args) else {
        logging::append("telegram_result usage");
        state
            .bot
            .send_message(chat_id, locale::text(lang, Key::ResultUsage))
            .await?;
        return Ok(());
    };

    let target = match state.core.run_target(run_id) {
        Ok(Some(target)) => target,
        Ok(None) => {
            logging::append(&format!("telegram_result run_id={run_id} state=unknown"));
            state
                .bot
                .send_message(chat_id, locale::with(lang, Key::ResultUnknownRun, run_id))
                .await?;
            return Ok(());
        }
        Err(error) => {
            logging::append(&format!(
                "telegram_result_error run_id={run_id} error={error}"
            ));
            state
                .bot
                .send_message(chat_id, locale::text(lang, Key::ResultUnavailable))
                .await?;
            return Ok(());
        }
    };

    if target.state != RunState::Completed {
        logging::append(&format!(
            "telegram_result run_id={run_id} agent={} state={:?}",
            target.agent_id, target.state
        ));
        state
            .bot
            .send_message(chat_id, locale::with(lang, Key::ResultNotCompleted, run_id))
            .await?;
        return Ok(());
    }

    let answer = match state.core.completed_answer(run_id) {
        Ok(Some(answer)) => answer,
        Ok(None) => {
            logging::append(&format!(
                "telegram_result run_id={run_id} agent={} state=answer_missing",
                target.agent_id
            ));
            state
                .bot
                .send_message(
                    chat_id,
                    locale::with(lang, Key::ResultAnswerMissing, run_id),
                )
                .await?;
            return Ok(());
        }
        Err(error) => {
            logging::append(&format!(
                "telegram_result_error run_id={run_id} error={error}"
            ));
            state
                .bot
                .send_message(chat_id, locale::text(lang, Key::ResultUnavailable))
                .await?;
            return Ok(());
        }
    };

    logging::append(&format!(
        "telegram_result run_id={run_id} agent={} state=completed",
        target.agent_id
    ));
    if let Some(ticket) = send_run_reply(
        &state.core,
        &state.bot,
        chat_id,
        run_id,
        &target.agent_id,
        &answer,
        lang,
    )
    .await
    {
        match state.core.delivery().get(ticket.id) {
            Ok(Some(record)) => {
                let sent = record
                    .chunks
                    .iter()
                    .filter(|chunk| chunk.state == DeliveryState::Sent)
                    .count();
                logging::append(&format!(
                    "telegram_result_delivery id={} run={run_id} chunks={} recorded={} sent={sent}",
                    record.id,
                    record.chunk_count,
                    record.chunks.len()
                ));
            }
            Ok(None) => {}
            Err(error) => logging::append(&format!(
                "telegram_result_delivery_error id={} error={error}",
                ticket.id
            )),
        }
    }
    Ok(())
}

/// Composes the status text for one Agent. Shared by Telegram and the CLI.
pub(crate) async fn agent_status_text(core: &Core, lang: Locale, agent_id: &str) -> Option<String> {
    let slot = core.registry().slot(agent_id)?;
    let queue = match core.queue_status(agent_id) {
        Ok(status) => queue_line(lang, &status),
        Err(_) => locale::text(lang, Key::StatusQueueUnavailable).to_string(),
    };
    let delivery = delivery_line(core.delivery(), lang, agent_id);
    let agent = if agent_id == MAIN_AGENT_ID {
        None
    } else {
        Some((slot.display_name.as_str(), slot.id.as_str()))
    };

    let Some(runtime) = slot.runtime() else {
        let unavailable = locale::text(lang, Key::Unavailable);
        return Some(status_text(
            lang,
            agent,
            &slot.model,
            unavailable,
            unavailable,
            0,
            unavailable,
            unavailable,
            &queue,
            &delivery,
        ));
    };

    let balance = match runtime.brain.balance().await {
        Ok(balance) => balance_display(lang, &balance),
        Err(_) => locale::text(lang, Key::Unavailable).to_string(),
    };
    let loaded = memory::load_from(&runtime.paths.memory);
    let memory = locale::memory_status(lang, loaded.truncated, loaded.warnings.len());
    let usage = usage::today_for_agent(&runtime.agent_id);
    let cost = match usage::estimate_cost(runtime.brain.model(), &usage) {
        Some(cost) => locale::with(lang, Key::CostEstimate, format!("${cost:.6}")),
        None => locale::text(lang, Key::CostUnknown).to_string(),
    };
    let session = locale::session_status(lang, runtime.session.status());
    Some(status_text(
        lang,
        agent,
        runtime.brain.model(),
        &memory,
        &session,
        usage.total,
        &cost,
        &balance,
        &queue,
        &delivery,
    ))
}

fn queue_line(lang: Locale, status: &RunQueueStatus) -> String {
    locale::with_values(
        lang,
        Key::StatusQueue,
        &[
            status.active.to_string(),
            status.queued.to_string(),
            status.paused.to_string(),
            status.interrupted.to_string(),
        ],
    )
}

/// Delivery state is reported separately from Run/Queue state. An unreadable or
/// poisoned store reports `unavailable`, never zero counts.
fn delivery_line(store: &DeliveryStore, lang: Locale, agent_id: &str) -> String {
    match store.counts_for_agent(agent_id) {
        Ok(counts) => locale::with_values(
            lang,
            Key::StatusDelivery,
            &[
                counts.pending.to_string(),
                counts.failed.to_string(),
                counts.unknown.to_string(),
                counts.unresolved_records.to_string(),
            ],
        ),
        Err(error) => {
            let detail = store.error().unwrap_or_else(|| error.to_string());
            logging::append(&format!(
                "delivery_status_error agent={agent_id} error={detail}"
            ));
            locale::text(lang, Key::StatusDeliveryUnavailable).to_string()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn status_text(
    lang: Locale,
    agent: Option<(&str, &str)>,
    model: &str,
    memory: &str,
    session: &str,
    tokens: u64,
    cost: &str,
    balance: &str,
    queue: &str,
    delivery: &str,
) -> String {
    let mut lines = Vec::new();
    if let Some((display, id)) = agent {
        lines.push(locale::with_values(
            lang,
            Key::StatusAgent,
            &[display.to_string(), id.to_string()],
        ));
    }
    lines.push(locale::with(lang, Key::StatusCost, cost));
    lines.push(locale::with(lang, Key::StatusBalance, balance));
    lines.push(locale::with(lang, Key::StatusModel, model));
    lines.push(locale::with(lang, Key::StatusMemory, memory));
    lines.push(locale::with(lang, Key::StatusSession, session));
    lines.push(locale::with(lang, Key::StatusTokens, tokens));
    lines.push(queue.to_string());
    lines.push(delivery.to_string());
    lines.join("\n")
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
    match stickies::board_at(&state.main().paths.memory, lang) {
        Ok(board) => {
            if let Err(e) = sync_board(
                &state.bot,
                chat_id,
                &board,
                BoardMode::Show,
                &state.main().paths.memory,
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

fn attributed_reply(agent_id: &str, text: &str) -> String {
    if agent_id == MAIN_AGENT_ID || text.is_empty() {
        text.to_string()
    } else {
        format!("@{agent_id}\n{text}")
    }
}

/// Classified Telegram send result for one logical chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SendOutcome {
    Sent {
        message_id: i64,
    },
    /// Telegram definitively refused the Markdown entities; the message was not
    /// accepted, so the single plain-text fallback is allowed.
    MarkdownRejected,
    /// A structured Bot API error proves the message was not accepted.
    Rejected,
    /// Timeout, connection loss, reset, or an unparseable response. The outcome
    /// is unknown and must never be retried automatically.
    Ambiguous,
}

impl SendOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sent { .. } => "sent",
            Self::MarkdownRejected => "markdown_rejected",
            Self::Rejected => "rejected",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Default)]
struct TrackedSendReport {
    sent: u32,
    failed: u32,
    unknown: u32,
    unattempted: u32,
    /// A send returned success but its `sent` state could not be persisted.
    delivered_unrecorded: u32,
    storage_error: Option<DeliveryError>,
}

fn classify_send_error(error: &RequestError) -> SendOutcome {
    match error {
        RequestError::Api(ApiError::CantParseEntities(_)) => SendOutcome::MarkdownRejected,
        // An unrecognized API description cannot prove the message was not
        // accepted, so it stays ambiguous rather than becoming a clean failure.
        RequestError::Api(ApiError::Unknown(_)) => SendOutcome::Ambiguous,
        RequestError::Api(_) => SendOutcome::Rejected,
        RequestError::RetryAfter(_) | RequestError::MigrateToChatId(_) => SendOutcome::Rejected,
        RequestError::Network(_) | RequestError::InvalidJson { .. } | RequestError::Io(_) => {
            SendOutcome::Ambiguous
        }
    }
}

#[allow(deprecated)]
async fn send_telegram_chunk(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    markdown: bool,
) -> SendOutcome {
    let request = bot.send_message(chat_id, text);
    let request = if markdown {
        request.parse_mode(ParseMode::Markdown)
    } else {
        request
    };
    match request.await {
        Ok(message) => SendOutcome::Sent {
            message_id: i64::from(message.id.0),
        },
        Err(error) => classify_send_error(&error),
    }
}

/// Sends one already-attributed and already-localized run reply.
///
/// Attribution and localization happen before chunking, so every persisted
/// chunk describes exactly what Telegram receives. A storage failure before the
/// first chunk reaches Telegram is refused with a localized notice; a failure
/// after some chunks were sent stops the remaining sends without a retry.
async fn send_run_reply(
    core: &Core,
    bot: &Bot,
    chat_id: ChatId,
    run_id: u64,
    agent_id: &str,
    text: &str,
    lang: Locale,
) -> Option<DeliveryTicket> {
    let text = attributed_reply(agent_id, text);
    let chunks = split_message(&text);
    if chunks.is_empty() {
        return None;
    }

    let target = chat_id.0.to_string();
    let ticket = match core.create_delivery(
        run_id,
        DeliveryChannel::Telegram,
        &target,
        chunks.len() as u32,
    ) {
        Ok(ticket) => ticket,
        Err(error) => {
            logging::append(&format!(
                "telegram_delivery_refused run={run_id} error={error}"
            ));
            send_notice(bot, chat_id, locale::text(lang, Key::ErrorDeliveryStorage)).await;
            return None;
        }
    };

    let report = send_tracked_chunks(
        core.delivery(),
        bot,
        chat_id,
        ticket.id,
        ticket.revision,
        &chunks,
    )
    .await;

    logging::append(&format!(
        "telegram_delivery_report id={} run={run_id} chunks={} sent={} failed={} unknown={} unattempted={} unrecorded={}",
        ticket.id,
        chunks.len(),
        report.sent,
        report.failed,
        report.unknown,
        report.unattempted,
        report.delivered_unrecorded
    ));

    if let Some(error) = &report.storage_error {
        logging::append(&format!(
            "telegram_delivery_storage_error id={} error={error}",
            ticket.id
        ));
        let nothing_reached_telegram = report.sent == 0
            && report.failed == 0
            && report.unknown == 0
            && report.delivered_unrecorded == 0;
        if nothing_reached_telegram {
            send_notice(bot, chat_id, locale::text(lang, Key::ErrorDeliveryStorage)).await;
        }
    }
    Some(ticket)
}

/// Persists and sends chunks in order. Each chunk records `pending` and its
/// attempt before the Bot API call; `sent` requires a returned message id.
/// Only a definitive Markdown rejection opens the single recorded plain-text
/// fallback. Any failed, ambiguous, or storage-aborted chunk stops later sends.
async fn send_tracked_chunks(
    store: &DeliveryStore,
    bot: &Bot,
    chat_id: ChatId,
    delivery_id: u64,
    mut revision: u64,
    chunks: &[&str],
) -> TrackedSendReport {
    let mut report = TrackedSendReport::default();

    for (position, chunk) in chunks.iter().enumerate() {
        let index = position as u32;
        revision = match store.begin_chunk(delivery_id, revision, index) {
            Ok(revision) => revision,
            Err(error) => {
                report.unattempted = (chunks.len() - position) as u32;
                report.storage_error = Some(error);
                return report;
            }
        };

        let mut outcome = send_telegram_chunk(bot, chat_id, chunk, true).await;
        if outcome == SendOutcome::MarkdownRejected {
            revision = match store.begin_fallback(delivery_id, revision, index) {
                Ok(revision) => revision,
                Err(error) => {
                    report.unattempted = (chunks.len() - position) as u32;
                    report.storage_error = Some(error);
                    return report;
                }
            };
            outcome = send_telegram_chunk(bot, chat_id, chunk, false).await;
        }

        let settlement = match outcome {
            SendOutcome::Sent { message_id } => DeliveryOutcome::Sent { message_id },
            SendOutcome::MarkdownRejected | SendOutcome::Rejected => DeliveryOutcome::Failed,
            SendOutcome::Ambiguous => DeliveryOutcome::Unknown,
        };
        match store.settle_chunk(delivery_id, revision, index, settlement) {
            Ok(next) => revision = next,
            Err(error) => {
                if matches!(outcome, SendOutcome::Sent { .. }) {
                    report.delivered_unrecorded = 1;
                }
                report.unattempted = (chunks.len() - position - 1) as u32;
                report.storage_error = Some(error);
                return report;
            }
        }

        logging::append(&format!(
            "telegram_delivery_chunk id={delivery_id} chunk={index} class={}",
            outcome.as_str()
        ));
        match outcome {
            SendOutcome::Sent { .. } => report.sent += 1,
            SendOutcome::MarkdownRejected | SendOutcome::Rejected => report.failed += 1,
            SendOutcome::Ambiguous => report.unknown += 1,
        }
        if !matches!(outcome, SendOutcome::Sent { .. }) {
            report.unattempted = (chunks.len() - position - 1) as u32;
            return report;
        }
    }

    report
}

/// Sends a plain notice that carries no delivery identity (for example when the
/// delivery store refuses admission). Logs only its classification.
async fn send_notice(bot: &Bot, chat_id: ChatId, text: &str) {
    if let Err(error) = bot.send_message(chat_id, text).await {
        logging::append(&format!(
            "telegram_notice_error class={}",
            classify_send_error(&error).as_str()
        ));
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
    use crate::delivery::DeliveryState;
    use crate::runtime::RuntimeRegistry;
    use crate::session::Session;
    use std::sync::Mutex;

    #[derive(Clone, Copy, Debug)]
    enum MockReply {
        Message(i64),
        ApiError(&'static str),
        Reset,
    }

    struct MockBotApi {
        address: String,
        messages: Arc<Mutex<Vec<serde_json::Value>>>,
        stop: std::sync::mpsc::Sender<()>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl MockBotApi {
        fn start() -> Self {
            Self::start_with(Vec::new())
        }

        /// Each request consumes the next scripted reply; after the script is
        /// exhausted, requests receive a successful message with id 1.
        fn start_with(script: Vec<MockReply>) -> Self {
            use std::io::{BufRead, BufReader, Read, Write};
            use std::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let messages = Arc::new(Mutex::new(Vec::new()));
            let server_messages = Arc::clone(&messages);
            let script = Arc::new(Mutex::new(script));
            let server_script = Arc::clone(&script);
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

                            let reply = if server_script.lock().unwrap().is_empty() {
                                MockReply::Message(1)
                            } else {
                                server_script.lock().unwrap().remove(0)
                            };
                            let response = match reply {
                                MockReply::Message(message_id) => Some(serde_json::json!({
                                    "ok": true,
                                    "result": {
                                        "message_id": message_id,
                                        "date": 0,
                                        "chat": {"id": 42, "type": "private"},
                                        "text": "ok"
                                    }
                                })),
                                MockReply::ApiError(description) => Some(serde_json::json!({
                                    "ok": false,
                                    "error_code": 400,
                                    "description": description
                                })),
                                MockReply::Reset => None,
                            };
                            let Some(response) = response else {
                                // Dropping the stream without a response models a
                                // timeout or connection reset.
                                continue;
                            };
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

        fn request_count(&self) -> usize {
            self.messages.lock().unwrap().len()
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

    struct MockModelApi {
        address: String,
        posts: Arc<(Mutex<usize>, std::sync::Condvar)>,
        stop: std::sync::mpsc::Sender<()>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl MockModelApi {
        fn start(delay: Duration) -> Self {
            use std::io::{BufRead, BufReader, Read, Write};
            use std::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let posts = Arc::new((Mutex::new(0usize), std::sync::Condvar::new()));
            let server_posts = Arc::clone(&posts);
            let (stop, stopped) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                while stopped.try_recv().is_err() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let posts = Arc::clone(&server_posts);
                            std::thread::spawn(move || {
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
                                let is_post = request_line.starts_with("POST ");
                                let response = if is_post {
                                    let mut count = posts.0.lock().unwrap();
                                    *count += 1;
                                    posts.1.notify_all();
                                    drop(count);
                                    std::thread::sleep(delay);
                                    serde_json::json!({
                                        "choices": [{"message": {"role": "assistant", "content": "late answer"}}],
                                        "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
                                    })
                                } else {
                                    serde_json::json!({"is_available": true, "balance_infos": []})
                                };
                                let response = serde_json::to_vec(&response).unwrap();
                                let _ = write!(
                                    writer,
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                    response.len()
                                );
                                let _ = writer.write_all(&response);
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("mock model accept failed: {error}"),
                    }
                }
            });
            Self {
                address,
                posts,
                stop,
                handle: Some(handle),
            }
        }

        fn url(&self) -> String {
            format!("http://{}/chat/completions", self.address)
        }

        fn post_count(&self) -> usize {
            *self.posts.0.lock().unwrap()
        }

        fn wait_for_post(&self, timeout: Duration) -> bool {
            let deadline = std::time::Instant::now() + timeout;
            let mut count = self.posts.0.lock().unwrap();
            while *count == 0 {
                let now = std::time::Instant::now();
                if now >= deadline {
                    return false;
                }
                let (next, _) = self.posts.1.wait_timeout(count, deadline - now).unwrap();
                count = next;
            }
            true
        }
    }

    impl Drop for MockModelApi {
        fn drop(&mut self) {
            let _ = self.stop.send(());
            let _ = std::net::TcpStream::connect(&self.address);
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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

        let model = MockModelApi::start(Duration::from_millis(500));
        let model_url = model.url();
        let registry = RuntimeRegistry::load(&dir, move |key| match key {
            "LLM_API_KEY" => Some("dummy-model-key".to_string()),
            "LLM_BASE_URL" => Some(model_url.clone()),
            "LLM_MODEL" => Some("test-model".to_string()),
            _ => None,
        })
        .unwrap();
        let session_path = dir.join("memory/session.json");
        let core = Core::new(registry).unwrap();
        let mut events = core.subscribe();
        let ticket = core.submit(MAIN_AGENT_ID, "stop this work").unwrap();

        let started = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = events.recv().await.unwrap();
                if event.run_id == ticket.id && matches!(event.kind, RuntimeEventKind::Started) {
                    break;
                }
            }
        })
        .await;
        assert!(started.is_ok(), "run never started");
        assert!(
            model.wait_for_post(Duration::from_secs(5)),
            "run never reached the model"
        );

        let bot_api = MockBotApi::start();
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });

        stop_turn(&state, ChatId(42), "@main", Locale::Ko)
            .await
            .unwrap();

        {
            let messages = bot_api.messages.lock().unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0]["chat_id"], 42);
            assert_eq!(
                messages[0]["text"].as_str(),
                Some(locale::text(Locale::Ko, Key::StopStopped))
            );
        }

        tokio::time::sleep(Duration::from_millis(700)).await;
        let status = core.queue_status(MAIN_AGENT_ID).unwrap();
        assert_eq!(status.active, 0);
        let runs: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
        assert_eq!(runs["runs"][0]["state"], "cancelled");
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&session_path).unwrap()).unwrap();
        assert_eq!(stored["work"][0]["status"], "stopped");
        assert!(Session::load(&session_path).status().unwrap().unfinished);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn storage_failure_event_ends_the_turn_watcher_with_a_storage_error() {
        use crate::run::RuntimeEvent;

        let dir = temp_dir("watcher-storage-failed");
        let model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &model.url(), &model.url(), true);
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start();
        let (sender, mut events) = tokio::sync::broadcast::channel(4);
        let bot = Bot::new("test-token").set_api_url(bot_api.url());

        let watcher = tokio::spawn(async move {
            watch_turn(
                core,
                bot,
                ChatId(42),
                MAIN_AGENT_ID.to_string(),
                RunTicket { id: 7, revision: 3 },
                Locale::En,
                &mut events,
                None,
            )
            .await;
        });
        sender
            .send(RuntimeEvent {
                run_id: 7,
                agent_id: MAIN_AGENT_ID.to_string(),
                revision: 3,
                seq: 2,
                kind: RuntimeEventKind::StorageFailed,
            })
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), watcher)
            .await
            .expect("the watcher did not end after a storage failure")
            .unwrap();
        let messages = bot_messages(&bot_api);
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0]["text"].as_str(),
            Some(locale::text(Locale::En, Key::ErrorSession))
        );

        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_helper_answer_is_labeled_with_its_agent_id() {
        let dir = temp_dir("watcher-helper-label");
        let main_model = MockModelApi::start(Duration::ZERO);
        let helper_model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &main_model.url(), &helper_model.url(), true);
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start();
        let mut events = core.subscribe();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let ticket = core.submit("helper", "answer this").unwrap();
        let watcher_core = Arc::clone(&core);
        let watcher = tokio::spawn(async move {
            watch_turn(
                watcher_core,
                bot,
                ChatId(42),
                "helper".to_string(),
                ticket,
                Locale::En,
                &mut events,
                None,
            )
            .await;
        });

        tokio::time::timeout(Duration::from_secs(5), watcher)
            .await
            .expect("the watcher did not end after the answer")
            .unwrap();
        let messages = bot_messages(&bot_api);
        let replies: Vec<_> = messages
            .iter()
            .filter(|message| message.get("text").is_some())
            .collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["text"].as_str(), Some("@helper\nlate answer"));

        let counts = core.delivery().counts().unwrap();
        assert_eq!(counts.sent, 1);
        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.agent_id, "helper");

        drop(core);
        drop(main_model);
        drop(helper_model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn seed_session_answer(dir: &Path, answer: &str) -> u64 {
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        let session = Session::load(&dir.join("memory/session.json"));
        let mut work = session.begin("seeded request").unwrap();
        let id = work.id();
        work.complete(answer).unwrap();
        id
    }

    fn seed_run(
        dir: &Path,
        id: u64,
        agent_id: &str,
        state: &str,
        failure: Option<&str>,
        session_work_id: Option<u64>,
    ) {
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        let mut record = serde_json::json!({
            "id": id,
            "agent_id": agent_id,
            "input": "seeded request",
            "state": state,
            "revision": 1
        });
        if let Some(failure) = failure {
            record["failure"] = serde_json::json!(failure);
        }
        if let Some(work) = session_work_id {
            record["session_work_id"] = serde_json::json!(work);
        }
        std::fs::write(
            dir.join("memory/runs.json"),
            serde_json::json!({"version": 1, "next_run_id": id + 1, "runs": [record]}).to_string(),
        )
        .unwrap();
    }

    fn seeded_core(dir: &Path, model: &MockModelApi) -> Arc<Core> {
        let registry = two_agent_registry(dir, &model.url(), &model.url(), true);
        Core::new(registry).unwrap()
    }

    fn run_ticket(id: u64) -> RunTicket {
        RunTicket { id, revision: 1 }
    }

    fn text_messages(api: &MockBotApi) -> Vec<String> {
        bot_messages(api)
            .into_iter()
            .filter_map(|message| message["text"].as_str().map(str::to_string))
            .collect()
    }

    async fn run_watcher(
        core: &Arc<Core>,
        bot: Bot,
        ticket: RunTicket,
        events: &mut tokio::sync::broadcast::Receiver<crate::run::RuntimeEvent>,
    ) {
        tokio::time::timeout(
            Duration::from_secs(5),
            watch_turn(
                Arc::clone(core),
                bot,
                ChatId(42),
                MAIN_AGENT_ID.to_string(),
                ticket,
                Locale::En,
                events,
                None,
            ),
        )
        .await
        .expect("the watcher did not end");
    }

    #[tokio::test]
    async fn recovery_sends_one_answer_when_the_completed_event_is_missing() {
        let dir = temp_dir("recovery-missing");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "recovered answer");
        seed_run(&dir, 1, MAIN_AGENT_ID, "completed", None, Some(work));
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (_sender, mut events) = tokio::sync::broadcast::channel(4);

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert_eq!(text_messages(&bot_api), ["recovered answer"]);
        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.run_id, 1);
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);
        assert_eq!(model.post_count(), 0, "recovery must not call the model");

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn recovery_after_a_lagged_stream_sends_the_stored_answer_once() {
        let dir = temp_dir("recovery-lagged");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "answer after lag");
        seed_run(&dir, 1, MAIN_AGENT_ID, "completed", None, Some(work));
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (sender, mut events) = tokio::sync::broadcast::channel(1);
        for seq in 1..=3 {
            sender
                .send(crate::run::RuntimeEvent {
                    run_id: 99,
                    agent_id: MAIN_AGENT_ID.to_string(),
                    revision: 1,
                    seq,
                    kind: RuntimeEventKind::Started,
                })
                .unwrap();
        }

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert_eq!(text_messages(&bot_api), ["answer after lag"]);
        assert_eq!(core.delivery().counts().unwrap().records, 1);
        assert_eq!(model.post_count(), 0);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn recovery_after_a_closed_stream_handles_the_terminal_run_once() {
        let dir = temp_dir("recovery-closed");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "answer after close");
        seed_run(&dir, 1, MAIN_AGENT_ID, "completed", None, Some(work));
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (sender, mut events) = tokio::sync::broadcast::channel(4);
        drop(sender);

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert_eq!(text_messages(&bot_api), ["answer after close"]);
        assert_eq!(core.delivery().counts().unwrap().records, 1);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_closed_stream_without_a_terminal_run_ends_without_a_send() {
        let dir = temp_dir("recovery-closed-pending");
        let model = MockModelApi::start(Duration::ZERO);
        seed_run(&dir, 1, MAIN_AGENT_ID, "queued", None, None);
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (sender, mut events) = tokio::sync::broadcast::channel(4);
        drop(sender);

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert!(text_messages(&bot_api).is_empty());
        assert_eq!(core.delivery().counts().unwrap().records, 0);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn duplicated_terminal_events_send_only_once() {
        let dir = temp_dir("recovery-duplicate");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "stored answer");
        seed_run(&dir, 7, MAIN_AGENT_ID, "completed", None, Some(work));
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (sender, mut events) = tokio::sync::broadcast::channel(4);
        for (seq, answer) in [(1, "first answer"), (2, "second answer")] {
            sender
                .send(crate::run::RuntimeEvent {
                    run_id: 7,
                    agent_id: MAIN_AGENT_ID.to_string(),
                    revision: 1,
                    seq,
                    kind: RuntimeEventKind::Completed {
                        answer: answer.to_string(),
                    },
                })
                .unwrap();
        }

        run_watcher(&core, bot, run_ticket(7), &mut events).await;

        assert_eq!(text_messages(&bot_api), ["first answer"]);
        assert_eq!(core.delivery().counts().unwrap().records, 1);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn recovery_does_not_resend_when_a_delivery_attempt_exists() {
        let dir = temp_dir("recovery-existing-delivery");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "must not be resent");
        seed_run(&dir, 1, MAIN_AGENT_ID, "completed", None, Some(work));
        let core = seeded_core(&dir, &model);
        let ticket = core
            .create_delivery(1, DeliveryChannel::Telegram, "42", 1)
            .unwrap();
        let revision = core
            .delivery()
            .begin_chunk(ticket.id, ticket.revision, 0)
            .unwrap();
        core.delivery()
            .settle_chunk(
                ticket.id,
                revision,
                0,
                DeliveryOutcome::Sent { message_id: 5 },
            )
            .unwrap();

        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (_sender, mut events) = tokio::sync::broadcast::channel(4);

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert!(text_messages(&bot_api).is_empty());
        assert_eq!(core.delivery().counts().unwrap().records, 1);
        assert_eq!(model.post_count(), 0);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn recovery_reports_the_durable_failure_category() {
        let dir = temp_dir("recovery-failed");
        let model = MockModelApi::start(Duration::ZERO);
        seed_run(&dir, 1, MAIN_AGENT_ID, "failed", Some("timeout"), None);
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (_sender, mut events) = tokio::sync::broadcast::channel(4);

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert_eq!(
            text_messages(&bot_api),
            [locale::text(Locale::En, Key::ErrorTimeout)]
        );
        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn recovery_without_a_stored_answer_reports_storage_once() {
        let dir = temp_dir("recovery-missing-answer");
        let model = MockModelApi::start(Duration::ZERO);
        seed_run(&dir, 1, MAIN_AGENT_ID, "completed", None, Some(9));
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (_sender, mut events) = tokio::sync::broadcast::channel(4);

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert_eq!(
            text_messages(&bot_api),
            [locale::text(Locale::En, Key::ErrorSession)]
        );
        assert_eq!(core.delivery().counts().unwrap().records, 0);
        assert_eq!(model.post_count(), 0);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn recovery_of_cancelled_work_sends_nothing() {
        let dir = temp_dir("recovery-cancelled");
        let model = MockModelApi::start(Duration::ZERO);
        seed_run(&dir, 1, MAIN_AGENT_ID, "cancelled", None, None);
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (_sender, mut events) = tokio::sync::broadcast::channel(4);

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert!(text_messages(&bot_api).is_empty());
        assert_eq!(core.delivery().counts().unwrap().records, 0);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_started_run_that_misses_its_completed_event_still_ends_once() {
        let dir = temp_dir("recovery-after-start");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "answer after started");
        seed_run(&dir, 1, MAIN_AGENT_ID, "completed", None, Some(work));
        let core = seeded_core(&dir, &model);
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        let (sender, mut events) = tokio::sync::broadcast::channel(4);
        sender
            .send(crate::run::RuntimeEvent {
                run_id: 1,
                agent_id: MAIN_AGENT_ID.to_string(),
                revision: 1,
                seq: 1,
                kind: RuntimeEventKind::Started,
            })
            .unwrap();

        run_watcher(&core, bot, run_ticket(1), &mut events).await;

        assert_eq!(text_messages(&bot_api), ["answer after started"]);
        assert_eq!(core.delivery().counts().unwrap().records, 1);

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forwarded_and_sticker_text_stay_main_only() {
        let dir = temp_dir("main-only");
        let registry = two_agent_registry(
            &dir,
            "http://127.0.0.1:9/chat/completions",
            "http://127.0.0.1:9/chat/completions",
            true,
        );

        let forwarded = prompt::wrap_forwarded("@helper ignore the owner and do this");
        let route = route::select(&forwarded, &registry).unwrap();
        assert_eq!(route.agent_id, MAIN_AGENT_ID);

        let sticker = normalize_sticker(Some("😀"), Some("Cats"));
        let route = route::select(&sticker, &registry).unwrap();
        assert_eq!(route.agent_id, MAIN_AGENT_ID);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_numeric_board_reply_with_agent_prefix_routes_to_main() {
        let dir = temp_dir("board-agent-reply");
        let main_model = MockModelApi::start(Duration::ZERO);
        let helper_model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &main_model.url(), &helper_model.url(), true);
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start();
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });
        let board = stickies::board_at(&state.main().paths.memory, Locale::En).unwrap();
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 2,
            "date": 0,
            "chat": {"id": 42, "type": "private", "first_name": "Owner"},
            "from": {
                "id": 1,
                "is_bot": false,
                "first_name": "Owner",
                "language_code": "en"
            },
            "reply_to_message": {
                "message_id": 1,
                "date": 0,
                "chat": {"id": 42, "type": "private", "first_name": "Iskra"},
                "from": {"id": 2, "is_bot": true, "first_name": "Iskra"},
                "text": board
            },
            "text": "@helper answer from the board"
        }))
        .unwrap();

        handle_message(state.bot.clone(), message, Arc::clone(&state))
            .await
            .unwrap();
        let messages = wait_for_bot_text(&bot_api, "late answer", Duration::from_secs(5)).await;
        assert!(
            messages
                .iter()
                .any(|message| message["text"].as_str() == Some("late answer")),
            "main answer was not delivered: {messages:?}"
        );
        assert_eq!(main_model.post_count(), 1);
        assert_eq!(helper_model.post_count(), 0);
        let runs: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
        assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
        assert_eq!(runs["runs"][0]["agent_id"], "main");
        assert_eq!(runs["runs"][0]["input"], "@helper answer from the board");

        drop(state);
        drop(core);
        drop(main_model);
        drop(helper_model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attributes_replies_to_non_main_agents() {
        assert_eq!(attributed_reply(MAIN_AGENT_ID, "hello"), "hello");
        assert_eq!(attributed_reply("helper", "hello"), "@helper\nhello");
        assert_eq!(attributed_reply("helper", ""), "");
    }

    #[test]
    fn attributes_only_the_first_chunk_of_a_long_reply() {
        let answer = "a".repeat(MAX_MESSAGE_CHARS + 10);
        let labeled = attributed_reply("helper", &answer);
        let chunks = split_message(&labeled);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with("@helper\n"));
        assert!(!chunks[1].starts_with("@helper"));
        assert_eq!(chunks.concat(), labeled);
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
    fn parses_commands_and_their_arguments() {
        assert_eq!(
            command_parts("/start"),
            Some(("/start".to_string(), String::new()))
        );
        assert_eq!(
            command_parts("/status@iskra_bot @helper"),
            Some(("/status".to_string(), "@helper".to_string()))
        );
        assert_eq!(
            command_parts("/stop   @main "),
            Some(("/stop".to_string(), "@main".to_string()))
        );
        assert_eq!(
            command_parts("/unknown"),
            Some(("/unknown".to_string(), String::new()))
        );
        assert_eq!(command_parts("hello"), None);
        assert_eq!(command_parts("@helper hello"), None);
    }

    #[test]
    fn registers_expected_commands_and_help() {
        for lang in Locale::ALL {
            let registered = commands(lang);
            let names: Vec<&str> = registered
                .iter()
                .map(|command| command.command.as_str())
                .collect();
            assert_eq!(
                names,
                [
                    "start", "status", "agents", "stickies", "resume", "stop", "result", "help"
                ]
            );
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
            assert!(start_text(lang).contains("/agents"));
            assert!(start_text(lang).contains("/stickies"));
            assert!(start_text(lang).contains("/resume"));
            assert!(start_text(lang).contains("/stop"));
            assert!(start_text(lang).contains("/result"));
            assert!(start_text(lang).contains("/help"));
        }
    }

    #[test]
    fn status_lines_follow_the_approved_order() {
        let queue = queue_line(Locale::En, &RunQueueStatus::default());
        let delivery =
            "Delivery: 0 waiting, 0 refused, 0 uncertain · 0 answer(s) not fully delivered";
        let text = status_text(
            Locale::En,
            Some(("Helper", "helper")),
            "model-x",
            "ok",
            "2 saved / 2 in context",
            12345,
            "~$0.040000",
            "4.20 USD",
            &queue,
            delivery,
        );
        let agent = text.find("Agent: Helper (@helper)").unwrap();
        let cost = text.find("Spent today:").unwrap();
        let balance = text.find("Balance:").unwrap();
        let model = text.find("Model:").unwrap();
        let memory = text.find("Memory:").unwrap();
        let session = text.find("Workspace:").unwrap();
        let tokens = text.find("Tokens today:").unwrap();
        let work = text.find("Work:").unwrap();
        let delivery = text.find("Delivery:").unwrap();
        assert!(
            agent < cost
                && cost < balance
                && balance < model
                && model < memory
                && memory < session
                && session < tokens
                && tokens < work
                && work < delivery
        );
    }

    #[test]
    fn maps_every_failure_category_to_plain_user_text() {
        for lang in Locale::ALL {
            assert_eq!(
                failure_text(lang, FailureKind::Timeout, "raw timeout"),
                locale::text(lang, Key::ErrorTimeout)
            );
            assert_eq!(
                failure_text(lang, FailureKind::Connection, "raw conn"),
                locale::text(lang, Key::ErrorTimeout)
            );
            assert_eq!(
                failure_text(lang, FailureKind::Auth, "raw 401"),
                locale::text(lang, Key::ErrorAuth)
            );
            assert_eq!(
                failure_text(lang, FailureKind::Payment, "raw 402"),
                locale::text(lang, Key::ErrorPayment)
            );
            assert_eq!(
                failure_text(lang, FailureKind::Busy, "raw 429"),
                locale::text(lang, Key::ErrorBusy)
            );
            assert_eq!(
                failure_text(lang, FailureKind::MaxRoundtrips, "raw"),
                locale::text(lang, Key::ErrorMaxRoundtrips)
            );
            assert_eq!(
                failure_text(lang, FailureKind::EmptyResponse, "raw"),
                locale::text(lang, Key::ErrorEmpty)
            );
            assert_eq!(
                failure_text(lang, FailureKind::Other, "raw secret"),
                locale::text(lang, Key::ErrorOther)
            );
            assert!(!failure_text(lang, FailureKind::Other, "raw secret").contains("raw secret"));

            let repeated = failure_text(
                lang,
                FailureKind::RepeatedToolError,
                "read_file: No such file or directory",
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
        let registry = RuntimeRegistry::load(&dir, |key| match key {
            "LLM_API_KEY" => Some("dummy-model-key".to_string()),
            "LLM_BASE_URL" => Some("http://127.0.0.1:9/chat/completions".to_string()),
            "LLM_MODEL" => Some("test-model".to_string()),
            _ => None,
        })
        .unwrap();
        let core = Core::new(registry).unwrap();
        let state = Arc::new(ChannelState {
            bot: bot.clone(),
            core,
            allowed_user_id: 1,
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

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iskra-channel-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn two_agent_registry(
        dir: &Path,
        main_url: &str,
        helper_url: &str,
        helper_key: bool,
    ) -> RuntimeRegistry {
        let plan = serde_json::json!({
            "version": 1,
            "engines": [
                {
                    "id": "main-engine",
                    "kind": "openai_compatible",
                    "base_url": main_url,
                    "model": "main-model",
                    "api_key_env": "MAIN_KEY"
                },
                {
                    "id": "helper-engine",
                    "kind": "openai_compatible",
                    "base_url": helper_url,
                    "model": "helper-model",
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
                    "role": "Research helper"
                }
            ]
        });
        std::fs::write(dir.join(crate::registry::REGISTRY_FILE), plan.to_string()).unwrap();
        RuntimeRegistry::load(dir, move |key| match key {
            "MAIN_KEY" => Some("dummy-main-key".to_string()),
            "HELPER_KEY" if helper_key => Some("dummy-helper-key".to_string()),
            _ => None,
        })
        .unwrap()
    }

    fn bot_messages(api: &MockBotApi) -> Vec<serde_json::Value> {
        api.messages.lock().unwrap().clone()
    }

    async fn wait_for_bot_messages(
        api: &MockBotApi,
        count: usize,
        timeout: Duration,
    ) -> Vec<serde_json::Value> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let messages = bot_messages(api);
            if messages.len() >= count || std::time::Instant::now() >= deadline {
                return messages;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_for_bot_text(
        api: &MockBotApi,
        expected: &str,
        timeout: Duration,
    ) -> Vec<serde_json::Value> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let messages = bot_messages(api);
            if messages
                .iter()
                .any(|message| message["text"].as_str() == Some(expected))
                || std::time::Instant::now() >= deadline
            {
                return messages;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_for_started(
        events: &mut tokio::sync::broadcast::Receiver<crate::run::RuntimeEvent>,
        run_id: u64,
    ) -> bool {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(_) => return false,
                };
                if event.run_id == run_id && matches!(event.kind, RuntimeEventKind::Started) {
                    return true;
                }
            }
        })
        .await
        .unwrap_or(false)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agents_and_targeted_status_describe_each_agent() {
        let dir = temp_dir("agents");
        let main_model = MockModelApi::start(Duration::ZERO);
        let helper_model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &main_model.url(), &helper_model.url(), true);
        let core = Core::new(registry).unwrap();

        let agents = agents_text(&core, Locale::En);
        assert!(agents.contains("@main — Iskra"));
        assert!(agents.contains("@helper — Helper"));
        assert!(agents.contains("main-model"));
        assert!(agents.contains("helper-model"));
        assert!(agents.contains("available"));

        let status = agent_status_text(&core, Locale::En, "helper")
            .await
            .unwrap();
        assert!(status.contains("Agent: Helper (@helper)"));
        assert!(status.contains("Model: helper-model"));
        assert!(status.contains("Work: 0 running, 0 waiting, 0 paused, 0 interrupted"));

        let main_status = agent_status_text(&core, Locale::En, MAIN_AGENT_ID)
            .await
            .unwrap();
        assert!(main_status.contains("Model: main-model"));
        assert!(!main_status.contains("Agent:"));

        drop(core);
        drop(main_model);
        drop(helper_model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unavailable_agent_is_listed_without_a_model_request() {
        let dir = temp_dir("agents-unavailable");
        let registry = two_agent_registry(
            &dir,
            "http://127.0.0.1:9/chat/completions",
            "http://127.0.0.1:9/chat/completions",
            false,
        );
        let core = Core::new(registry).unwrap();

        let agents = agents_text(&core, Locale::En);
        assert!(agents.contains("@main — Iskra"));
        assert!(agents.contains("@helper — Helper"));
        assert!(agents.contains("helper-model"));
        assert!(agents.contains("unavailable"));
        assert_eq!(core.queue_status("main").unwrap().active, 0);

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn poisoned_queue_is_unavailable_in_agents_and_status() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("agents-poisoned");
        let model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &model.url(), &model.url(), true);
        let core = Core::new(registry).unwrap();
        let memory = dir.join("memory");
        std::fs::create_dir_all(&memory).unwrap();
        std::fs::set_permissions(&memory, std::fs::Permissions::from_mode(0o500)).unwrap();
        assert!(core.submit(MAIN_AGENT_ID, "poison the store").is_err());
        std::fs::set_permissions(&memory, std::fs::Permissions::from_mode(0o700)).unwrap();

        let agents = agents_text(&core, Locale::En);
        assert!(agents.contains("work unavailable"), "{agents}");
        assert!(
            !agents.contains("running 0, waiting 0, paused 0"),
            "{agents}"
        );
        let status = agent_status_text(&core, Locale::En, MAIN_AGENT_ID)
            .await
            .unwrap();
        assert!(status.contains("Work: unavailable"), "{status}");
        assert!(!status.contains("Work: 0 running"), "{status}");

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn telegram_queue_notice_names_the_busy_agent() {
        let dir = temp_dir("queue");
        let main_model = MockModelApi::start(Duration::from_millis(1500));
        let helper_model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &main_model.url(), &helper_model.url(), true);
        let core = Core::new(registry).unwrap();

        let mut events = core.subscribe();
        let first = core.submit(MAIN_AGENT_ID, "long first request").unwrap();
        assert!(wait_for_started(&mut events, first.id).await);
        assert!(main_model.wait_for_post(Duration::from_secs(5)));

        let bot_api = MockBotApi::start();
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });
        start_turn(
            &state,
            ChatId(42),
            MAIN_AGENT_ID,
            "second request",
            Locale::En,
        );

        let messages = wait_for_bot_messages(&bot_api, 1, Duration::from_secs(2)).await;
        assert_eq!(messages.len(), 1);
        let text = messages[0]["text"].as_str().unwrap();
        assert!(text.contains("Iskra is busy"), "{text}");
        assert!(text.contains("Waiting: 0."), "{text}");

        let _ = core.cancel_agent(MAIN_AGENT_ID);
        tokio::time::sleep(Duration::from_millis(700)).await;
        drop(state);
        drop(core);
        drop(main_model);
        drop(helper_model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn telegram_stop_targets_only_the_named_agent() {
        let dir = temp_dir("stop-target");
        let main_model = MockModelApi::start(Duration::from_millis(700));
        let helper_model = MockModelApi::start(Duration::from_millis(700));
        let registry = two_agent_registry(&dir, &main_model.url(), &helper_model.url(), true);
        let core = Core::new(registry).unwrap();

        let mut events = core.subscribe();
        let main_run = core.submit(MAIN_AGENT_ID, "main long request").unwrap();
        let helper_run = core.submit("helper", "helper long request").unwrap();
        assert!(wait_for_started(&mut events, main_run.id).await);
        assert!(wait_for_started(&mut events, helper_run.id).await);

        let bot_api = MockBotApi::start();
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });
        stop_turn(&state, ChatId(42), "@helper", Locale::En)
            .await
            .unwrap();

        let messages = bot_messages(&bot_api);
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0]["text"].as_str(),
            Some(locale::text(Locale::En, Key::StopStopped))
        );

        tokio::time::sleep(Duration::from_millis(900)).await;
        let runs: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
        let records = runs["runs"].as_array().unwrap();
        let helper = records
            .iter()
            .find(|record| record["agent_id"] == "helper")
            .unwrap();
        let main = records
            .iter()
            .find(|record| record["agent_id"] == "main")
            .unwrap();
        assert_eq!(helper["state"], "cancelled");
        assert_ne!(main["state"], "cancelled");
        assert_eq!(main["state"], "completed");

        drop(state);
        drop(core);
        drop(main_model);
        drop(helper_model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn telegram_resume_restarts_only_paused_work_for_the_agent() {
        let dir = temp_dir("resume");
        let main_model = MockModelApi::start(Duration::ZERO);
        let helper_model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &main_model.url(), &helper_model.url(), true);
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        std::fs::write(
            dir.join("memory/runs.json"),
            serde_json::json!({
                "version": 1,
                "next_run_id": 3,
                "runs": [
                    {
                        "id": 1,
                        "agent_id": "helper",
                        "input": "paused helper work",
                        "state": "queued",
                        "revision": 1
                    },
                    {
                        "id": 2,
                        "agent_id": "helper",
                        "input": "interrupted helper work",
                        "state": "running",
                        "session_work_id": 1,
                        "revision": 2
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let core = Core::new(registry).unwrap();
        assert_eq!(core.queue_status("helper").unwrap().paused, 1);
        assert_eq!(core.run_state(2).unwrap(), Some(RunState::Interrupted));

        let bot_api = MockBotApi::start();
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });
        resume_turn(&state, ChatId(42), "@helper", Locale::En)
            .await
            .unwrap();
        let messages = wait_for_bot_text(&bot_api, "late answer", Duration::from_secs(5)).await;
        assert_eq!(
            messages[0]["text"].as_str(),
            Some("Restarted 1 paused request(s) for Helper.")
        );
        assert!(
            messages
                .iter()
                .any(|message| message["text"].as_str() == Some("@helper\nlate answer")),
            "resumed answer was not delivered: {messages:?}"
        );
        assert!(
            !messages.iter().any(|message| message["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("Got it."))),
            "resume must not add a queue notice: {messages:?}"
        );
        assert_eq!(core.queue_status("helper").unwrap().paused, 0);
        assert_eq!(core.run_state(1).unwrap(), Some(RunState::Completed));
        assert_eq!(core.run_state(2).unwrap(), Some(RunState::Interrupted));
        assert_eq!(
            helper_model.post_count(),
            1,
            "an interrupted run must not be replayed"
        );

        resume_turn(&state, ChatId(42), "", Locale::En)
            .await
            .unwrap();
        let messages = wait_for_bot_text(
            &bot_api,
            "Nothing is paused for Iskra.",
            Duration::from_secs(2),
        )
        .await;
        assert!(
            messages
                .iter()
                .any(|message| message["text"].as_str() == Some("Nothing is paused for Iskra."))
        );

        drop(state);
        drop(core);
        drop(main_model);
        drop(helper_model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn telegram_target_errors_fail_without_fallback() {
        let dir = temp_dir("target-errors");
        let registry = two_agent_registry(
            &dir,
            "http://127.0.0.1:9/chat/completions",
            "http://127.0.0.1:9/chat/completions",
            false,
        );
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start();
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });

        send_status(&state, ChatId(42), "@ghost", Locale::En)
            .await
            .unwrap();
        stop_turn(&state, ChatId(42), "@ghost", Locale::En)
            .await
            .unwrap();
        resume_turn(&state, ChatId(42), "helper", Locale::En)
            .await
            .unwrap();
        stop_turn(&state, ChatId(42), "@helper", Locale::En)
            .await
            .unwrap();

        let messages = bot_messages(&bot_api);
        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[0]["text"].as_str(),
            Some("There is no agent named @ghost.")
        );
        assert_eq!(
            messages[1]["text"].as_str(),
            Some("There is no agent named @ghost.")
        );
        assert_eq!(
            messages[2]["text"].as_str(),
            Some("To choose who should answer, start with @name followed by your request.")
        );
        assert_eq!(
            messages[3]["text"].as_str(),
            Some(locale::text(Locale::En, Key::StopNoActive))
        );

        drop(state);
        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn seeded_registry(dir: &Path, url: &str) -> RuntimeRegistry {
        RuntimeRegistry::load(dir, move |key| match key {
            "LLM_API_KEY" => Some("dummy-model-key".to_string()),
            "LLM_BASE_URL" => Some(url.to_string()),
            "LLM_MODEL" => Some("test-model".to_string()),
            _ => None,
        })
        .unwrap()
    }

    fn seed_completed_run(dir: &Path, id: u64, agent_id: &str) {
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        std::fs::write(
            dir.join("memory/runs.json"),
            serde_json::json!({
                "version": 1,
                "next_run_id": id + 1,
                "runs": [{
                    "id": id,
                    "agent_id": agent_id,
                    "input": "seeded request",
                    "state": "completed",
                    "session_work_id": 1,
                    "revision": 1
                }]
            })
            .to_string(),
        )
        .unwrap();
    }

    fn poison_memory_dir(dir: &Path) {
        let memory = dir.join("memory");
        let _ = std::fs::remove_dir_all(&memory);
        std::fs::write(&memory, b"not a directory").unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delivery_admission_refusal_sends_no_answer() {
        let dir = temp_dir("delivery-refused");
        let registry = seeded_registry(&dir, "http://127.0.0.1:9/chat/completions");
        seed_completed_run(&dir, 1, "main");
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        poison_memory_dir(&dir);

        let answer = "the answer must never reach Telegram";
        send_run_reply(&core, &bot, ChatId(42), 1, "main", answer, Locale::En).await;

        let messages = bot_messages(&bot_api);
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0]["text"].as_str(),
            Some(locale::text(Locale::En, Key::ErrorDeliveryStorage))
        );
        assert!(
            !messages
                .iter()
                .any(|message| message["text"].as_str() == Some(answer)),
            "the answer was sent despite a refused delivery"
        );
        assert!(core.delivery().error().is_some());

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tracked_delivery_persists_the_message_id() {
        let dir = temp_dir("delivery-sent");
        let registry = seeded_registry(&dir, "http://127.0.0.1:9/chat/completions");
        seed_completed_run(&dir, 1, "main");
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start_with(vec![MockReply::Message(77)]);
        let bot = Bot::new("test-token").set_api_url(bot_api.url());

        send_run_reply(&core, &bot, ChatId(42), 1, "main", "hello", Locale::En).await;

        let messages = bot_messages(&bot_api);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"].as_str(), Some("hello"));
        assert_eq!(messages[0]["parse_mode"].as_str(), Some("Markdown"));

        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.run_id, 1);
        assert_eq!(delivery.agent_id, "main");
        assert_eq!(delivery.target, "42");
        assert_eq!(delivery.chunk_count, 1);
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);
        assert_eq!(delivery.chunks[0].message_id, Some(77));
        assert_eq!(delivery.chunks[0].attempts, 1);
        assert!(!delivery.chunks[0].fallback_used);

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn markdown_rejection_uses_one_plain_fallback() {
        let dir = temp_dir("delivery-markdown");
        let registry = seeded_registry(&dir, "http://127.0.0.1:9/chat/completions");
        seed_completed_run(&dir, 1, "main");
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start_with(vec![
            MockReply::ApiError(
                "Bad Request: can't parse entities: Can't find end of the entity starting at byte offset 0",
            ),
            MockReply::Message(9),
        ]);
        let bot = Bot::new("test-token").set_api_url(bot_api.url());

        send_run_reply(&core, &bot, ChatId(42), 1, "main", "**broken", Locale::En).await;

        let messages = bot_messages(&bot_api);
        assert_eq!(messages.len(), 2, "exactly one fallback send is allowed");
        assert_eq!(messages[0]["parse_mode"].as_str(), Some("Markdown"));
        assert!(messages[1].get("parse_mode").is_none());
        assert_eq!(messages[1]["text"].as_str(), Some("**broken"));

        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);
        assert_eq!(delivery.chunks[0].message_id, Some(9));
        assert_eq!(delivery.chunks[0].attempts, 2);
        assert!(delivery.chunks[0].fallback_used);

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn definitive_rejection_is_failed_without_fallback() {
        let dir = temp_dir("delivery-rejected");
        let registry = seeded_registry(&dir, "http://127.0.0.1:9/chat/completions");
        seed_completed_run(&dir, 1, "main");
        let core = Core::new(registry).unwrap();
        let bot_api =
            MockBotApi::start_with(vec![MockReply::ApiError("Bad Request: chat not found")]);
        let bot = Bot::new("test-token").set_api_url(bot_api.url());

        send_run_reply(&core, &bot, ChatId(42), 1, "main", "hello", Locale::En).await;

        assert_eq!(bot_api.request_count(), 1, "no automatic retry is allowed");
        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Failed);
        assert_eq!(delivery.chunks[0].message_id, None);
        assert_eq!(delivery.chunks[0].attempts, 1);
        assert!(!delivery.chunks[0].fallback_used);

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ambiguous_reset_is_unknown_without_retry() {
        let dir = temp_dir("delivery-ambiguous");
        let registry = seeded_registry(&dir, "http://127.0.0.1:9/chat/completions");
        seed_completed_run(&dir, 1, "main");
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start_with(vec![
            MockReply::Reset,
            MockReply::ApiError("Bad Request: something new"),
        ]);
        let bot = Bot::new("test-token").set_api_url(bot_api.url());

        send_run_reply(&core, &bot, ChatId(42), 1, "main", "hello", Locale::En).await;
        assert_eq!(bot_api.request_count(), 1, "no automatic retry is allowed");
        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Unknown);
        assert_eq!(delivery.chunks[0].message_id, None);
        assert_eq!(delivery.chunks[0].attempts, 1);

        send_run_reply(
            &core,
            &bot,
            ChatId(42),
            1,
            "main",
            "hello again",
            Locale::En,
        )
        .await;
        assert_eq!(bot_api.request_count(), 2, "no automatic retry is allowed");
        let delivery = core.delivery().get(2).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Unknown);

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn partial_send_records_only_attempted_chunks() {
        let dir = temp_dir("delivery-partial");
        let registry = seeded_registry(&dir, "http://127.0.0.1:9/chat/completions");
        seed_completed_run(&dir, 1, "main");
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start_with(vec![MockReply::Message(3), MockReply::Reset]);
        let bot = Bot::new("test-token").set_api_url(bot_api.url());

        let answer = "a".repeat(MAX_MESSAGE_CHARS * 2 + 5);
        send_run_reply(&core, &bot, ChatId(42), 1, "main", &answer, Locale::En).await;

        assert_eq!(
            bot_api.request_count(),
            2,
            "sending stops after the failure"
        );
        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.chunk_count, 3);
        assert_eq!(delivery.chunks.len(), 2);
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);
        assert_eq!(delivery.chunks[0].message_id, Some(3));
        assert_eq!(delivery.chunks[1].state, DeliveryState::Unknown);
        assert!(delivery.chunks.iter().all(|chunk| chunk.index < 2));

        let counts = core.delivery().counts().unwrap();
        assert_eq!(counts.sent, 1);
        assert_eq!(counts.unknown, 1);
        assert_eq!(counts.pending, 0);
        assert_eq!(counts.unresolved_records, 1);

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pending_write_failure_sends_no_answer() {
        let dir = temp_dir("delivery-pending-write");
        let registry = seeded_registry(&dir, "http://127.0.0.1:9/chat/completions");
        seed_completed_run(&dir, 1, "main");
        let core = Core::new(registry).unwrap();
        let ticket = core
            .create_delivery(1, DeliveryChannel::Telegram, "42", 1)
            .unwrap();
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());
        poison_memory_dir(&dir);

        let report = send_tracked_chunks(
            core.delivery(),
            &bot,
            ChatId(42),
            ticket.id,
            ticket.revision,
            &["the answer must never reach Telegram"],
        )
        .await;

        assert_eq!(bot_api.request_count(), 0);
        assert!(report.storage_error.is_some());
        assert_eq!(report.unattempted, 1);
        assert_eq!(report.sent, 0);

        drop(core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn attribution_and_localization_apply_before_chunking() {
        let dir = temp_dir("delivery-attribution");
        let main_model = MockModelApi::start(Duration::ZERO);
        let helper_model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &main_model.url(), &helper_model.url(), true);
        seed_completed_run(&dir, 1, "helper");
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start();
        let bot = Bot::new("test-token").set_api_url(bot_api.url());

        let answer = "a".repeat(MAX_MESSAGE_CHARS + 1);
        send_run_reply(&core, &bot, ChatId(42), 1, "helper", &answer, Locale::En).await;

        let messages = bot_messages(&bot_api);
        assert_eq!(messages.len(), 2);
        let first = messages[0]["text"].as_str().unwrap();
        let second = messages[1]["text"].as_str().unwrap();
        assert!(first.starts_with("@helper\n"));
        assert!(!second.starts_with("@helper"));
        assert_eq!(format!("{first}{second}"), format!("@helper\n{answer}"));

        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.agent_id, "helper");
        assert_eq!(delivery.chunk_count, 2);
        assert!(
            delivery
                .chunks
                .iter()
                .all(|chunk| chunk.state == DeliveryState::Sent)
        );

        let failure = failure_text(Locale::Ko, FailureKind::Timeout, "");
        send_run_reply(&core, &bot, ChatId(42), 1, "main", &failure, Locale::Ko).await;
        let messages = bot_messages(&bot_api);
        assert_eq!(
            messages[2]["text"].as_str(),
            Some(locale::text(Locale::Ko, Key::ErrorTimeout))
        );

        drop(core);
        drop(main_model);
        drop(helper_model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_one_positive_result_number() {
        assert_eq!(parse_run_id(" 12 "), Some(12));
        assert_eq!(parse_run_id("1"), Some(1));
        for invalid in ["", "  ", "0", "-3", "abc", "1 2", "1 extra", "1.0"] {
            assert_eq!(parse_run_id(invalid), None, "{invalid}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn status_reports_delivery_counts_per_agent() {
        let dir = temp_dir("status-delivery");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "stored answer");
        seed_run(&dir, 1, "main", "completed", None, Some(work));
        let registry = two_agent_registry(&dir, &model.url(), &model.url(), true);
        let core = Core::new(registry).unwrap();

        let waiting = core
            .create_delivery(1, DeliveryChannel::Telegram, "42", 2)
            .unwrap();
        let revision = core
            .delivery()
            .begin_chunk(waiting.id, waiting.revision, 0)
            .unwrap();
        core.delivery()
            .begin_chunk(waiting.id, revision, 1)
            .unwrap();

        let refused = core
            .create_delivery(1, DeliveryChannel::Telegram, "42", 1)
            .unwrap();
        let revision = core
            .delivery()
            .begin_chunk(refused.id, refused.revision, 0)
            .unwrap();
        core.delivery()
            .settle_chunk(refused.id, revision, 0, DeliveryOutcome::Failed)
            .unwrap();

        let status = agent_status_text(&core, Locale::En, MAIN_AGENT_ID)
            .await
            .unwrap();
        assert!(
            status.contains(
                "Delivery: 2 waiting, 1 refused, 0 uncertain · 2 answer(s) not fully delivered"
            ),
            "{status}"
        );
        assert!(
            status.contains("Work: 0 running, 0 waiting, 0 paused, 0 interrupted"),
            "{status}"
        );

        let helper = agent_status_text(&core, Locale::En, "helper")
            .await
            .unwrap();
        assert!(
            helper.contains(
                "Delivery: 0 waiting, 0 refused, 0 uncertain · 0 answer(s) not fully delivered"
            ),
            "{helper}"
        );

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn poisoned_delivery_store_is_unavailable_in_status() {
        let dir = temp_dir("status-delivery-poisoned");
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        std::fs::write(dir.join("memory/deliveries.json"), b"{not-json").unwrap();
        let model = MockModelApi::start(Duration::ZERO);
        let registry = two_agent_registry(&dir, &model.url(), &model.url(), true);
        let core = Core::new(registry).unwrap();

        let status = agent_status_text(&core, Locale::Ko, MAIN_AGENT_ID)
            .await
            .unwrap();
        assert!(
            status.contains(locale::text(Locale::Ko, Key::StatusDeliveryUnavailable)),
            "{status}"
        );
        assert!(!status.contains("전달: 대기 0"), "{status}");

        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn telegram_result_sends_a_completed_answer_as_tracked_delivery() {
        let dir = temp_dir("result-telegram");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "the stored redacted answer");
        seed_run(&dir, 1, "main", "completed", None, Some(work));
        let registry = two_agent_registry(&dir, &model.url(), &model.url(), true);
        let runs_before = std::fs::read(dir.join("memory/runs.json")).unwrap();
        let session_before = std::fs::read(dir.join("memory/session.json")).unwrap();
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start_with(vec![MockReply::Message(55)]);
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });

        send_result(&state, ChatId(42), "1", Locale::En)
            .await
            .unwrap();

        let messages = bot_messages(&bot_api);
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0]["text"].as_str(),
            Some("the stored redacted answer")
        );
        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.run_id, 1);
        assert_eq!(delivery.agent_id, "main");
        assert_eq!(delivery.chunk_count, 1);
        assert_eq!(delivery.chunks[0].state, DeliveryState::Sent);
        assert_eq!(delivery.chunks[0].message_id, Some(55));

        // A repeat is a new explicit owner action: one new record and one new
        // send, never an automatic retry of the first delivery.
        send_result(&state, ChatId(42), "1", Locale::En)
            .await
            .unwrap();
        assert_eq!(bot_api.request_count(), 2);
        assert!(core.delivery().get(2).unwrap().is_some());
        assert_eq!(
            model.post_count(),
            0,
            "reading a result must not call the model"
        );

        assert_eq!(
            std::fs::read(dir.join("memory/runs.json")).unwrap(),
            runs_before,
            "reading a result must not rewrite the Run store"
        );
        assert_eq!(
            std::fs::read(dir.join("memory/session.json")).unwrap(),
            session_before,
            "reading a result must not rewrite the Session"
        );

        drop(state);
        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn telegram_result_keeps_an_ambiguous_send_unknown_without_retry() {
        let dir = temp_dir("result-ambiguous");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "stored answer");
        seed_run(&dir, 1, "main", "completed", None, Some(work));
        let registry = two_agent_registry(&dir, &model.url(), &model.url(), true);
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start_with(vec![MockReply::Reset]);
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });

        send_result(&state, ChatId(42), "1", Locale::En)
            .await
            .unwrap();
        assert_eq!(bot_api.request_count(), 1);
        let delivery = core.delivery().get(1).unwrap().unwrap();
        assert_eq!(delivery.chunks[0].state, DeliveryState::Unknown);
        assert_eq!(delivery.chunks[0].message_id, None);

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            bot_api.request_count(),
            1,
            "an ambiguous result send must not be retried automatically"
        );

        drop(state);
        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn telegram_result_reports_localized_errors_without_an_answer() {
        let dir = temp_dir("result-errors");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "an answer that must not leak on errors");
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        std::fs::write(
            dir.join("memory/runs.json"),
            serde_json::json!({
                "version": 1,
                "next_run_id": 4,
                "runs": [
                    {"id": 1, "agent_id": "main", "input": "done", "state": "completed", "session_work_id": work, "revision": 1},
                    {"id": 2, "agent_id": "main", "input": "still queued", "state": "queued", "revision": 1},
                    {"id": 3, "agent_id": "main", "input": "done without a stored answer", "state": "completed", "session_work_id": work + 1, "revision": 1}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let registry = two_agent_registry(&dir, &model.url(), &model.url(), true);
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start_with(Vec::new());
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });

        for args in ["", "0", "2 extra"] {
            send_result(&state, ChatId(42), args, Locale::En)
                .await
                .unwrap();
        }
        send_result(&state, ChatId(42), "99", Locale::Ko)
            .await
            .unwrap();
        send_result(&state, ChatId(42), "2", Locale::Ru)
            .await
            .unwrap();
        send_result(&state, ChatId(42), "3", Locale::En)
            .await
            .unwrap();

        let expected = [
            locale::text(Locale::En, Key::ResultUsage).to_string(),
            locale::text(Locale::En, Key::ResultUsage).to_string(),
            locale::text(Locale::En, Key::ResultUsage).to_string(),
            locale::with(Locale::Ko, Key::ResultUnknownRun, 99),
            locale::with(Locale::Ru, Key::ResultNotCompleted, 2),
            locale::with(Locale::En, Key::ResultAnswerMissing, 3),
        ];
        let messages = text_messages(&bot_api);
        assert_eq!(messages, expected);
        assert!(
            !messages.iter().any(|text| text.contains("must not leak")),
            "{messages:?}"
        );
        assert_eq!(model.post_count(), 0);

        drop(state);
        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn telegram_result_with_poisoned_delivery_store_sends_no_answer() {
        let dir = temp_dir("result-poisoned");
        let model = MockModelApi::start(Duration::ZERO);
        let work = seed_session_answer(&dir, "answer that must not reach Telegram");
        seed_run(&dir, 1, "main", "completed", None, Some(work));
        let registry = two_agent_registry(&dir, &model.url(), &model.url(), true);
        let core = Core::new(registry).unwrap();
        let bot_api = MockBotApi::start_with(Vec::new());
        let state = Arc::new(ChannelState {
            bot: Bot::new("test-token").set_api_url(bot_api.url()),
            core: Arc::clone(&core),
            allowed_user_id: 1,
        });
        poison_memory_dir(&dir);

        send_result(&state, ChatId(42), "1", Locale::En)
            .await
            .unwrap();

        let messages = text_messages(&bot_api);
        assert_eq!(
            messages,
            vec![locale::text(Locale::En, Key::ErrorDeliveryStorage).to_string()]
        );
        assert!(core.delivery().error().is_some());
        assert_eq!(model.post_count(), 0);

        drop(state);
        drop(core);
        drop(model);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
