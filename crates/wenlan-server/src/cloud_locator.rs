// SPDX-License-Identifier: AGPL-3.0-only
//! Personal cloud: where each person's Wenlan runs.
//!
//! The entrance asks one question, "where is this person's Wenlan?", through
//! [`Locator`]. Everything else (sign-in, the pass it signs, the desktop app,
//! AI connectors) only sees that question, so how people are run can change
//! without touching them.
//!
//! The first answer is [`ProcessPool`]: on one machine, each person gets their
//! own Wenlan program with its own data folder and ports, started when they
//! connect and stopped when idle. People are kept apart by the operating
//! system: one program only ever opens one person's folder.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A person's account in the personal cloud: random, never reused, and the
/// only name their folder, backups and keys go by (never an email or name).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(String);

impl AccountId {
    /// A new random account id.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    /// Accepts only the shape [`AccountId::new`] makes: 32 lowercase hex
    /// digits, so an id can never name a path outside its own folder.
    pub fn parse(value: &str) -> Option<Self> {
        let valid = value.len() == 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        valid.then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AccountId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a person's Wenlan answers: its team listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub addr: SocketAddr,
}

#[derive(Debug)]
pub enum LocateError {
    /// As many are running as the machine allows.
    Busy,
    /// The program could not start, or exited while starting.
    Start(String),
    /// It started but did not answer in time; it was stopped.
    NotReady(Duration),
}

impl std::fmt::Display for LocateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("too many people are using the personal cloud right now"),
            Self::Start(why) => write!(f, "this person's Wenlan could not start: {why}"),
            Self::NotReady(within) => {
                write!(f, "this person's Wenlan did not answer within {within:?}")
            }
        }
    }
}

impl std::error::Error for LocateError {}

/// The seam: the entrance's only way to reach a person's Wenlan, waking it
/// first when needed.
pub trait Locator: Send + Sync + 'static {
    fn locate(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<Endpoint, LocateError>> + Send;
}

/// Starts one person's Wenlan. [`ProcessLauncher`] runs the real program;
/// tests use a stand-in.
pub trait Launcher: Send + Sync + 'static {
    type Instance: Instance;

    /// Starts it with `dir` as its whole data folder, its team listener at
    /// `listen` and its own daemon at `daemon_port`, both on loopback.
    fn start(
        &self,
        account: &AccountId,
        dir: &Path,
        listen: SocketAddr,
        daemon_port: u16,
    ) -> std::io::Result<Self::Instance>;
}

/// A started Wenlan.
pub trait Instance: Send + 'static {
    /// False once it has exited.
    fn running(&mut self) -> bool;
    /// Stops it and waits until it has.
    fn stop(self) -> impl Future<Output = ()> + Send;
}

/// Runs `wenlan-server team` for one person: their folder is its
/// `WENLAN_DATA_DIR`, so it can open nothing else.
pub struct ProcessLauncher {
    /// The `wenlan-server` program.
    pub program: PathBuf,
    /// The address people reach the personal cloud at.
    pub public_url: String,
}

pub struct ProcessInstance(tokio::process::Child);

impl Launcher for ProcessLauncher {
    type Instance = ProcessInstance;

    fn start(
        &self,
        _account: &AccountId,
        dir: &Path,
        listen: SocketAddr,
        daemon_port: u16,
    ) -> std::io::Result<ProcessInstance> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("server.log"))?;
        let child = tokio::process::Command::new(&self.program)
            .arg("team")
            .arg(&self.public_url)
            .arg("--listen")
            .arg(listen.to_string())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir)
            .env("WENLAN_DATA_DIR", dir)
            .env("WENLAN_PORT", daemon_port.to_string())
            .env("WENLAN_NO_AUTOSTART", "1")
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(true)
            .spawn()?;
        Ok(ProcessInstance(child))
    }
}

impl Instance for ProcessInstance {
    fn running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }

    async fn stop(mut self) {
        let _ = self.0.kill().await;
    }
}

/// How a [`ProcessPool`] runs people.
#[derive(Clone, Debug)]
pub struct PoolSettings {
    /// Each person's folder is `<root>/<account id>`.
    pub root: PathBuf,
    /// Stopped after this long without a request.
    pub idle: Duration,
    /// How long a start may take before it counts as failed.
    pub ready_within: Duration,
    /// At most this many running at once; more are refused, not queued.
    pub max_running: usize,
}

struct Running<I> {
    instance: I,
    endpoint: Endpoint,
    last_used: Instant,
}

