use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::memory;

pub const STICKIES_FILE: &str = "stickies.md";
pub const BOARD_STATE_FILE: &str = "board_state.json";
pub const BOARD_TITLE: &str = "Sticky board";
const MAX_TEXT_CHARS: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sticky {
    pub number: u64,
    pub created: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BoardState {
    pub chat_id: i64,
    pub message_id: i32,
}

pub fn add(text: &str) -> Result<String, String> {
    add_to(Path::new(memory::MEMORY_DIR), text)
}

pub fn board() -> Result<String, String> {
    board_from(Path::new(memory::MEMORY_DIR))
}

pub fn remove(number: u64) -> Result<String, String> {
    remove_from(Path::new(memory::MEMORY_DIR), number)
}

pub fn is_board_text(text: &str) -> bool {
    text.trim_start().starts_with(BOARD_TITLE)
}

pub fn is_empty_board(text: &str) -> bool {
    text == empty_board_text()
}

fn empty_board_text() -> String {
    format!("{BOARD_TITLE} is empty.")
}

pub fn load_board_state() -> Result<Option<BoardState>, String> {
    load_board_state_from(Path::new(memory::MEMORY_DIR))
}

fn load_board_state_from(dir: &Path) -> Result<Option<BoardState>, String> {
    let path = dir.join(BOARD_STATE_FILE);
    match fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("{}: cannot read as UTF-8", path.display()))?;
            serde_json::from_str::<Option<BoardState>>(&text)
                .map_err(|e| format!("Failed to parse board state: {e}"))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Failed to read board state: {e}")),
    }
}

pub fn save_board_state(state: &BoardState) -> Result<(), String> {
    save_board_state_to(Path::new(memory::MEMORY_DIR), Some(state))
}

pub fn clear_board_state() -> Result<(), String> {
    save_board_state_to(Path::new(memory::MEMORY_DIR), None)
}

fn save_board_state_to(dir: &Path, state: Option<&BoardState>) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Failed to create board state directory: {e}"))?;
    let content = serde_json::to_string(&state)
        .map_err(|e| format!("Failed to serialize board state: {e}"))?;
    memory::atomic_write(
        &dir.join(BOARD_STATE_FILE),
        format!("{content}\n").as_bytes(),
    )
}

fn add_to(dir: &Path, text: &str) -> Result<String, String> {
    let text = normalize_text(text)?;
    let (_, redactions) = memory::redact_credentials(&text);
    if redactions > 0 {
        return Err(
            "Content that looks like a credential cannot be saved in a sticky. Remove the secret and try again"
                .to_string(),
        );
    }

    let mut stickies = load_from(dir)?;
    let number = stickies
        .iter()
        .map(|sticky| sticky.number)
        .max()
        .unwrap_or(0)
        + 1;
    stickies.push(Sticky {
        number,
        created: memory::local_date(),
        text,
    });
    save_to(dir, &stickies)?;
    Ok(format!(
        "Sticky #{number} added.\n\n{}",
        format_board(&stickies)
    ))
}

fn board_from(dir: &Path) -> Result<String, String> {
    Ok(format_board(&load_from(dir)?))
}

fn remove_from(dir: &Path, number: u64) -> Result<String, String> {
    let mut stickies = load_from(dir)?;
    let Some(index) = stickies.iter().position(|sticky| sticky.number == number) else {
        return Err(format!("Sticky #{number} does not exist"));
    };
    stickies.remove(index);
    save_to(dir, &stickies)?;
    Ok(format!(
        "Sticky #{number} removed.\n\n{}",
        format_board(&stickies)
    ))
}

fn normalize_text(text: &str) -> Result<String, String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err("Sticky content is empty".to_string());
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!(
            "Each sticky can contain at most {MAX_TEXT_CHARS} characters"
        ));
    }
    Ok(text)
}

fn load_from(dir: &Path) -> Result<Vec<Sticky>, String> {
    let path = dir.join(STICKIES_FILE);
    match fs::read(&path) {
        Ok(bytes) => {
            let content = String::from_utf8(bytes)
                .map_err(|_| format!("{}: cannot read as UTF-8", path.display()))?;
            Ok(content.lines().filter_map(parse_line).collect())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!(
            "Failed to read stickies file ({}): {e}",
            path.display()
        )),
    }
}

fn save_to(dir: &Path, stickies: &[Sticky]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Failed to create stickies directory: {e}"))?;
    let mut content = String::from("# Stickies\n\n");
    for sticky in stickies {
        content.push_str(&format!(
            "- #{} ({}) {}\n",
            sticky.number, sticky.created, sticky.text
        ));
    }
    memory::atomic_write(&dir.join(STICKIES_FILE), content.as_bytes())
}

