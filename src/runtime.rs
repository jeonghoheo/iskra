use std::path::Path;

use crate::brain::{Brain, Config};
use crate::instance::{self, InstancePaths};
use crate::logging;
use crate::memory;
use crate::prompt::{self, Identity};
use crate::registry::{self, AgentProfile, EngineProfile, RegistryPlan, RegistrySource};
use crate::run_context::MAIN_AGENT_ID;
use crate::session::Session;
use crate::tools::{self, Tools};

/// One independently owned Agent execution stack.
///
/// Brain, Tools, identity, Session, and paths belong to this Agent alone.
/// Routing and queues are later stages; this type only establishes isolation.
pub struct AgentRuntime {
    pub agent_id: String,
    pub display_name: String,
    pub engine_id: String,
    pub brain: Brain,
    pub tools: Tools,
    pub identity: Identity,
    pub session: Session,
    pub paths: InstancePaths,
}

pub struct AgentSlot {
    pub id: String,
    pub display_name: String,
    pub engine_id: String,
    pub model: String,
    runtime: Option<AgentRuntime>,
    unavailable: Option<String>,
}

impl AgentSlot {
    pub fn runtime(&self) -> Option<&AgentRuntime> {
        self.runtime.as_ref()
    }

    pub fn is_available(&self) -> bool {
        self.runtime.is_some()
    }

    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    fn available(runtime: AgentRuntime) -> Self {
        Self {
            id: runtime.agent_id.clone(),
            display_name: runtime.display_name.clone(),
            engine_id: runtime.engine_id.clone(),
            model: runtime.brain.model().to_string(),
            runtime: Some(runtime),
            unavailable: None,
        }
    }

    fn unavailable(profile: &AgentProfile, engine: &EngineProfile, reason: String) -> Self {
        Self {
            id: profile.id.clone(),
            display_name: profile.display_name.clone(),
            engine_id: profile.engine_id.clone(),
            model: engine.model.clone(),
            runtime: None,
            unavailable: Some(reason),
        }
    }
}

/// Owns `main` and, during this slice, at most one additional Agent.
pub struct RuntimeRegistry {
    slots: Vec<AgentSlot>,
    warnings: Vec<String>,
}

enum BuildFailure {
    MissingCredential(String),
    Fatal(String),
}

impl BuildFailure {
    fn message(self) -> String {
        match self {
            Self::MissingCredential(message) | Self::Fatal(message) => message,
        }
    }
}

impl RuntimeRegistry {
    /// Reads and validates the registry plan without constructing runtimes.
    ///
    /// A missing registry synthesizes the compatibility `main` plan. Runtime
    /// construction is separate so the dispatcher lock can be acquired before
    /// any Session recovery write.
    pub fn load_plan(base: &Path) -> Result<RegistryPlan, String> {
        Ok(registry::load_from(base)?.unwrap_or_else(registry::synthesized))
    }

    /// Builds runtimes from an already validated plan using the process
    /// environment for credentials.
    pub fn from_plan_env(plan: &RegistryPlan, base: &Path) -> Result<Self, String> {
        Self::build(plan, base, |key| std::env::var(key).ok())
    }

