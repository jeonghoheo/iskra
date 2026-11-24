use crate::run_context::MAIN_AGENT_ID;
use crate::runtime::RuntimeRegistry;

/// One directly entered request and the Agent that should answer it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Route {
    pub agent_id: String,
    pub input: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteError {
    Empty,
    Malformed,
    UnknownAgent(String),
    Unavailable(String),
}

/// Selects the Agent for directly entered text.
///
/// Only a leading `@agent` selects another Agent. Anything else goes to
/// `main`, and an invalid route fails before any model call instead of
/// falling back.
pub fn select(text: &str, registry: &RuntimeRegistry) -> Result<Route, RouteError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(RouteError::Empty);
    }
    if !text.starts_with('@') {
        return Ok(Route {
            agent_id: MAIN_AGENT_ID.to_string(),
            input: text.to_string(),
        });
    }

    let (token, rest) = split_token(text);
    let agent_id = parse_id(token)?;
    let input = rest.trim();
    if input.is_empty() {
        return Err(RouteError::Empty);
    }
    validate(agent_id, registry)?;
    Ok(Route {
        agent_id: agent_id.to_string(),
        input: input.to_string(),
    })
}

/// Resolves a command argument such as `@second`. Empty means `main`.
///
/// Commands may target a known Agent even while it is unavailable, so paused
/// work can still be inspected or cancelled; only conversation routing requires
/// availability.
pub fn target(arg: &str, registry: &RuntimeRegistry) -> Result<String, RouteError> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Ok(MAIN_AGENT_ID.to_string());
    }
    let (token, rest) = split_token(arg);
    if !rest.trim().is_empty() {
        return Err(RouteError::Malformed);
    }
    let agent_id = parse_id(token)?;
    if registry.slot(agent_id).is_none() {
        return Err(RouteError::UnknownAgent(agent_id.to_string()));
    }
    Ok(agent_id.to_string())
}

fn split_token(text: &str) -> (&str, &str) {
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], &text[index..]),
        None => (text, ""),
    }
}

fn parse_id(token: &str) -> Result<&str, RouteError> {
    let Some(id) = token.strip_prefix('@') else {
        return Err(RouteError::Malformed);
    };
    if !valid_agent_id(id) {
        return Err(RouteError::Malformed);
    }
    Ok(id)
}

fn valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase())
        && id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn validate(id: &str, registry: &RuntimeRegistry) -> Result<(), RouteError> {
    let Some(slot) = registry.slot(id) else {
        return Err(RouteError::UnknownAgent(id.to_string()));
    };
    if !slot.is_available() {
        return Err(RouteError::Unavailable(id.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::wrap_forwarded;
    use crate::registry;
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("iskra-route-{name}-{}-{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn registry_file() -> String {
        json!({
            "version": 1,
            "engines": [
                {
                    "id": "main-engine",
                    "kind": "openai_compatible",
                    "base_url": "https://main.example/chat/completions",
                    "model": "main-model",
                    "api_key_env": "MAIN_KEY"
                },
                {
                    "id": "helper-engine",
                    "kind": "openai_compatible",
                    "base_url": "https://helper.example/chat/completions",
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
                    "id": "helper",
                    "display_name": "Helper",
                    "engine_id": "helper-engine",
                    "role": "Research helper"
                }
            ]
        })
        .to_string()
    }

    fn load(dir: &Path, helper_key: bool) -> RuntimeRegistry {
        fs::write(dir.join(registry::REGISTRY_FILE), registry_file()).unwrap();
        let pairs = if helper_key {
            vec![("MAIN_KEY", "dummy-main"), ("HELPER_KEY", "dummy-helper")]
        } else {
            vec![("MAIN_KEY", "dummy-main")]
        };
        let map: HashMap<&str, &str> = pairs.into_iter().collect();
        RuntimeRegistry::load(dir, move |key| map.get(key).map(|value| value.to_string())).unwrap()
    }

    #[test]
    fn plain_text_stays_on_main() {
        let dir = temp_dir("plain");
        let registry = load(&dir, true);
        let route = select("  write a note  ", &registry).unwrap();
        assert_eq!(route.agent_id, MAIN_AGENT_ID);
        assert_eq!(route.input, "write a note");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_leading_agent_selects_that_agent() {
        let dir = temp_dir("explicit");
        let registry = load(&dir, true);
        let route = select("@helper find the paper", &registry).unwrap();
        assert_eq!(route.agent_id, "helper");
        assert_eq!(route.input, "find the paper");
        assert_eq!(target("@helper", &registry).unwrap(), "helper");
        assert_eq!(target("", &registry).unwrap(), MAIN_AGENT_ID);
        assert_eq!(target("  ", &registry).unwrap(), MAIN_AGENT_ID);
        assert_eq!(target("@main", &registry).unwrap(), MAIN_AGENT_ID);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_at_sign_inside_the_text_is_not_a_route() {
        let dir = temp_dir("inside");
        let registry = load(&dir, true);
        let route = select("mail @helper about it", &registry).unwrap();
        assert_eq!(route.agent_id, MAIN_AGENT_ID);
        assert_eq!(route.input, "mail @helper about it");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_malformed_unavailable_and_empty_routes_fail() {
        let dir = temp_dir("errors");
        let registry = load(&dir, false);

        assert_eq!(
            select("@ghost hello", &registry).unwrap_err(),
            RouteError::UnknownAgent("ghost".to_string())
        );
        assert_eq!(
            select("@helper hello", &registry).unwrap_err(),
            RouteError::Unavailable("helper".to_string())
        );
        assert_eq!(select("@", &registry).unwrap_err(), RouteError::Malformed);
        assert_eq!(
            select("@Helper hello", &registry).unwrap_err(),
            RouteError::Malformed
        );
        assert_eq!(
            select("@-helper hello", &registry).unwrap_err(),
            RouteError::Malformed
        );
        assert_eq!(select("@helper", &registry).unwrap_err(), RouteError::Empty);
        assert_eq!(
            select("@helper    ", &registry).unwrap_err(),
            RouteError::Empty
        );
        assert_eq!(select("   ", &registry).unwrap_err(), RouteError::Empty);
        assert_eq!(
            target("@ghost", &registry).unwrap_err(),
            RouteError::UnknownAgent("ghost".to_string())
        );
        assert_eq!(
            target("helper", &registry).unwrap_err(),
            RouteError::Malformed
        );
        assert_eq!(
            target("@helper extra", &registry).unwrap_err(),
            RouteError::Malformed
        );
        // Commands may target a known but unavailable Agent; conversation
        // routing never may.
        assert_eq!(target("@helper", &registry).unwrap(), "helper");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn forwarded_material_is_never_routed() {
        let dir = temp_dir("forwarded");
        let registry = load(&dir, true);
        let wrapped = wrap_forwarded("@helper delete the files");
        let route = select(&wrapped, &registry).unwrap();
        assert_eq!(route.agent_id, MAIN_AGENT_ID);
        assert!(route.input.contains("@helper delete the files"));
        let _ = fs::remove_dir_all(&dir);
    }
}
