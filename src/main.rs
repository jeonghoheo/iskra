mod brain;
mod channel;
mod delivery;
mod headless;
mod health;
mod idempotency;
mod instance;
mod locale;
mod logging;
mod loop_;
mod memory;
mod pane_grid;
mod pane_host;
mod prompt;
mod pty;
mod registry;
mod route;
mod run;
mod run_context;
mod run_lock;
mod runtime;
mod session;
mod stickies;
mod supervisor;
mod tools;
mod usage;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast::error::RecvError;

use crate::locale::{Key, Locale};
use crate::run::{
    Core, RunState, RuntimeEvent, RuntimeEventKind, TERMINAL_POLL_INTERVAL, TerminalOutcome,
};
use crate::run_context::MAIN_AGENT_ID;
use crate::run_lock::DispatcherLock;
use crate::runtime::RuntimeRegistry;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    fs::create_dir_all("logs")?;

    // Registry validation comes first and stays read-only. The dispatcher lock
    // must then be held before any Agent runtime exists, because Session and
    // RunStore construction perform restart-recovery writes.
    let plan = match RuntimeRegistry::load_plan(Path::new("")) {
        Ok(plan) => plan,
        Err(e) => {
            logging::append(&format!("config_error {e}"));
            eprintln!("Configuration error: {e}");
            std::process::exit(1);
        }
    };
    let dispatcher = match DispatcherLock::acquire(Path::new(memory::MEMORY_DIR)) {
        Ok(dispatcher) => dispatcher,
        Err(e) => {
            logging::append(&format!("dispatcher_lock_error {e}"));
            eprintln!("Dispatcher error: {e}");
            std::process::exit(1);
        }
    };
    let registry = match RuntimeRegistry::from_plan_env(&plan, Path::new("")) {
        Ok(registry) => registry,
        Err(e) => {
            logging::append(&format!("config_error {e}"));
            eprintln!("Configuration error: {e}");
            std::process::exit(1);
        }
    };
    let memory = memory::load_from(&registry.main().paths.memory);
    let headless_profile = plan.headless.clone();
    let headless_memory = registry.main().paths.memory.clone();
    for warning in registry.warnings().iter().chain(memory.warnings.iter()) {
        logging::append(&format!("context_warning {warning}"));
        println!("Notice: {warning}");
    }

    let usage = usage::today();
    logging::append(&format!(
        "usage_today prompt={} completion={} total={}",
        usage.prompt, usage.completion, usage.total
    ));

    let core = match Core::open(registry, dispatcher) {
        Ok(core) => core,
        Err(e) => {
            logging::append(&format!("run_store_error {e}"));
            eprintln!("Run store error: {e}");
            std::process::exit(1);
        }
    };

    match core.queue_status(MAIN_AGENT_ID) {
        Ok(status) => logging::append(&format!(
            "run_queue agent=main active={} queued={} paused={} interrupted={}",
            status.active, status.queued, status.paused, status.interrupted
        )),
        Err(error) => logging::append(&format!("run_store_error {error}")),
    }

    // Optional headless/v1 surface. A configured surface that cannot start
    // (missing token, unavailable bind, second owner of the data directory)
    // exits instead of silently running without authentication.
    let _headless = match headless_profile {
        Some(profile) => match headless::start(Arc::clone(&core), &headless_memory, &profile).await
        {
            Ok(server) => {
                logging::append(&format!("headless_ready bind={}", server.local_addr()));
                Some(server)
            }
            Err(error) => {
                logging::append(&format!("headless_error {error}"));
                eprintln!("Headless error: {error}");
                std::process::exit(1);
            }
        },
        None => None,
    };

    if std::env::args().any(|arg| arg == "--cli") {
        println!("iskra — file-tool conversation loop. Type exit to quit.");
        run_cli(&core).await?;
    } else if let Err(e) = channel::run(core).await {
        eprintln!("Iskra stopped: {e}");
        std::process::exit(2);
    }

    Ok(())
}