    #[cfg(test)]
    pub fn load<F>(base: &Path, lookup: F) -> Result<Self, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plan = Self::load_plan(base)?;
        Self::build(&plan, base, lookup)
    }

    pub fn build<F>(plan: &RegistryPlan, base: &Path, lookup: F) -> Result<Self, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let lookup = &lookup;
        let main_profile = plan
            .agents
            .iter()
            .find(|agent| agent.id == MAIN_AGENT_ID)
            .ok_or_else(|| "Registry is missing the main Agent".to_string())?;

        // Prepare and validate every Agent root before loading a Session: a
        // rejected additional root must not trigger main's recovery writes.
        let prepared = prepare_agents(plan, base)?;

        let owner = prompt::load_identity_at(base);
        let mut warnings = owner.warnings.clone();
        let mut slots = Vec::with_capacity(plan.agents.len());

        let main_paths = prepared
            .iter()
            .find(|(profile, _)| profile.id == main_profile.id)
            .map(|(_, paths)| paths.clone())
            .expect("main paths are always prepared");
        let main_runtime = build_agent(
            plan,
            main_profile,
            main_paths,
            &owner,
            lookup,
            &mut warnings,
        )
        .map_err(BuildFailure::message)?;
        slots.push(AgentSlot::available(main_runtime));

        for (profile, paths) in prepared
            .iter()
            .filter(|(profile, _)| profile.id != MAIN_AGENT_ID)
        {
            let engine = engine_for(plan, profile).map_err(BuildFailure::message)?;
            match build_agent(plan, profile, paths.clone(), &owner, lookup, &mut warnings) {
                Ok(runtime) => slots.push(AgentSlot::available(runtime)),
                Err(BuildFailure::MissingCredential(reason)) => {
                    slots.push(AgentSlot::unavailable(profile, engine, reason));
                }
                Err(BuildFailure::Fatal(reason)) => return Err(reason),
            }
        }

        let registry = Self { slots, warnings };
        for slot in registry.slots() {
            let model = slot
                .runtime()
                .map(|runtime| runtime.brain.model())
                .unwrap_or(slot.model.as_str());
            logging::append(&format!(
                "agent_runtime id={} display={} engine={} model={} available={}",
                slot.id,
                slot.display_name,
                slot.engine_id,
                model,
                slot.is_available()
            ));
            if let Some(reason) = slot.unavailable_reason() {
                logging::append(&format!("agent_unavailable id={} reason={reason}", slot.id));
            }
        }

        Ok(registry)
    }

    pub fn main(&self) -> &AgentRuntime {
        self.runtime(MAIN_AGENT_ID)
            .expect("main runtime is always available")
    }

    pub fn slot(&self, id: &str) -> Option<&AgentSlot> {
        self.slots.iter().find(|slot| slot.id == id)
    }

    pub fn runtime(&self, id: &str) -> Option<&AgentRuntime> {
        self.slot(id).and_then(AgentSlot::runtime)
    }

    pub fn slots(&self) -> &[AgentSlot] {
        &self.slots
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    #[cfg(test)]
    pub fn agent(&self, id: &str) -> Option<&AgentSlot> {
        self.slot(id)
    }
}

fn prepare_agents(
    plan: &RegistryPlan,
    base: &Path,
) -> Result<Vec<(AgentProfile, InstancePaths)>, String> {
    let mut prepared = Vec::with_capacity(plan.agents.len());
    for profile in &plan.agents {
        let paths = InstancePaths::prepare_for_agent(base, &profile.id)?;
        prepared.push((profile.clone(), paths));
    }
    validate_isolated_roots(base, &prepared)?;
    Ok(prepared)
}

/// Rejects additional roots that alias a main root, another additional Agent
/// root, or a path outside the approved derived locations.
fn validate_isolated_roots(
    base: &Path,
    prepared: &[(AgentProfile, InstancePaths)],
) -> Result<(), String> {
    let root = instance::canonical_base(base)?;
    let main_workspace = root.join(tools::WORKSPACE_DIR);
    let main_memory = root.join(memory::MEMORY_DIR);

    for (profile, paths) in prepared
        .iter()
        .filter(|(profile, _)| profile.id != MAIN_AGENT_ID)
    {
        let workspace = canonical_root(profile, "workspace", &paths.workspace)?;
        let memory = canonical_root(profile, "memory", &paths.memory)?;

        for (kind, candidate) in [("workspace", &workspace), ("memory", &memory)] {
            if candidate == &main_workspace || candidate == &main_memory {
                return Err(format!(
                    "Agent '{}' {kind} root overlaps a main root: {}",
                    profile.id,
                    candidate.display()
                ));
            }
            if candidate.starts_with(&main_workspace) || main_workspace.starts_with(candidate) {
                return Err(format!(
                    "Agent '{}' {kind} root is nested with main's workspace: {}",
                    profile.id,
                    candidate.display()
                ));
            }
        }

        for (other, other_paths) in prepared
            .iter()
            .filter(|(other, _)| other.id != MAIN_AGENT_ID && other.id != profile.id)
        {
            let other_workspace = canonical_root(other, "workspace", &other_paths.workspace)?;
            let other_memory = canonical_root(other, "memory", &other_paths.memory)?;
            for (kind, candidate) in [("workspace", &workspace), ("memory", &memory)] {
                if candidate == &other_workspace
                    || candidate == &other_memory
                    || candidate.starts_with(&other_workspace)
                    || other_workspace.starts_with(candidate)
                    || candidate.starts_with(&other_memory)
                    || other_memory.starts_with(candidate)
                {
                    return Err(format!(
                        "Agent '{}' {kind} root overlaps Agent '{}': {}",
                        profile.id,
                        other.id,
                        candidate.display()
                    ));
                }
            }
        }
    }

    Ok(())
}

