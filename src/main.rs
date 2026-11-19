mod brain;
mod logging;
mod loop_;
mod memory;
mod prompt;
mod tools;

use std::fs;
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let key = std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY가 .env에 없습니다");

    fs::create_dir_all("workspace")?;
    fs::create_dir_all("logs")?;

    let identity = prompt::load_identity();
    let memory = memory::load();
    for warning in memory.warnings.iter().chain(identity.warnings.iter()) {
        logging::append(&format!("context_warning {warning}"));
        println!("알림: {warning}");
    }
    let system_prompt = prompt::assemble(&identity, &memory);

    let tools = tools::Tools::new()?;
    let brain = brain::Brain::new(key);

    println!("iskra — 파일 도구 대화 루프. 종료하려면 exit 입력.");

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

        if let Err(e) = loop_::run_turn(&brain, &tools, &system_prompt, input).await {
            eprintln!("중단: {e}");
        }
    }

    Ok(())
}