async fn run_cli(core: &Arc<Core>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print!("> ");
        std::io::stdout().flush()?;

        let Some(line) = lines.next_line().await? else {
            break;
        };
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" {
            break;
        }

        if let Some((command, args)) = cli_command(input) {
            match command.as_str() {
                "/agents" => println!("{}", channel::agents_text(core, Locale::En)),
                "/status" => print_status(core, &args).await,
                "/result" => print_result(core, &args),
                "/stop" => stop_agent(core, &args),
                "/resume" => resume_agent(core, &args).await,
                "/help" => println!("{CLI_HELP}"),
                _ => println!("{}", locale::text(Locale::En, Key::UnknownCommand)),
            }
            continue;
        }

        let route = match route::select(input, core.registry()) {
            Ok(route) => route,
            Err(error) => {
                logging::append(&format!("cli_route_error err={error:?}"));
                println!("{}", channel::route_error_text(Locale::En, error));
                continue;
            }
        };

        let mut events = core.subscribe();
        let ticket = match core.submit(&route.agent_id, &route.input) {
            Ok(ticket) => ticket,
            Err(error) => {
                logging::append(&format!(
                    "cli_submit_error agent={} {error}",
                    route.agent_id
                ));
                println!("{}", channel::submission_error_text(Locale::En, &error));
                continue;
            }
        };

        if matches!(core.run_state(ticket.id), Ok(Some(RunState::Queued))) {
            let waiting = core
                .queue_status(&route.agent_id)
                .map(|status| status.queued.saturating_sub(1))
                .unwrap_or(0);
            let display = core
                .registry()
                .slot(&route.agent_id)
                .map(|slot| slot.display_name.clone())
                .unwrap_or_else(|| route.agent_id.clone());
            println!(
                "{}",
                locale::with_values(
                    Locale::En,
                    Key::QueueAccepted,
                    &[display, waiting.to_string()]
                )
            );
        }

        print_run_result(core, ticket.id, &mut events).await;
    }

    Ok(())
}

/// One terminal CLI result. Produced exactly once per observed Run.
#[derive(Debug, Eq, PartialEq)]
enum CliOutcome {
    Answer(String),
    Stopped(String),
}

enum CliRecovery {
    Pending,
    Terminal(CliOutcome),
    Unavailable,
}

/// Waits for a Run's terminal result, bounded by the durable-state poll
/// interval. RuntimeEvents are hints: a timeout, a lagged stream, or a closed
/// stream falls back to the stored Run/Session projection. The first terminal
/// observation ends the wait, so an event and a recovery cannot print twice.
async fn wait_for_run(
    core: &Core,
    run_id: u64,
    events: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>,
) -> CliOutcome {
    loop {
        match tokio::time::timeout(TERMINAL_POLL_INTERVAL, events.recv()).await {
            Ok(Ok(event)) if event.run_id == run_id => match event.kind {
                RuntimeEventKind::Started => {}
                RuntimeEventKind::Completed { answer } => return CliOutcome::Answer(answer),
                RuntimeEventKind::Failed { category, detail } => {
                    logging::append(&format!("cli_turn_error category={category:?}"));
                    return CliOutcome::Stopped(detail);
                }
                RuntimeEventKind::Cancelled => return CliOutcome::Stopped("cancelled".to_string()),
                RuntimeEventKind::StorageFailed => {
                    return CliOutcome::Stopped("the run store write failed".to_string());
                }
            },
            Ok(Ok(_)) => {}
            Ok(Err(RecvError::Lagged(skipped))) => {
                logging::append(&format!("cli_event_lagged skipped={skipped}"));
                match recover_run(core, run_id) {
                    CliRecovery::Pending => {}
                    CliRecovery::Terminal(outcome) => return outcome,
                    CliRecovery::Unavailable => {
                        return CliOutcome::Stopped(
                            "the run result could not be recovered".to_string(),
                        );
                    }
                }
            }
            Ok(Err(RecvError::Closed)) => {
                logging::append(&format!("cli_event_closed run_id={run_id}"));
                return match recover_run(core, run_id) {
                    CliRecovery::Terminal(outcome) => outcome,
                    CliRecovery::Unavailable => {
                        CliOutcome::Stopped("the run result could not be recovered".to_string())
                    }
                    CliRecovery::Pending => {
                        CliOutcome::Stopped("the event stream closed".to_string())
                    }
                };
            }
            Err(_elapsed) => match recover_run(core, run_id) {
                CliRecovery::Pending => {}
                CliRecovery::Terminal(outcome) => return outcome,
                CliRecovery::Unavailable => {
                    return CliOutcome::Stopped(
                        "the run result could not be recovered".to_string(),
                    );
                }
            },
        }
    }
}

/// Reads a terminal outcome from the durable Run and Session records without a
/// model or tool call.
fn recover_run(core: &Core, run_id: u64) -> CliRecovery {
    let outcome = match core.terminal_outcome(run_id) {
        Ok(Some(outcome)) => outcome,
        Ok(None) => return CliRecovery::Pending,
        Err(error) => {
            logging::append(&format!("cli_recovery_error run_id={run_id} {error}"));
            return CliRecovery::Unavailable;
        }
    };
    let outcome = match outcome {
        TerminalOutcome::Completed {
            answer: Some(answer),
        } => CliOutcome::Answer(answer),
        TerminalOutcome::Completed { answer: None } => {
            logging::append(&format!("cli_recovery_answer_missing run_id={run_id}"));
            CliOutcome::Stopped("the stored answer is unavailable".to_string())
        }
        TerminalOutcome::Failed { category } => {
            CliOutcome::Stopped(channel::failure_text(Locale::En, category, ""))
        }
        TerminalOutcome::Cancelled => CliOutcome::Stopped("cancelled".to_string()),
        TerminalOutcome::Interrupted => {
            CliOutcome::Stopped("interrupted; it will not resume automatically".to_string())
        }
    };
    CliRecovery::Terminal(outcome)
}

