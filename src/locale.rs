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
    StatusCost,
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
    BusyNotice,
    StopStopped,
    StopNoActive,
    ErrorTimeout,
    ErrorAuth,
    ErrorPayment,
    ErrorBusy,
    ErrorRepeatedTool,
    ErrorMaxRoundtrips,
    ErrorEmpty,
    ErrorOther,
    ReasonNotFound,
    ReasonAccessRefused,
    ReasonTooLong,
    ReasonInvalid,
    ReasonGeneric,
    NonTextNotice,
    PrivateOnly,
    CostEstimate,
    CostUnknown,
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
            "Показать траты за сегодня и баланс",
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
            "Hi, I'm Iskra.\nWhat I can do:\n• Chat, and remember what you tell me (\"remember how I take my coffee\")\n• Create and read files in my work folder\n• Pin notes to the sticky board (\"stick this on the board\")\nEverything I remember and every file stays on this computer.\nCommands: /status · /stickies · /stop · /help",
            "안녕하세요, 이스크라예요.\n할 수 있는 일:\n• 대화하고, 알려 준 것을 기억해요 (\"내 커피 취향 기억해 줘\")\n• 작업 폴더에서 파일을 만들고 읽어요\n• 스티키 보드에 메모를 붙여요 (\"이거 보드에 붙여 줘\")\n기억과 파일은 모두 이 컴퓨터에만 저장돼요.\n명령어: /status 상태 · /stickies 보드 · /stop 멈추기 · /help 도움말",
            "Привет, я Искра.\nЧто я умею:\n• Общаться и запоминать то, что вы рассказываете («запомни, какой кофе я пью»)\n• Создавать и читать файлы в рабочей папке\n• Прикреплять заметки на доску стикеров («прикрепи это на доску»)\nВсё, что я помню, и все файлы хранятся только на этом компьютере.\nКоманды: /status · /stickies · /stop · /help",
        ),
        Key::HelpHeader => (
            "Available commands:",
            "사용할 수 있는 명령어:",
            "Доступные команды:",
        ),
        Key::StatusModel => ("Model: {}", "모델: {}", "Модель: {}"),
        Key::StatusMemory => ("Memory: {}", "기억: {}", "Память: {}"),
        Key::StatusTokens => ("Tokens today: {}", "오늘 토큰: {}", "Токены сегодня: {}"),
        Key::StatusCost => (
            "Spent today: {}",
            "오늘 쓴 금액: {}",
            "Потрачено сегодня: {}",
        ),
        Key::StatusBalance => ("Balance: {}", "남은 잔액: {}", "Баланс: {}"),
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
        Key::BusyNotice => (
            "I'm still working on your last request. Send this again when I'm done, or /stop to cancel.",
            "아직 이전 일을 하는 중이에요. 끝나면 다시 보내 주세요. 멈추려면 /stop",
            "Я ещё занята предыдущей задачей. Отправьте это снова, когда закончу, или /stop, чтобы остановить.",
        ),
        Key::StopStopped => ("Stopped.", "하던 일을 멈췄어요.", "Остановила."),
        Key::StopNoActive => (
            "I'm not working on anything right now.",
            "지금 하고 있는 일이 없어요.",
            "Сейчас я ничем не занята.",
        ),
        Key::ErrorTimeout => (
            "The model server isn't responding. Please try again in a moment.",
            "모델 서버가 응답하지 않아요. 잠시 뒤 다시 보내 주세요.",
            "Сервер модели не отвечает. Попробуйте ещё раз чуть позже.",
        ),
        Key::ErrorAuth => (
            "The model API key isn't working. Please check the key on your computer.",
            "모델 API 키가 맞지 않아요. 컴퓨터에서 키 설정을 확인해 주세요.",
            "Ключ API модели не подходит. Проверьте настройку ключа на компьютере.",
        ),
        Key::ErrorPayment => (
            "Your model account balance is too low. Top it up and try again.",
            "모델 계정 잔액이 부족해요. 충전 후 다시 보내 주세요.",
            "На счёте модели недостаточно средств. Пополните его и попробуйте снова.",
        ),
        Key::ErrorBusy => (
            "The model server is busy right now. Please try again in about a minute.",
            "모델 서버가 지금 붐벼요. 1분쯤 뒤 다시 보내 주세요.",
            "Сервер модели сейчас перегружен. Попробуйте примерно через минуту.",
        ),
        Key::ErrorRepeatedTool => (
            "I stopped because the same problem kept happening: {}. Try rephrasing your request.",
            "같은 문제가 계속 나서 멈췄어요: {}. 요청을 조금 바꿔서 다시 보내 주세요.",
            "Я остановилась: одна и та же ошибка повторялась ({}). Попробуйте сформулировать запрос иначе.",
        ),
        Key::ErrorMaxRoundtrips => (
            "This was taking too many steps, so I stopped. Try splitting it into smaller requests.",
            "일이 너무 길어져서 멈췄어요. 더 작게 나눠서 부탁해 주세요.",
            "Задача заняла слишком много шагов, и я остановилась. Попробуйте разбить её на части.",
        ),
        Key::ErrorEmpty => (
            "I couldn't produce an answer. Please send it once more.",
            "답을 만들지 못했어요. 다시 한 번 보내 주세요.",
            "Не получилось сформулировать ответ. Отправьте, пожалуйста, ещё раз.",
        ),
        Key::ErrorOther => (
            "Something went wrong and I stopped. Please try again — /status shows what's going on.",
            "문제가 생겨서 멈췄어요. 다시 보내 주세요. 계속되면 /status로 상태를 확인할 수 있어요.",
            "Что-то пошло не так, и я остановилась. Попробуйте снова; /status покажет состояние.",
        ),
        Key::ReasonNotFound => (
            "it couldn't find the file or folder",
            "파일이나 폴더를 찾지 못했어요",
            "не удалось найти файл или папку",
        ),
        Key::ReasonAccessRefused => (
            "access to that file or folder was refused",
            "그 파일이나 폴더에 접근할 수 없었어요",
            "доступ к этому файлу или папке был запрещён",
        ),
        Key::ReasonTooLong => (
            "it was taking too long",
            "시간이 너무 오래 걸렸어요",
            "это заняло слишком много времени",
        ),
        Key::ReasonInvalid => (
            "the request wasn't understood",
            "요청을 이해하지 못했어요",
            "не удалось понять запрос",
        ),
        Key::ReasonGeneric => (
            "something kept failing",
            "무언가 계속 실패했어요",
            "что-то постоянно не получалось",
        ),
        Key::NonTextNotice => (
            "I can't read photos, voice messages or files yet. Please send text.",
            "아직 사진·음성·파일은 읽지 못해요. 글로 보내 주세요.",
            "Я пока не умею читать фото, голосовые и файлы. Напишите, пожалуйста, текстом.",
        ),
        Key::PrivateOnly => (
            "I only work in private chats. Please talk to me one-on-one.",
            "이스크라는 개인 채팅에서만 일해요. 저와 1:1 대화에서 불러 주세요.",
            "Я работаю только в личных чатах. Напишите мне напрямую.",
        ),
        Key::CostEstimate => ("~{} (estimate)", "약 {} (추정)", "~{} (оценка)"),
        Key::CostUnknown => (
            "unknown (no price data for this model)",
            "확인 불가 (이 모델의 가격 정보 없음)",
            "неизвестно (нет данных о цене этой модели)",
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
    fn user_strings_avoid_busy_jargon() {
        let ko_jargon = '\u{D134}';
        let ru_jargon = concat!("х", "од");
        let ru_plural = concat!("х", "од", "ы");
        let en_jargon = concat!("t", "urn");
        let en_plural = concat!("t", "urn", "s");
        for &key in Key::ALL {
            for locale in Locale::ALL {
                let value = text(locale, key).to_lowercase();
                assert!(
                    !value.contains(ko_jargon),
                    "jargon in {key:?} ({})",
                    locale.code()
                );
                assert!(
                    !value
                        .split(|c: char| !c.is_alphabetic())
                        .any(|word| word == ru_jargon || word == ru_plural),
                    "jargon in {key:?} ({})",
                    locale.code()
                );
                assert!(
                    !value
                        .split(|c: char| !c.is_alphabetic())
                        .any(|word| word == en_jargon || word == en_plural),
                    "jargon in {key:?} ({})",
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
