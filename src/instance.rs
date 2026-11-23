use std::path::PathBuf;

use crate::memory::MEMORY_DIR;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