fn parse_line(line: &str) -> Option<Sticky> {
    let rest = line.strip_prefix("- #")?;
    let (number, rest) = rest.split_once(' ')?;
    let number = number.parse::<u64>().ok()?;
    let rest = rest.strip_prefix('(')?;
    let (created, text) = rest.split_once(") ")?;
    if created.is_empty() {
        return None;
    }
    Some(Sticky {
        number,
        created: created.to_string(),
        text: text.to_string(),
    })
}

fn format_board(stickies: &[Sticky]) -> String {
    if stickies.is_empty() {
        return empty_board_text();
    }
    let mut text = format!("{BOARD_TITLE}:");
    for sticky in stickies {
        text.push_str(&format!(
            "\n{}. {} — {}",
            sticky.number, sticky.created, sticky.text
        ));
    }
    text.push_str("\n\nDelete: /stickies remove N");
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "iskra-stickies-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn adds_lists_and_removes_with_stable_numbers() {
        let dir = temp_dir("board");

        let board = add_to(&dir, "first note").unwrap();
        assert!(board.contains("Sticky #1"));
        assert!(board.contains("\n1. "));
        assert!(board.contains("first note"));
        assert!(board.contains("/stickies remove N"));

        let board = add_to(&dir, "second note").unwrap();
        assert!(board.contains("\n2. "));
        assert!(board.contains("second note"));

        let listed = board_from(&dir).unwrap();
        assert!(listed.contains("first note"));
        assert!(listed.contains("second note"));

        let removed = remove_from(&dir, 1).unwrap();
        assert!(!removed.contains("first note"));
        assert!(removed.contains("second note"));

        let board = add_to(&dir, "third note").unwrap();
        assert!(board.contains("\n3. "));
        assert!(board.contains("third note"));

        let content = fs::read_to_string(dir.join(STICKIES_FILE)).unwrap();
        assert!(content.starts_with("# Stickies"));
        assert!(content.contains("- #2 ("));
        assert!(content.contains("- #3 ("));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(STICKIES_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_credential_looking_text() {
        let dir = temp_dir("credentials");
        add_to(&dir, "safe note").unwrap();
        let before = fs::read_to_string(dir.join(STICKIES_FILE)).unwrap();

        let telegram_token = format!("{}{}", "123456789", ":AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw");
        let texts = [
            "key is sk-test1234567890".to_string(),
            "password=hunter2".to_string(),
            "-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----".to_string(),
            format!("token {telegram_token}"),
        ];
        for text in &texts {
            let error = add_to(&dir, text).unwrap_err();
            assert!(error.contains("credential"), "{error}");
        }

        let after = fs::read_to_string(dir.join(STICKIES_FILE)).unwrap();
        assert_eq!(before, after);
        assert!(!after.contains("sk-test"));
        assert!(!after.contains("hunter2"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_board_is_recognizable() {
        let dir = temp_dir("empty");
        let board = board_from(&dir).unwrap();
        assert!(is_board_text(&board));
        assert!(board.contains("empty"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folds_newlines_into_one_line() {
        let dir = temp_dir("newlines");
        let board = add_to(&dir, "line one\nline two").unwrap();
        assert!(board.contains("line one line two"));

        let content = fs::read_to_string(dir.join(STICKIES_FILE)).unwrap();
        assert!(!content.contains("line one\nline two"));
        assert!(content.contains("line one line two"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn board_state_roundtrips_and_clears() {
        let dir = temp_dir("board-state");
        assert_eq!(load_board_state_from(&dir).unwrap(), None);

        let state = BoardState {
            chat_id: 42,
            message_id: 7,
        };
        save_board_state_to(&dir, Some(&state)).unwrap();
        assert_eq!(load_board_state_from(&dir).unwrap(), Some(state));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(BOARD_STATE_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        save_board_state_to(&dir, None).unwrap();
        assert_eq!(load_board_state_from(&dir).unwrap(), None);
        assert_eq!(
            fs::read_to_string(dir.join(BOARD_STATE_FILE)).unwrap(),
            "null\n"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_board_state_is_an_error() {
        let dir = temp_dir("board-state-corrupt");
        fs::write(dir.join(BOARD_STATE_FILE), "not json").unwrap();
        assert!(load_board_state_from(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_board_text_is_detected() {
        assert!(is_empty_board(&empty_board_text()));
        assert!(!is_empty_board(&format_board(&[Sticky {
            number: 1,
            created: "2026-11-19".to_string(),
            text: "note".to_string(),
        }])));
    }
}
