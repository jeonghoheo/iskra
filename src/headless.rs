use std::fs::{self, File, OpenOptions};
use std::io;
use std::net::SocketAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::logging;
use crate::registry::HeadlessProfile;
use crate::run::{Core, RunError, RunState};
use crate::run_context::MAIN_AGENT_ID;

/// Maximum accepted JSON request body. Anything larger is refused before parsing.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Maximum accepted idempotency key length. The Core-owned dedupe index keeps a
/// bounded number of keys and ignores a key whose Run left the Run store.
pub const MAX_IDEMPOTENCY_KEY_CHARS: usize = crate::idempotency::MAX_KEY_CHARS;
const SCHEMA: &str = "headless/v1";
const LOCK_FILE: &str = "headless.lock";

/// One running loopback HTTP surface for the `headless/v1` endpoints.
pub struct Headless {
    local_addr: SocketAddr,
    task: JoinHandle<()>,
    _guard: HeadlessGuard,
}

impl Headless {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for Headless {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct HeadlessState {
    core: Arc<Core>,
    token: Arc<Vec<u8>>,
}

/// Starts the headless HTTP surface using the token named by `profile.token_env`.
///
/// The token value is read once and never written to logs or responses. A
/// missing or empty token is a startup error, not an unauthenticated server.
pub async fn start(
    core: Arc<Core>,
    memory_dir: &Path,
    profile: &HeadlessProfile,
) -> Result<Headless, String> {
    let token = std::env::var(&profile.token_env).map_err(|_| {
        format!(
            "headless token environment variable {} is missing",
            profile.token_env
        )
    })?;
    if token.is_empty() {
        return Err(format!(
            "headless token environment variable {} is empty",
            profile.token_env
        ));
    }
    start_with_token(core, memory_dir, profile, token).await
}

async fn start_with_token(
    core: Arc<Core>,
    memory_dir: &Path,
    profile: &HeadlessProfile,
    token: String,
) -> Result<Headless, String> {
    let address: SocketAddr = profile
        .bind
        .parse()
        .map_err(|error| format!("Invalid headless bind {}: {error}", profile.bind))?;
    if !address.ip().is_loopback() && !profile.allow_non_loopback {
        return Err(format!(
            "Refusing a non-loopback headless bind {address} without owner confirmation"
        ));
    }

    // One live headless surface per data directory. The production process also
    // holds the dispatcher lock, so this is defense in depth for composition
    // paths that construct a Core without a dispatcher.
    let guard = HeadlessGuard::acquire(memory_dir)?;
    let listener = TcpListener::bind(address)
        .await
        .map_err(|error| format!("Cannot bind the headless server to {address}: {error}"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|error| format!("Cannot read the headless server address: {error}"))?;

    let state = HeadlessState {
        core,
        token: Arc::new(token.into_bytes()),
    };
    let app = router(state);
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            logging::append(&format!("headless_server_error {error}"));
        }
    });

    Ok(Headless {
        local_addr,
        task,
        _guard: guard,
    })
}

