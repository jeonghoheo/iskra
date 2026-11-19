use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Locale {
    En = 0,
    Ko = 1,
    Ru = 2,
}

impl Locale {
    pub const ALL: [Locale; 3] = [Locale::En, Locale::Ko, Locale::Ru];

    pub fn from_language_code(code: Option<&str>) -> Self {
        let Some(code) = code else {
            return Self::En;
        };
        let primary = code
            .split(|c| c == '-' || c == '_')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match primary.as_str() {
            "ko" => Self::Ko,
            "ru" => Self::Ru,
            _ => Self::En,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ko => "ko",
            Self::Ru => "ru",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Ko,
            2 => Self::Ru,
            _ => Self::En,
        }
    }
}

static LATEST: AtomicU8 = AtomicU8::new(Locale::En as u8);

pub fn set_latest(locale: Locale) {
    LATEST.store(locale as u8, Ordering::Relaxed);
}

pub fn latest() -> Locale {
    Locale::from_u8(LATEST.load(Ordering::Relaxed))
}

macro_rules! define_keys {
    ($($name:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Key {
            $($name),*
        }

        impl Key {
            #[cfg(test)]
            pub const ALL: &'static [Key] = &[$(Key::$name),*];
        }
    };
}

define_keys!(
    StartDescription,
    StatusDescription,
    StickiesDescription,
    StopDescription,
    HelpDescription,
    StartGreeting,
    HelpHeader,
    StatusModel,
    StatusMemory,
    StatusTokens,
    StatusBalance,
    BoardTitle,
    BoardEmpty,
    BoardFooter,
    StickyAdded,
    StickyRemoved,
    StickyNotFound,
    CredentialRefused,
    MemoryOk,
    MemoryPartial,
    MemoryWarnings,
    TurnBusy,
    TurnStopped,
    StopStopped,
    StopNoActive,
    StickyRemovedButBoardReadFailed,
    StickyRemoveFailed,
    StickyBoardSyncFailed,
    StickyBoardDisplayFailed,
    StickyBoardReadFailed,
    StickyRemovedButBoardUpdateFailed,
    StickiesUsage,
    Unavailable,
    BalanceInsufficient,
);

pub fn text(locale: Locale, key: Key) -> &'static str {
    let (en, ko, ru) = match key {
        Key::StartDescription => (
            "Show the welcome message",
            "시작 인사 보기",
            "Показать приветствие",
        ),
        Key::StatusDescription => (
            "Show today's usage and remaining balance",
            "오늘 사용량과 남은 금액 보기",
            "Показать расход за сегодня и баланс",
        ),
        Key::StickiesDescription => (
            "Show the sticky board",
            "스티키 보드 보기",
            "Показать доску стикеров",
        ),
        Key::StopDescription => (
            "Stop the current task",
            "하던 일 멈추기",
            "Остановить текущую задачу",
        ),
        Key::HelpDescription => (
            "List available commands",
            "명령어 목록 보기",
            "Показать список команд",
        ),
        Key::StartGreeting => (
            "I am Iskra. Send me a message to chat.",
            "이스크라입니다. 메시지를 보내면 대화합니다.",
            "Я Искра. Отправьте сообщение, чтобы начать разговор.",
        ),
        Key::HelpHeader => (
            "Available commands:",
            "사용할 수 있는 명령어:",
            "Доступные команды:",
        ),
        Key::StatusModel => ("Model: {}", "모델: {}", "Модель: {}"),
        Key::StatusMemory => ("Memory: {}", "기억: {}", "Память: {}"),
        Key::StatusTokens => ("Tokens today: {}", "오늘 토큰: {}", "Токены сегодня: {}"),
        Key::StatusBalance => ("Balance: {}", "잔액: {}", "Баланс: {}"),
        Key::BoardTitle => ("Sticky board", "스티키 보드", "Доска стикеров"),
        Key::BoardEmpty => (
            "Sticky board is empty.",
            "스티키 보드가 비어 있습니다.",
            "Доска стикеров пуста.",
        ),
        Key::BoardFooter => (
            "Delete: /stickies remove N",
            "삭제: /stickies remove N",
            "Удалить: /stickies remove N",
        ),
        Key::StickyAdded => (
            "Sticky #{} added.",
            "스티키 #{}을 추가했습니다.",
            "Стикер #{} добавлен.",
        ),
        Key::StickyRemoved => (
            "Sticky #{} removed.",
            "스티키 #{}을 삭제했습니다.",
            "Стикер #{} удалён.",
        ),
        Key::StickyNotFound => (
            "Sticky #{} does not exist.",
            "스티키 #{}이 없습니다.",
            "Стикер #{} не найден.",
        ),
        Key::CredentialRefused => (
            "Content that looks like a credential cannot be saved in a sticky. Remove the secret and try again",
            "자격증명처럼 보이는 내용은 스티키에 저장할 수 없습니다. 비밀값을 지우고 다시 시도해 주세요",
            "Содержимое, похожее на учётные данные, нельзя сохранить в стикере. Удалите секрет и попробуйте снова",
        ),
        Key::MemoryOk => ("ok", "정상", "в порядке"),
        Key::MemoryPartial => ("partially loaded", "일부만 로드됨", "загружено частично"),
        Key::MemoryWarnings => ("{} warning(s)", "경고 {}건", "предупреждений: {}"),
        Key::TurnBusy => (
            "A previous turn is still running. Use /stop to interrupt it.",
            "이전 턴이 아직 진행 중입니다. /stop으로 중단할 수 있습니다.",
            "Предыдущий ход ещё выполняется. Используйте /stop, чтобы прервать его.",
        ),
        Key::TurnStopped => ("Stopped: {}", "중단: {}", "Остановлено: {}"),
        Key::StopStopped => (
            "The current turn was stopped.",
            "현재 턴을 중단했습니다.",
            "Текущий ход остановлен.",
        ),
        Key::StopNoActive => (
            "There is no turn in progress.",
            "진행 중인 턴이 없습니다.",
            "Сейчас нет выполняющегося хода.",
        ),
        Key::StickyRemovedButBoardReadFailed => (
            "Sticky #{} was removed, but the board could not be read.",
            "스티키 #{}을 삭제했지만 보드를 읽지 못했습니다.",
            "Стикер #{} удалён, но не удалось прочитать доску.",
        ),
        Key::StickyRemoveFailed => (
            "Failed to remove the sticky: {}",
            "스티키를 삭제하지 못했습니다: {}",
            "Не удалось удалить стикер: {}",
        ),
        Key::StickyBoardSyncFailed => (
            "Failed to sync the sticky board.",
            "스티키 보드를 동기화하지 못했습니다.",
            "Не удалось синхронизировать доску стикеров.",
        ),
        Key::StickyBoardDisplayFailed => (
            "Failed to display the sticky board.",
            "스티키 보드를 표시하지 못했습니다.",
            "Не удалось показать доску стикеров.",
        ),
        Key::StickyBoardReadFailed => (
            "Could not read the sticky board.",
            "스티키 보드를 읽을 수 없습니다.",
            "Не удалось прочитать доску стикеров.",
        ),
        Key::StickyRemovedButBoardUpdateFailed => (
            "Sticky #{} was removed, but the board could not be updated.",
            "스티키 #{}을 삭제했지만 보드를 갱신하지 못했습니다.",
            "Стикер #{} удалён, но не удалось обновить доску.",
        ),
        Key::StickiesUsage => (
            "Usage: /stickies (show board), /stickies remove N (remove N)",
            "사용법: /stickies (보드 보기), /stickies remove N (N번 삭제)",
            "Использование: /stickies (показать доску), /stickies remove N (удалить N)",
        ),
        Key::Unavailable => ("unavailable", "확인 불가", "недоступно"),
        Key::BalanceInsufficient => (
            " (insufficient for API calls)",
            " (API 호출에 부족)",
            " (недостаточно для вызовов API)",
        ),
    };

    match locale {
        Locale::En => en,
        Locale::Ko => ko,
        Locale::Ru => ru,
    }
}

