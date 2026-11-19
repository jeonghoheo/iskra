use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOG_PATH: &str = "logs/agent.log";

pub fn append(line: &str) {
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