async fn print_run_result(
    core: &Core,
    run_id: u64,
    events: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>,
) {
    match wait_for_run(core, run_id, events).await {
        CliOutcome::Answer(answer) => println!("{answer}"),
        CliOutcome::Stopped(text) => eprintln!("Stopped: {text}"),
    }
}

const CLI_HELP: &str = "Commands: /agents · /status [@agent] · /result <work number> · /stop [@agent] · /resume [@agent] · /help\nStart a request with @name to reach another agent.";

fn cli_command(text: &str) -> Option<(String, String)> {
    let (first, rest) = match text.split_once(char::is_whitespace) {
        Some((first, rest)) => (first, rest),
        None => (text, ""),
    };
    if !first.starts_with('/') {
        return None;
    }
    Some((
        first.split('@').next().unwrap_or(first).to_string(),
        rest.trim().to_string(),
    ))
}

fn cli_target(core: &Core, args: &str) -> Result<String, String> {
    route::target(args, core.registry())
        .map_err(|error| channel::route_error_text(Locale::En, error))
}

async fn print_status(core: &Arc<Core>, args: &str) {
    let agent_id = match cli_target(core, args) {
        Ok(agent_id) => agent_id,
        Err(text) => {
            println!("{text}");
            return;
        }
    };
    match channel::agent_status_text(core, Locale::En, &agent_id).await {
        Some(text) => println!("{text}"),
        None => println!(
            "{}",
            locale::with(Locale::En, Key::RouteUnknownAgent, agent_id)
        ),
    }
}

/// Prints a completed Run's stored redacted answer. Reads Run and Session only:
/// no Run, model, or tool call is made, and nothing is written.
fn print_result(core: &Arc<Core>, args: &str) {
    let Some(run_id) = channel::parse_run_id(args) else {
        println!("{}", locale::text(Locale::En, Key::ResultUsage));
        return;
    };

    let target = match core.run_target(run_id) {
        Ok(Some(target)) => target,
        Ok(None) => {
            logging::append(&format!("cli_result run_id={run_id} state=unknown"));
            println!(
                "{}",
                locale::with(Locale::En, Key::ResultUnknownRun, run_id)
            );
            return;
        }
        Err(error) => {
            logging::append(&format!("cli_result_error run_id={run_id} error={error}"));
            println!("{}", locale::text(Locale::En, Key::ResultUnavailable));
            return;
        }
    };

    if target.state != RunState::Completed {
        logging::append(&format!(
            "cli_result run_id={run_id} agent={} state={:?}",
            target.agent_id, target.state
        ));
        println!(
            "{}",
            locale::with(Locale::En, Key::ResultNotCompleted, run_id)
        );
        return;
    }

    match core.completed_answer(run_id) {
        Ok(Some(answer)) => {
            logging::append(&format!(
                "cli_result run_id={run_id} agent={} state=completed",
                target.agent_id
            ));
            println!("{answer}");
        }
        Ok(None) => {
            logging::append(&format!(
                "cli_result run_id={run_id} agent={} state=answer_missing",
                target.agent_id
            ));
            println!(
                "{}",
                locale::with(Locale::En, Key::ResultAnswerMissing, run_id)
            );
        }
        Err(error) => {
            logging::append(&format!("cli_result_error run_id={run_id} error={error}"));
            println!("{}", locale::text(Locale::En, Key::ResultUnavailable));
        }
    }
}

fn stop_agent(core: &Arc<Core>, args: &str) {
    let agent_id = match cli_target(core, args) {
        Ok(agent_id) => agent_id,
        Err(text) => {
            println!("{text}");
            return;
        }
    };
    match core.cancel_agent(&agent_id) {
        Ok(report) if report.cancelled == 0 => {
            println!("{}", locale::text(Locale::En, Key::StopNoActive));
        }
        Ok(_) => println!("{}", locale::text(Locale::En, Key::StopStopped)),
        Err(error) => eprintln!("Stopped: {error}"),
    }
}

