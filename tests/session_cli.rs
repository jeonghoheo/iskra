use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct MockServer {
    address: String,
    posts: Arc<(Mutex<Vec<String>>, Condvar)>,
    stop: Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    fn start(answer: &str, answer_delay: Duration) -> Self {
        Self::start_with_first_response(answer, None, answer_delay)
    }

    fn start_with_tool(answer: &str, tool_content: Option<&str>, answer_delay: Duration) -> Self {
        let first = tool_content.map(|content| {
            let arguments =
                serde_json::json!({"path": "agent.txt", "content": content}).to_string();
            serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "write_file", "arguments": arguments}
                        }]
                    }
                }],
                "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
            })
        });
        Self::start_with_first_response(answer, first, answer_delay)
    }

    fn start_with_first_response(
        answer: &str,
        first_response: Option<serde_json::Value>,
        answer_delay: Duration,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let posts = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let server_posts = Arc::clone(&posts);
        let (stop, stopped) = mpsc::channel();
        let answer = answer.to_string();
        let handle = thread::spawn(move || {
            while stopped.try_recv().is_err() {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(
                        stream,
                        &answer,
                        first_response.as_ref(),
                        answer_delay,
                        &server_posts,
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("mock server accept failed: {error}"),
                }
            }
        });
        Self {
            address,
            posts,
            stop,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://{}/chat/completions", self.address)
    }

    fn post_bodies(&self) -> Vec<String> {
        self.posts.0.lock().unwrap().clone()
    }

    fn wait_for_post(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut posts = self.posts.0.lock().unwrap();
        while posts.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, _) = self.posts.1.wait_timeout(posts, deadline - now).unwrap();
            posts = next;
        }
        true
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        let _ = TcpStream::connect(&self.address);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

