//! Real-process checks for the CLI-only pane control surface.
//!
//! Each test runs the built `iskra --cli` binary in an isolated temporary data
//! directory with dummy LLM configuration and a loopback counting server, so a
//! pane command making a model request would be observable. The counting server
//! answers with an error; no pane command is expected to reach it.

use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Records every accepted connection so a test can prove zero model calls.
struct CountingServer {
    address: String,
    hits: Arc<AtomicUsize>,
    stop: Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl CountingServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&hits);
        let (stop, stopped) = mpsc::channel();
        let handle = thread::spawn(move || {
            while stopped.try_recv().is_err() {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        server_hits.fetch_add(1, Ordering::SeqCst);
                        let _ = stream.write_all(
                            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("counting server accept failed: {error}"),
                }
            }
        });
        Self {
            address,
            hits,
            stop,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://{}/chat/completions", self.address)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for CountingServer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        let _ = TcpStream::connect(&self.address);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "iskra-pane-cli-{name}-{}-{nanos}",
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
        .env("LLM_API_KEY", "dummy-pane-key")
        .env("LLM_BASE_URL", url)
        .env("LLM_MODEL", "pane-test-model")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("TELEGRAM_BOT_TOKEN")
        .env_remove("ALLOWED_USER_ID")
        .env_remove("ISKRA_HEADLESS_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

struct Cli {
    child: Child,
}

impl Cli {
    fn spawn(dir: &Path, url: &str) -> Self {
        Self {
            child: command(dir, url).spawn().unwrap(),
        }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.child.stdin.as_mut().expect("piped stdin");
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn send_and_wait(&mut self, line: &str, millis: u64) {
        self.send(line);
        thread::sleep(Duration::from_millis(millis));
    }

    fn finish(mut self) -> Output {
        drop(self.child.stdin.take());
        self.child.wait_with_output().unwrap()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn pane_cli_rejects_malformed_input_without_spawning_or_calling_a_model() {
    let dir = temp_dir("malformed");
    let server = CountingServer::start();
    let mut cli = Cli::spawn(&dir, &server.url());

    for line in [
        "/panes",
        "/pane",
        "/pane bogus",
        "/pane start 0 80",
        "/pane start 201 80",
        "/pane resize 1 0 80",
        "/pane stop 0",
        "/pane stop 99",
        "/pane send 1",
        "/pane text",
        "/pane text 99",
        "/panes extra",
    ] {
        cli.send(line);
    }
    cli.send("exit");

    let output = cli.finish();
    let text = stdout(&output);
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("No panes."), "{text}");
    assert!(text.contains("Unknown pane command: bogus."), "{text}");
    assert!(text.contains("1..=200"), "{text}");
    assert!(text.contains("Usage: /panes"), "{text}");
    assert!(text.contains("Unknown pane: 99"), "{text}");
    assert!(!text.contains(" live "), "no pane may start: {text}");
    assert_eq!(server.hits(), 0, "a pane command must not call a model");
    assert!(
        !dir.join("memory/panes.json").exists(),
        "malformed input must not create a pane store"
    );
    assert!(
        !dir.join("memory/runs.json").exists(),
        "pane commands must not create a Run"
    );

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pane_cli_reports_a_poisoned_store_without_spawning() {
    let dir = temp_dir("poisoned");
    fs::create_dir_all(dir.join("memory")).unwrap();
    let store = dir.join("memory/panes.json");
    let original = b"not a pane store".to_vec();
    fs::write(&store, &original).unwrap();

    let server = CountingServer::start();
    let mut cli = Cli::spawn(&dir, &server.url());
    for line in ["/panes", "/pane start", "/pane text 1"] {
        cli.send(line);
    }
    cli.send("exit");

    let output = cli.finish();
    let text = stdout(&output);
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("Pane error: Pane store error:"), "{text}");
    assert!(
        !text.contains(" live "),
        "a poisoned store must not spawn: {text}"
    );
    assert_eq!(server.hits(), 0);
    assert_eq!(
        fs::read(&store).unwrap(),
        original,
        "the poisoned store must be preserved byte for byte"
    );
    assert!(!dir.join("memory/runs.json").exists());

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pane_cli_controls_a_live_pane_end_to_end() {
    let dir = temp_dir("end-to-end");
    let server = CountingServer::start();
    let mut cli = Cli::spawn(&dir, &server.url());

    cli.send("/panes");
    cli.send("/pane start");
    cli.send_and_wait("/panes", 300);
    cli.send_and_wait("/pane send 1 printf 'SURFACE_OK\\n'", 700);
    cli.send("/pane text 1");

    cli.send("/pane resize 1 30 100");
    cli.send_and_wait("/pane send 1 stty size", 700);
    cli.send("/pane text 1");

    cli.send_and_wait("/pane stop 1", 300);
    cli.send("/pane text 1");
    cli.send("/pane send 1 echo should-not-run");

    // After the stop, the slot is reusable by explicit start only. Eight live
    // panes are allowed; the ninth start is refused.
    for _ in 0..8 {
        cli.send_and_wait("/pane start", 150);
    }
    cli.send("/pane start");
    for id in 1..=8 {
        cli.send_and_wait(&format!("/pane stop {id}"), 100);
    }
    cli.send("exit");

    let output = cli.finish();
    let text = stdout(&output);
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(text.contains("No panes."), "{text}");
    assert!(text.contains("Pane 1 live 24x80"), "{text}");
    assert!(text.contains("1 live 24x80"), "{text}");
    assert!(text.contains("SURFACE_OK"), "pane output missing: {text}");
    assert!(text.contains("Pane 1 resized to 30x100"), "{text}");
    assert!(text.contains("30 100"), "stty size missing: {text}");
    assert!(text.contains("Pane 1 finished"), "{text}");
    assert!(
        text.contains("Pane 1 screen is unavailable (state finished)."),
        "{text}"
    );
    assert!(
        text.contains("Pane 1 is not live (state finished)."),
        "{text}"
    );
    assert!(text.contains("Pane limit reached (8)"), "{text}");

    assert_eq!(server.hits(), 0, "a pane command must not call a model");
    assert!(
        !dir.join("memory/runs.json").exists(),
        "pane commands must not create a Run"
    );

    let store = dir.join("memory/panes.json");
    assert!(store.exists(), "starting a pane must persist the store");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&store).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    fs::remove_dir_all(&dir).unwrap();
}
