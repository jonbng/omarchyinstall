use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Serializes operations that inspect or mutate installer-owned disk state.
#[derive(Clone, Default)]
pub struct OperationGate(Arc<Mutex<()>>);

impl OperationGate {
    pub async fn lock(&self) -> OwnedMutexGuard<()> {
        self.0.clone().lock_owned().await
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
}
