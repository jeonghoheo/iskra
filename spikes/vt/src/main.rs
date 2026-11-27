//! VT spike harness for the terminal-ownership gate (SESSION.md 2026-11-27).
//!
//! Usage:
//!   vt-spike                 run G0–G8 and print the gate report
//!   vt-spike --json <path>   also write the report as JSON
//!   vt-spike --probe N SECS  hold N live bash panes for SECS (resource probe)

mod gates;
mod grid;
mod pty;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(|s| s.as_str()) == Some("--probe") {
        let panes = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
        let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
        gates::probe(panes, secs);
        return;
    }

    let json_path = if args.first().map(|s| s.as_str()) == Some("--json") {
        args.get(1).cloned()
    } else {
        None
    };
    let only = args
        .iter()
        .position(|a| a == "--only")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    let report = gates::run_all(only);
    if let Some(path) = json_path {
        let text = report.to_json();
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("could not write {path}: {e}");
        }
    }
    report.print();
    std::process::exit(if report.ok() { 0 } else { 1 });
}
