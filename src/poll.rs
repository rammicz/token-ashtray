use crate::{
    model::{ProviderCard, Status},
    providers::{self, Provider, ProviderError},
    refresh::{RefreshPhase, RefreshSnapshot},
};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};
use tokio::{runtime::Runtime, sync::watch, time::Instant};

pub type SharedBoard = Arc<RwLock<BTreeMap<String, ProviderCard>>>;
#[derive(Clone, Debug)]
pub struct PollSettings {
    pub interval_secs: u64,
    pub hidden: HashSet<String>,
}
impl Default for PollSettings {
    fn default() -> Self {
        Self {
            interval_secs: 300,
            hidden: HashSet::new(),
        }
    }
}

pub fn initial_board() -> SharedBoard {
    Arc::new(RwLock::new(
        providers::all()
            .iter()
            .map(|p| {
                let d = p.descriptor();
                (d.id.into(), d.empty())
            })
            .collect(),
    ))
}

#[derive(Default)]
struct RefreshWork {
    snapshot: RefreshSnapshot,
    inflight: HashSet<String>,
}
impl RefreshWork {
    fn request(&mut self, ids: &[String], hidden: &HashSet<String>, startup: bool) -> u64 {
        if self.snapshot.active() {
            return self.snapshot.generation;
        }
        self.snapshot = RefreshSnapshot {
            generation: self.snapshot.generation.wrapping_add(1).max(1),
            startup,
            providers: ids
                .iter()
                .filter(|id| !hidden.contains(*id))
                .map(|id| {
                    (
                        id.clone(),
                        if self.inflight.contains(id) {
                            RefreshPhase::Running
                        } else {
                            RefreshPhase::Waiting
                        },
                    )
                })
                .collect(),
        };
        self.snapshot.generation
    }
    fn waiting(&self, id: &str) -> bool {
        self.snapshot.providers.get(id) == Some(&RefreshPhase::Waiting)
    }
    fn begin(&mut self, id: &str) -> bool {
        self.inflight.insert(id.into());
        if self.waiting(id) {
            self.snapshot
                .providers
                .insert(id.into(), RefreshPhase::Running);
            true
        } else {
            false
        }
    }
    fn finish(&mut self, id: &str, success: bool) {
        self.inflight.remove(id);
        if self.snapshot.providers.get(id) == Some(&RefreshPhase::Running) {
            self.snapshot.providers.insert(
                id.into(),
                if success {
                    RefreshPhase::Succeeded
                } else {
                    RefreshPhase::Failed
                },
            );
        }
    }
    fn hide(&mut self, hidden: &HashSet<String>) {
        for (id, phase) in &mut self.snapshot.providers {
            if hidden.contains(id) && phase.active() {
                *phase = RefreshPhase::Disabled;
            }
        }
    }
}