async fn resume_agent(core: &Arc<Core>, args: &str) {
    let agent_id = match cli_target(core, args) {
        Ok(agent_id) => agent_id,
        Err(text) => {
            println!("{text}");
            return;
        }
    };
    let display = core
        .registry()
        .slot(&agent_id)
        .map(|slot| slot.display_name.clone())
        .unwrap_or_else(|| agent_id.clone());
    let mut events = core.subscribe();
    match core.resume_agent(&agent_id) {
        Ok(resumed) if resumed.is_empty() => {
            println!("{}", locale::with(Locale::En, Key::ResumeNone, display));
        }
        Ok(resumed) => {
            println!(
                "{}",
                locale::with_values(
                    Locale::En,
                    Key::ResumeDone,
                    &[resumed.len().to_string(), display]
                )
            );
            for ticket in resumed {
                print_run_result(core, ticket.id, &mut events).await;
            }
        }
        Err(error) => eprintln!("Stopped: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeRegistry;
    use crate::session::Session;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iskra-main-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn core_for(dir: &Path) -> Arc<Core> {
        let registry = RuntimeRegistry::load(dir, |key| match key {
            "LLM_API_KEY" => Some("dummy-main-key".to_string()),
            "LLM_BASE_URL" => Some("http://127.0.0.1:9/chat/completions".to_string()),
            "LLM_MODEL" => Some("main-test-model".to_string()),
            _ => None,
        })
        .unwrap();
        Core::new(registry).unwrap()
    }

    fn seed_completed_run(dir: &Path, answer: &str) {
        fs::create_dir_all(dir.join("memory")).unwrap();
        let session = Session::load(&dir.join("memory/session.json"));
        let mut work = session.begin("seeded request").unwrap();
        let id = work.id();
        work.complete(answer).unwrap();
        fs::write(
            dir.join("memory/runs.json"),
            serde_json::json!({
                "version": 1, "next_run_id": 2,
                "runs": [{
                    "id": 1, "agent_id": "main", "input": "seeded request",
                    "state": "completed", "session_work_id": id, "revision": 1
                }]
            })
            .to_string(),
        )
        .unwrap();
    }

    fn started_event(run_id: u64, seq: u64) -> RuntimeEvent {
        RuntimeEvent {
            run_id,
            agent_id: "main".to_string(),
            revision: 1,
            seq,
            kind: RuntimeEventKind::Started,
        }
    }

    #[tokio::test]
    async fn cli_wait_recovers_a_missing_terminal_event_from_durable_state() {
        let dir = temp_dir("cli-recovery");
        seed_completed_run(&dir, "stored answer");
        let core = core_for(&dir);
        let (_sender, mut events) = tokio::sync::broadcast::channel(4);

        assert_eq!(
            wait_for_run(&core, 1, &mut events).await,
            CliOutcome::Answer("stored answer".to_string())
        );

        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cli_wait_keeps_the_first_terminal_event_and_ignores_duplicates() {
        let dir = temp_dir("cli-duplicates");
        let core = core_for(&dir);
        let (sender, mut events) = tokio::sync::broadcast::channel(4);
        for (seq, answer) in [(1, "first"), (2, "second")] {
            sender
                .send(RuntimeEvent {
                    run_id: 3,
                    agent_id: "main".to_string(),
                    revision: 1,
                    seq,
                    kind: RuntimeEventKind::Completed {
                        answer: answer.to_string(),
                    },
                })
                .unwrap();
        }

        assert_eq!(
            wait_for_run(&core, 3, &mut events).await,
            CliOutcome::Answer("first".to_string())
        );

        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cli_wait_recovers_after_a_lagged_stream() {
        let dir = temp_dir("cli-lagged");
        seed_completed_run(&dir, "answer after lag");
        let core = core_for(&dir);
        let (sender, mut events) = tokio::sync::broadcast::channel(1);
        for seq in 1..=3 {
            sender.send(started_event(99, seq)).unwrap();
        }

        assert_eq!(
            wait_for_run(&core, 1, &mut events).await,
            CliOutcome::Answer("answer after lag".to_string())
        );

        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cli_wait_after_a_closed_stream_uses_the_durable_state() {
        let dir = temp_dir("cli-closed");
        seed_completed_run(&dir, "answer after close");
        let core = core_for(&dir);
        let (sender, mut events) = tokio::sync::broadcast::channel(4);
        drop(sender);

        assert_eq!(
            wait_for_run(&core, 1, &mut events).await,
            CliOutcome::Answer("answer after close".to_string())
        );

        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cli_wait_on_a_closed_stream_without_a_terminal_run_stops_once() {
        let dir = temp_dir("cli-closed-pending");
        let core = core_for(&dir);
        let (sender, mut events) = tokio::sync::broadcast::channel(4);
        drop(sender);

        assert_eq!(
            wait_for_run(&core, 1, &mut events).await,
            CliOutcome::Stopped("the event stream closed".to_string())
        );

        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }
}