fn router(state: HeadlessState) -> Router {
    Router::new()
        .route("/v1/runs", post(submit_run))
        .route("/v1/runs/{run_id}", get(read_run))
        .route("/v1/runs/{run_id}/evidence", get(read_evidence))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitRequest {
    #[serde(default)]
    agent_id: Option<String>,
    prompt: String,
    idempotency_key: String,
}

async fn submit_run(
    State(state): State<HeadlessState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match declared_body_size(&headers) {
        Ok(Some(size)) if size > MAX_BODY_BYTES => {
            return error_response(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
        }
        Ok(_) => {}
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid_request"),
    }
    let bytes = match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return error_response(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
    };
    let request: SubmitRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    if request.idempotency_key.trim().is_empty()
        || request.idempotency_key.chars().count() > MAX_IDEMPOTENCY_KEY_CHARS
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    }

    let agent_id = request
        .agent_id
        .unwrap_or_else(|| MAIN_AGENT_ID.to_string());
    match state
        .core
        .admit_idempotent(&agent_id, &request.prompt, &request.idempotency_key)
    {
        Ok(outcome) if outcome.deduplicated => json_response(
            StatusCode::OK,
            json!({
                "schema": SCHEMA,
                "run_id": outcome.run_id,
                "state": outcome.state,
                "deduplicated": true,
            }),
        ),
        Ok(outcome) => json_response(
            StatusCode::CREATED,
            json!({
                "schema": SCHEMA,
                "run_id": outcome.run_id,
                "state": outcome.state,
                "created_at": now_rfc3339(),
            }),
        ),
        Err(error) => run_error_response(error),
    }
}

async fn read_run(
    State(state): State<HeadlessState>,
    AxumPath(run_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Ok(run_id) = run_id.parse::<u64>() else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    let target = match state.core.run_target(run_id) {
        Ok(Some(target)) => target,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "not_found"),
        Err(_) => return error_response(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
    };

    // The stored redacted Session answer is the same projection `/result`
    // exposes. Unfinished work skips the Session read entirely, so `result` is
    // absent rather than guessed.
    let result = if target.state == RunState::Completed {
        match state.core.completed_answer(run_id) {
            Ok(answer) => answer,
            Err(_) => {
                return error_response(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable");
            }
        }
    } else {
        None
    };

    let mut body = Map::new();
    body.insert("schema".to_string(), json!(SCHEMA));
    body.insert("run_id".to_string(), json!(target.id));
    body.insert("agent_id".to_string(), json!(target.agent_id));
    body.insert("state".to_string(), json!(target.state));
    // `runs.json` persists no timestamps, so a read never invents them.
    body.insert("created_at".to_string(), json!("unknown"));
    body.insert("updated_at".to_string(), json!("unknown"));
    if let Some(result) = result {
        body.insert("result".to_string(), json!(result));
    }
    json_response(StatusCode::OK, Value::Object(body))
}

async fn read_evidence(
    State(state): State<HeadlessState>,
    AxumPath(run_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&headers, &state.token) {
        return error_response(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Ok(run_id) = run_id.parse::<u64>() else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    match state.core.run_target(run_id) {
        // Mechanical evidence only. The existing stores persist no per-Run exit
        // code, artifact hashes, delivery reference, or model-call count, so
        // those values stay explicitly "unknown" instead of being estimated.
        Ok(Some(target)) => json_response(
            StatusCode::OK,
            json!({
                "schema": SCHEMA,
                "run_id": target.id,
                "agent_id": target.agent_id,
                "state": target.state,
                "exit_code": "unknown",
                "artifact_hashes": [],
                "delivery": {"id": "unknown", "state": "unknown", "message_id": "unknown"},
                "usage": {"model_calls": "unknown", "cost_known": false},
            }),
        ),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "not_found"),
        Err(_) => error_response(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
    }
}

async fn not_found() -> Response {
    error_response(StatusCode::NOT_FOUND, "not_found")
}

async fn method_not_allowed() -> Response {
    error_response(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed")
}

fn run_error_response(error: RunError) -> Response {
    match error {
        RunError::QueueFull => error_response(StatusCode::TOO_MANY_REQUESTS, "admission_full"),
        RunError::Store(_) => {
            error_response(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable")
        }
        _ => error_response(StatusCode::BAD_REQUEST, "invalid_request"),
    }
}

fn declared_body_size(headers: &HeaderMap) -> Result<Option<usize>, ()> {
    let Some(value) = headers.get(header::CONTENT_LENGTH) else {
        return Ok(None);
    };
    let text = value.to_str().map_err(|_| ())?;
    let size = text.parse::<usize>().map_err(|_| ())?;
    Ok(Some(size))
}

fn authorized(headers: &HeaderMap, token: &[u8]) -> bool {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some((scheme, presented)) = value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer") && constant_time_eq(presented.as_bytes(), token)
}

/// Compares without short-circuiting on the first differing byte.
fn constant_time_eq(presented: &[u8], expected: &[u8]) -> bool {
    let mut difference = (presented.len() ^ expected.len()) as u8;
    let length = presented.len().max(expected.len());
    for index in 0..length {
        let left = presented.get(index).copied().unwrap_or(0);
        let right = expected.get(index).copied().unwrap_or(0);
        difference |= left ^ right;
    }
    difference == 0
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"error\":\"internal\"}".to_vec());
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn error_response(status: StatusCode, code: &'static str) -> Response {
    json_response(status, json!({ "error": code }))
}

fn now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let remaining = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remaining / 3_600,
        (remaining % 3_600) / 60,
        remaining % 60
    )
}

/// Converts days since 1970-01-01 to a proleptic Gregorian date.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Lifetime lock preventing two headless surfaces from owning one data directory.
struct HeadlessGuard {
    file: File,
}

impl HeadlessGuard {
    fn acquire(memory_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(memory_dir).map_err(|error| {
            format!(
                "Cannot create the headless lock directory {}: {error}",
                memory_dir.display()
            )
        })?;
        let path = memory_dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|error| headless_open_error(&path, error))?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(headless_open_error(&path, io::Error::last_os_error()));
        }
        Ok(Self { file })
    }
}

impl Drop for HeadlessGuard {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn headless_open_error(path: &Path, error: io::Error) -> String {
    if error.kind() == io::ErrorKind::WouldBlock {
        return format!(
            "Another Iskra headless server already owns this data directory (lock: {})",
            path.display()
        );
    }
    if error.raw_os_error() == Some(libc::ELOOP) {
        return format!(
            "The headless lock must not be a symlink: {}",
            path.display()
        );
    }
    format!("Cannot lock {}: {error}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;
    use crate::runtime::RuntimeRegistry;
    use std::fs;
    use std::path::PathBuf;

    const TOKEN: &str = "test-owner-token";

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "iskra-headless-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn core_for(dir: &Path) -> Arc<Core> {
        core_for_url(dir, "http://127.0.0.1:9/chat/completions")
    }

    fn core_for_url(dir: &Path, url: &str) -> Arc<Core> {
        let url = url.to_string();
        let registry = RuntimeRegistry::load(dir, move |key| match key {
            "LLM_API_KEY" => Some("dummy-main-key".to_string()),
            "LLM_BASE_URL" => Some(url.clone()),
            "LLM_MODEL" => Some("main-test-model".to_string()),
            _ => None,
        })
        .unwrap();
        Core::new(registry).unwrap()
    }

    /// Builds a Core whose registry holds the compatibility `main` Agent plus
    /// one test-only Agent. Usage counters are per Agent and log-derived, so a
    /// unique Agent id keeps a read test's counter snapshot isolated from every
    /// other test running in the same process.
    fn core_for_agent_url(dir: &Path, agent_id: &str, url: &str) -> Arc<Core> {
        let registry_text = json!({
            "version": 1,
            "engines": [{
                "id": "main-engine",
                "kind": "openai_compatible",
                "base_url": url,
                "model": "main-test-model",
                "api_key_env": "LLM_API_KEY"
            }],
            "agents": [
                {
                    "id": "main",
                    "display_name": "Iskra",
                    "engine_id": "main-engine",
                    "role": "Personal assistant"
                },
                {
                    "id": agent_id,
                    "display_name": "Read Test",
                    "engine_id": "main-engine",
                    "role": "Read-only test agent"
                }
            ]
        });
        fs::write(dir.join(registry::REGISTRY_FILE), registry_text.to_string()).unwrap();
        let url = url.to_string();
        let registry = RuntimeRegistry::load(dir, move |key| match key {
            "LLM_API_KEY" => Some("dummy-main-key".to_string()),
            "LLM_BASE_URL" => Some(url.clone()),
            "LLM_MODEL" => Some("main-test-model".to_string()),
            _ => None,
        })
        .unwrap();
        Core::new(registry).unwrap()
    }

    fn loopback_profile() -> HeadlessProfile {
        HeadlessProfile {
            bind: "127.0.0.1:0".to_string(),
            token_env: "ISKRA_HEADLESS_TOKEN".to_string(),
            allow_non_loopback: false,
        }
    }

    async fn serve(core: Arc<Core>, dir: &Path) -> Headless {
        start_with_token(core, dir, &loopback_profile(), TOKEN.to_string())
            .await
            .unwrap()
    }

    fn assert_no_run_store(dir: &Path) {
        assert!(
            !dir.join("memory/runs.json").exists(),
            "a failed request must not create a Run"
        );
    }

    async fn status_for(
        method: reqwest::Method,
        address: SocketAddr,
        path: &str,
        token: Option<&str>,
        body: String,
    ) -> (reqwest::StatusCode, String) {
        let mut request = reqwest::Client::new()
            .request(method, format!("http://{address}{path}"))
            .body(body);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        (status, body)
    }

    async fn submit_request(
        address: SocketAddr,
        token: Option<&str>,
        body: &str,
    ) -> (reqwest::StatusCode, String) {
        status_for(
            reqwest::Method::POST,
            address,
            "/v1/runs",
            token,
            body.to_string(),
        )
        .await
    }

    #[test]
    fn headless_binds_loopback_by_default() {
        let text = r#"{
            "version": 1,
            "engines": [{
                "id": "main-engine",
                "kind": "openai_compatible",
                "base_url": "https://api.deepseek.com/chat/completions",
                "model": "deepseek-flash",
                "api_key_env": "LLM_API_KEY"
            }],
            "agents": [{
                "id": "main",
                "display_name": "Iskra",
                "engine_id": "main-engine",
                "role": "Personal assistant"
            }],
            "headless": {"token_env": "ISKRA_HEADLESS_TOKEN"}
        }"#;
        let profile = registry::parse(text).unwrap().headless.unwrap();
        let address: SocketAddr = profile.bind.parse().unwrap();
        assert!(address.ip().is_loopback(), "default bind was {address}");
    }

    #[tokio::test]
    async fn headless_second_bind_fails_closed() {
        let dir = temp_dir("second-bind");
        let core = core_for(&dir);
        let first = serve(Arc::clone(&core), &dir).await;
        let error = match start_with_token(
            Arc::clone(&core),
            &dir,
            &loopback_profile(),
            "other-token".to_string(),
        )
        .await
        {
            Ok(_) => panic!("a second headless server on one data directory must fail"),
            Err(error) => error,
        };
        assert!(
            error.contains("already owns this data directory"),
            "{error}"
        );

        drop(first);
        let third = serve(core, &dir).await;
        drop(third);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_missing_token_401() {
        let dir = temp_dir("missing-token");
        let server = serve(core_for(&dir), &dir).await;
        let (status, body) = submit_request(server.local_addr(), None, "{}").await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
        assert!(body.contains("unauthorized"));
        assert!(!body.contains(TOKEN), "the token must never be echoed");
        assert_no_run_store(&dir);
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_wrong_token_401() {
        let dir = temp_dir("wrong-token");
        let server = serve(core_for(&dir), &dir).await;
        let (status, body) = submit_request(server.local_addr(), Some("wrong-token"), "{}").await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
        assert!(body.contains("unauthorized"));
        assert!(!body.contains(TOKEN), "the token must never be echoed");
        assert!(!body.contains("wrong-token"));
        assert_no_run_store(&dir);
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_malformed_body_400() {
        let dir = temp_dir("malformed");
        let server = serve(core_for(&dir), &dir).await;
        let (status, body) = submit_request(server.local_addr(), Some(TOKEN), "not json").await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert!(body.contains("invalid_request"));
        assert_no_run_store(&dir);
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_unknown_fields_400() {
        let dir = temp_dir("unknown-field");
        let server = serve(core_for(&dir), &dir).await;
        let body = r#"{"prompt":"hi","idempotency_key":"k1","extra":true}"#;
        let (status, _) = submit_request(server.local_addr(), Some(TOKEN), body).await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert_no_run_store(&dir);
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_oversized_body_413() {
        let dir = temp_dir("oversized");
        let server = serve(core_for(&dir), &dir).await;
        let body = "a".repeat(MAX_BODY_BYTES + 1);
        let (status, text) = submit_request(server.local_addr(), Some(TOKEN), &body).await;
        assert_eq!(status, reqwest::StatusCode::PAYLOAD_TOO_LARGE);
        assert!(text.contains("payload_too_large"));
        assert_no_run_store(&dir);
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_unknown_path_404() {
        let dir = temp_dir("unknown-path");
        let server = serve(core_for(&dir), &dir).await;
        let (status, body) = status_for(
            reqwest::Method::GET,
            server.local_addr(),
            "/v1/nope",
            Some(TOKEN),
            String::new(),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
        assert!(body.contains("not_found"));

        let (status, _) = status_for(
            reqwest::Method::GET,
            server.local_addr(),
            "/v1/runs/999",
            Some(TOKEN),
            String::new(),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_wrong_method_405() {
        let dir = temp_dir("method");
        let server = serve(core_for(&dir), &dir).await;
        let (status, body) = status_for(
            reqwest::Method::PATCH,
            server.local_addr(),
            "/v1/runs",
            Some(TOKEN),
            String::new(),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::METHOD_NOT_ALLOWED);
        assert!(body.contains("method_not_allowed"));
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_valid_submit_returns_201_without_echoing_the_body() {
        let dir = temp_dir("submit");
        let server = serve(core_for(&dir), &dir).await;
        let body = r#"{"prompt":"say hello","idempotency_key":"key-1"}"#;
        let (status, text) = submit_request(server.local_addr(), Some(TOKEN), body).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], SCHEMA);
        assert_eq!(value["state"], "queued");
        assert_eq!(value["run_id"], 1);
        assert!(
            !text.contains("say hello"),
            "request bodies must never be echoed"
        );
        assert!(!text.contains(TOKEN), "the token must never be echoed");
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    fn read_runs(dir: &Path) -> serde_json::Value {
        serde_json::from_slice(&fs::read(dir.join("memory/runs.json")).unwrap()).unwrap()
    }

    fn read_entries(dir: &Path) -> Option<serde_json::Value> {
        let path = dir.join("memory/idempotency.json");
        let bytes = fs::read(path).ok()?;
        Some(serde_json::from_slice(&bytes).unwrap())
    }

    fn runs_path(dir: &Path) -> PathBuf {
        dir.join("memory/runs.json")
    }

    fn session_path(dir: &Path, agent_id: &str) -> PathBuf {
        dir.join("memory")
            .join("agents")
            .join(agent_id)
            .join("session.json")
    }

    fn snapshot(path: &Path) -> Option<Vec<u8>> {
        fs::read(path).ok()
    }

    async fn wait_for_state(core: &Core, run_id: u64, expected: crate::run::RunState) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if core.run_state(run_id).unwrap() == Some(expected) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "run {run_id} never reached {expected:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// A model endpoint that accepts a request and never answers. It keeps an
    /// in-flight Run in `running` so a test can model a process kill.
    struct HangingModel {
        address: String,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        stop: Option<std::sync::mpsc::Sender<()>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl HangingModel {
        fn start() -> Self {
            use std::io::Read;
            use std::net::TcpListener;
            use std::sync::atomic::{AtomicUsize, Ordering};

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let calls = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&calls);
            let (stop, stopped) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                let mut held = Vec::new();
                loop {
                    if stopped.try_recv().is_ok() {
                        break;
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream
                                .set_read_timeout(Some(std::time::Duration::from_millis(500)))
                                .unwrap();
                            let mut buffer = [0u8; 8192];
                            let _ = stream.read(&mut buffer);
                            counter.fetch_add(1, Ordering::SeqCst);
                            held.push(stream);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
                drop(held);
            });
            Self {
                address,
                calls,
                stop: Some(stop),
                handle: Some(handle),
            }
        }

        fn url(&self) -> String {
            format!("http://{}/chat/completions", self.address)
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Drop for HangingModel {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    /// A loopback model endpoint that answers one chat completion with a fixed
    /// text and counts POSTs. Balance lookups are answered without counting.
    struct AnsweringModel {
        address: String,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        stop: Option<std::sync::mpsc::Sender<()>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl AnsweringModel {
        fn start(answer: &str) -> Self {
            use std::net::TcpListener;
            use std::sync::atomic::AtomicUsize;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let calls = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&calls);
            let answer = answer.to_string();
            let (stop, stopped) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                while stopped.try_recv().is_err() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let counter = Arc::clone(&counter);
                            let answer = answer.clone();
                            std::thread::spawn(move || {
                                answer_connection(stream, &answer, &counter);
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                address,
                calls,
                stop: Some(stop),
                handle: Some(handle),
            }
        }

        fn url(&self) -> String {
            format!("http://{}/chat/completions", self.address)
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Drop for AnsweringModel {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            let _ = std::net::TcpStream::connect(&self.address);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn answer_connection(
        stream: std::net::TcpStream,
        answer: &str,
        calls: &std::sync::atomic::AtomicUsize,
    ) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::atomic::Ordering;

        stream.set_nonblocking(false).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        let mut content_length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).is_err() || header == "\r\n" || header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; content_length];
        let _ = reader.read_exact(&mut body);

        let response = if request_line.starts_with("POST ") {
            calls.fetch_add(1, Ordering::SeqCst);
            json!({
                "choices": [{"message": {"role": "assistant", "content": answer}}],
                "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
            })
        } else {
            json!({"is_available": true, "balance_infos": []})
        };
        let response = serde_json::to_vec(&response).unwrap();
        let _ = write!(
            writer,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        );
        let _ = writer.write_all(&response);
    }

    #[tokio::test]
    async fn headless_duplicate_submit_creates_no_second_run() {
        let dir = temp_dir("duplicate");
        let model = HangingModel::start();
        let core = core_for_url(&dir, &model.url());
        let server = serve(Arc::clone(&core), &dir).await;
        let address = server.local_addr();

        let first_body = r#"{"prompt":"duplicate canary one","idempotency_key":"dup-key-1"}"#;
        let (status, text) = submit_request(address, Some(TOKEN), first_body).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        wait_for_state(&core, 1, crate::run::RunState::Running).await;

        let (status, text) = submit_request(address, Some(TOKEN), first_body).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{text}");
        let duplicate: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(duplicate["run_id"], 1);
        assert_eq!(duplicate["deduplicated"], true);
        assert!(!text.contains("duplicate canary one"));
        assert!(!text.contains(TOKEN));

        // Concurrent duplicates of one fresh key: exactly one admission wins.
        let body = r#"{"prompt":"duplicate canary two","idempotency_key":"dup-key-2"}"#;
        let mut tasks = Vec::new();
        for _ in 0..8 {
            tasks.push(tokio::spawn(submit_request(address, Some(TOKEN), body)));
        }
        let mut created = 0;
        let mut run_ids = std::collections::HashSet::new();
        for task in tasks {
            let (status, text) = task.await.unwrap();
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            match status {
                reqwest::StatusCode::CREATED => created += 1,
                reqwest::StatusCode::OK => assert_eq!(value["deduplicated"], true),
                other => panic!("unexpected duplicate status {other}: {text}"),
            }
            run_ids.insert(value["run_id"].as_u64().unwrap());
        }
        assert_eq!(created, 1, "exactly one concurrent submit may create a Run");
        assert_eq!(run_ids.len(), 1, "every duplicate must name one Run");

        let runs = read_runs(&dir);
        assert_eq!(runs["runs"].as_array().unwrap().len(), 2);
        assert_eq!(runs["next_run_id"], 3);

        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_storage_failure_returns_503_and_creates_nothing() {
        // A corrupt dedupe index fails the submit closed before any Run write.
        let dir = temp_dir("storage-dedupe");
        fs::create_dir_all(dir.join("memory")).unwrap();
        let corrupt = "{ not json";
        fs::write(dir.join("memory/idempotency.json"), corrupt).unwrap();
        let core = core_for(&dir);
        let server = serve(Arc::clone(&core), &dir).await;
        let body = r#"{"prompt":"never accepted","idempotency_key":"storage-key-a"}"#;
        let (status, text) = submit_request(server.local_addr(), Some(TOKEN), body).await;
        assert_eq!(status, reqwest::StatusCode::SERVICE_UNAVAILABLE, "{text}");
        assert!(text.contains("storage_unavailable"));
        assert!(!dir.join("memory/runs.json").exists());
        assert_eq!(
            fs::read_to_string(dir.join("memory/idempotency.json")).unwrap(),
            corrupt,
            "a failed submit must not rewrite the dedupe file"
        );
        drop(server);
        drop(core);
        let _ = fs::remove_dir_all(&dir);

        // A RunStore persist failure releases the reserved key and creates no Run.
        let dir = temp_dir("storage-run");
        let core = core_for(&dir);
        let server = serve(Arc::clone(&core), &dir).await;
        fs::create_dir_all(dir.join("memory/runs.json")).unwrap();
        let key = "storage-key-b";
        let body = format!(r#"{{"prompt":"never accepted","idempotency_key":"{key}"}}"#);
        let (status, text) = submit_request(server.local_addr(), Some(TOKEN), &body).await;
        assert_eq!(status, reqwest::StatusCode::SERVICE_UNAVAILABLE, "{text}");
        assert!(text.contains("storage_unavailable"));
        assert!(
            dir.join("memory/runs.json").is_dir(),
            "no Run file may replace the blocked path"
        );
        let entries = read_entries(&dir).expect("the dedupe file still validates");
        assert_eq!(entries["version"], 1);
        assert_eq!(entries["entries"], serde_json::json!([]));
        drop(server);
        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_submit_then_kill_leaves_interrupted_with_zero_model_calls() {
        let dir = temp_dir("kill");
        let model = HangingModel::start();
        let core = core_for_url(&dir, &model.url());
        let server = serve(Arc::clone(&core), &dir).await;
        let address = server.local_addr();

        let first = r#"{"prompt":"started before the kill","idempotency_key":"kill-key-1"}"#;
        let (status, text) = submit_request(address, Some(TOKEN), first).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        wait_for_state(&core, 1, crate::run::RunState::Running).await;

        let second = r#"{"prompt":"never started before the kill","idempotency_key":"kill-key-2"}"#;
        let (status, text) = submit_request(address, Some(TOKEN), second).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        wait_for_state(&core, 2, crate::run::RunState::Queued).await;
        assert_eq!(model.calls(), 1, "only the running Run reached the model");

        // Kill: abort in-flight work without settling, then drop the process
        // state. The restarted Core must recover from runs.json alone.
        core.abandon_active_runs_for_test();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(server);
        drop(core);

        let restarted = core_for_url(&dir, &model.url());
        assert_eq!(
            restarted.run_state(1).unwrap(),
            Some(crate::run::RunState::Interrupted)
        );
        assert_eq!(
            restarted.run_state(2).unwrap(),
            Some(crate::run::RunState::QueuedPaused)
        );
        assert_eq!(restarted.active_handles(), 0);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(
            model.calls(),
            1,
            "recovery must perform zero model calls before an owner resume"
        );
        assert_eq!(
            restarted.run_state(1).unwrap(),
            Some(crate::run::RunState::Interrupted)
        );
        assert_eq!(
            restarted.run_state(2).unwrap(),
            Some(crate::run::RunState::QueuedPaused)
        );

        drop(restarted);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_submitted_prompt_never_logged() {
        let dir = temp_dir("log");
        let server = serve(core_for(&dir), &dir).await;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let probe = format!("headless_log_probe {nanos}");
        let prompt = format!("submitted-prompt-canary-{nanos}");
        let key = format!("submitted-key-canary-{nanos}");
        let body = serde_json::json!({
            "prompt": prompt,
            "idempotency_key": key,
        })
        .to_string();

        fs::create_dir_all("logs").unwrap();
        logging::append(&probe);
        let (status, text) = submit_request(server.local_addr(), Some(TOKEN), &body).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        assert!(
            !text.contains(&prompt),
            "responses must not echo the prompt"
        );

        let log = fs::read_to_string("logs/agent.log").unwrap();
        assert!(
            log.contains(&probe),
            "the submit path must write its own log line"
        );
        assert!(
            !log.contains(&prompt),
            "the submitted prompt reached the log"
        );
        assert!(!log.contains(&key), "the idempotency key reached the log");
        assert!(!log.contains(TOKEN), "the token reached the log");
        assert!(!log.contains(&body), "the raw request body reached the log");

        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_read_returns_stored_answer_with_zero_model_calls() {
        let dir = temp_dir("read-result");
        let agent = "g3-read-a";
        let answer = "the stored headless answer";
        let model = AnsweringModel::start(answer);
        let core = core_for_agent_url(&dir, agent, &model.url());
        let server = serve(Arc::clone(&core), &dir).await;
        let address = server.local_addr();

        let body = format!(
            r#"{{"agent_id":"{agent}","prompt":"produce the stored answer","idempotency_key":"read-key-1"}}"#
        );
        let (status, text) = submit_request(address, Some(TOKEN), &body).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        wait_for_state(&core, 1, RunState::Completed).await;

        let calls_before = model.calls();
        let usage_before = crate::usage::today_for_agent(agent);
        let runs_before = snapshot(&runs_path(&dir));
        let session_before = snapshot(&session_path(&dir, agent));
        assert!(
            !dir.join("memory/deliveries.json").exists(),
            "a read must not create a delivery record"
        );

        let (status, text) = status_for(
            reqwest::Method::GET,
            address,
            "/v1/runs/1",
            Some(TOKEN),
            String::new(),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{text}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], SCHEMA);
        assert_eq!(value["run_id"], 1);
        assert_eq!(value["agent_id"], agent);
        assert_eq!(value["state"], "completed");
        assert_eq!(value["result"], answer);
        assert_eq!(value["created_at"], "unknown");
        assert_eq!(value["updated_at"], "unknown");
        assert!(!text.contains("produce the stored answer"));

        assert_eq!(
            model.calls(),
            calls_before,
            "a read must make zero model calls"
        );
        assert_eq!(
            crate::usage::today_for_agent(agent),
            usage_before,
            "a read must not change usage counters"
        );
        assert_eq!(
            snapshot(&runs_path(&dir)),
            runs_before,
            "a read must not write runs.json"
        );
        assert_eq!(
            snapshot(&session_path(&dir, agent)),
            session_before,
            "a read must not write session/v1"
        );
        assert!(!dir.join("memory/deliveries.json").exists());

        drop(server);
        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_read_omits_result_for_unfinished_work() {
        let dir = temp_dir("read-unfinished");
        let agent = "g3-read-d";
        let model = HangingModel::start();
        let core = core_for_agent_url(&dir, agent, &model.url());
        let server = serve(Arc::clone(&core), &dir).await;
        let address = server.local_addr();

        let body = format!(
            r#"{{"agent_id":"{agent}","prompt":"never finishes","idempotency_key":"read-key-4"}}"#
        );
        let (status, text) = submit_request(address, Some(TOKEN), &body).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        wait_for_state(&core, 1, RunState::Running).await;

        // The Run is persisted as `running` before the model request arrives, so
        // wait until the hanging request is observed before snapshotting counts.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while model.calls() == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the model request never arrived"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let calls_before = model.calls();
        let usage_before = crate::usage::today_for_agent(agent);
        let runs_before = snapshot(&runs_path(&dir));

        let (status, text) = status_for(
            reqwest::Method::GET,
            address,
            "/v1/runs/1",
            Some(TOKEN),
            String::new(),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{text}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["state"], "running");
        assert!(
            value.get("result").is_none(),
            "unfinished work must not expose a result: {text}"
        );

        assert_eq!(model.calls(), calls_before);
        assert_eq!(
            crate::usage::today_for_agent(agent),
            usage_before,
            "a read must not change usage counters"
        );
        assert_eq!(snapshot(&runs_path(&dir)), runs_before);
        assert!(!dir.join("memory/deliveries.json").exists());

        core.abandon_active_runs_for_test();
        drop(server);
        drop(core);
        drop(model);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_read_evidence_reports_unknown_without_estimating() {
        let dir = temp_dir("read-evidence");
        let agent = "g3-read-b";
        let answer = format!(
            "evidence answer canary {}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let model = AnsweringModel::start(&answer);
        let core = core_for_agent_url(&dir, agent, &model.url());
        let server = serve(Arc::clone(&core), &dir).await;
        let address = server.local_addr();

        let body = format!(
            r#"{{"agent_id":"{agent}","prompt":"evidence prompt canary","idempotency_key":"read-key-2"}}"#
        );
        let (status, text) = submit_request(address, Some(TOKEN), &body).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        wait_for_state(&core, 1, RunState::Completed).await;

        let calls_before = model.calls();
        let usage_before = crate::usage::today_for_agent(agent);
        let runs_before = snapshot(&runs_path(&dir));

        let (status, text) = status_for(
            reqwest::Method::GET,
            address,
            "/v1/runs/1/evidence",
            Some(TOKEN),
            String::new(),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{text}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], SCHEMA);
        assert_eq!(value["run_id"], 1);
        assert_eq!(value["state"], "completed");
        assert_eq!(value["exit_code"], "unknown");
        assert!(
            value["exit_code"].is_string(),
            "an unknown exit code must never become a number"
        );
        assert_eq!(value["artifact_hashes"], serde_json::json!([]));
        assert_eq!(value["delivery"]["id"], "unknown");
        assert_eq!(value["delivery"]["state"], "unknown");
        assert_eq!(value["delivery"]["message_id"], "unknown");
        assert_eq!(value["usage"]["model_calls"], "unknown");
        assert!(
            value["usage"]["model_calls"].is_string(),
            "a model-call count must never be estimated"
        );
        assert_eq!(value["usage"]["cost_known"], false);
        assert!(
            !text.contains(&answer),
            "evidence must not reuse the stored answer as proof"
        );
        assert!(!text.contains("evidence prompt canary"));

        assert_eq!(model.calls(), calls_before);
        assert_eq!(
            crate::usage::today_for_agent(agent),
            usage_before,
            "a read must not change usage counters"
        );
        assert_eq!(snapshot(&runs_path(&dir)), runs_before);
        assert!(!dir.join("memory/deliveries.json").exists());

        drop(server);
        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_read_does_not_change_usage_counters() {
        let dir = temp_dir("read-usage");
        let agent = "g3-read-c";
        let model = AnsweringModel::start("usage-stable answer");
        let core = core_for_agent_url(&dir, agent, &model.url());
        let server = serve(Arc::clone(&core), &dir).await;
        let address = server.local_addr();

        let body = format!(
            r#"{{"agent_id":"{agent}","prompt":"count usage once","idempotency_key":"read-key-3"}}"#
        );
        let (status, text) = submit_request(address, Some(TOKEN), &body).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{text}");
        wait_for_state(&core, 1, RunState::Completed).await;

        let calls_before = model.calls();
        let usage_before = crate::usage::today_for_agent(agent);
        assert!(
            usage_before.total > 0,
            "the executed Run must have recorded usage before the read"
        );
        let runs_before = snapshot(&runs_path(&dir));

        for _ in 0..2 {
            let (status, _) = status_for(
                reqwest::Method::GET,
                address,
                "/v1/runs/1",
                Some(TOKEN),
                String::new(),
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK);
            let (status, _) = status_for(
                reqwest::Method::GET,
                address,
                "/v1/runs/1/evidence",
                Some(TOKEN),
                String::new(),
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK);
        }

        assert_eq!(
            crate::usage::today_for_agent(agent),
            usage_before,
            "reads must leave every usage counter unchanged"
        );
        assert_eq!(
            model.calls(),
            calls_before,
            "reads must make zero model calls"
        );
        assert_eq!(
            snapshot(&runs_path(&dir)),
            runs_before,
            "reads must not write runs.json"
        );

        drop(server);
        drop(core);
        let _ = fs::remove_dir_all(&dir);
    }
}