pub fn with<T: std::fmt::Display>(locale: Locale, key: Key, value: T) -> String {
    text(locale, key).replace("{}", &value.to_string())
}

pub fn memory_status(locale: Locale, truncated: bool, warnings: usize) -> String {
    let base = text(
        locale,
        if truncated {
            Key::MemoryPartial
        } else {
            Key::MemoryOk
        },
    );
    if warnings == 0 {
        base.to_string()
    } else {
        format!("{}, {}", base, with(locale, Key::MemoryWarnings, warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_language_codes() {
        assert_eq!(Locale::from_language_code(None), Locale::En);
        assert_eq!(Locale::from_language_code(Some("")), Locale::En);
        assert_eq!(Locale::from_language_code(Some("en-US")), Locale::En);
        assert_eq!(Locale::from_language_code(Some("ko")), Locale::Ko);
        assert_eq!(Locale::from_language_code(Some("ko-KR")), Locale::Ko);
        assert_eq!(Locale::from_language_code(Some("KO_kr")), Locale::Ko);
        assert_eq!(Locale::from_language_code(Some("ru-RU")), Locale::Ru);
        assert_eq!(Locale::from_language_code(Some("de-DE")), Locale::En);
        assert_eq!(Locale::from_language_code(Some("pt-BR")), Locale::En);
    }

    #[test]
    fn table_is_complete_in_every_language() {
        for &key in Key::ALL {
            for locale in Locale::ALL {
                assert!(
                    !text(locale, key).trim().is_empty(),
                    "missing {key:?} for {}",
                    locale.code()
                );
            }
        }
    }

    #[test]
    fn formats_templates_and_memory_status() {
        assert_eq!(with(Locale::En, Key::StickyAdded, 3), "Sticky #3 added.");
        assert_eq!(
            with(Locale::Ko, Key::StickyAdded, 3),
            "스티키 #3을 추가했습니다."
        );
        assert_eq!(memory_status(Locale::En, false, 0), "ok");
        assert_eq!(
            memory_status(Locale::En, true, 2),
            "partially loaded, 2 warning(s)"
        );
        assert_eq!(memory_status(Locale::Ko, false, 0), "정상");
    }
}
