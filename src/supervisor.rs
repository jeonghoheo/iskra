use std::collections::HashMap;
use std::sync::Mutex;

use tokio::task::JoinHandle;

/// Owns the active run tasks for one instance.
///
/// Admission (which run may start and when) is decided by the core RunStore and
/// its single dispatch lock. This type only holds the resulting handles so the
/// core can cancel them. The store, not this map, is the execution authority.
#[derive(Default)]
pub struct RunSupervisor {
    active: Mutex<HashMap<u64, ActiveRun>>,
}

struct ActiveRun {
    agent_id: String,
    handle: JoinHandle<()>,
}

impl RunSupervisor {
    pub fn insert(&self, run_id: u64, agent_id: &str, handle: JoinHandle<()>) {
        let mut active = self.active.lock().unwrap();
        if handle.is_finished() {
            return;
        }
        active.insert(
            run_id,
            ActiveRun {
                agent_id: agent_id.to_string(),
                handle,
            },
        );
        if active
            .get(&run_id)
            .is_some_and(|run| run.handle.is_finished())
        {
            active.remove(&run_id);
        }
    }

    /// Removes one handle without aborting it. Used by the task when it finishes.
    pub fn remove(&self, run_id: u64) -> Option<JoinHandle<()>> {
        self.active
            .lock()
            .unwrap()
            .remove(&run_id)
            .map(|run| run.handle)
    }

    /// Removes every handle for one Agent so the caller can stop them in order.
    pub fn take_agent(&self, agent_id: &str) -> Vec<JoinHandle<()>> {
        let mut active = self.active.lock().unwrap();
        let ids: Vec<u64> = active
            .iter()
            .filter(|(_, run)| run.agent_id == agent_id)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| active.remove(&id).map(|run| run.handle))
            .collect()
    }

    #[cfg(test)]
    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
    }

    /// Removes every handle without aborting it so a test can model a process
    /// kill: the caller aborts them and no Run settlement is written.
    #[cfg(test)]
    pub fn take_all(&self) -> Vec<JoinHandle<()>> {
        let mut active = self.active.lock().unwrap();
        std::mem::take(&mut *active)
            .into_values()
            .map(|run| run.handle)
            .collect()
    }

    #[cfg(test)]
    pub fn contains(&self, run_id: u64) -> bool {
        self.active.lock().unwrap().contains_key(&run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn holds_and_removes_multiple_runs() {
        let supervisor = RunSupervisor::default();
        supervisor.insert(1, "main", tokio::spawn(std::future::pending::<()>()));
        supervisor.insert(2, "helper", tokio::spawn(std::future::pending::<()>()));

        assert_eq!(supervisor.active_count(), 2);
        assert!(supervisor.contains(1));
        assert!(supervisor.contains(2));

        let removed = supervisor.remove(1).expect("run 1 handle");
        removed.abort();
        assert_eq!(supervisor.active_count(), 1);
        assert!(!supervisor.contains(1));
    }

    #[tokio::test]
    async fn takes_only_the_named_agents_handles() {
        let supervisor = RunSupervisor::default();
        supervisor.insert(1, "main", tokio::spawn(std::future::pending::<()>()));
        supervisor.insert(2, "main", tokio::spawn(std::future::pending::<()>()));
        supervisor.insert(3, "helper", tokio::spawn(std::future::pending::<()>()));

        let main = supervisor.take_agent("main");
        assert_eq!(main.len(), 2);
        for handle in main {
            handle.abort();
        }
        assert_eq!(supervisor.active_count(), 1);
        assert!(supervisor.contains(3));
    }

    #[tokio::test]
    async fn finished_handles_are_removed_once() {
        let supervisor = RunSupervisor::default();
        let handle = tokio::spawn(async {});
        let _ = handle.await;
        supervisor.insert(7, "main", tokio::spawn(async {}));

        assert!(supervisor.remove(7).is_some());
        assert!(supervisor.remove(7).is_none());
    }
}
