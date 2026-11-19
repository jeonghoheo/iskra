use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MEMORY_DIR: &str = "memory";
pub const MEMORY_FILE: &str = "MEMORY.md";
const MAX_MEMORY_LINES: usize = 200;
const MAX_MEMORY_BYTES: usize = 24 * 1024;
const MAX_ENTRY_CHARS: usize = 1_000;

pub struct LoadedMemory {
    pub text: String,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

pub fn load() -> LoadedMemory {
    load_from(Path::new(MEMORY_DIR))
}

fn load_from(dir: &Path) -> LoadedMemory {
    let mut warnings = Vec::new();

    if let Err(e) = fs::create_dir_all(dir) {
        warnings.push(format!("기억 폴더 생성 실패({}): {e}", dir.display()));
        return LoadedMemory {
            text: String::new(),
            truncated: false,
            warnings,
        };
    }

    let memory_path = dir.join(MEMORY_FILE);
    let (memory_text, truncated) = match read_utf8(&memory_path) {
        Ok(Some(content)) => {
            let (text, truncated) = apply_budget(&content);
            (text, truncated)
        }
        Ok(None) => {
            if let Err(e) = fs::write(&memory_path, b"") {
                warnings.push(format!("{} 생성 실패: {e}", memory_path.display()));
            } else if let Err(e) = set_private_permissions(&memory_path) {
                warnings.push(format!("{} 권한 설정 실패: {e}", memory_path.display()));
            }
            (String::new(), false)
        }
        Err(e) => {
            warnings.push(format!("{}: {e}", memory_path.display()));
            (String::new(), false)
        }
    };

    let mut text = String::new();
    if !memory_text.is_empty() {
        text.push_str("[MEMORY.md]\n");
        text.push_str(&memory_text);
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }

    for offset in [0i64, -1i64] {
        let date = date_for_offset(offset);
        let path = dir.join(format!("{date}.md"));
        match read_utf8(&path) {
            Ok(Some(content)) if !content.trim().is_empty() => {
                text.push_str(&format!("\n[{date}.md]\n"));
                text.push_str(&content);
                if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            Ok(_) => {}
            Err(e) => warnings.push(format!("{}: {e}", path.display())),
        }
    }

    LoadedMemory {
        text,
        truncated,
        warnings,
    }
}

fn read_utf8(path: &Path) -> Result<Option<String>, String> {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "UTF-8로 읽을 수 없어 건너뜁니다".to_string()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("읽기 실패: {e}")),
    }
}

fn apply_budget(content: &str) -> (String, bool) {
    let mut out = String::new();
    let mut lines = 0usize;

    for line in content.split_inclusive('\n') {
        if lines >= MAX_MEMORY_LINES || out.len() + line.len() > MAX_MEMORY_BYTES {
            return (out, true);
        }
        out.push_str(line);
        lines += 1;
    }

    (out, false)
}

pub fn save(text: &str) -> Result<String, String> {
    save_to(Path::new(MEMORY_DIR), text)
}

fn save_to(dir: &Path, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("저장할 내용이 비어 있습니다".to_string());
    }

    let input_chars = text.chars().count();
    if input_chars > MAX_ENTRY_CHARS {
        return Err(format!(
            "항목 1개는 최대 {MAX_ENTRY_CHARS}자까지 저장할 수 있습니다 (입력 {input_chars}자)"
        ));
    }

    let (redacted, redactions) = redact_credentials(text);
    let entry = redacted.replace('\n', " ").replace('\r', " ");
    if entry.chars().count() > MAX_ENTRY_CHARS {
        return Err(format!(
            "치환 후 항목이 최대 {MAX_ENTRY_CHARS}자를 넘습니다 ({}자)",
            entry.chars().count()
        ));
    }

    fs::create_dir_all(dir).map_err(|e| format!("기억 폴더 생성 실패: {e}"))?;
    let path = dir.join(MEMORY_FILE);
    let existing = match fs::read(&path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map_err(|_| "기존 MEMORY.md를 UTF-8로 읽을 수 없어 저장을 중단했습니다".to_string())?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("기존 MEMORY.md 읽기 실패: {e}")),
    };

    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("- ");
    content.push_str(&entry);
    content.push('\n');

    atomic_write(&path, content.as_bytes())?;

    if redactions == 0 {
        Ok(format!("{MEMORY_FILE}에 저장했습니다."))
    } else {
        Ok(format!(
            "{MEMORY_FILE}에 저장했습니다. 자격증명 {redactions}건을 «redacted …»로 치환했습니다."
        ))
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| "기억 파일 경로 오류".to_string())?;
    let tmp = dir.join(format!(".MEMORY.md.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);

    let write_result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        set_file_permissions(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(format!("임시 파일 쓰기 실패: {e}"));
    }

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("기억 파일 교체 실패: {e}"));
    }

    Ok(())
}