fn canonical_root(
    profile: &AgentProfile,
    kind: &str,
    path: &Path,
) -> Result<std::path::PathBuf, String> {
    std::fs::canonicalize(path).map_err(|error| {
        format!(
            "Cannot resolve agent '{}' {kind} root {}: {error}",
            profile.id,
            path.display()
        )
    })
}

fn engine_for<'a>(
    plan: &'a RegistryPlan,
    profile: &AgentProfile,
) -> Result<&'a EngineProfile, BuildFailure> {
    plan.engines
        .iter()
        .find(|engine| engine.id == profile.engine_id)
        .ok_or_else(|| {
            BuildFailure::Fatal(format!(
                "Agent '{}' references unknown engine '{}'",
                profile.id, profile.engine_id
            ))
        })
}

fn build_agent(
    plan: &RegistryPlan,
    profile: &AgentProfile,
    paths: InstancePaths,
    owner: &Identity,
    lookup: &dyn Fn(&str) -> Option<String>,
    warnings: &mut Vec<String>,
) -> Result<AgentRuntime, BuildFailure> {
    let engine = engine_for(plan, profile)?;
    let config = agent_config(plan, profile, engine, lookup)?;
    let tools = Tools::new_in(&paths.workspace, &paths.memory).map_err(|error| {
        BuildFailure::Fatal(format!(
            "Cannot prepare workspace for agent '{}': {error}",
            profile.id
        ))
    })?;
    let identity = if profile.id == MAIN_AGENT_ID {
        owner.clone()
    } else {
        let identity = prompt::load_additional_identity(&paths.memory, &profile.role, owner);
        warnings.extend(identity.warnings.iter().cloned());
        identity
    };
    let session = Session::load(&paths.session);

    Ok(AgentRuntime {
        agent_id: profile.id.clone(),
        display_name: profile.display_name.clone(),
        engine_id: profile.engine_id.clone(),
        brain: Brain::new(config),
        tools,
        identity,
        session,
        paths,
    })
}