pub struct Poller {
    runtime: Option<Runtime>,
    feedback: Arc<Mutex<RefreshWork>>,
    provider_ids: Vec<String>,
    wake: Arc<dyn Fn() + Send + Sync>,
    settings: watch::Sender<PollSettings>,
    refresh: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
}
impl Poller {
    pub fn start(
        board: SharedBoard,
        settings: PollSettings,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> std::result::Result<Self, String> {
        Self::with_providers(
            board,
            settings,
            wake,
            providers::all(),
            Duration::from_secs(20),
            Duration::from_secs(60),
        )
    }
    pub(crate) fn with_providers(
        board: SharedBoard,
        settings: PollSettings,
        wake: Arc<dyn Fn() + Send + Sync>,
        providers: Vec<Arc<dyn Provider>>,
        timeout: Duration,
        minimum: Duration,
    ) -> std::result::Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| "Could not start polling runtime")?;
        let client = providers::client().map_err(|e| e.message)?;
        let provider_ids = providers
            .iter()
            .map(|provider| provider.descriptor().id.into())
            .collect::<Vec<_>>();
        let feedback = Arc::new(Mutex::new(RefreshWork::default()));
        feedback
            .lock()
            .expect("refresh lock")
            .request(&provider_ids, &settings.hidden, true);
        let (settings, settings_rx) = watch::channel(settings);
        let (refresh, refresh_rx) = watch::channel(0);
        let (shutdown, shutdown_rx) = watch::channel(false);
        for provider in providers {
            runtime.spawn(provider_loop(
                provider,
                client.clone(),
                board.clone(),
                wake.clone(),
                Controls {
                    feedback: feedback.clone(),
                    settings: settings_rx.clone(),
                    refresh: refresh_rx.clone(),
                    shutdown: shutdown_rx.clone(),
                },
                timeout,
                minimum,
            ));
        }
        Ok(Self {
            runtime: Some(runtime),
            feedback,
            provider_ids,
            wake,
            settings,
            refresh,
            shutdown,
        })
    }
    pub fn set_settings(&self, settings: PollSettings) {
        self.feedback
            .lock()
            .expect("refresh lock")
            .hide(&settings.hidden);
        self.settings.send_replace(settings);
        (self.wake)();
    }
    pub fn feedback(&self) -> RefreshSnapshot {
        self.feedback.lock().expect("refresh lock").snapshot.clone()
    }
    pub fn refresh(&self) -> Result<u64, String> {
        let hidden = self.settings.borrow().hidden.clone();
        if self.refresh.receiver_count() == 0
            && self.provider_ids.iter().any(|id| !hidden.contains(id))
        {
            return Err("Refresh unavailable: polling has disconnected".into());
        }
        let generation =
            self.feedback
                .lock()
                .expect("refresh lock")
                .request(&self.provider_ids, &hidden, false);
        self.refresh
            .send_modify(|counter| *counter = counter.wrapping_add(1));
        (self.wake)();
        Ok(generation)
    }
}
impl Drop for Poller {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(25));
        }
    }
}
struct Controls {
    feedback: Arc<Mutex<RefreshWork>>,
    settings: watch::Receiver<PollSettings>,
    refresh: watch::Receiver<u64>,
    shutdown: watch::Receiver<bool>,
}
async fn provider_loop(
    provider: Arc<dyn Provider>,
    client: reqwest::Client,
    board: SharedBoard,
    wake: Arc<dyn Fn() + Send + Sync>,
    mut control: Controls,
    timeout: Duration,
    minimum: Duration,
) {
    let id = provider.descriptor().id;
    let mut due = Instant::now();
    let mut last_attempt = None::<Instant>;
    let mut backoff = Instant::now();
    loop {
        if *control.shutdown.borrow() {
            return;
        }
        let settings = control.settings.borrow().clone();
        if settings.hidden.contains(id) {
            tokio::select! {_ = control.shutdown.changed()=>return,_=control.settings.changed()=>{},_=control.refresh.changed()=>{}}
            continue;
        }
        tokio::select! {
            _=control.shutdown.changed()=>return,
            changed=control.settings.changed()=>{if changed.is_err(){return;}let interval=Duration::from_secs(control.settings.borrow().interval_secs.max(minimum.as_secs()));due=last_attempt.map(|t|t+interval).unwrap_or_else(Instant::now).max(backoff);continue;},
            changed = control.refresh.changed() => {
                if changed.is_err() { return; }
                // The notification may arrive after the adopted attempt finishes.
                // Only an actual waiting request is allowed to reschedule work.
                if control.feedback.lock().expect("refresh lock").waiting(id) {
                    due = last_attempt.map(|t| t + minimum).unwrap_or_else(Instant::now).max(backoff);
                }
                continue;
            },
            _=tokio::time::sleep_until(due)=>{}
        }
        if control.settings.borrow().hidden.contains(id) {
            continue;
        }
        if control.feedback.lock().expect("refresh lock").begin(id) {
            wake();
        }
        last_attempt = Some(Instant::now());
        let task_provider = provider.clone();
        let task_client = client.clone();
        // Isolate a provider panic from every other task. Timeouts also cover
        // authentication and a provider's complete fallback sequence.
        let mut task = tokio::spawn(async move { task_provider.poll(&task_client).await });
        let outcome = tokio::select! {
            _=control.shutdown.changed()=>{task.abort();return;},

            result=tokio::time::timeout(timeout,&mut task)=>match result{
                Ok(Ok(value))=>value,
                Ok(Err(_))=>Err(ProviderError::error("Provider failed unexpectedly; other providers are still running")),
                Err(_)=>{task.abort();Err(ProviderError::error("Provider request timed out"))}
            }
        };
        let success = outcome.is_ok();
        let mut card = provider.descriptor().empty();
        match outcome {
            Ok(value) => card = value,
            Err(error) => {
                card.status = error.status;
                card.detail = error.message;
                if let Some(seconds) = error.retry_after_secs {
                    backoff = Instant::now() + Duration::from_secs(seconds.max(minimum.as_secs()));
                }
                if let Some(previous) = board.read().ok().and_then(|b| b.get(id).cloned()) {
                    card.fetched_at = previous.fetched_at;
                    // Authentication errors must not look like a still-valid login.
                    if card.status != Status::Reconnect {
                        card.meters = previous.meters;
                        card.plan = previous.plan;
                        card.price = previous.price;
                    }
                }
            }
        }
        if let Ok(mut board) = board.write() {
            board.insert(id.into(), card);
        }
        control
            .feedback
            .lock()
            .expect("refresh lock")
            .finish(id, success);
        wake();
        due = last_attempt.unwrap()
            + Duration::from_secs(
                control
                    .settings
                    .borrow()
                    .interval_secs
                    .max(minimum.as_secs()),
            );
        due = due.max(backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{Credentials, Descriptor, Usage};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fake {
        calls: Arc<AtomicUsize>,
        hang: bool,
        id: &'static str,
    }
    #[async_trait]
    impl Provider for Fake {
        fn descriptor(&self) -> Descriptor {
            Descriptor {
                id: self.id,
                name: self.id,
                link: "",
                login: "login",
            }
        }
        fn discover(&self) -> providers::Result<Credentials> {
            Ok(Credentials::token("fake".into()))
        }
        async fn fetch(
            &self,
            _: &reqwest::Client,
            _: &mut Credentials,
        ) -> providers::Result<Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.hang {
                std::future::pending::<()>().await;
            }
            Ok(Value::Null)
        }
        fn parse(&self, _: &Value) -> providers::Result<Usage> {
            Ok(Usage::default())
        }
    }
    #[test]
    fn initial_fetch_exposes_running_before_a_provider_finishes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed = wakes.clone();
        let poller = Poller::with_providers(
            Arc::new(RwLock::new(BTreeMap::new())),
            PollSettings::default(),
            Arc::new(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            }),
            vec![Arc::new(Fake {
                calls: calls.clone(),
                hang: true,
                id: "held",
            })],
            Duration::from_secs(1),
            Duration::from_secs(60),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while calls.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            wakes.load(Ordering::SeqCst) > 0,
            "initial refresh must publish running feedback before quota results exist"
        );
        drop(poller);
    }

    #[test]
    fn failing_provider_does_not_block_peers_and_hidden_is_not_polled() {
        let good = Arc::new(AtomicUsize::new(0));
        let hidden = Arc::new(AtomicUsize::new(0));
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(Fake {
                calls: good.clone(),
                hang: false,
                id: "good",
            }),
            Arc::new(Fake {
                calls: Arc::new(AtomicUsize::new(0)),
                hang: true,
                id: "hung",
            }),
            Arc::new(Fake {
                calls: hidden.clone(),
                hang: false,
                id: "hidden",
            }),
        ];
        let board = Arc::new(RwLock::new(BTreeMap::new()));
        let settings = PollSettings {
            interval_secs: 300,
            hidden: HashSet::from(["hidden".into()]),
        };
        let poller = Poller::with_providers(
            board.clone(),
            settings,
            Arc::new(|| {}),
            providers,
            Duration::from_millis(30),
            Duration::from_secs(60),
        )
        .unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while board.read().unwrap().len() < 2 && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(board.read().unwrap()["good"].status, Status::Live);
        assert_eq!(board.read().unwrap()["hung"].status, Status::Error);
        assert_eq!(poller.feedback().providers["hung"], RefreshPhase::Failed);
        for _ in 0..20 {
            poller.refresh().unwrap();
        }
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(good.load(Ordering::SeqCst), 1);
        assert_eq!(hidden.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod backoff_tests {
    use super::*;
    use crate::providers::{Credentials, Descriptor, Result, Usage};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Limited(Arc<AtomicUsize>);
    #[async_trait]
    impl Provider for Limited {
        fn descriptor(&self) -> Descriptor {
            Descriptor {
                id: "limited",
                name: "Limited",
                link: "",
                login: "login",
            }
        }
        fn discover(&self) -> Result<Credentials> {
            Ok(Credentials::token("synthetic".into()))
        }
        async fn fetch(&self, _: &reqwest::Client, _: &mut Credentials) -> Result<Value> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError {
                status: Status::Error,
                message: "HTTP 429".into(),
                retry_after_secs: Some(3600),
            })
        }
        fn parse(&self, _: &Value) -> Result<Usage> {
            unreachable!("rate-limited response must not be parsed")
        }
    }
    #[test]
    fn manual_refresh_cannot_bypass_server_backoff() {
        let calls = Arc::new(AtomicUsize::new(0));
        let board = Arc::new(RwLock::new(BTreeMap::new()));
        let poller = Poller::with_providers(
            board.clone(),
            PollSettings::default(),
            Arc::new(|| {}),
            vec![Arc::new(Limited(calls.clone()))],
            Duration::from_secs(1),
            Duration::from_millis(10),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while board.read().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(30));
        let generation = poller.refresh().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(poller.feedback().generation, generation);
        assert_eq!(
            poller.feedback().providers["limited"],
            RefreshPhase::Waiting
        );
        poller.set_settings(PollSettings {
            hidden: HashSet::from(["limited".into()]),
            ..Default::default()
        });
        assert_eq!(
            poller.feedback().providers["limited"],
            RefreshPhase::Disabled
        );
        poller.refresh().unwrap();
        assert!(poller.feedback().providers.is_empty());
        assert_eq!(poller.feedback().summary(), "No enabled providers");
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;
    use crate::{
        model::Meter,
        providers::{Credentials, Descriptor, Usage},
    };
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    struct Gated {
        id: &'static str,
        calls: Arc<AtomicUsize>,
        gate: Arc<Notify>,
        scheduled_failure: bool,
    }
    #[async_trait]
    impl Provider for Gated {
        fn descriptor(&self) -> Descriptor {
            Descriptor {
                id: self.id,
                name: self.id,
                link: "",
                login: "synthetic",
            }
        }
        fn discover(&self) -> providers::Result<Credentials> {
            Ok(Credentials::token("synthetic".into()))
        }
        async fn fetch(
            &self,
            _: &reqwest::Client,
            _: &mut Credentials,
        ) -> providers::Result<Value> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            self.gate.notified().await;
            if self.scheduled_failure && call == 3 {
                return Err(ProviderError {
                    status: Status::Error,
                    message: "synthetic scheduled limit".into(),
                    retry_after_secs: Some(1),
                });
            }
            Ok(Value::Null)
        }
        fn parse(&self, _: &Value) -> providers::Result<Usage> {
            Ok(Usage {
                meters: vec![Meter {
                    label: "Weekly".into(),
                    used_percent: 25.0,
                    resets_at: None,
                }],
                ..Default::default()
            })
        }
    }
    fn wait_for(mut predicate: impl FnMut() -> bool) {
        let until = std::time::Instant::now() + Duration::from_secs(3);
        while !predicate() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            predicate(),
            "synthetic provider did not reach the expected state"
        );
    }
    #[test]
    fn cards_finish_independently_unchanged_manual_deduplicates_and_scheduled_retry_is_quiet() {
        let fast_calls = Arc::new(AtomicUsize::new(0));
        let slow_calls = Arc::new(AtomicUsize::new(0));
        let fast = Arc::new(Notify::new());
        let slow = Arc::new(Notify::new());
        let board = Arc::new(RwLock::new(BTreeMap::new()));
        let poller = Poller::with_providers(
            board.clone(),
            PollSettings {
                interval_secs: 1,
                hidden: HashSet::new(),
            },
            Arc::new(|| {}),
            vec![
                Arc::new(Gated {
                    id: "fast",
                    calls: fast_calls.clone(),
                    gate: fast.clone(),
                    scheduled_failure: true,
                }),
                Arc::new(Gated {
                    id: "slow",
                    calls: slow_calls.clone(),
                    gate: slow.clone(),
                    scheduled_failure: false,
                }),
            ],
            Duration::from_secs(2),
            Duration::from_millis(150),
        )
        .unwrap();
        wait_for(|| {
            fast_calls.load(Ordering::SeqCst) == 1 && slow_calls.load(Ordering::SeqCst) == 1
        });
        let startup = poller.feedback();
        assert!(startup.startup);
        assert!(
            startup
                .providers
                .values()
                .all(|phase| *phase == RefreshPhase::Running)
        );
        // A click during work adopts this actual attempt and starts no second call.
        assert_eq!(poller.refresh().unwrap(), startup.generation);
        fast.notify_one();
        wait_for(|| poller.feedback().providers["fast"] == RefreshPhase::Succeeded);
        assert_eq!(poller.feedback().providers["slow"], RefreshPhase::Running);
        slow.notify_one();
        wait_for(|| !poller.feedback().active());
        let original = board.read().unwrap()["fast"].meters.clone();
        let manual = poller.refresh().unwrap();
        assert!(manual > startup.generation);
        assert!(!poller.feedback().startup);
        assert_eq!(poller.feedback().providers["fast"], RefreshPhase::Waiting);
        for _ in 0..20 {
            assert_eq!(poller.refresh().unwrap(), manual);
        }
        assert_eq!(
            fast_calls.load(Ordering::SeqCst),
            1,
            "minimum interval must be preserved"
        );
        wait_for(|| {
            fast_calls.load(Ordering::SeqCst) == 2 && slow_calls.load(Ordering::SeqCst) == 2
        });
        fast.notify_one();
        wait_for(|| poller.feedback().providers["fast"] == RefreshPhase::Succeeded);
        assert_eq!(poller.feedback().providers["slow"], RefreshPhase::Running);
        assert_eq!(board.read().unwrap()["fast"].meters, original);
        slow.notify_one();
        wait_for(|| !poller.feedback().active());
        let completed = poller.feedback();
        wait_for(|| {
            fast_calls.load(Ordering::SeqCst) == 3 && slow_calls.load(Ordering::SeqCst) == 3
        });
        assert_eq!(
            poller.feedback(),
            completed,
            "scheduled starts must not produce cues"
        );
        fast.notify_one();
        slow.notify_one();
        wait_for(|| board.read().unwrap()["fast"].detail == "synthetic scheduled limit");
        assert_eq!(
            poller.feedback(),
            completed,
            "scheduled failure must be quiet"
        );
        wait_for(|| fast_calls.load(Ordering::SeqCst) == 4);
        assert_eq!(
            poller.feedback(),
            completed,
            "scheduled retry must be quiet"
        );
        fast.notify_one();
        slow.notify_one();
    }
}
