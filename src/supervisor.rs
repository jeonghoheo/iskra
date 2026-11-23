use std::future::Future;
use std::sync::Mutex;

use tokio::task::JoinHandle;

/// Owns the active run task for one instance.
///
/// Admission and handle ownership live here so channels cannot start or abort
/// model work directly. Durable queues and multi-run admission are later stages.
#[derive(Default)]
pub struct RunSupervisor {
    active: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Busy;

impl RunSupervisor {
    /// Atomically admits one run: reject while the stored handle is running,
    /// otherwise spawn and store the new handle.
    pub fn start<F>(&self, task: F) -> Result<(), Busy>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut active = self.active.lock().unwrap();
        if active.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return Err(Busy);
        }
        *active = Some(tokio::spawn(task));
        Ok(())
    }

    /// Removes the handle only when the run is still active. A finished task is
    /// cleared and reported as no active run.
    pub fn take_running(&self) -> Option<JoinHandle<()>> {
        let mut active = self.active.lock().unwrap();
        match active.take() {
            Some(handle) if !handle.is_finished() => Some(handle),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn wait_finished(supervisor: &RunSupervisor) {
        for _ in 0..100 {
            if supervisor
                .active
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|handle| handle.is_finished())
            {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("run did not finish");
    }

    #[tokio::test]
    async fn rejects_a_second_start_while_the_first_is_running() {
        let supervisor = RunSupervisor::default();
        supervisor.start(std::future::pending::<()>()).unwrap();
        assert_eq!(supervisor.start(async {}), Err(Busy));

        let handle = supervisor.take_running().expect("active run");
        handle.abort();
        supervisor.start(async {}).unwrap();
    }

    #[tokio::test]
    async fn take_running_clears_the_slot_exactly_once() {
        let supervisor = RunSupervisor::default();
        supervisor.start(std::future::pending::<()>()).unwrap();

        assert!(supervisor.take_running().is_some());
        assert!(supervisor.take_running().is_none());
    }

    #[tokio::test]
    async fn finished_runs_are_not_treated_as_active() {
        let supervisor = RunSupervisor::default();
        supervisor.start(async {}).unwrap();
        wait_finished(&supervisor).await;

        assert!(supervisor.take_running().is_none());
        supervisor.start(async {}).unwrap();
    }
}
