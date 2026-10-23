// SPDX-License-Identifier: Apache-2.0
//! Coordinates startup preload and explicit on-device model selection.

use std::sync::Arc;
use tokio::sync::{watch, Mutex, OwnedMutexGuard};
use tokio::task::JoinError;

#[derive(Clone)]
pub struct ModelLoadCoordinator {
    lock: Arc<Mutex<()>>,
    startup_superseded: watch::Sender<bool>,
}

impl Default for ModelLoadCoordinator {
    fn default() -> Self {
        let (startup_superseded, _) = watch::channel(false);
        Self {
            lock: Arc::new(Mutex::new(())),
            startup_superseded,
        }
    }
}

impl ModelLoadCoordinator {
    pub async fn lock(&self) -> OwnedMutexGuard<()> {
        self.lock.clone().lock_owned().await
    }

    /// Run a blocking constructor while retaining exclusive ownership through
    /// publication. The caller publishes the value before releasing the guard.
    #[cfg(test)]
    pub async fn construct<T, E, F>(
        &self,
        build: F,
    ) -> Result<Result<(T, OwnedMutexGuard<()>), E>, JoinError>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    {
        let guard = self.lock().await;
        Self::construct_with_guard(guard, build).await
    }

    pub async fn construct_with_guard<T, E, F>(
        guard: OwnedMutexGuard<()>,
        build: F,
    ) -> Result<Result<(T, OwnedMutexGuard<()>), E>, JoinError>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    {
        tokio::task::spawn_blocking(move || build().map(|value| (value, guard))).await
    }

    pub fn startup_superseded(&self) -> watch::Receiver<bool> {
        self.startup_superseded.subscribe()
    }

    /// Retire pending startup preload only after the manual selection is live.
    pub fn manual_load_succeeded(&self) {
        self.startup_superseded.send_replace(true);
    }

    pub fn is_startup_superseded(&self) -> bool {
        *self.startup_superseded.borrow()
    }

    pub fn startup_selection_is_current(
        &self,
        startup_model: &str,
        selected_model: Option<&str>,
    ) -> bool {
        !self.is_startup_superseded() && selected_model == Some(startup_model)
    }

