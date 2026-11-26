use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use serde::Deserialize;

use crate::brain::{DEFAULT_BASE_URL, DEFAULT_MODEL};
use crate::run_context::MAIN_AGENT_ID;

pub const REGISTRY_FILE: &str = "iskra.json";
pub const VERSION: u32 = 1;
pub const MAX_AGENTS: usize = 2;
pub const DEFAULT_HEADLESS_BIND: &str = "127.0.0.1:8787";
const MAX_AGENT_ID_CHARS: usize = 32;
const MAX_ENGINE_ID_CHARS: usize = 64;
const MAX_ENV_NAME_CHARS: usize = 128;
const MAX_DISPLAY_CHARS: usize = 80;
const MAX_ROLE_CHARS: usize = 2_000;
const SUPPORTED_ENGINE_KIND: &str = "openai_compatible";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineKind {
    OpenAiCompatible,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineProfile {
    pub id: String,
    pub kind: EngineKind,
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProfile {
    pub id: String,
    pub display_name: String,
    pub engine_id: String,
    pub role: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrySource {
    Missing,
    File,
}

/// Optional owner configuration for the `headless/v1` HTTP surface.
///
/// The bearer token itself never lives in the registry: `token_env` names the
/// environment variable the server reads at startup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeadlessProfile {
    pub bind: String,
    pub token_env: String,
    pub allow_non_loopback: bool,
}

#[derive(Clone, Debug)]
pub struct RegistryPlan {
    pub source: RegistrySource,
    pub engines: Vec<EngineProfile>,
    pub agents: Vec<AgentProfile>,
    pub headless: Option<HeadlessProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistry {
    version: u32,
    engines: Vec<RawEngine>,
    agents: Vec<RawAgent>,
    #[serde(default)]
    headless: Option<RawHeadless>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHeadless {
    #[serde(default)]
    bind: Option<String>,
    token_env: String,
    #[serde(default)]
    allow_non_loopback: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEngine {
    id: String,
    kind: String,
    base_url: String,
    model: String,
    api_key_env: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAgent {
    id: String,
    display_name: String,
    engine_id: String,
    role: String,
}

/// Reads `iskra.json` from the instance root. A missing file is not an error:
/// the caller synthesizes the compatibility `main` Agent instead.
pub fn load_from(base: &Path) -> Result<Option<RegistryPlan>, String> {
    let path = base.join(REGISTRY_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot read {}: {error}", path.display())),
    };
    let text =
        String::from_utf8(bytes).map_err(|_| format!("{} is not valid UTF-8", path.display()))?;
    parse(&text)
        .map(Some)
        .map_err(|error| format!("Invalid {}: {error}", path.display()))
}

pub fn parse(text: &str) -> Result<RegistryPlan, String> {
    let raw: RawRegistry = serde_json::from_str(text)
        .map_err(|error| format!("Cannot parse registry JSON: {error}"))?;
    if raw.version != VERSION {
        return Err(format!(
            "Unsupported registry version: {} (expected {VERSION})",
            raw.version
        ));
    }

    let mut engines = Vec::with_capacity(raw.engines.len());
    for engine in raw.engines {
        engines.push(validate_engine(engine, &engines)?);
    }

    let mut agents = Vec::with_capacity(raw.agents.len());
    for agent in raw.agents {
        agents.push(validate_agent(agent)?);
    }
    let headless = raw.headless.map(validate_headless).transpose()?;

    if engines.is_empty() {
        return Err("Registry must contain at least one engine".to_string());
    }
    if agents.is_empty() {
        return Err("Registry must contain an agent with id main".to_string());
    }
    if agents.len() > MAX_AGENTS {
        return Err(format!(
            "Registry may contain at most {MAX_AGENTS} agents (found {})",
            agents.len()
        ));
    }

    let main_count = agents
        .iter()
        .filter(|agent| agent.id == MAIN_AGENT_ID)
        .count();
    if main_count != 1 {
        return Err(format!(
            "Registry must contain exactly one agent with id {MAIN_AGENT_ID} (found {main_count})"
        ));
    }

    for agent in &agents {
        if !engines.iter().any(|engine| engine.id == agent.engine_id) {
            return Err(format!(
                "Agent '{}' references unknown engine '{}'",
                agent.id, agent.engine_id
            ));
        }
    }

    for (index, agent) in agents.iter().enumerate() {
        for other in agents.iter().skip(index + 1) {
            if agent.id == other.id {
                return Err(format!("Duplicate agent id '{}'", agent.id));
            }
            if state_paths(&agent.id) == state_paths(&other.id) {
                return Err(format!(
                    "Agents '{}' and '{}' collide on state paths",
                    agent.id, other.id
                ));
            }
        }
    }

    Ok(RegistryPlan {
        source: RegistrySource::File,
        engines,
        agents,
        headless,
    })
}

/// Compatibility plan used when `iskra.json` is missing. The runtime still
/// reads the values from `LLM_*` / `DEEPSEEK_API_KEY`, preserving the current
/// startup behavior and error text.
pub fn synthesized() -> RegistryPlan {
    RegistryPlan {
        source: RegistrySource::Missing,
        engines: vec![EngineProfile {
            id: "main-engine".to_string(),
            kind: EngineKind::OpenAiCompatible,
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            api_key_env: "LLM_API_KEY".to_string(),
        }],
        agents: vec![AgentProfile {
            id: MAIN_AGENT_ID.to_string(),
            display_name: "Iskra".to_string(),
            engine_id: "main-engine".to_string(),
            role: "Personal assistant".to_string(),
        }],
        headless: None,
    }
}

fn validate_engine(raw: RawEngine, seen: &[EngineProfile]) -> Result<EngineProfile, String> {
    if !valid_engine_id(&raw.id) {
        return Err(format!("Invalid engine id '{}'", raw.id));
    }
    if seen.iter().any(|engine| engine.id == raw.id) {
        return Err(format!("Duplicate engine id '{}'", raw.id));
    }
    if raw.kind != SUPPORTED_ENGINE_KIND {
        return Err(format!(
            "Engine '{}' has unsupported kind '{}' (expected {SUPPORTED_ENGINE_KIND})",
            raw.id, raw.kind
        ));
    }

    let base_url = raw.base_url.trim();
    let url = reqwest::Url::parse(base_url)
        .map_err(|error| format!("Engine '{}' has an invalid base_url: {error}", raw.id))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "Engine '{}' base_url must use http or https",
            raw.id
        ));
    }
    if url.host_str().is_none() {
        return Err(format!("Engine '{}' base_url has no host", raw.id));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "Engine '{}' base_url must not contain credentials",
            raw.id
        ));
    }
    if url.fragment().is_some() {
        return Err(format!(
            "Engine '{}' base_url must not contain a fragment",
            raw.id
        ));
    }

    let model = raw.model.trim();
    if model.is_empty() {
        return Err(format!("Engine '{}' has an empty model", raw.id));
    }
    if !valid_env_name(&raw.api_key_env) {
        return Err(format!(
            "Engine '{}' api_key_env must be an environment-variable name, not a credential",
            raw.id
        ));
    }

    Ok(EngineProfile {
        id: raw.id,
        kind: EngineKind::OpenAiCompatible,
        base_url: base_url.to_string(),
        model: model.to_string(),
        api_key_env: raw.api_key_env,
    })
}

fn validate_agent(raw: RawAgent) -> Result<AgentProfile, String> {
    if !valid_agent_id(&raw.id) {
        return Err(format!(
            "Invalid agent id '{}': use 1-{MAX_AGENT_ID_CHARS} characters, starting with a lowercase ASCII letter, then a-z, 0-9, or '-'",
            raw.id
        ));
    }
    let display_name = raw.display_name.trim();
    if display_name.is_empty() {
        return Err(format!("Agent '{}' has an empty display_name", raw.id));
    }
    if display_name.chars().count() > MAX_DISPLAY_CHARS {
        return Err(format!(
            "Agent '{}' display_name exceeds {MAX_DISPLAY_CHARS} characters",
            raw.id
        ));
    }
    let role = raw.role.trim();
    if role.is_empty() {
        return Err(format!("Agent '{}' has an empty role", raw.id));
    }
    if role.chars().count() > MAX_ROLE_CHARS {
        return Err(format!(
            "Agent '{}' role exceeds {MAX_ROLE_CHARS} characters",
            raw.id
        ));
    }

    Ok(AgentProfile {
        id: raw.id,
        display_name: display_name.to_string(),
        engine_id: raw.engine_id,
        role: role.to_string(),
    })
}

fn validate_headless(raw: RawHeadless) -> Result<HeadlessProfile, String> {
    if !valid_env_name(&raw.token_env) {
        return Err(
            "headless token_env must be an environment-variable name, not a credential".to_string(),
        );
    }
    let bind = raw
        .bind
        .as_deref()
        .unwrap_or(DEFAULT_HEADLESS_BIND)
        .trim()
        .to_string();
    let address: SocketAddr = bind
        .parse()
        .map_err(|error| format!("headless bind '{bind}' is not a socket address: {error}"))?;
    if !address.ip().is_loopback() && !raw.allow_non_loopback {
        return Err(format!(
            "headless bind {address} is not loopback; set headless.allow_non_loopback=true to confirm an owner-configured non-loopback bind"
        ));
    }
    Ok(HeadlessProfile {
        bind: address.to_string(),
        token_env: raw.token_env,
        allow_non_loopback: raw.allow_non_loopback,
    })
}

fn valid_agent_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase())
        && id.chars().count() <= MAX_AGENT_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_engine_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ENGINE_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && name.chars().count() <= MAX_ENV_NAME_CHARS
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn state_paths(agent_id: &str) -> (String, String) {
    if agent_id == MAIN_AGENT_ID {
        ("workspace".to_string(), "memory".to_string())
    } else {
        (
            format!("agent-workspaces/{agent_id}"),
            format!("memory/agents/{agent_id}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "iskra-registry-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn valid_registry() -> &'static str {
        r#"{
            "version": 1,
            "engines": [
                {
                    "id": "main-engine",
                    "kind": "openai_compatible",
                    "base_url": "https://api.deepseek.com/chat/completions",
                    "model": "deepseek-flash",
                    "api_key_env": "LLM_API_KEY"
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
                    "id": "helper",
                    "display_name": "Helper",
                    "engine_id": "main-engine",
                    "role": "Research helper"
                }
            ]
        }"#
    }

    #[test]
    fn missing_file_is_not_an_error_and_is_not_created() {
        let dir = temp_dir("missing");
        assert!(load_from(&dir).unwrap().is_none());
        assert!(!dir.join(REGISTRY_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_the_two_agent_registry() {
        let plan = parse(valid_registry()).unwrap();
        assert_eq!(plan.source, RegistrySource::File);
        assert_eq!(plan.engines.len(), 1);
        assert_eq!(plan.agents.len(), 2);
        assert_eq!(plan.agents[0].id, MAIN_AGENT_ID);
        assert_eq!(plan.agents[1].engine_id, "main-engine");
    }

    #[test]
    fn the_public_example_parses() {
        let plan = parse(include_str!("../iskra.example.json")).unwrap();
        assert_eq!(plan.agents.len(), 1);
        assert_eq!(plan.agents[0].id, MAIN_AGENT_ID);
    }

    #[test]
    fn synthesized_plan_uses_the_compatibility_defaults() {
        let plan = synthesized();
        assert_eq!(plan.source, RegistrySource::Missing);
        assert_eq!(plan.agents[0].id, MAIN_AGENT_ID);
        assert_eq!(plan.engines[0].model, DEFAULT_MODEL);
        assert_eq!(plan.engines[0].base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn rejects_unknown_versions_and_fields() {
        let version = valid_registry().replace("\"version\": 1", "\"version\": 2");
        assert!(
            parse(&version)
                .unwrap_err()
                .contains("Unsupported registry version")
        );

        let root_field =
            valid_registry().replace("\"version\": 1,", "\"version\": 1, \"extra\": true,");
        assert!(parse(&root_field).unwrap_err().contains("unknown field"));

        let engine_field = valid_registry().replace(
            "\"api_key_env\": \"LLM_API_KEY\"",
            "\"api_key_env\": \"LLM_API_KEY\", \"secret\": \"x\"",
        );
        assert!(parse(&engine_field).unwrap_err().contains("unknown field"));

        let agent_field = valid_registry().replace(
            "\"role\": \"Personal assistant\"",
            "\"role\": \"Personal assistant\", \"workspace\": \"elsewhere\"",
        );
        assert!(parse(&agent_field).unwrap_err().contains("unknown field"));
    }

    #[test]
    fn rejects_missing_duplicate_and_excess_agents() {
        let missing_main = valid_registry().replace("\"id\": \"main\"", "\"id\": \"other\"");
        assert!(
            parse(&missing_main)
                .unwrap_err()
                .contains("exactly one agent with id main")
        );

        let duplicate_main = valid_registry().replace("\"id\": \"helper\"", "\"id\": \"main\"");
        assert!(
            parse(&duplicate_main)
                .unwrap_err()
                .contains("exactly one agent with id main")
        );

        let three = valid_registry().replace(
            r#""agents": ["#,
            r#""agents": [
                {
                    "id": "third",
                    "display_name": "Third",
                    "engine_id": "main-engine",
                    "role": "Third role"
                },"#,
        );
        assert!(parse(&three).unwrap_err().contains("at most 2 agents"));
    }

    #[test]
    fn rejects_invalid_agent_ids() {
        for id in [
            "Main",
            "1main",
            "main_agent",
            "",
            "averyveryveryverylongagentidentifier",
        ] {
            let registry =
                valid_registry().replace("\"id\": \"helper\"", &format!("\"id\": \"{id}\""));
            assert!(
                parse(&registry).unwrap_err().contains("Invalid agent id"),
                "id {id:?} was accepted"
            );
        }
    }

    #[test]
    fn rejects_duplicate_ids() {
        let duplicate_engine = valid_registry().replace(
            r#""engines": ["#,
            r#""engines": [
                {
                    "id": "main-engine",
                    "kind": "openai_compatible",
                    "base_url": "https://example.com/v1",
                    "model": "other",
                    "api_key_env": "OTHER_KEY"
                },"#,
        );
        assert!(
            parse(&duplicate_engine)
                .unwrap_err()
                .contains("Duplicate engine id")
        );

        let duplicate_agent = valid_registry().replace("\"id\": \"helper\"", "\"id\": \"main\"");
        assert!(parse(&duplicate_agent).is_err());
    }

    #[test]
    fn rejects_unknown_engine_references_and_kinds() {
        let unknown = valid_registry().replace(
            "\"engine_id\": \"main-engine\"",
            "\"engine_id\": \"missing-engine\"",
        );
        assert!(parse(&unknown).unwrap_err().contains("unknown engine"));

        let kind = valid_registry().replace("openai_compatible", "anthropic");
        assert!(parse(&kind).unwrap_err().contains("unsupported kind"));
    }

    #[test]
    fn rejects_empty_models_and_invalid_urls() {
        let empty_model =
            valid_registry().replace("\"model\": \"deepseek-flash\"", "\"model\": \"  \"");
        assert!(parse(&empty_model).unwrap_err().contains("empty model"));

        for url in [
            "ftp://example.com/chat",
            "https://user:pass@example.com/chat",
            "https://user@example.com/chat",
            "not-a-url",
            "https://example.com/chat#fragment",
        ] {
            let registry =
                valid_registry().replace("https://api.deepseek.com/chat/completions", url);
            assert!(parse(&registry).is_err(), "url {url:?} was accepted");
        }
    }

    #[test]
    fn rejects_credential_like_api_key_env_values() {
        for name in ["sk-live-123", "1KEY", "", "KEY=value", "KEY VALUE"] {
            let registry = valid_registry().replace("\"LLM_API_KEY\"", &format!("\"{name}\""));
            assert!(
                parse(&registry).unwrap_err().contains("api_key_env"),
                "env name {name:?} was accepted"
            );
        }
    }

    #[test]
    fn rejects_empty_display_names_and_roles() {
        let display =
            valid_registry().replace("\"display_name\": \"Helper\"", "\"display_name\": \" \"");
        assert!(parse(&display).unwrap_err().contains("empty display_name"));

        let role = valid_registry().replace("\"role\": \"Research helper\"", "\"role\": \" \"");
        assert!(parse(&role).unwrap_err().contains("empty role"));
    }

    #[test]
    fn invalid_file_preserves_the_original_bytes() {
        let dir = temp_dir("invalid");
        let path = dir.join(REGISTRY_FILE);
        let contents = "{ not json";
        fs::write(&path, contents).unwrap();

        let error = load_from(&dir).unwrap_err();
        assert!(error.contains("Invalid"));
        assert!(error.contains("iskra.json"));
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn headless_defaults_to_loopback_and_requires_a_token_env_name() {
        let text = valid_registry().replace(
            "\"agents\": [",
            "\"headless\": { \"token_env\": \"ISKRA_OWNER_TOKEN\" },\n            \"agents\": [",
        );
        let plan = parse(&text).unwrap();
        let headless = plan.headless.expect("headless configuration");
        assert_eq!(headless.bind, DEFAULT_HEADLESS_BIND);
        assert!(
            headless
                .bind
                .parse::<SocketAddr>()
                .unwrap()
                .ip()
                .is_loopback()
        );
        assert_eq!(headless.token_env, "ISKRA_OWNER_TOKEN");

        let credential = valid_registry().replace(
            "\"agents\": [",
            "\"headless\": { \"token_env\": \"sk-live-secret\" },\n            \"agents\": [",
        );
        assert!(
            parse(&credential)
                .unwrap_err()
                .contains("token_env must be an environment-variable name")
        );
    }

    #[test]
    fn headless_rejects_a_non_loopback_bind_without_confirmation() {
        let text = valid_registry().replace(
            "\"agents\": [",
            "\"headless\": { \"bind\": \"0.0.0.0:8787\", \"token_env\": \"ISKRA_OWNER_TOKEN\" },\n            \"agents\": [",
        );
        assert!(parse(&text).unwrap_err().contains("is not loopback"));

        let confirmed = valid_registry().replace(
            "\"agents\": [",
            "\"headless\": { \"bind\": \"0.0.0.0:8787\", \"token_env\": \"ISKRA_OWNER_TOKEN\", \"allow_non_loopback\": true },\n            \"agents\": [",
        );
        assert_eq!(
            parse(&confirmed).unwrap().headless.unwrap().bind,
            "0.0.0.0:8787"
        );
    }
}
