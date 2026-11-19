mod brain;
mod channel;
mod logging;
mod loop_;
mod memory;
mod prompt;
mod stickies;
mod tools;
mod usage;

use std::fs;
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();

    let config = match brain::Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            logging::append(&format!("config_error {e}"));
            eprintln!("Configuration error: {e}");
            std::process::exit(1);
        }
    };

    fs::create_dir_all("workspace")?;
    fs::create_dir_all("logs")?;

    let identity = prompt::load_identity();
    let memory = memory::load();
    for warning in memory.warnings.iter().chain(identity.warnings.iter()) {
        logging::append(&format!("context_warning {warning}"));
        println!("Notice: {warning}");
    }
    let system_prompt = prompt::assemble(&identity, &memory);
    let memory_status = {
        let mut status = if memory.truncated {
            "partially loaded".to_string()
        } else {
            "ok".to_string()
        };
        if !memory.warnings.is_empty() {
            status.push_str(&format!(", {} warning(s)", memory.warnings.len()));
        }
        status
    };

    let tools = tools::Tools::new()?;
    let brain = brain::Brain::new(config);

    if std::env::args().any(|arg| arg == "--cli") {
        println!("iskra — file-tool conversation loop. Type exit to quit.");
        run_cli(&brain, &tools, &system_prompt).await?;
    } else {
        channel::run(brain, tools, system_prompt, memory_status).await?;
    }

    Ok(())
}

async fn run_cli(
    brain: &brain::Brain,
    tools: &tools::Tools,
    system_prompt: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

        match loop_::run_turn(brain, tools, system_prompt, input).await {
            Ok(answer) => println!("{answer}"),
            Err(e) => eprintln!("Stopped: {e}"),
        }
    }

    Ok(())
}
