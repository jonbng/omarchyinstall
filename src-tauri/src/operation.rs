use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Serializes operations that inspect or mutate installer-owned disk state.
#[derive(Clone, Default)]
pub struct OperationGate {
    mutex: Arc<Mutex<()>>,
    cancellation_requested: Arc<AtomicBool>,
}

impl OperationGate {
    pub async fn lock(&self) -> OwnedMutexGuard<()> {
        self.mutex.clone().lock_owned().await
    }

    pub fn reset_cancellation(&self) {
        self.cancellation_requested.store(false, Ordering::SeqCst);
    }

    pub fn request_cancellation(&self) {
        self.cancellation_requested.store(true, Ordering::SeqCst);
    }

    pub fn cancellation_requested(&self) -> bool {
        self.cancellation_requested.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn serializes_operations() {
        tauri::async_runtime::block_on(async {
            let gate = OperationGate::default();
            let first = gate.lock().await;
            let entered = Arc::new(AtomicBool::new(false));
            let entered_task = entered.clone();
            let waiting_gate = gate.clone();
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let task = tauri::async_runtime::spawn(async move {
                let _ = started_tx.send(());
                let _guard = waiting_gate.lock().await;
                entered_task.store(true, Ordering::SeqCst);
            });

            started_rx.await.unwrap();
            tokio::task::yield_now().await;
            assert!(!entered.load(Ordering::SeqCst));
            drop(first);
            task.await.unwrap();
            assert!(entered.load(Ordering::SeqCst));
        });
    }

    #[test]
    fn cancellation_is_shared_and_resettable() {
        let gate = OperationGate::default();
        let observer = gate.clone();
        gate.request_cancellation();
        assert!(observer.cancellation_requested());
        observer.reset_cancellation();
        assert!(!gate.cancellation_requested());
    }
}
