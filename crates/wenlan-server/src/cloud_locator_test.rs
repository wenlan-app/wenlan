// SPDX-License-Identifier: AGPL-3.0-only
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Stands in for a person's Wenlan: answers `/health` at its listen address
/// (or never, when `answers` is off) and records where it was started.
#[derive(Clone, Default)]
struct Fake {
    starts: Arc<AtomicUsize>,
    folders: Arc<std::sync::Mutex<Vec<PathBuf>>>,
    silent: Arc<AtomicBool>,
}

struct FakeInstance {
    task: tokio::task::JoinHandle<()>,
    alive: Arc<AtomicBool>,
}

impl Launcher for Fake {
    type Instance = FakeInstance;

    fn start(
        &self,
        _: &AccountId,
        dir: &Path,
        listen: SocketAddr,
        _: u16,
    ) -> std::io::Result<FakeInstance> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.folders.lock().unwrap().push(dir.to_path_buf());
        let silent = self.silent.load(Ordering::SeqCst);
        let listener = std::net::TcpListener::bind(listen)?;
        listener.set_nonblocking(true)?;
        let task = tokio::spawn(async move {
            if silent {
                std::future::pending::<()>().await;
            }
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let app = axum::Router::new().route("/health", axum::routing::get(|| async { "ok" }));
            let _ = axum::serve(listener, app).await;
        });
        Ok(FakeInstance {
            task,
            alive: Arc::new(AtomicBool::new(true)),
        })
    }
}

impl Instance for FakeInstance {
    fn running(&mut self) -> bool {
        self.alive.load(Ordering::SeqCst) && !self.task.is_finished()
    }

    async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

fn pool(fake: &Fake, root: &Path, settings: impl FnOnce(&mut PoolSettings)) -> ProcessPool<Fake> {
    let mut chosen = PoolSettings {
        root: root.to_path_buf(),
        idle: Duration::from_secs(600),
        ready_within: Duration::from_secs(5),
        max_running: 10,
    };
    settings(&mut chosen);
    ProcessPool::new(fake.clone(), chosen)
}

#[test]
fn an_account_id_is_only_what_new_makes() {
    let made = AccountId::new();
    assert_eq!(AccountId::parse(made.as_str()), Some(made));
    for bad in [
        "",
        "../etc",
        "ABCDEF0123456789abcdef0123456789",
        "abc",
        &"a".repeat(33),
        "0123456789abcdef0123456789abcde/",
    ] {
        assert_eq!(AccountId::parse(bad), None, "{bad:?}");
    }
}

#[tokio::test]
async fn each_person_starts_once_in_their_own_folder_and_is_reused() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let pool = Arc::new(pool(&fake, root.path(), |_| {}));
    let (alice, bob) = (AccountId::new(), AccountId::new());

    // Two requests for Alice at once start her once.
    let (first, second) = tokio::join!(pool.locate(&alice), pool.locate(&alice));
    assert_eq!(first.unwrap(), second.unwrap());
    let bobs = pool.locate(&bob).await.unwrap();
    assert_ne!(bobs, pool.locate(&alice).await.unwrap());
    assert_eq!(fake.starts.load(Ordering::SeqCst), 2);
    assert_eq!(pool.running(), 2);

    let folders = fake.folders.lock().unwrap().clone();
    assert_eq!(
        folders,
        vec![
            root.path().join(alice.as_str()),
            root.path().join(bob.as_str())
        ]
    );
    assert!(folders.iter().all(|folder| folder.is_dir()));
}

#[tokio::test]
async fn one_that_exited_is_started_again() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let pool = pool(&fake, root.path(), |_| {});
    let alice = AccountId::new();
    pool.locate(&alice).await.unwrap();
    {
        let slot = pool.slot(&alice);
        let held = slot.lock().await;
        held.as_ref()
            .unwrap()
            .instance
            .alive
            .store(false, Ordering::SeqCst);
    }
    pool.locate(&alice).await.unwrap();
    assert_eq!(fake.starts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn idle_people_are_stopped_and_woken_on_their_next_request() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let pool = pool(&fake, root.path(), |settings| {
        settings.idle = Duration::ZERO
    });
    let alice = AccountId::new();
    pool.locate(&alice).await.unwrap();
    assert_eq!(pool.stop_idle().await, 1);
    assert_eq!(pool.running(), 0);
    pool.locate(&alice).await.unwrap();
    assert_eq!(fake.starts.load(Ordering::SeqCst), 2);
    pool.stop_all().await;
    assert_eq!(pool.running(), 0);
}

#[tokio::test]
async fn a_full_machine_refuses_a_new_person_but_serves_those_running() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let pool = pool(&fake, root.path(), |settings| settings.max_running = 1);
    let (alice, bob) = (AccountId::new(), AccountId::new());
    pool.locate(&alice).await.unwrap();
    assert!(matches!(pool.locate(&bob).await, Err(LocateError::Busy)));
    pool.locate(&alice).await.unwrap();
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn one_that_never_answers_is_stopped_and_reported() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    fake.silent.store(true, Ordering::SeqCst);
    let pool = pool(&fake, root.path(), |settings| {
        settings.ready_within = Duration::from_millis(200)
    });
    let alice = AccountId::new();
    assert!(matches!(
        pool.locate(&alice).await,
        Err(LocateError::NotReady(_))
    ));
    assert_eq!(pool.running(), 0);
}
