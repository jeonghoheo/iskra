mod brain;
mod channel;
mod instance;
mod locale;
mod logging;
mod loop_;
mod memory;
mod prompt;
mod run_context;
mod session;
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

    let paths = instance::InstancePaths::default();
    fs::create_dir_all(&paths.workspace)?;
    fs::create_dir_all("logs")?;

    let identity = prompt::load_identity();
    let memory = memory::load_from(&paths.memory);
    for warning in memory.warnings.iter().chain(identity.warnings.iter()) {
        logging::append(&format!("context_warning {warning}"));
        println!("Notice: {warning}");
    }

    let tools = tools::Tools::new_in(&paths.workspace, &paths.memory)?;
    let brain = brain::Brain::new(config);
    let session = session::Session::load(&paths.session);

    if std::env::args().any(|arg| arg == "--cli") {
        println!("iskra — file-tool conversation loop. Type exit to quit.");
        run_cli(&brain, &tools, &identity, &session, &paths).await?;
    } else {
        channel::run(brain, tools, identity, session, paths).await?;
    }

    Ok(())
}

async fn run_cli(
    brain: &brain::Brain,
    tools: &tools::Tools,
    identity: &prompt::Identity,
    session: &session::Session,
    paths: &instance::InstancePaths,
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

        let (system_prompt, _) = prompt::turn_prompt(identity, &paths.memory);
        match loop_::run_turn(brain, tools, session, &system_prompt, input).await {
            Ok(answer) => println!("{answer}"),
            Err(e) => eprintln!("Stopped: {e}"),
        }
    }

    Ok(())
}
