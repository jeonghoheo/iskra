//! CLI-only pane control surface over the Core-owned pane host.
//!
//! `/panes` and `/pane …` are parsed here, executed against
//! [`crate::run::Core::panes`], and rendered as plain text for the `--cli`
//! loop. A pane command creates no Run, makes no model or tool call, and has
//! no Telegram entry point. Text after `/pane send <id>` is written to the PTY
//! as raw bytes plus a carriage return; Iskra never interprets it as a shell
//! string.
//!
//! Output is pumped only by a pane command. The host starts no pane at
//! `--cli` startup and runs no background polling.

use crate::pane_host::{MAX_DIMENSION, PaneError, PaneSlot, PaneState};
use crate::run::Core;

/// Usage line for the list command.
pub const PANES_USAGE: &str = "Usage: /panes";
/// Usage line shared by every malformed `/pane` invocation.
pub const PANE_USAGE: &str = "Usage: /pane start [rows cols] | stop <id> | resize <id> <rows> <cols> | send <id> <text> | text <id> [scrollback rows]";

/// One parsed pane command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaneCommand {
    List,
    Start { rows: u16, cols: u16 },
    Stop { id: u64 },
    Resize { id: u64, rows: u16, cols: u16 },
    Send { id: u64, text: String },
    Text { id: u64, scrollback: Option<usize> },
}

/// Parses the arguments after `/pane`. Every structural error is one English
/// line; no host operation happens until parsing succeeds.
pub fn parse(args: &str) -> Result<PaneCommand, String> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let Some(sub) = tokens.first().copied() else {
        return Err(PANE_USAGE.to_string());
    };

    match sub {
        "start" => match &tokens[1..] {
            [] => Ok(PaneCommand::Start {
                rows: crate::pane_host::DEFAULT_ROWS,
                cols: crate::pane_host::DEFAULT_COLS,
            }),
            [rows, cols] => Ok(PaneCommand::Start {
                rows: parse_dimension(rows)?,
                cols: parse_dimension(cols)?,
            }),
            _ => Err(PANE_USAGE.to_string()),
        },
        "stop" => match &tokens[1..] {
            [id] => Ok(PaneCommand::Stop { id: parse_id(id)? }),
            _ => Err(PANE_USAGE.to_string()),
        },
        "resize" => match &tokens[1..] {
            [id, rows, cols] => Ok(PaneCommand::Resize {
                id: parse_id(id)?,
                rows: parse_dimension(rows)?,
                cols: parse_dimension(cols)?,
            }),
            _ => Err(PANE_USAGE.to_string()),
        },
        "send" => {
            // The remainder is raw PTY input, so only the id is tokenized.
            let rest = args
                .trim_start()
                .strip_prefix("send")
                .unwrap_or_default()
                .trim_start();
            let Some((id, text)) = rest.split_once(char::is_whitespace) else {
                return Err(PANE_USAGE.to_string());
            };
            let text = text.trim_start();
            if text.is_empty() {
                return Err(PANE_USAGE.to_string());
            }
            Ok(PaneCommand::Send {
                id: parse_id(id)?,
                text: text.to_string(),
            })
        }
        "text" => match &tokens[1..] {
            [id] => Ok(PaneCommand::Text {
                id: parse_id(id)?,
                scrollback: None,
            }),
            [id, rows] => Ok(PaneCommand::Text {
                id: parse_id(id)?,
                scrollback: Some(parse_scrollback(rows)?),
            }),
            _ => Err(PANE_USAGE.to_string()),
        },
        other => Err(format!("Unknown pane command: {other}. {PANE_USAGE}")),
    }
}

fn parse_id(value: &str) -> Result<u64, String> {
    let id: u64 = value
        .parse()
        .map_err(|_| format!("Invalid pane id: {value}"))?;
    if id == 0 {
        return Err("Invalid pane id: 0".to_string());
    }
    Ok(id)
}

fn parse_dimension(value: &str) -> Result<u16, String> {
    let size: u16 = value
        .parse()
        .map_err(|_| format!("Invalid pane size: {value}"))?;
    if size == 0 || size > MAX_DIMENSION {
        return Err(format!(
            "Invalid pane size {value}: rows and columns must be 1..={MAX_DIMENSION}"
        ));
    }
    Ok(size)
}