/// Answers every request, chat and balance, with one fixed HTTP error body.
struct ErrorServer {
    address: String,
    stop: Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl ErrorServer {
    fn start(status: u16, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (stop, stopped) = mpsc::channel();
        let body = body.to_string();
        let reason = match status {
            401 => "Unauthorized",
            500 => "Internal Server Error",
            _ => "Error",
        };
        let handle = thread::spawn(move || {
            while stopped.try_recv().is_err() {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut writer = stream.try_clone().unwrap();
                        let mut reader = BufReader::new(stream);
                        let mut content_length = 0usize;
                        loop {
                            let mut header = String::new();
                            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                                break;
                            }
                            if header == "\r\n" || header.is_empty() {
                                break;
                            }
                            if let Some((name, value)) = header.split_once(':')
                                && name.eq_ignore_ascii_case("content-length")
                            {
                                content_length = value.trim().parse().unwrap_or(0);
                            }
                        }
                        let mut request_body = vec![0; content_length];
                        let _ = reader.read_exact(&mut request_body);
                        let _ = write!(
                            writer,
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = writer.write_all(body.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("error server accept failed: {error}"),
                }
            }
        });
        Self {
            address,
            stop,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://{}/chat/completions", self.address)
    }
}

impl Drop for ErrorServer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        let _ = TcpStream::connect(&self.address);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

fn handle_connection(
    stream: TcpStream,
    answer: &str,
    first_response: Option<&serde_json::Value>,
    answer_delay: Duration,
    posts: &Arc<(Mutex<Vec<String>>, Condvar)>,
) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        if header == "\r\n" || header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    let body = String::from_utf8(body).unwrap();
    let is_post = request_line.starts_with("POST ");

    let response = if is_post {
        let mut saved = posts.0.lock().unwrap();
        let is_first = saved.is_empty();
        saved.push(body);
        posts.1.notify_all();
        drop(saved);
        if !answer_delay.is_zero() {
            thread::sleep(answer_delay);
        }
        match first_response {
            Some(response) if is_first => response.clone(),
            _ => serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": answer}}],
                "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
            }),
        }
    } else {
        serde_json::json!({"is_available": true, "balance_infos": []})
    };
    let response = serde_json::to_vec(&response).unwrap();
    write!(
        writer,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.len()
    )
    .unwrap();
    let _ = writer.write_all(&response);
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "iskra-session-cli-{name}-{}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn command(dir: &Path, url: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_iskra"));
    command
        .arg("--cli")
        .current_dir(dir)
        .env("LLM_API_KEY", "dummy-session-key")
        .env("LLM_BASE_URL", url)
        .env("LLM_MODEL", "session-test-model")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("TELEGRAM_BOT_TOKEN")
        .env_remove("ALLOWED_USER_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run_cli(dir: &Path, url: &str, input: &str) -> Output {
    let mut child = command(dir, url).spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn cli_replays_owner_and_assistant_context_after_process_restart() {
    let dir = temp_dir("restart");
    let first_server = MockServer::start("Conversation answer saved.", Duration::ZERO);
    let first = run_cli(
        &dir,
        &first_server.url(),
        "My test phrase is ember-session-42.\nexit\n",
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("Conversation answer saved."));
    drop(first_server);

    let second_server = MockServer::start("Your phrase is ember-session-42.", Duration::ZERO);
    let second = run_cli(
        &dir,
        &second_server.url(),
        "What is my test phrase?\nexit\n",
    );
    assert!(String::from_utf8_lossy(&second.stdout).contains("ember-session-42"));

    let posts = second_server.post_bodies();
    assert_eq!(posts.len(), 1);
    let runs: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
    assert_eq!(runs["runs"].as_array().unwrap().len(), 2);
    assert_eq!(runs["runs"][0]["state"], "completed");
    assert_eq!(runs["runs"][1]["state"], "completed");
    let request: serde_json::Value = serde_json::from_str(&posts[0]).unwrap();
    let messages = request["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["role"], "system");
    assert!(
        messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("# Session rules")
    );
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(
        messages[1]["content"],
        "My test phrase is ember-session-42."
    );
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["content"], "Conversation answer saved.");
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[3]["content"], "What is my test phrase?");
    assert!(
        fs::read_to_string(dir.join("memory/MEMORY.md"))
            .unwrap()
            .is_empty()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.join("memory/session.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cli_restart_does_not_reprint_or_resend_completed_work() {
    let dir = temp_dir("restart-no-replay");
    let server = MockServer::start("completed-once-answer", Duration::ZERO);

    let first = run_cli(&dir, &server.url(), "finish this work\nexit\n");
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert_eq!(
        first_stdout.matches("completed-once-answer").count(),
        1,
        "the completed answer must be printed exactly once: {first_stdout}"
    );
    assert_eq!(server.post_bodies().len(), 1);

    let second = run_cli(&dir, &server.url(), "exit\n");
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        !second_stdout.contains("completed-once-answer"),
        "a restart must not reprint a finished answer: {second_stdout}"
    );
    assert_eq!(
        server.post_bodies().len(),
        1,
        "a restart must not call the model for finished work"
    );

    let runs: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
    assert_eq!(runs["runs"][0]["state"], "completed");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn killed_work_recovers_as_interrupted_without_any_new_model_call() {
    let dir = temp_dir("interrupted");
    let server = MockServer::start("This answer must not arrive.", Duration::from_millis(500));
    let url = server.url();
    let mut child = command(&dir, &url).spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"Do not repeat this work.\n")
        .unwrap();
    let received = server.wait_for_post();
    let _ = child.kill();
    let _ = child.wait();
    assert!(received, "the first process never reached the model");

    let restarted = run_cli(&dir, &url, "exit\n");
    assert!(restarted.status.success());
    assert_eq!(server.post_bodies().len(), 1);
    let session: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/session.json")).unwrap()).unwrap();
    assert_eq!(session["work"][0]["status"], "interrupted");
    let runs: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
    assert_eq!(runs["runs"][0]["state"], "interrupted");
    assert_eq!(runs["next_run_id"], 2);
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_second_process_fails_closed_while_the_dispatcher_is_live() {
    let dir = temp_dir("second-dispatcher");
    let server = MockServer::start("First process answer.", Duration::from_millis(1_500));
    let url = server.url();

    let mut first = command(&dir, &url).spawn().unwrap();
    first
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"hold the dispatcher\n")
        .unwrap();
    assert!(
        server.wait_for_post(),
        "the first process never reached the model"
    );

    // Stop the first process so its stores stay stable during the comparison.
    let pid = first.id().to_string();
    let stopped = Command::new("kill").args(["-STOP", &pid]).status().unwrap();
    assert!(stopped.success(), "cannot stop the first process");
    let runs_before = fs::read(dir.join("memory/runs.json")).unwrap();
    let session_before = fs::read(dir.join("memory/session.json")).unwrap();

    let mut second = command(&dir, &url).spawn().unwrap();
    let _ = second
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"second process work\n");
    let second_output = second.wait_with_output().unwrap();
    assert!(
        !second_output.status.success(),
        "a second dispatcher must exit nonzero"
    );
    let stderr = String::from_utf8_lossy(&second_output.stderr);
    assert!(
        stderr.contains("Another Iskra dispatcher"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        server.post_bodies().len(),
        1,
        "the second process must not call the model"
    );
    assert_eq!(
        fs::read(dir.join("memory/runs.json")).unwrap(),
        runs_before,
        "the second process must not rewrite the Run store"
    );
    assert_eq!(
        fs::read(dir.join("memory/session.json")).unwrap(),
        session_before,
        "the second process must not rewrite the Session"
    );

    let resumed = Command::new("kill").args(["-CONT", &pid]).status().unwrap();
    assert!(resumed.success(), "cannot resume the first process");
    first.stdin.as_mut().unwrap().write_all(b"exit\n").unwrap();
    let first_output = first.wait_with_output().unwrap();
    assert!(
        first_output.status.success(),
        "the first process failed: {}",
        String::from_utf8_lossy(&first_output.stderr)
    );
    let first_stdout = String::from_utf8_lossy(&first_output.stdout);
    assert!(
        first_stdout.contains("First process answer."),
        "{first_stdout}"
    );

    // After the normal exit a new process owns the lock and replays nothing.
    let restart_server = MockServer::start("Restart must not call the model.", Duration::ZERO);
    let restarted = run_cli(&dir, &restart_server.url(), "exit\n");
    assert!(restarted.status.success());
    assert_eq!(restart_server.post_bodies().len(), 0);
    let runs: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
    assert_eq!(runs["runs"][0]["state"], "completed");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cli_restart_resume_prints_every_answer_and_preserves_interrupted_work() {
    let dir = temp_dir("restart-resume");
    fs::create_dir_all(dir.join("memory")).unwrap();
    fs::write(
        dir.join("memory/runs.json"),
        serde_json::json!({
            "version": 1,
            "next_run_id": 4,
            "runs": [
                {
                    "id": 1,
                    "agent_id": "main",
                    "input": "leave this interrupted",
                    "state": "running",
                    "session_work_id": 1,
                    "revision": 2
                },
                {
                    "id": 2,
                    "agent_id": "main",
                    "input": "first paused answer",
                    "state": "queued",
                    "revision": 1
                },
                {
                    "id": 3,
                    "agent_id": "main",
                    "input": "second paused answer",
                    "state": "queued",
                    "revision": 1
                }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let recovery_server = MockServer::start("Recovery must not call the model.", Duration::ZERO);
    let recovery = run_cli(&dir, &recovery_server.url(), "exit\n");
    assert!(recovery.status.success());
    assert_eq!(recovery_server.post_bodies().len(), 0);
    drop(recovery_server);

    let resumed_server = MockServer::start("Resumed answer delivered.", Duration::ZERO);
    let resumed = run_cli(&dir, &resumed_server.url(), "/resume\nexit\n");
    assert!(resumed.status.success());
    let stdout = String::from_utf8_lossy(&resumed.stdout);
    assert!(stdout.contains("Restarted 2 paused request(s)"), "{stdout}");
    assert_eq!(
        stdout.matches("Resumed answer delivered.").count(),
        2,
        "{stdout}"
    );
    let posts = resumed_server.post_bodies();
    assert_eq!(posts.len(), 2);
    assert!(posts[0].contains("first paused answer"));
    assert!(posts[1].contains("second paused answer"));

    let runs: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
    let records = runs["runs"].as_array().unwrap();
    assert_eq!(records[0]["state"], "interrupted");
    assert_eq!(records[0]["input"], "leave this interrupted");
    assert_eq!(records[1]["state"], "completed");
    assert_eq!(records[2]["state"], "completed");
    fs::remove_dir_all(&dir).unwrap();
}

fn registry_command(dir: &Path, input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_iskra"))
        .arg("--cli")
        .current_dir(dir)
        .env("MAIN_KEY", "dummy-main-key")
        .env("HELPER_KEY", "dummy-helper-key")
        .env_remove("LLM_API_KEY")
        .env_remove("LLM_BASE_URL")
        .env_remove("LLM_MODEL")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("TELEGRAM_BOT_TOKEN")
        .env_remove("ALLOWED_USER_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn two_real_cli_agents_use_their_own_models_and_isolated_files() {
    let dir = temp_dir("two-agents");
    let main = MockServer::start_with_tool("Done by main.", Some("main"), Duration::ZERO);
    let helper = MockServer::start_with_tool("Done by helper.", Some("helper"), Duration::ZERO);
    let registry = serde_json::json!({
        "version": 1,
        "engines": [
            {
                "id": "main-engine",
                "kind": "openai_compatible",
                "base_url": main.url(),
                "model": "main-model",
                "api_key_env": "MAIN_KEY"
            },
            {
                "id": "helper-engine",
                "kind": "openai_compatible",
                "base_url": helper.url(),
                "model": "helper-model",
                "api_key_env": "HELPER_KEY"
            }
        ],
        "agents": [
            {
                "id": "main",
                "display_name": "Iskra",
                "engine_id": "main-engine",
                "role": "Personal assistant"
            },
            {
                "id": "assistant",
                "display_name": "Assistant",
                "engine_id": "helper-engine",
                "role": "Research helper"
            }
        ]
    });
    fs::write(dir.join("iskra.json"), registry.to_string()).unwrap();

    let output = registry_command(
        &dir,
        "@assistant note this for the assistant\nnote this for main\nexit\n",
    );
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Done by helper."), "{stdout}");
    assert!(stdout.contains("Done by main."), "{stdout}");

    let main_posts = main.post_bodies();
    let helper_posts = helper.post_bodies();
    assert_eq!(main_posts.len(), 2);
    assert_eq!(helper_posts.len(), 2);
    let main_request: serde_json::Value = serde_json::from_str(&main_posts[0]).unwrap();
    let helper_request: serde_json::Value = serde_json::from_str(&helper_posts[0]).unwrap();
    assert_eq!(main_request["model"], "main-model");
    assert_eq!(helper_request["model"], "helper-model");

    assert_eq!(
        fs::read_to_string(dir.join("workspace/agent.txt")).unwrap(),
        "main"
    );
    assert_eq!(
        fs::read_to_string(dir.join("agent-workspaces/assistant/agent.txt")).unwrap(),
        "helper"
    );
    assert!(!dir.join("workspace/helper.txt").exists());
    assert!(!dir.join("agent-workspaces/assistant/main.txt").exists());

    let main_session: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/session.json")).unwrap()).unwrap();
    let helper_session: serde_json::Value = serde_json::from_slice(
        &fs::read(dir.join("memory/agents/assistant/session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(main_session["work"][0]["user"], "note this for main");
    assert_eq!(main_session["work"][0]["status"], "completed");
    assert_eq!(
        helper_session["work"][0]["user"],
        "note this for the assistant"
    );
    assert_eq!(helper_session["work"][0]["status"], "completed");

    let runs: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
    let records = runs["runs"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["agent_id"], "assistant");
    assert_eq!(records[0]["state"], "completed");
    assert_eq!(records[1]["agent_id"], "main");
    assert_eq!(records[1]["state"], "completed");
    assert_eq!(runs["next_run_id"], 3);

    assert!(
        fs::read_to_string(dir.join("memory/MEMORY.md"))
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        fs::read_to_string(dir.join("memory/agents/assistant/MEMORY.md"))
            .unwrap_or_default()
            .is_empty()
    );

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn invalid_routes_fail_before_any_model_call() {
    let dir = temp_dir("invalid-route");
    let server = MockServer::start("This answer must not arrive.", Duration::ZERO);
    let output = run_cli(&dir, &server.url(), "@ghost hello\n@\nexit\n");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("There is no agent named @ghost."),
        "{stdout}"
    );
    assert!(
        stdout.contains("To choose who should answer, start with @name"),
        "{stdout}"
    );
    assert_eq!(server.post_bodies().len(), 0);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cli_result_prints_a_stored_answer_without_a_model_call() {
    let dir = temp_dir("cli-result");
    let server = MockServer::start("stored-cli-answer-31", Duration::ZERO);
    let first = run_cli(&dir, &server.url(), "finish this work\nexit\n");
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert_eq!(
        first_stdout.matches("stored-cli-answer-31").count(),
        1,
        "{first_stdout}"
    );
    assert_eq!(server.post_bodies().len(), 1);

    let runs_before = fs::read(dir.join("memory/runs.json")).unwrap();
    let session_before = fs::read(dir.join("memory/session.json")).unwrap();

    let result = run_cli(&dir, &server.url(), "/result 1\nexit\n");
    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.matches("stored-cli-answer-31").count(),
        1,
        "{stdout}"
    );
    assert_eq!(
        server.post_bodies().len(),
        1,
        "reading a result must not call the model"
    );
    assert_eq!(
        fs::read(dir.join("memory/runs.json")).unwrap(),
        runs_before,
        "reading a result must not rewrite the Run store"
    );
    assert_eq!(
        fs::read(dir.join("memory/session.json")).unwrap(),
        session_before,
        "reading a result must not rewrite the Session"
    );

    let log = fs::read_to_string(dir.join("logs/agent.log")).unwrap();
    assert!(log.contains("cli_result run_id=1"), "{log}");
    assert!(
        !log.contains("stored-cli-answer-31"),
        "the stored answer leaked into the log: {log}"
    );

    let bad = run_cli(
        &dir,
        &server.url(),
        "/result\n/result 0\n/result nope\n/result 99\nexit\n",
    );
    assert!(bad.status.success());
    let bad_stdout = String::from_utf8_lossy(&bad.stdout);
    assert_eq!(
        bad_stdout.matches("Usage: /result <work number>").count(),
        3,
        "{bad_stdout}"
    );
    assert!(
        bad_stdout.contains("There is no work with number 99."),
        "{bad_stdout}"
    );
    assert_eq!(
        server.post_bodies().len(),
        1,
        "result errors must not call the model"
    );

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn provider_failure_canary_stays_out_of_the_log_but_reaches_the_owner() {
    let dir = temp_dir("provider-canary");
    let prompt_canary = "PROMPT-CANARY-provider-7f3a91";
    let body_canary = "PROVIDER-BODY-CANARY-9d1c42";
    let body = serde_json::json!({
        "error": {
            "message": body_canary,
            "prompt": prompt_canary
        }
    })
    .to_string();
    let server = ErrorServer::start(401, &body);
    let output = run_cli(&dir, &server.url(), &format!("{prompt_canary}\nexit\n"));
    assert!(output.status.success(), "CLI failed after a run failure");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(body_canary),
        "the owner-facing error must keep the detail: {stderr}"
    );

    let log = fs::read_to_string(dir.join("logs/agent.log")).unwrap();
    assert!(
        log.contains(
            "model_error agent=main run_id=1 engine=main-engine model=session-test-model category=auth status=401"
        ),
        "{log}"
    );
    assert!(
        log.contains("balance_error model=session-test-model category=http status=401"),
        "{log}"
    );
    assert!(
        !log.contains(prompt_canary),
        "prompt leaked into the log: {log}"
    );
    assert!(
        !log.contains(body_canary),
        "provider body leaked into the log: {log}"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn tool_failure_canary_stays_out_of_the_log() {
    let dir = temp_dir("tool-canary");
    let prompt_canary = "PROMPT-CANARY-tool-4b2e17";
    let path_canary = "TOOL-ARG-CANARY-6a9f.txt";
    let arguments = serde_json::json!({ "path": path_canary }).to_string();
    let first_response = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "read_file", "arguments": arguments}
                }]
            }
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
    });
    let server = MockServer::start_with_first_response(
        "Recovered answer.",
        Some(first_response),
        Duration::ZERO,
    );
    let output = run_cli(&dir, &server.url(), &format!("{prompt_canary}\nexit\n"));
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Recovered answer."), "{stdout}");

    let log = fs::read_to_string(dir.join("logs/agent.log")).unwrap();
    assert!(
        log.contains("tool_error agent=main run_id=1 engine=main-engine name=read_file"),
        "{log}"
    );
    assert!(
        !log.contains(prompt_canary),
        "prompt leaked into the log: {log}"
    );
    assert!(
        !log.contains(path_canary),
        "tool argument leaked into the log: {log}"
    );
    assert!(
        !log.contains("Failed to read file"),
        "tool error text leaked into the log: {log}"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn legacy_session_v1_replays_main_history_and_attributes_new_work() {
    let dir = temp_dir("legacy-v1");
    fs::create_dir_all(dir.join("memory")).unwrap();
    fs::create_dir_all(dir.join("logs")).unwrap();
    fs::write(
        dir.join("memory/session.json"),
        serde_json::json!({
            "version": 1,
            "next_id": 2,
            "work": [{
                "id": 1,
                "status": "completed",
                "user": "legacy-question-marker",
                "answer": "legacy-answer-marker"
            }]
        })
        .to_string(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    fs::write(
        dir.join("logs/agent.log"),
        format!("ts={now} model_call tools=[none] usage=prompt=10 completion=2 total=12\n"),
    )
    .unwrap();

    let main = MockServer::start("New main answer.", Duration::ZERO);
    let helper = MockServer::start("New helper answer.", Duration::ZERO);
    let registry = serde_json::json!({
        "version": 1,
        "engines": [
            {
                "id": "main-engine",
                "kind": "openai_compatible",
                "base_url": main.url(),
                "model": "main-model",
                "api_key_env": "MAIN_KEY"
            },
            {
                "id": "helper-engine",
                "kind": "openai_compatible",
                "base_url": helper.url(),
                "model": "helper-model",
                "api_key_env": "HELPER_KEY"
            }
        ],
        "agents": [
            {
                "id": "main",
                "display_name": "Iskra",
                "engine_id": "main-engine",
                "role": "Personal assistant"
            },
            {
                "id": "assistant",
                "display_name": "Assistant",
                "engine_id": "helper-engine",
                "role": "Research helper"
            }
        ]
    });
    fs::write(dir.join("iskra.json"), registry.to_string()).unwrap();

    let output = registry_command(
        &dir,
        "/status\nnew main question\n@assistant helper question\nexit\n",
    );
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Tokens today: 12"), "{stdout}");
    assert!(stdout.contains("New main answer."), "{stdout}");
    assert!(stdout.contains("New helper answer."), "{stdout}");

    let main_posts = main.post_bodies();
    assert_eq!(main_posts.len(), 1);
    let main_request: serde_json::Value = serde_json::from_str(&main_posts[0]).unwrap();
    let main_messages = main_request["messages"].as_array().unwrap();
    let main_text = main_messages
        .iter()
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(main_text.contains("legacy-question-marker"), "{main_text}");
    assert!(main_text.contains("legacy-answer-marker"), "{main_text}");
    assert!(main_text.contains("new main question"), "{main_text}");

    let helper_posts = helper.post_bodies();
    assert_eq!(helper_posts.len(), 1);
    let helper_request: serde_json::Value = serde_json::from_str(&helper_posts[0]).unwrap();
    let helper_text = helper_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !helper_text.contains("legacy-question-marker"),
        "{helper_text}"
    );
    assert!(
        !helper_text.contains("legacy-answer-marker"),
        "{helper_text}"
    );
    assert!(helper_text.contains("helper question"), "{helper_text}");

    let runs: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/runs.json")).unwrap()).unwrap();
    let records = runs["runs"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["agent_id"], "main");
    assert_eq!(records[0]["input"], "new main question");
    assert_eq!(records[0]["state"], "completed");
    assert_eq!(records[1]["agent_id"], "assistant");
    assert_eq!(records[1]["input"], "helper question");
    assert_eq!(records[1]["state"], "completed");

    let main_session: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("memory/session.json")).unwrap()).unwrap();
    let main_work = main_session["work"].as_array().unwrap();
    assert_eq!(main_work.len(), 2);
    assert_eq!(main_work[0]["user"], "legacy-question-marker");
    assert_eq!(main_work[1]["user"], "new main question");
    assert_eq!(main_work[1]["status"], "completed");

    let helper_session: serde_json::Value = serde_json::from_slice(
        &fs::read(dir.join("memory/agents/assistant/session.json")).unwrap(),
    )
    .unwrap();
    let helper_work = helper_session["work"].as_array().unwrap();
    assert_eq!(helper_work.len(), 1);
    assert_eq!(helper_work[0]["user"], "helper question");

    let log = fs::read_to_string(dir.join("logs/agent.log")).unwrap();
    assert!(
        log.contains("model_call agent=main run_id=1 engine=main-engine model=main-model"),
        "{log}"
    );
    assert!(
        log.contains("model_call agent=assistant run_id=2 engine=helper-engine model=helper-model"),
        "{log}"
    );
    fs::remove_dir_all(&dir).unwrap();
}
