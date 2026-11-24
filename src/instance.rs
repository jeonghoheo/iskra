use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::memory::MEMORY_DIR;
use crate::run_context::MAIN_AGENT_ID;
use crate::session::SESSION_FILE;
use crate::tools::WORKSPACE_DIR;

/// Paths owned by one Iskra instance.
///
/// The compatibility `main` instance uses the existing `workspace/` and
/// `memory/` layout. Additional instances pass their own roots instead of
/// relying on process-global constants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstancePaths {
    pub workspace: PathBuf,
    pub memory: PathBuf,
    pub session: PathBuf,
}

impl Default for InstancePaths {
    fn default() -> Self {
        Self::new(WORKSPACE_DIR, MEMORY_DIR)
    }
}

impl InstancePaths {
    pub fn new(workspace: impl Into<PathBuf>, memory: impl Into<PathBuf>) -> Self {
        let memory = memory.into();
        Self {
            workspace: workspace.into(),
            session: memory.join(SESSION_FILE),
            memory,
        }
    }

    /// Derives the state layout for one validated registry Agent.
    ///
    /// `main` keeps the existing root layout. An additional Agent gets
    /// `agent-workspaces/<id>/` and `memory/agents/<id>/`, so the core owns
    /// path derivation and the registry cannot point at arbitrary roots.
    pub fn for_agent(base: &Path, agent_id: &str) -> Self {
        if agent_id == MAIN_AGENT_ID {
            Self::new(base.join(WORKSPACE_DIR), base.join(MEMORY_DIR))
        } else {
            Self::new(
                base.join("agent-workspaces").join(agent_id),
                base.join(MEMORY_DIR).join("agents").join(agent_id),
            )
        }
    }

    /// Derives and prepares the paths for one validated registry Agent.
    ///
    /// `main` is returned unchanged for compatibility. Additional roots are
    /// created one component at a time without following symlinks, then
    /// canonicalized and compared with the approved derived location. A
    /// symlinked component, an existing non-directory, or a canonical target
    /// outside the derived path is refused before any Session recovery write.
    pub fn prepare_for_agent(base: &Path, agent_id: &str) -> Result<Self, String> {
        if agent_id == MAIN_AGENT_ID {
            return Ok(Self::for_agent(base, agent_id));
        }

        let root = canonical_base(base)?;
        prepare_derived_root(
            &root,
            &Path::new("agent-workspaces").join(agent_id),
            agent_id,
            "workspace",
        )?;
        prepare_derived_root(
            &root,
            &Path::new(MEMORY_DIR).join("agents").join(agent_id),
            agent_id,
            "memory",
        )?;
        Ok(Self::for_agent(base, agent_id))
    }
}

/// Resolves the instance root; the empty path means the current directory.
pub fn canonical_base(base: &Path) -> Result<PathBuf, String> {
    let input = if base.as_os_str().is_empty() {
        Path::new(".")
    } else {
        base
    };
    fs::canonicalize(input).map_err(|error| {
        format!(
            "Cannot resolve the instance root {}: {error}",
            input.display()
        )
    })
}