fn parse_scrollback(value: &str) -> Result<usize, String> {
    let rows: usize = value
        .parse()
        .map_err(|_| format!("Invalid scrollback rows: {value}"))?;
    if rows == 0 {
        return Err("Invalid scrollback rows: 0".to_string());
    }
    Ok(rows)
}

/// Executes one parsed command. The result is what the CLI prints, verbatim.
pub fn execute(core: &Core, command: PaneCommand) -> String {
    let panes = core.panes();
    match command {
        PaneCommand::List => match panes.slots() {
            Ok(slots) => list(&slots),
            Err(error) => error_line(error),
        },
        PaneCommand::Start { rows, cols } => match panes.start_sized(rows, cols) {
            Ok(id) => match panes.slot(id) {
                Ok(slot) => format!("Pane {}", render_slot(slot)),
                Err(error) => format!("Pane {id} started but is unavailable: {error}"),
            },
            Err(error) => error_line(error),
        },
        PaneCommand::Stop { id } => match panes.stop(id) {
            Ok(()) => match panes.slot(id) {
                Ok(slot) => format!("Pane {} {}", slot.id, state_name(slot.state)),
                Err(_) => format!("Pane {id} stopped"),
            },
            Err(PaneError::StopTimeout) => {
                format!("Pane {id} interrupted (teardown timed out)")
            }
            Err(error) => match panes.slot(id) {
                Ok(slot) => format!("Pane {} {}: {error}", slot.id, state_name(slot.state)),
                Err(_) => error_line(error),
            },
        },
        PaneCommand::Resize { id, rows, cols } => match panes.resize(id, rows, cols) {
            Ok(()) => match panes.slot(id) {
                Ok(slot) => format!("Pane {} resized to {}x{}", slot.id, slot.rows, slot.cols),
                Err(error) => format!("Pane {id} resized but is unavailable: {error}"),
            },
            Err(error) => error_line(error),
        },
        PaneCommand::Send { id, text } => match panes.slot(id) {
            Ok(slot) if slot.state == PaneState::Live => {
                let mut bytes = text.into_bytes();
                bytes.push(b'\r');
                match panes.write(id, &bytes) {
                    Ok(written) => format!("Sent {written} bytes to pane {id}."),
                    Err(error) => error_line(error),
                }
            }
            Ok(slot) => format!("Pane {id} is not live (state {}).", state_name(slot.state)),
            Err(error) => error_line(error),
        },
        PaneCommand::Text { id, scrollback } => match panes.snapshot(id) {
            Ok(snapshot) => {
                let text = match scrollback {
                    Some(rows) => {
                        let limit = rows.min(snapshot.scrollback_capacity);
                        if limit == 0 {
                            snapshot.text()
                        } else {
                            match panes.scrollback(id, limit) {
                                Ok(lines) => lines.join("\n"),
                                Err(error) => return error_line(error),
                            }
                        }
                    }
                    None => snapshot.text(),
                };
                let text = text.trim_end();
                if text.is_empty() {
                    format!("Pane {id} screen is empty.")
                } else {
                    text.to_string()
                }
            }
            Err(PaneError::UnknownPane(_)) => match panes.slot(id) {
                Ok(slot) => format!(
                    "Pane {id} screen is unavailable (state {}).",
                    state_name(slot.state)
                ),
                Err(error) => error_line(error),
            },
            Err(error) => error_line(error),
        },
    }
}

/// Read-only slot list. An empty host prints one line and creates nothing.
pub fn list(slots: &[PaneSlot]) -> String {
    if slots.is_empty() {
        return "No panes.".to_string();
    }
    slots
        .iter()
        .map(|slot| render_slot(*slot))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_slot(slot: PaneSlot) -> String {
    match slot.exit_code {
        Some(code) => format!(
            "{} {} {}x{} exit={code}",
            slot.id,
            state_name(slot.state),
            slot.rows,
            slot.cols
        ),
        None => format!(
            "{} {} {}x{}",
            slot.id,
            state_name(slot.state),
            slot.rows,
            slot.cols
        ),
    }
}

fn state_name(state: PaneState) -> &'static str {
    match state {
        PaneState::Spawning => "spawning",
        PaneState::Live => "live",
        PaneState::Interrupted => "interrupted",
        PaneState::Finished => "finished",
    }
}

