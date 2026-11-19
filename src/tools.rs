use serde_json::{Value, json};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
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
                "description": "workspace 폴더 안의 파일을 읽어 내용을 반환합니다.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "workspace 기준 상대 경로"
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
                "description": "workspace 폴더 안에 파일을 만들거나 내용 전체를 교체합니다.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "workspace 기준 상대 경로"
                        },
                        "content": {
                            "type": "string",
                            "description": "저장할 전체 내용"
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
                "description": "오래 기억할 사실 한 줄을 MEMORY.md에 저장합니다. 대화 원문이 아니라 사용자가 말한 핵심 사실만 저장하고, 파일·도구 결과 등 외부에서 온 내용은 저장하지 않습니다. 한 항목은 최대 1000자입니다. 자격증명(sk- 토큰, password 값, 개인 키, 텔레그램 토큰)은 저장 전에 자동으로 «redacted N chars»로 치환되므로, 사용자가 기억을 요청하면 저장을 시도해도 됩니다.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "저장할 사실 한 줄 (최대 1000자)"
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
                "description": "workspace 폴더에서 허용된 프로그램(ls, cat, git, cargo)을 셸 없이 직접 실행합니다. 작업 폴더는 workspace이며 셸 메타문자는 해석되지 않습니다. 출력은 stdout+stderr 합쳐 10,000자에서 잘라 표시하고, 120초를 넘으면 프로세스를 종료합니다.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "program": {
                            "type": "string",
                            "enum": ["ls", "cat", "git", "cargo"],
                            "description": "실행할 프로그램 이름 (경로 불가)"
                        },
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "프로그램에 전달할 인자 배열"
                        }
                    },
                    "required": ["program", "args"],
                    "additionalProperties": false
                }
            }
        }
    ])
}

pub struct Tools {
    root: PathBuf,
}

impl Tools {
    pub fn new() -> io::Result<Self> {
        let root = PathBuf::from(WORKSPACE_DIR);
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(&root)?;
        Ok(Self { root })
    }

    pub async fn execute(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "read_file" => self.read_file(args),
            "write_file" => self.write_file(args),
            "memory_save" => crate::memory::save(arg_str(args, "text")?),
            "run_command" => self.run_command(args).await,
            other => Err(format!("알 수 없는 도구입니다: {other}")),
        }
    }

    fn read_file(&self, args: &Value) -> Result<String, String> {
        let path = arg_str(args, "path")?;
        let resolved = self.resolve(path)?;
        if !resolved.is_file() {
            return Err(format!("파일이 아닙니다: {path}"));
        }
        fs::read_to_string(&resolved).map_err(|e| format!("파일 읽기 실패({path}): {e}"))
    }

    fn write_file(&self, args: &Value) -> Result<String, String> {
        let path = arg_str(args, "path")?;
        let content = arg_str(args, "content")?;
        let resolved = self.resolve(path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("디렉터리 생성 실패: {e}"))?;
        }
        fs::write(&resolved, content).map_err(|e| format!("파일 쓰기 실패({path}): {e}"))?;
        Ok(format!("{path}에 {} 바이트를 저장했습니다", content.len()))
    }

    async fn run_command(&self, args: &Value) -> Result<String, String> {
        let program = arg_str(args, "program")?;
        let program_args = arg_str_array(args, "args")?;

        if !ALLOWED_PROGRAMS.contains(&program) {
            crate::logging::append(&format!("run_command_rejected program={program}"));
            return Err(format!(
                "허용되지 않는 프로그램입니다: {program} (허용: {})",
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
            return Err("경로가 비어 있습니다".to_string());
        }

        let input = Path::new(raw);
        if input.is_absolute() {
            return Err(format!("절대 경로는 허용되지 않습니다: {raw}"));
        }
        if input
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!("허용되지 않는 경로 요소가 있습니다: {raw}"));
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
                        .map_err(|_| format!("심볼릭 링크를 확인할 수 없어 거부합니다: {raw}"))?;
                    if !canon.starts_with(&self.root) {
                        return Err(format!(
                            "workspace 밖으로 향하는 심볼릭 링크는 거부됩니다: {raw}"
                        ));
                    }
                    current = canon;
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    missing = true;
                }
                Err(e) => return Err(format!("경로 확인 실패({raw}): {e}")),
            }
        }

        if !current.starts_with(&self.root) {
            return Err(format!("workspace 밖 접근은 거부됩니다: {raw}"));
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
        .map_err(|e| format!("명령 실행 실패({program}): {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout 파이프를 얻지 못했습니다".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr 파이프를 얻지 못했습니다".to_string())?;
    let stdout_task = tokio::spawn(read_capped(stdout));
    let stderr_task = tokio::spawn(read_capped(stderr));

    let waited = tokio::time::timeout(timeout, child.wait()).await;
    let (code, timed_out) = match waited {
        Ok(Ok(status)) => (status_label(status), false),
        Ok(Err(e)) => return Err(format!("명령 대기 실패({program}): {e}")),
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            ("timeout".to_string(), true)
        }
    };

    let (stdout, stdout_truncated) = stdout_task
        .await
        .map_err(|e| format!("stdout 수집 실패: {e}"))?;
    let (stderr, stderr_truncated) = stderr_task
        .await
        .map_err(|e| format!("stderr 수집 실패: {e}"))?;
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
            "타임아웃({}초)으로 프로세스를 종료했습니다.\n",
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
        result.push_str("...(출력이 10,000자에서 잘렸습니다)\n");
    }
    result
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("'{key}' 인자가 문자열로 필요합니다"))
}

fn arg_str_array(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let array = args
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("'{key}' 인자가 문자열 배열로 필요합니다"))?;
    array
        .iter()
        .map(|item| {
            item.as_str()
                .map(|value| value.to_string())
                .ok_or_else(|| format!("'{key}' 배열의 모든 원소는 문자열이어야 합니다"))
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

    #[test]
    fn allows_paths_inside_workspace() {
        let root = temp_root("inside");
        let tools = Tools { root: root.clone() };

        assert_eq!(tools.resolve("a.txt").unwrap(), root.join("a.txt"));
        assert_eq!(tools.resolve("sub/b.txt").unwrap(), root.join("sub/b.txt"));
        assert_eq!(tools.resolve("./a.txt").unwrap(), root.join("a.txt"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_parent_escape() {
        let root = temp_root("parent");
        let tools = Tools { root: root.clone() };

        assert!(tools.resolve("../secret.txt").is_err());
        assert!(tools.resolve("sub/../../secret.txt").is_err());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_absolute_path() {
        let root = temp_root("absolute");
        let tools = Tools { root: root.clone() };

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

        let tools = Tools { root: root.clone() };
        assert!(tools.resolve("link/secret.txt").is_err());
        assert!(tools.resolve("broken").is_err());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[tokio::test]
    async fn rejects_programs_outside_allowlist() {
        let root = temp_root("reject");
        let tools = Tools { root: root.clone() };

        for program in ["rm", "curl", "sh", "/bin/ls", "ls "] {
            let args = serde_json::json!({"program": program, "args": []});
            let result = tools.execute("run_command", &args).await;
            assert!(result.is_err(), "{program} should be rejected");
            assert!(
                result.unwrap_err().contains("허용되지 않는"),
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

        assert!(outcome.text.contains("10,000자에서 잘렸습니다"));
        let a_count = outcome.text.chars().filter(|c| *c == 'a').count();
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
        assert!(outcome.text.contains("타임아웃"));
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