fn prepare_derived_root(
    root: &Path,
    relative: &Path,
    agent_id: &str,
    kind: &str,
) -> Result<(), String> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(format!(
                "Agent '{agent_id}' {kind} root contains an unsupported path component"
            ));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(format!(
                        "Agent '{agent_id}' {kind} root must not be a symlink: {}",
                        current.display()
                    ));
                }
                if !metadata.is_dir() {
                    return Err(format!(
                        "Agent '{agent_id}' {kind} root is not a directory: {}",
                        current.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    format!(
                        "Cannot create agent '{agent_id}' {kind} root {}: {error}",
                        current.display()
                    )
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "Cannot inspect agent '{agent_id}' {kind} root {}: {error}",
                    current.display()
                ));
            }
        }
    }

    let canonical = fs::canonicalize(&current).map_err(|error| {
        format!(
            "Cannot resolve agent '{agent_id}' {kind} root {}: {error}",
            current.display()
        )
    })?;
    let expected = root.join(relative);
    if canonical != expected {
        return Err(format!(
            "Agent '{agent_id}' {kind} root resolves outside its approved location: {} -> {}",
            current.display(),
            canonical.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "iskra-instance-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_keep_the_main_layout() {
        let paths = InstancePaths::default();
        assert_eq!(paths.workspace, PathBuf::from("workspace"));
        assert_eq!(paths.memory, PathBuf::from("memory"));
        assert_eq!(paths.session, PathBuf::from("memory/session.json"));
    }

    #[test]
    fn new_derives_the_session_file_from_the_memory_root() {
        let paths = InstancePaths::new("/tmp/iskra-workspace", "/tmp/iskra-memory");
        assert_eq!(
            paths.session,
            PathBuf::from("/tmp/iskra-memory/session.json")
        );
    }

    #[test]
    fn main_keeps_the_root_layout_and_additional_agents_are_scoped() {
        let base = Path::new("/tmp/iskra-root");
        let main = InstancePaths::for_agent(base, MAIN_AGENT_ID);
        assert_eq!(main.workspace, base.join("workspace"));
        assert_eq!(main.memory, base.join("memory"));
        assert_eq!(main.session, base.join("memory/session.json"));

        let helper = InstancePaths::for_agent(base, "helper");
        assert_eq!(helper.workspace, base.join("agent-workspaces/helper"));
        assert_eq!(helper.memory, base.join("memory/agents/helper"));
        assert_eq!(
            helper.session,
            base.join("memory/agents/helper/session.json")
        );
        assert_ne!(main.workspace, helper.workspace);
        assert_ne!(main.session, helper.session);
    }

    #[test]
    fn preparing_an_additional_agent_creates_only_its_derived_roots() {
        let dir = temp_dir("prepare");
        let paths = InstancePaths::prepare_for_agent(&dir, "helper").unwrap();
        assert_eq!(paths, InstancePaths::for_agent(&dir, "helper"));
        assert!(dir.join("agent-workspaces/helper").is_dir());
        assert!(dir.join("memory/agents/helper").is_dir());
        assert!(!dir.join("workspace").exists());
        assert!(!dir.join("memory/session.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_preparation_is_unchanged_for_compatibility() {
        let dir = temp_dir("prepare-main");
        let paths = InstancePaths::prepare_for_agent(&dir, MAIN_AGENT_ID).unwrap();
        assert_eq!(paths, InstancePaths::for_agent(&dir, MAIN_AGENT_ID));
        assert!(!dir.join("workspace").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_additional_roots_are_refused() {
        let outside = temp_dir("outside");
        let cases = [
            "agent-workspaces/helper",
            "memory/agents/helper",
            "memory/agents",
        ];

        for case in cases {
            let dir = temp_dir("symlink");
            fs::create_dir_all(dir.join("workspace")).unwrap();
            fs::create_dir_all(dir.join("memory")).unwrap();
            let link = dir.join(case);
            if let Some(parent) = link.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            std::os::unix::fs::symlink(&outside, &link).unwrap();

            let error = InstancePaths::prepare_for_agent(&dir, "helper")
                .expect_err("symlinked root must fail");
            assert!(error.contains("symlink"), "{case}: {error}");
            assert!(
                fs::read_dir(&outside).unwrap().next().is_none(),
                "{case}: nothing may be written through the symlink"
            );
            let _ = fs::remove_dir_all(&dir);
        }
        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn a_root_symlink_to_the_sibling_root_is_refused() {
        let dir = temp_dir("alias");
        fs::create_dir_all(dir.join("workspace")).unwrap();
        fs::create_dir_all(dir.join("memory")).unwrap();
        fs::create_dir_all(dir.join("agent-workspaces")).unwrap();
        std::os::unix::fs::symlink(dir.join("workspace"), dir.join("agent-workspaces/helper"))
            .unwrap();

        let error = InstancePaths::prepare_for_agent(&dir, "helper")
            .expect_err("aliasing main workspace must fail");
        assert!(error.contains("symlink"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_file_at_a_derived_root_is_refused() {
        let dir = temp_dir("file");
        fs::create_dir_all(dir.join("agent-workspaces")).unwrap();
        fs::write(dir.join("agent-workspaces/helper"), "not a directory").unwrap();

        let error = InstancePaths::prepare_for_agent(&dir, "helper")
            .expect_err("a file at the derived root must fail");
        assert!(error.contains("not a directory"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }
}