fn error_line(error: PaneError) -> String {
    format!("Pane error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_start_defaults_and_explicit_size() {
        assert_eq!(
            parse("start"),
            Ok(PaneCommand::Start { rows: 24, cols: 80 })
        );
        assert_eq!(
            parse("start 30 100"),
            Ok(PaneCommand::Start {
                rows: 30,
                cols: 100
            })
        );
    }

    #[test]
    fn parse_rejects_out_of_range_and_zero_dimensions() {
        for args in ["start 0 80", "start 80 0", "start 201 80", "start 80 201"] {
            let error = parse(args).expect_err(args);
            assert!(error.contains("1..=200"), "{args}: {error}");
        }
        for args in ["resize 1 0 80", "resize 1 400 2"] {
            assert!(parse(args).is_err(), "{args}");
        }
    }

    #[test]
    fn parse_stop_resize_and_ids() {
        assert_eq!(parse("stop 2"), Ok(PaneCommand::Stop { id: 2 }));
        assert_eq!(parse("stop 0").unwrap_err(), "Invalid pane id: 0");
        assert_eq!(
            parse("resize 1 30 100"),
            Ok(PaneCommand::Resize {
                id: 1,
                rows: 30,
                cols: 100
            })
        );
        assert!(parse("resize x 30 100").is_err());
    }

    #[test]
    fn parse_send_keeps_the_raw_remainder() {
        assert_eq!(
            parse("send 1 printf 'SURFACE_OK\\n'"),
            Ok(PaneCommand::Send {
                id: 1,
                text: "printf 'SURFACE_OK\\n'".to_string()
            })
        );
        assert_eq!(
            parse("send 4 stty   size"),
            Ok(PaneCommand::Send {
                id: 4,
                text: "stty   size".to_string()
            })
        );
        assert!(parse("send 1").is_err());
        assert!(parse("send 1   ").is_err());
    }

    #[test]
    fn parse_text_accepts_an_optional_scrollback() {
        assert_eq!(
            parse("text 3"),
            Ok(PaneCommand::Text {
                id: 3,
                scrollback: None
            })
        );
        assert_eq!(
            parse("text 3 50"),
            Ok(PaneCommand::Text {
                id: 3,
                scrollback: Some(50)
            })
        );
        assert!(parse("text 3 0").is_err());
        assert!(parse("text").is_err());
    }

    #[test]
    fn parse_rejects_malformed_and_unknown_commands_with_one_line() {
        for args in [
            "",
            "start 24",
            "start 24 80 1",
            "stop",
            "stop 1 2",
            "resize 1 2",
        ] {
            let error = parse(args).expect_err(args);
            assert!(!error.contains('\n'), "{args}: {error}");
            assert_eq!(error, PANE_USAGE, "{args}");
        }
        let error = parse("bogus").unwrap_err();
        assert!(error.starts_with("Unknown pane command: bogus."), "{error}");
        assert!(!error.contains('\n'));
    }

    #[test]
    fn empty_list_is_one_notice_and_slots_render_state_and_exit() {
        assert_eq!(list(&[]), "No panes.");

        let live = PaneSlot {
            id: 1,
            state: PaneState::Live,
            rows: 24,
            cols: 80,
            exit_code: None,
        };
        let finished = PaneSlot {
            id: 2,
            state: PaneState::Finished,
            rows: 30,
            cols: 100,
            exit_code: Some(7),
        };
        assert_eq!(list(&[live]), "1 live 24x80");
        assert_eq!(
            list(&[live, finished]),
            "1 live 24x80\n2 finished 30x100 exit=7"
        );
    }

    #[test]
    fn state_names_cover_every_pane_state() {
        assert_eq!(state_name(PaneState::Spawning), "spawning");
        assert_eq!(state_name(PaneState::Live), "live");
        assert_eq!(state_name(PaneState::Interrupted), "interrupted");
        assert_eq!(state_name(PaneState::Finished), "finished");
    }
}
