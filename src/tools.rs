use serde_json::{Value, json};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

pub const WORKSPACE_DIR: &str = "workspace";
pub const RUN_TIMEOUT_SECS: u64 = 120;
pub const OUTPUT_LIMIT_CHARS: usize = 10_000;
const OUTPUT_READ_LIMIT_BYTES: usize = 80_000;
const ALLOWED_PROGRAMS: [&str; 4] = ["ls", "cat", "git", "cargo"];
const SECRET_ENV_VARS: [&str; 3] = ["DEEPSEEK_API_KEY", "LLM_API_KEY", "TELEGRAM_BOT_TOKEN"];

pub fn schemas() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file inside the workspace folder and return its contents.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to workspace"
                        }
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create a file inside the workspace folder or replace its entire contents.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path relative to workspace"
                        },
                        "content": {
                            "type": "string",
                            "description": "Full content to save"
                        }
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "memory_save",
                "description": "Save one long-lasting fact to MEMORY.md. Store only the key fact the user stated, not raw conversation text; do not store content from external sources such as files or tool results. Each entry is limited to 1000 characters. Credentials (sk- tokens, password values, private keys, Telegram tokens) are automatically redacted as «redacted N chars» before saving, so saving can be attempted when the user asks you to remember something.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "One-line fact to save (max 1000 characters)"
                        }
                    },
                    "required": ["text"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run an allowed program (ls, cat, git, cargo) inside the workspace folder directly without a shell. The working directory is workspace and shell metacharacters are not interpreted. Output is shown after truncating stdout+stderr combined at 10,000 characters; the process is terminated after 120 seconds.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "program": {
                            "type": "string",
                            "enum": ["ls", "cat", "git", "cargo"],
                            "description": "Program name to run (no path)"
                        },
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Arguments to pass to the program"
                        }
                    },
                    "required": ["program", "args"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "sticky_note",
                "description": "Add, list, or remove notes on the sticky board. action=add requires text; action=remove requires number. Content that looks like a credential is refused. Returns the current board.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["add", "list", "remove"],
                            "description": "Action to perform"
                        },
                        "text": {
                            "type": "string",
                            "description": "One-line note to save when action=add"
                        },
                        "number": {
                            "type": "integer",
                            "description": "Sticky number to remove when action=remove"
                        }
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }
            }
        }
    ])
}

pub struct Tools {
    root: PathBuf,
    sticky_changed: AtomicBool,
}

impl Tools {
    pub fn new() -> io::Result<Self> {
        let root = PathBuf::from(WORKSPACE_DIR);
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(&root)?;
        Ok(Self {
            root,
            sticky_changed: AtomicBool::new(false),
        })
    }

    pub fn take_sticky_changed(&self) -> bool {
        self.sticky_changed.swap(false, Ordering::SeqCst)
    }