fn set_private_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn set_file_permissions(file: &fs::File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

fn redact_credentials(text: &str) -> (String, usize) {
    let (text, private_keys) = redact_private_keys(text);
    let (text, passwords) = redact_password_values(&text);
    let (text, sk_tokens) = redact_sk_tokens(&text);
    let (text, telegram_tokens) = redact_telegram_tokens(&text);
    (text, private_keys + passwords + sk_tokens + telegram_tokens)
}

fn redact_private_keys(text: &str) -> (String, usize) {
    let mut out = String::new();
    let mut rest = text;
    let mut count = 0usize;

    while let Some(begin) = rest.find("-----BEGIN ") {
        let after_begin = begin + "-----BEGIN ".len();
        let Some(private_key) = rest[after_begin..].find("PRIVATE KEY-----") else {
            break;
        };
        let begin_end = after_begin + private_key + "PRIVATE KEY-----".len();
        let Some(end) = rest[begin_end..].find("-----END ") else {
            break;
        };
        let end_start = begin_end + end;
        let after_end = end_start + "-----END ".len();
        let Some(end_key) = rest[after_end..].find("PRIVATE KEY-----") else {
            break;
        };
        let end_end = after_end + end_key + "PRIVATE KEY-----".len();

        out.push_str(&rest[..begin]);
        out.push_str(&marker(rest[begin..end_end].chars().count()));
        count += 1;
        rest = &rest[end_end..];
    }

    out.push_str(rest);
    (out, count)
}

fn redact_password_values(text: &str) -> (String, usize) {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut last = 0usize;
    let mut search = 0usize;
    let mut count = 0usize;

    while let Some(rel) = lower[search..].find("password") {
        let start = search + rel;
        let after_word = start + "password".len();
        let boundary_ok = start == 0 || {
            let prev = bytes[start - 1];
            !(prev.is_ascii_alphanumeric() || prev == b'_')
        };
        if !boundary_ok {
            search = after_word;
            continue;
        }

        let mut i = after_word;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i >= bytes.len() || (bytes[i] != b'=' && bytes[i] != b':') {
            search = after_word;
            continue;
        }
        i += 1;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }

        let value_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b','
            && bytes[i] != b';'
        {
            i += 1;
        }
        if i == value_start {
            search = value_start;
            continue;
        }

        out.push_str(&text[last..value_start]);
        out.push_str(&marker(text[value_start..i].chars().count()));
        count += 1;
        last = i;
        search = i;
    }

    out.push_str(&text[last..]);
    (out, count)
}

fn redact_sk_tokens(text: &str) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut last = 0usize;
    let mut search = 0usize;
    let mut count = 0usize;

    while let Some(rel) = text[search..].find("sk-") {
        let start = search + rel;
        let boundary_ok = start == 0 || {
            let prev = bytes[start - 1];
            !(prev.is_ascii_alphanumeric() || prev == b'_')
        };
        if !boundary_ok {
            search = start + "sk-".len();
            continue;
        }

        let mut end = start + "sk-".len();
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'-')
        {
            end += 1;
        }

        out.push_str(&text[last..start]);
        out.push_str(&marker(text[start..end].chars().count()));
        count += 1;
        last = end;
        search = end;
    }

    out.push_str(&text[last..]);
    (out, count)
}

fn redact_telegram_tokens(text: &str) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut last = 0usize;
    let mut i = 0usize;
    let mut count = 0usize;

    while i < bytes.len() {
        let boundary_ok = i == 0 || {
            let prev = bytes[i - 1];
            !(prev.is_ascii_alphanumeric() || prev == b'_')
        };
        if !bytes[i].is_ascii_digit() || !boundary_ok {
            i += 1;
            continue;
        }

        let digit_start = i;
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j - digit_start < 5 || j >= bytes.len() || bytes[j] != b':' {
            i = j;
            continue;
        }

        let token_start = j + 1;
        let mut k = token_start;
        while k < bytes.len()
            && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_' || bytes[k] == b'-')
        {
            k += 1;
        }
        let end_boundary_ok = k == bytes.len() || {
            let next = bytes[k];
            !(next.is_ascii_alphanumeric() || next == b'_')
        };
        if k - token_start < 20 || !end_boundary_ok {
            i = j;
            continue;
        }

        out.push_str(&text[last..digit_start]);
        out.push_str(&marker(text[digit_start..k].chars().count()));
        count += 1;
        last = k;
        i = k;
    }

    out.push_str(&text[last..]);
    (out, count)
}