    pub fn reusable_loaded_provider(
        loaded_model: Option<&str>,
        requested_model: &str,
        provider: Option<Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    ) -> Option<Arc<dyn wenlan_core::llm_provider::LlmProvider>> {
        if loaded_model != Some(requested_model) {
            return None;
        }
        provider.filter(|provider| {
            provider.is_available()
                && provider.backend() == wenlan_core::llm_provider::LlmBackend::OnDevice
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn success_cancels_waiting_startup_but_failure_does_not() {
        let coordinator = ModelLoadCoordinator::default();
        let mut startup_watch = coordinator.startup_superseded();
        assert!(coordinator.startup_selection_is_current("qwen3-4b", Some("qwen3-4b")));
        // A failed manual constructor never calls manual_load_succeeded.
        assert!(!*startup_watch.borrow());
        coordinator.manual_load_succeeded();
        startup_watch.changed().await.unwrap();
        assert!(*startup_watch.borrow());
        assert!(!coordinator.startup_selection_is_current("qwen3-4b", Some("qwen3-4b")));
        assert!(!coordinator.startup_selection_is_current("qwen3-4b", Some("qwen3-8b")));
    }

    struct ReuseProvider {
        available: bool,
        backend: wenlan_core::llm_provider::LlmBackend,
    }

    #[async_trait::async_trait]
    impl wenlan_core::llm_provider::LlmProvider for ReuseProvider {
        async fn generate(
            &self,
            _request: wenlan_core::llm_provider::LlmRequest,
        ) -> Result<String, wenlan_core::llm_provider::LlmError> {
            unreachable!("provider reuse test never generates")
        }

        fn is_available(&self) -> bool {
            self.available
        }

        fn name(&self) -> &str {
            "reuse-test"
        }

        fn backend(&self) -> wenlan_core::llm_provider::LlmBackend {
            self.backend
        }
    }

    fn reuse_provider(
        available: bool,
        backend: wenlan_core::llm_provider::LlmBackend,
    ) -> Arc<dyn wenlan_core::llm_provider::LlmProvider> {
        Arc::new(ReuseProvider { available, backend })
    }

    #[test]
    fn only_matching_available_on_device_provider_is_reused() {
        let live = reuse_provider(true, wenlan_core::llm_provider::LlmBackend::OnDevice);
        assert!(ModelLoadCoordinator::reusable_loaded_provider(
            Some("qwen3-4b"),
            "qwen3-4b",
            Some(live),
        )
        .is_some());

        let unavailable = reuse_provider(false, wenlan_core::llm_provider::LlmBackend::OnDevice);
        assert!(ModelLoadCoordinator::reusable_loaded_provider(
            Some("qwen3-4b"),
            "qwen3-4b",
            Some(unavailable),
        )
        .is_none());

        let api = reuse_provider(true, wenlan_core::llm_provider::LlmBackend::Api);
        assert!(ModelLoadCoordinator::reusable_loaded_provider(
            Some("qwen3-4b"),
            "qwen3-4b",
            Some(api),
        )
        .is_none());

        let wrong_id = reuse_provider(true, wenlan_core::llm_provider::LlmBackend::OnDevice);
        assert!(ModelLoadCoordinator::reusable_loaded_provider(
            Some("qwen3-8b"),
            "qwen3-4b",
            Some(wrong_id),
        )
        .is_none());
    }

    #[tokio::test]
    async fn failed_production_constructor_releases_lock_without_superseding_startup() {
        let coordinator = ModelLoadCoordinator::default();
        let result = ModelLoadCoordinator::construct_with_guard(coordinator.lock().await, || {
            Err::<(), _>("constructor failed")
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "constructor failed");
        assert!(coordinator.startup_selection_is_current("qwen3-4b", Some("qwen3-4b")));
        let _guard = coordinator.lock().await;
    }

    #[tokio::test]
    async fn shared_lock_serializes_both_manual_and_startup_race_orders() {
        // Startup wins the lock first, then a manual choice waits and becomes
        // the final installed provider.
        let coordinator = ModelLoadCoordinator::default();
        let installs = Arc::new(AtomicUsize::new(0));
        let startup_guard = coordinator.lock().await;
        let manual = {
            let c = coordinator.clone();
            let installs = installs.clone();
            tokio::spawn(async move {
                let _guard = c.lock().await;
                installs.fetch_add(1, Ordering::SeqCst);
                c.manual_load_succeeded();
            })
        };
        installs.fetch_add(1, Ordering::SeqCst); // startup publishes under its lock
        drop(startup_guard);
        manual.await.unwrap();
        assert_eq!(installs.load(Ordering::SeqCst), 2);

        // Manual wins first; startup's lock-time recheck sees the successful
        // selection and skips constructing or installing its stale model.
        let coordinator = ModelLoadCoordinator::default();
        let installs = Arc::new(AtomicUsize::new(0));
        let manual_guard = coordinator.lock().await;
        installs.fetch_add(1, Ordering::SeqCst); // manual provider installed
        coordinator.manual_load_succeeded();
        let startup = {
            let c = coordinator.clone();
            let installs = installs.clone();
            tokio::spawn(async move {
                let _guard = c.lock().await;
                if c.startup_selection_is_current("qwen3-4b", Some("qwen3-4b")) {
                    installs.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        drop(manual_guard);
        startup.await.unwrap();
        assert_eq!(installs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn canceled_request_keeps_constructor_lock_until_blocking_work_finishes() {
        let coordinator = ModelLoadCoordinator::default();
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = mpsc::sync_channel::<()>(0);
        let manual = {
            let c = coordinator.clone();
            tokio::spawn(async move {
                c.construct(move || {
                    let _ = started_tx.send(());
                    release_rx.recv().unwrap();
                    Ok::<_, ()>(())
                })
                .await
            })
        };
        started_rx.await.unwrap();
        manual.abort();
        let (attempting_tx, attempting_rx) = oneshot::channel();
        let (acquired_tx, mut acquired_rx) = oneshot::channel();
        let c = coordinator.clone();
        let waiting = tokio::spawn(async move {
            let _ = attempting_tx.send(());
            let _guard = c.lock().await;
            let _ = acquired_tx.send(());
        });
        attempting_rx.await.unwrap();
        assert!(matches!(
            acquired_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        acquired_rx.await.unwrap();
        waiting.await.unwrap();
    }
}