    pub async fn execute(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "read_file" => self.read_file(args),
            "write_file" => self.write_file(args),
            "memory_save" => crate::memory::save(arg_str(args, "text")?),
            "run_command" => self.run_command(args).await,
            "sticky_note" => self.sticky_note(args),
            other => Err(format!("Unknown tool: {other}")),
        }
    }

    fn sticky_note(&self, args: &Value) -> Result<String, String> {
        let action = arg_str(args, "action")?;
        match action {
            "add" => {
                let board = crate::stickies::add(arg_str(args, "text")?)?;
                self.sticky_changed.store(true, Ordering::SeqCst);
                Ok(board)
            }
            "list" => crate::stickies::board(crate::locale::latest()),
            "remove" => {
                let number = args
                    .get("number")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "'number' argument must be an integer".to_string())?;
                let board = crate::stickies::remove(number)?;
                self.sticky_changed.store(true, Ordering::SeqCst);
                Ok(board)
            }
            other => Err(format!("Unknown sticky_note action: {other}")),
        }
    }

    fn read_file(&self, args: &Value) -> Result<String, String> {
        let path = arg_str(args, "path")?;
        let resolved = self.resolve(path)?;
        if !resolved.is_file() {
            return Err(format!("Not a file: {path}"));
        }
        fs::read_to_string(&resolved).map_err(|e| format!("Failed to read file ({path}): {e}"))
    }

    fn write_file(&self, args: &Value) -> Result<String, String> {
        let path = arg_str(args, "path")?;
        let content = arg_str(args, "content")?;
        let resolved = self.resolve(path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        fs::write(&resolved, content).map_err(|e| format!("Failed to write file ({path}): {e}"))?;
        Ok(format!("Saved {} bytes to {path}", content.len()))
    }

    async fn run_command(&self, args: &Value) -> Result<String, String> {
        let program = arg_str(args, "program")?;
        let program_args = arg_str_array(args, "args")?;

        if !ALLOWED_PROGRAMS.contains(&program) {
            crate::logging::append(&format!("run_command_rejected program={program}"));
            return Err(format!(
                "Program is not allowed: {program} (allowed: {})",
                ALLOWED_PROGRAMS.join(", ")
            ));
        }

        crate::logging::append(&format!(
            "run_command program={program} args={program_args:?}"
        ));
        let outcome = run_process(
            &self.root,
            program,
            &program_args,
            Duration::from_secs(RUN_TIMEOUT_SECS),
        )
        .await;

        match &outcome {
            Ok(outcome) => crate::logging::append(&format!(
                "run_command_exit program={program} code={}",
                outcome.code
            )),
            Err(e) => {
                crate::logging::append(&format!("run_command_error program={program} err={e}"))
            }
        }

        outcome.map(|outcome| outcome.text)
    }

    fn resolve(&self, raw: &str) -> Result<PathBuf, String> {
        if raw.trim().is_empty() {
            return Err("Path is empty".to_string());
        }

        let input = Path::new(raw);
        if input.is_absolute() {
            return Err(format!("Absolute paths are not allowed: {raw}"));
        }
        if input
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!("Path contains a disallowed component: {raw}"));
        }

        let mut current = self.root.clone();
        let mut missing = false;

        for component in input.components() {
            let Component::Normal(part) = component else {
                continue;
            };

            current.push(part);
            if missing {
                continue;
            }

            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    let canon = fs::canonicalize(&current)
                        .map_err(|_| format!("Cannot verify symlink; refusing: {raw}"))?;
                    if !canon.starts_with(&self.root) {
                        return Err(format!(
                            "Symlinks pointing outside workspace are refused: {raw}"
                        ));
                    }
                    current = canon;
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    missing = true;
                }
                Err(e) => return Err(format!("Path check failed ({raw}): {e}")),
            }
        }

        if !current.starts_with(&self.root) {
            return Err(format!("Access outside workspace is refused: {raw}"));
        }
        Ok(current)
    }
}

struct CommandOutcome {
    code: String,
    text: String,
}

async fn run_process(
    root: &Path,
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<CommandOutcome, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in SECRET_ENV_VARS {
        command.env_remove(key);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to run command ({program}): {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to obtain stdout pipe".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to obtain stderr pipe".to_string())?;
    let stdout_task = tokio::spawn(read_capped(stdout));
    let stderr_task = tokio::spawn(read_capped(stderr));

    let waited = tokio::time::timeout(timeout, child.wait()).await;
    let (code, timed_out) = match waited {
        Ok(Ok(status)) => (status_label(status), false),
        Ok(Err(e)) => return Err(format!("Failed to wait for command ({program}): {e}")),
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            ("timeout".to_string(), true)
        }
    };

    let (stdout, stdout_truncated) = stdout_task
        .await
        .map_err(|e| format!("Failed to collect stdout: {e}"))?;
    let (stderr, stderr_truncated) = stderr_task
        .await
        .map_err(|e| format!("Failed to collect stderr: {e}"))?;
    let text = format_result(
        &code,
        &stdout,
        &stderr,
        stdout_truncated || stderr_truncated,
        timeout,
        timed_out,
    );

    Ok(CommandOutcome { code, text })
}

fn status_label(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "signal".to_string(),
    }
}

async fn read_capped<R>(mut reader: R) -> (String, bool)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0u8; 8192];
    let mut stored = Vec::new();
    let mut truncated = false;

    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let remaining = OUTPUT_READ_LIMIT_BYTES.saturating_sub(stored.len());
                if remaining > 0 {
                    let take = remaining.min(read);
                    stored.extend_from_slice(&buffer[..take]);
                    if take < read {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }

    (String::from_utf8_lossy(&stored).to_string(), truncated)
}

fn format_result(
    code: &str,
    stdout: &str,
    stderr: &str,
    stream_truncated: bool,
    timeout: Duration,
    timed_out: bool,
) -> String {
    let mut body = String::new();
    if !stdout.is_empty() {
        body.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("[stderr]\n");
        body.push_str(stderr);
    }

    let truncated = stream_truncated || body.chars().count() > OUTPUT_LIMIT_CHARS;
    let body: String = body.chars().take(OUTPUT_LIMIT_CHARS).collect();

    let mut result = String::new();
    if timed_out {
        result.push_str(&format!(
            "Timed out after {}s; the process was terminated.\n",
            timeout.as_secs_f64()
        ));
    }
    result.push_str(&format!("exit={code}\n"));
    if !body.is_empty() {
        result.push_str(&body);
        if !result.ends_with('\n') {
            result.push('\n');
        }
    }
    if truncated {
        result.push_str("...(output truncated at 10,000 characters)\n");
    }
    result
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("'{key}' argument must be a string"))
}