fn marker(chars: usize) -> String {
    format!("«redacted {chars} chars»")
}

fn date_for_offset(offset_days: i64) -> String {
    let base = local_today_days().unwrap_or_else(utc_today_days);
    let (year, month, day) = civil_from_days(base + offset_days);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(unix)]
fn local_today_days() -> Option<i64> {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        if now == -1 {
            return None;
        }
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        Some(days_from_civil(
            tm.tm_year as i64 + 1900,
            (tm.tm_mon + 1) as u32,
            tm.tm_mday as u32,
        ))
    }
}

#[cfg(not(unix))]
fn local_today_days() -> Option<i64> {
    None
}

fn utc_today_days() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let adjusted_month = if month > 2 { month - 3 } else { month + 9 } as i64;
    let doy = (153 * adjusted_month + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
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
            "iskra-memory-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn redacts_sk_tokens() {
        let (text, count) = redact_credentials("내 키는 sk-test1234567890이야");
        assert_eq!(count, 1);
        assert!(!text.contains("sk-test1234567890"));
        assert!(text.contains("«redacted 17 chars»"));
    }

    #[test]
    fn redacts_password_values() {
        let (text, count) = redact_credentials("password=hunter2 그리고 password: swordfish");
        assert_eq!(count, 2);
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("swordfish"));
        assert!(text.contains("password=«redacted 7 chars»"));
        assert!(text.contains("password: «redacted 9 chars»"));
    }

    #[test]
    fn redacts_private_key_blocks() {
        let input = "앞\n-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----\n뒤";
        let (text, count) = redact_credentials(input);
        assert_eq!(count, 1);
        assert!(!text.contains("MIIE"));
        assert!(text.contains("앞"));
        assert!(text.contains("뒤"));
        assert!(text.contains("«redacted "));
    }

    #[test]
    fn redacts_telegram_tokens() {
        let token = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";
        let (text, count) = redact_credentials(&format!("토큰 {token}"));
        assert_eq!(count, 1);
        assert!(!text.contains(token));
        assert!(text.contains("«redacted "));
    }

    #[test]
    fn rejects_entries_over_limit() {
        let dir = temp_dir("limit");
        let long = "가".repeat(MAX_ENTRY_CHARS + 1);
        assert!(save_to(&dir, &long).is_err());
        assert!(!dir.join(MEMORY_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_redacted_entry_and_permissions() {
        let dir = temp_dir("save");
        let result = save_to(&dir, "내 키는 sk-test1234567890").unwrap();
        assert!(result.contains("치환"));

        let path = dir.join(MEMORY_FILE);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("- "));
        assert!(content.contains("«redacted "));
        assert!(!content.contains("sk-test1234567890"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn budget_limits_lines() {
        let content = (0..MAX_MEMORY_LINES + 5)
            .map(|i| format!("line {i}\n"))
            .collect::<String>();
        let (text, truncated) = apply_budget(&content);
        assert!(truncated);
        assert_eq!(text.lines().count(), MAX_MEMORY_LINES);
    }

    #[test]
    fn budget_limits_bytes() {
        let content = "x".repeat(MAX_MEMORY_BYTES + 10);
        let (text, truncated) = apply_budget(&content);
        assert!(truncated);
        assert!(text.len() <= MAX_MEMORY_BYTES);
    }

    #[test]
    fn loads_memory_and_daily_notes() {
        let dir = temp_dir("load");
        fs::write(dir.join(MEMORY_FILE), "- 사실\n").unwrap();
        let today = date_for_offset(0);
        fs::write(dir.join(format!("{today}.md")), "오늘 노트\n").unwrap();

        let loaded = load_from(&dir);
        assert!(loaded.text.contains("[MEMORY.md]"));
        assert!(loaded.text.contains("- 사실"));
        assert!(loaded.text.contains(&format!("[{today}.md]")));
        assert!(loaded.text.contains("오늘 노트"));
        assert!(loaded.warnings.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_invalid_utf8_memory() {
        let dir = temp_dir("bad-utf8");
        fs::write(dir.join(MEMORY_FILE), [0xff, 0xfe, 0x00]).unwrap();

        let loaded = load_from(&dir);
        assert!(loaded.text.is_empty());
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("UTF-8"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn civil_date_roundtrip() {
        for days in [-719_468i64, 0, 1, 20_000, 20_700] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }
}
