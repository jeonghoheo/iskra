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
                    Ok((stream, _)) => {
                        handle_connection(stream, &answer, answer_delay, &server_posts)
                    }
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

fn handle_connection(
    stream: TcpStream,
    answer: &str,
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
        saved.push(body);
        posts.1.notify_all();
        drop(saved);
        if !answer_delay.is_zero() {
            thread::sleep(answer_delay);
        }
        serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": answer}}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
        })
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
    fs::remove_dir_all(&dir).unwrap();
}