fn arg_str_array(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let array = args
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("'{key}' argument must be an array of strings"))?;
    array
        .iter()
        .map(|item| {
            item.as_str()
                .map(|value| value.to_string())
                .ok_or_else(|| format!("Every element in the '{key}' array must be a string"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iskra-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(&dir).unwrap()
    }

    fn tools_for(root: PathBuf) -> Tools {
        Tools {
            root,
            sticky_changed: AtomicBool::new(false),
        }
    }

    #[test]
    fn allows_paths_inside_workspace() {
        let root = temp_root("inside");
        let tools = tools_for(root.clone());

        assert_eq!(tools.resolve("a.txt").unwrap(), root.join("a.txt"));
        assert_eq!(tools.resolve("sub/b.txt").unwrap(), root.join("sub/b.txt"));
        assert_eq!(tools.resolve("./a.txt").unwrap(), root.join("a.txt"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_parent_escape() {
        let root = temp_root("parent");
        let tools = tools_for(root.clone());

        assert!(tools.resolve("../secret.txt").is_err());
        assert!(tools.resolve("sub/../../secret.txt").is_err());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_absolute_path() {
        let root = temp_root("absolute");
        let tools = tools_for(root.clone());

        assert!(tools.resolve("/etc/passwd").is_err());

        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let outside = std::env::temp_dir().join(format!("iskra-outside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();

        symlink(&outside, root.join("link")).unwrap();
        symlink(outside.join("missing"), root.join("broken")).unwrap();

        let tools = tools_for(root.clone());
        assert!(tools.resolve("link/secret.txt").is_err());
        assert!(tools.resolve("broken").is_err());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[tokio::test]
    async fn rejects_programs_outside_allowlist() {
        let root = temp_root("reject");
        let tools = tools_for(root.clone());

        for program in ["rm", "curl", "sh", "/bin/ls", "ls "] {
            let args = serde_json::json!({"program": program, "args": []});
            let result = tools.execute("run_command", &args).await;
            assert!(result.is_err(), "{program} should be rejected");
            assert!(
                result.unwrap_err().contains("not allowed"),
                "{program} rejection should name the allowlist"
            );
        }

        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn does_not_interpret_shell_metacharacters() {
        let root = temp_root("metachar");
        fs::write(root.join("safe.txt"), "safe").unwrap();

        for (name, arg) in [
            ("pwned-semicolon", "safe.txt; touch pwned-semicolon"),
            ("pwned-and", "safe.txt && touch pwned-and"),
            ("pwned-pipe", "safe.txt | touch pwned-pipe"),
        ] {
            let outcome = run_process(&root, "cat", &[arg.to_string()], Duration::from_secs(5))
                .await
                .unwrap();
            assert!(!outcome.text.starts_with("exit=0"));
            assert!(!root.join(name).exists(), "{name} must not be created");
        }

        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn caps_command_output() {
        let root = temp_root("output-cap");
        let content = "a".repeat(OUTPUT_LIMIT_CHARS + 2_000);
        fs::write(root.join("big.txt"), content).unwrap();

        let outcome = run_process(
            &root,
            "cat",
            &["big.txt".to_string()],
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert!(
            outcome
                .text
                .contains("output truncated at 10,000 characters")
        );
        let body = outcome.text.split("\n...(").next().unwrap_or(&outcome.text);
        let a_count = body.chars().filter(|c| *c == 'a').count();
        assert_eq!(a_count, OUTPUT_LIMIT_CHARS);

        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kills_command_on_timeout() {
        let root = temp_root("timeout");
        let started = std::time::Instant::now();

        let outcome = run_process(
            &root,
            "sleep",
            &["5".to_string()],
            Duration::from_millis(100),
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(outcome.text.contains("Timed out"));
        assert_eq!(outcome.code, "timeout");

        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_commands_inside_workspace() {
        let root = temp_root("cwd");
        fs::write(root.join("inside.txt"), "inside-content").unwrap();

        let outcome = run_process(
            &root,
            "cat",
            &["inside.txt".to_string()],
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert!(outcome.text.starts_with("exit=0"));
        assert!(outcome.text.contains("inside-content"));

        fs::remove_dir_all(&root).unwrap();
    }
}
