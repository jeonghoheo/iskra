mod brain;
mod channel;
mod health;
mod instance;
mod locale;
mod logging;
mod loop_;
mod memory;
mod prompt;
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
use crate::run::{Core, RunState, RuntimeEvent, RuntimeEventKind};
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

        print_run_result(&mut events, ticket.id).await;
    }

    Ok(())
}

async fn print_run_result(
    events: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>,
    run_id: u64,
) {
    loop {
        match events.recv().await {
            Ok(event) if event.run_id == run_id => match event.kind {
                RuntimeEventKind::Started => {}
                RuntimeEventKind::Completed { answer } => {
                    println!("{answer}");
                    break;
                }
                RuntimeEventKind::Failed { category, detail } => {
                    logging::append(&format!("cli_turn_error category={category:?}"));
                    eprintln!("Stopped: {detail}");
                    break;
                }
                RuntimeEventKind::Cancelled => {
                    eprintln!("Stopped: cancelled");
                    break;
                }
                RuntimeEventKind::StorageFailed => {
                    eprintln!("Stopped: run store write failed");
                    break;
                }
            },
            Ok(_) => {}
            Err(RecvError::Lagged(skipped)) => {
                logging::append(&format!("cli_event_lagged skipped={skipped}"));
            }
            Err(RecvError::Closed) => {
                eprintln!("Stopped: event stream closed");
                break;
            }
        }
    }
}

const CLI_HELP: &str = "Commands: /agents · /status [@agent] · /stop [@agent] · /resume [@agent] · /help\nStart a request with @name to reach another agent.";

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
                print_run_result(&mut events, ticket.id).await;
            }
        }
        Err(error) => eprintln!("Stopped: {error}"),
    }
}