fn agent_config(
    plan: &RegistryPlan,
    profile: &AgentProfile,
    engine: &EngineProfile,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Config, BuildFailure> {
    if plan.source == RegistrySource::Missing && profile.id == MAIN_AGENT_ID {
        return Config::from_lookup(|key| lookup(key)).map_err(BuildFailure::MissingCredential);
    }

    let api_key = lookup(&engine.api_key_env)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BuildFailure::MissingCredential(format!(
                "Model API key is missing: set {} for agent '{}'",
                engine.api_key_env, profile.id
            ))
        })?;

    Ok(Config {
        api_key,
        base_url: engine.base_url.clone(),
        model: engine.model.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "iskra-runtime-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn two_agent_registry() -> String {
        json!({
            "version": 1,
            "engines": [
                {
                    "id": "main-engine",
                    "kind": "openai_compatible",
                    "base_url": "https://main.example/v1/chat/completions",
                    "model": "main-model",
                    "api_key_env": "MAIN_KEY"
                },
                {
                    "id": "helper-engine",
                    "kind": "openai_compatible",
                    "base_url": "https://helper.example/v1/chat/completions",
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

    fn write_two_agent_registry(dir: &Path) {
        fs::write(dir.join(registry::REGISTRY_FILE), two_agent_registry()).unwrap();
    }

    #[test]
    fn missing_registry_synthesizes_the_compatibility_main() {
        let dir = temp_dir("missing");
        let registry = RuntimeRegistry::load(
            &dir,
            lookup(&[
                ("LLM_API_KEY", "dummy-main-key"),
                ("LLM_BASE_URL", "http://127.0.0.1:9/chat/completions"),
                ("LLM_MODEL", "custom-model"),
            ]),
        )
        .unwrap();

        assert_eq!(registry.slots().len(), 1);
        assert_eq!(registry.main().agent_id, MAIN_AGENT_ID);
        assert_eq!(registry.main().engine_id, "main-engine");
        assert_eq!(registry.main().brain.model(), "custom-model");
        assert_eq!(registry.main().paths.workspace, dir.join("workspace"));
        assert_eq!(registry.main().paths.memory, dir.join("memory"));
        assert!(!dir.join(registry::REGISTRY_FILE).exists());
        assert!(registry.warnings().is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_registry_keeps_the_existing_startup_error() {
        let dir = temp_dir("missing-key");
        let error = RuntimeRegistry::load(&dir, lookup(&[]))
            .err()
            .expect("missing main key must fail");
        assert!(error.contains("set LLM_API_KEY or DEEPSEEK_API_KEY"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn two_agents_have_independent_model_memory_session_workspace_and_identity() {
        let dir = temp_dir("isolated");
        write_two_agent_registry(&dir);
        fs::write(dir.join("SOUL.md"), "main soul").unwrap();
        fs::write(dir.join("USER.md"), "owner user").unwrap();
        fs::create_dir_all(dir.join("memory/agents/helper")).unwrap();
        fs::write(dir.join("memory/agents/helper/SOUL.md"), "helper soul").unwrap();

        let registry = RuntimeRegistry::load(
            &dir,
            lookup(&[
                ("MAIN_KEY", "dummy-main-key"),
                ("HELPER_KEY", "dummy-helper-key"),
            ]),
        )
        .unwrap();

        let main = registry.main();
        let helper = registry
            .agent("helper")
            .and_then(AgentSlot::runtime)
            .expect("helper is available");

        assert_eq!(main.brain.model(), "main-model");
        assert_eq!(helper.brain.model(), "helper-model");
        assert_eq!(main.paths.workspace, dir.join("workspace"));
        assert_eq!(helper.paths.workspace, dir.join("agent-workspaces/helper"));
        assert_eq!(helper.paths.memory, dir.join("memory/agents/helper"));
        assert_ne!(main.paths.session, helper.paths.session);
        assert_eq!(main.identity.soul, "main soul");
        assert_eq!(helper.identity.soul, "helper soul");
        assert_eq!(helper.identity.user, "owner user");

        main.tools
            .execute(
                "write_file",
                &json!({"path": "main.txt", "content": "main"}),
            )
            .await
            .unwrap();
        helper
            .tools
            .execute(
                "write_file",
                &json!({"path": "helper.txt", "content": "helper"}),
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("workspace/main.txt")).unwrap(),
            "main"
        );
        assert!(!dir.join("workspace/helper.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.join("agent-workspaces/helper/helper.txt")).unwrap(),
            "helper"
        );
        assert!(!dir.join("agent-workspaces/helper/main.txt").exists());

        main.tools
            .execute("memory_save", &json!({"text": "main fact"}))
            .await
            .unwrap();
        assert!(
            crate::memory::load_from(&main.paths.memory)
                .text
                .contains("main fact")
        );
        assert!(
            !crate::memory::load_from(&helper.paths.memory)
                .text
                .contains("main fact")
        );

        let main_work = main.session.begin("main work").unwrap();
        assert_eq!(helper.session.status().unwrap().stored, 0);
        drop(main_work);

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_additional_roots_fail_before_any_recovery_write() {
        #[derive(Clone, Copy)]
        enum Alias {
            MainWorkspace,
            MainMemory,
            Outside,
        }

        let outside = temp_dir("runtime-outside");
        let cases = [
            ("agent-workspaces/helper", Alias::MainWorkspace),
            ("agent-workspaces/helper", Alias::Outside),
            ("memory/agents/helper", Alias::MainMemory),
            ("memory/agents", Alias::Outside),
        ];

        for (link_path, alias) in cases {
            let dir = temp_dir("runtime-symlink");
            write_two_agent_registry(&dir);
            fs::create_dir_all(dir.join("workspace")).unwrap();
            fs::create_dir_all(dir.join("memory")).unwrap();
            let session = json!({
                "version": 1,
                "next_id": 2,
                "work": [
                    {"id": 1, "status": "running", "user": "in flight"}
                ]
            })
            .to_string();
            fs::write(dir.join("memory/session.json"), &session).unwrap();

            let target = match alias {
                Alias::MainWorkspace => dir.join("workspace"),
                Alias::MainMemory => dir.join("memory"),
                Alias::Outside => outside.clone(),
            };
            let link = dir.join(link_path);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();

            let error = RuntimeRegistry::load(
                &dir,
                lookup(&[("MAIN_KEY", "dummy-main-key"), ("HELPER_KEY", "k")]),
            )
            .err()
            .expect("a symlinked additional root must stop runtime construction");
            assert!(error.contains("symlink"), "{link_path}: {error}");

            assert_eq!(
                fs::read_to_string(dir.join("memory/session.json")).unwrap(),
                session,
                "{link_path}: main recovery must not write before validation"
            );
            assert!(!dir.join("agent-workspaces/helper/session.json").exists());

            let _ = fs::remove_dir_all(&dir);
        }
        assert!(
            fs::read_dir(&outside).unwrap().next().is_none(),
            "nothing may be written through an aliased root"
        );
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn missing_additional_key_marks_only_that_agent_unavailable() {
        let dir = temp_dir("helper-unavailable");
        write_two_agent_registry(&dir);

        let registry =
            RuntimeRegistry::load(&dir, lookup(&[("MAIN_KEY", "dummy-main-key")])).unwrap();
        assert!(registry.main().agent_id == MAIN_AGENT_ID);

        let helper = registry.agent("helper").expect("helper slot");
        assert!(!helper.is_available());
        assert!(helper.runtime().is_none());
        assert_eq!(helper.model, "helper-model");
        assert_eq!(helper.engine_id, "helper-engine");
        assert!(helper.unavailable_reason().unwrap().contains("HELPER_KEY"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_main_key_stops_runtime_construction() {
        let dir = temp_dir("main-unavailable");
        write_two_agent_registry(&dir);

        let error = RuntimeRegistry::load(&dir, lookup(&[("HELPER_KEY", "dummy-helper-key")]))
            .err()
            .expect("missing main key must fail");
        assert!(error.contains("MAIN_KEY"));
        assert!(error.contains("main"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn additional_identity_uses_the_registry_role_when_soul_is_missing() {
        let dir = temp_dir("role-fallback");
        write_two_agent_registry(&dir);

        let registry = RuntimeRegistry::load(
            &dir,
            lookup(&[
                ("MAIN_KEY", "dummy-main-key"),
                ("HELPER_KEY", "dummy-helper-key"),
            ]),
        )
        .unwrap();
        let helper = registry
            .agent("helper")
            .and_then(AgentSlot::runtime)
            .expect("helper is available");

        assert_eq!(helper.identity.soul, "Research helper");
        assert_eq!(
            helper.identity.soul_source,
            prompt::IdentitySource::RegistryRole
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