type Slot<I> = Arc<tokio::sync::Mutex<Option<Running<I>>>>;

/// Answer A: every person's Wenlan on this machine, started on their first
/// request and stopped when idle.
pub struct ProcessPool<L: Launcher> {
    launcher: L,
    settings: PoolSettings,
    slots: std::sync::Mutex<HashMap<AccountId, Slot<L::Instance>>>,
    http: reqwest::Client,
}

impl<L: Launcher> ProcessPool<L> {
    pub fn new(launcher: L, settings: PoolSettings) -> Self {
        Self {
            launcher,
            settings,
            slots: std::sync::Mutex::default(),
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap_or_default(),
        }
    }

    /// The folder that holds everything of this person's.
    pub fn folder(&self, account: &AccountId) -> PathBuf {
        self.settings.root.join(account.as_str())
    }

    fn slot(&self, account: &AccountId) -> Slot<L::Instance> {
        self.slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(account.clone())
            .or_default()
            .clone()
    }

    fn slots(&self) -> Vec<Slot<L::Instance>> {
        self.slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// How many are running now. A slot being started counts.
    pub fn running(&self) -> usize {
        self.slots()
            .iter()
            .filter(|slot| slot.try_lock().map_or(true, |held| held.is_some()))
            .count()
    }

    /// Stops everyone idle for longer than [`PoolSettings::idle`]. One in
    /// the middle of a start or a request is left alone.
    pub async fn stop_idle(&self) -> usize {
        let mut stopped = 0;
        for slot in self.slots() {
            let Ok(mut held) = slot.try_lock() else {
                continue;
            };
            if held
                .as_ref()
                .is_some_and(|running| running.last_used.elapsed() >= self.settings.idle)
            {
                if let Some(running) = held.take() {
                    running.instance.stop().await;
                    stopped += 1;
                }
            }
        }
        stopped
    }

    /// Stops everyone, for shutdown.
    pub async fn stop_all(&self) {
        for slot in self.slots() {
            if let Some(running) = slot.lock().await.take() {
                running.instance.stop().await;
            }
        }
    }

    async fn start(&self, account: &AccountId) -> Result<Running<L::Instance>, LocateError> {
        let dir = self.folder(account);
        std::fs::create_dir_all(&dir).map_err(|error| LocateError::Start(error.to_string()))?;
        let listen = free_loopback().map_err(|error| LocateError::Start(error.to_string()))?;
        let daemon = free_loopback().map_err(|error| LocateError::Start(error.to_string()))?;
        let mut instance = self
            .launcher
            .start(account, &dir, listen, daemon.port())
            .map_err(|error| LocateError::Start(error.to_string()))?;
        let health = format!("http://{listen}/health");
        let deadline = Instant::now() + self.settings.ready_within;
        loop {
            if !instance.running() {
                return Err(LocateError::Start("it exited while starting".into()));
            }
            let answered = self
                .http
                .get(&health)
                .send()
                .await
                .is_ok_and(|response| response.status().is_success());
            if answered {
                return Ok(Running {
                    instance,
                    endpoint: Endpoint { addr: listen },
                    last_used: Instant::now(),
                });
            }
            if Instant::now() >= deadline {
                instance.stop().await;
                return Err(LocateError::NotReady(self.settings.ready_within));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl<L: Launcher> Locator for ProcessPool<L> {
    async fn locate(&self, account: &AccountId) -> Result<Endpoint, LocateError> {
        let slot = self.slot(account);
        let mut held = slot.lock().await;
        if let Some(running) = held.as_mut() {
            if running.instance.running() {
                running.last_used = Instant::now();
                return Ok(running.endpoint);
            }
            // It exited on its own: start it again.
            if let Some(gone) = held.take() {
                gone.instance.stop().await;
            }
        }
        // Count everyone else; this slot is held and empty.
        let others = self
            .slots()
            .iter()
            .filter(|other| !Arc::ptr_eq(other, &slot))
            .filter(|other| other.try_lock().map_or(true, |o| o.is_some()))
            .count();
        if others >= self.settings.max_running {
            return Err(LocateError::Busy);
        }
        let running = self.start(account).await?;
        let endpoint = running.endpoint;
        *held = Some(running);
        Ok(endpoint)
    }
}

/// A loopback address nothing listens on right now.
fn free_loopback() -> std::io::Result<SocketAddr> {
    std::net::TcpListener::bind(("127.0.0.1", 0))?.local_addr()
}

#[cfg(test)]
#[path = "cloud_locator_test.rs"]
mod tests;
