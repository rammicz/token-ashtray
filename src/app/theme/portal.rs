//! Public XDG Settings portal, including the v1 ABI shipped by Ubuntu 22.04.
use eframe::egui::Theme;
use futures_util::StreamExt;
use std::{
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};
use tokio::sync::watch;
use zbus::{
    Connection, Proxy,
    zvariant::{OwnedValue, Value},
};

const SERVICE: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";
const RETRY: Duration = Duration::from_secs(2);

pub(super) struct Watcher {
    value: Arc<RwLock<Option<Theme>>>,
    stop: watch::Sender<bool>,
    #[cfg(test)]
    finished: Arc<std::sync::atomic::AtomicBool>,
}

impl Watcher {
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start_at(None, wake)
    }

    fn start_at(address: Option<String>, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let value = Arc::new(RwLock::new(None));
        let (stop, mut stopped) = watch::channel(false);
        let output = value.clone();
        #[cfg(test)]
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(test)]
        let finished_worker = finished.clone();
        // No UI-thread bus calls. Cancellation also interrupts a missing or
        // hung portal; dropping the last viewport owner ends the worker.
        let _ = thread::Builder::new()
            .name("appearance-portal".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    tokio::select! {
                        _ = stopped.changed() => {},
                        _ = watch_portal(address.as_deref(), |next| {
                            let mut current = output.write().expect("appearance lock");
                            if *current != next {
                                *current = next;
                                drop(current);
                                wake();
                            }
                        }) => {},
                    }
                });
                #[cfg(test)]
                finished_worker.store(true, std::sync::atomic::Ordering::Release);
            });
        Self {
            value,
            stop,
            #[cfg(test)]
            finished,
        }
    }

    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    pub fn theme(&self) -> Option<Theme> {
        *self.value.read().expect("appearance lock")
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn decode(value: &Value<'_>) -> Option<Theme> {
    // Read returns v(v(u)); ReadOne and SettingChanged return v(u).
    match value {
        Value::Value(inner) => decode(inner),
        Value::U32(1) => Some(Theme::Dark),
        Value::U32(2) => Some(Theme::Light),
        _ => None,
    }
}

async fn read(proxy: &Proxy<'_>) -> zbus::Result<Option<Theme>> {
    let version = proxy.get_property::<u32>("version").await.unwrap_or(1);
    let value: OwnedValue = proxy
        .call(
            if version >= 2 { "ReadOne" } else { "Read" },
            &(NAMESPACE, KEY),
        )
        .await?;
    Ok(decode(&value))
}

async fn session(connection: &Connection, update: &impl Fn(Option<Theme>)) -> zbus::Result<()> {
    let proxy = Proxy::new(connection, SERVICE, PATH, INTERFACE).await?;
    // Subscribe before reading so changes during the initial call are queued.
    let mut changes = proxy.receive_signal("SettingChanged").await?;
    update(read(&proxy).await?);
    let mut verify = tokio::time::interval(RETRY);
    verify.tick().await;
    loop {
        tokio::select! {
            message = changes.next() => {
                let Some(message) = message else { return Ok(()) };
                if let Ok((namespace, key, value)) = message.body().deserialize::<(String, String, OwnedValue)>()
                    && namespace == NAMESPACE && key == KEY {
                    update(decode(&value));
                }
            }
            _ = verify.tick() => {
                // Revalidate the owner even when it disappears without sending
                // a settings signal. A failed read resets stale OS knowledge.
                update(read(&proxy).await?);
            }
        }
    }
}

async fn watch_portal(address: Option<&str>, update: impl Fn(Option<Theme>)) {
    loop {
        let builder = match address {
            Some(address) => zbus::connection::Builder::address(address),
            None => zbus::connection::Builder::session(),
        };
        if let Ok(connection) =
            builder.map(|builder| builder.method_timeout(Duration::from_secs(2)))
            && let Ok(Ok(connection)) = tokio::time::timeout(RETRY, connection.build()).await
        {
            let _ = session(&connection, &update).await;
        }
        update(None);
        tokio::time::sleep(RETRY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
        sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    };

    struct Bus {
        process: Child,
        address: String,
    }
    impl Bus {
        fn new() -> Self {
            let mut process = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("private dbus-daemon");
            let mut address = String::new();
            BufReader::new(process.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            assert!(address.starts_with("unix:"));
            Self {
                process,
                address: address.trim().into(),
            }
        }
    }
    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }

    #[derive(Clone)]
    struct Settings {
        value: Arc<AtomicU32>,
        version: u32,
        fail_version: bool,
        legacy_reads: Arc<AtomicUsize>,
        modern_reads: Arc<AtomicUsize>,
        race: Arc<AtomicBool>,
    }
    #[zbus::interface(name = "org.freedesktop.portal.Settings")]
    impl Settings {
        #[zbus(property, name = "version")]
        fn version(&self) -> zbus::fdo::Result<u32> {
            if self.fail_version {
                Err(zbus::fdo::Error::Failed("version unavailable".into()))
            } else {
                Ok(self.version)
            }
        }
        fn read(&self, namespace: &str, key: &str) -> OwnedValue {
            assert_eq!((namespace, key), (NAMESPACE, KEY));
            self.legacy_reads.fetch_add(1, Ordering::SeqCst);
            Value::Value(Box::new(Value::U32(self.value.load(Ordering::SeqCst))))
                .try_to_owned()
                .unwrap()
        }
        async fn read_one(
            &self,
            namespace: &str,
            key: &str,
            #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
        ) -> zbus::fdo::Result<OwnedValue> {
            assert_eq!((namespace, key), (NAMESPACE, KEY));
            self.modern_reads.fetch_add(1, Ordering::SeqCst);
            if self.version < 2 {
                return Err(zbus::fdo::Error::UnknownMethod("v1 has no ReadOne".into()));
            }
            let value = self.value.load(Ordering::SeqCst);
            if self.race.swap(false, Ordering::SeqCst) {
                self.value.store(2, Ordering::SeqCst);
                Self::setting_changed(&emitter, NAMESPACE, KEY, Value::U32(2))
                    .await
                    .unwrap();
            }
            Ok(OwnedValue::from(value))
        }
        #[zbus(signal)]
        async fn setting_changed(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            namespace: &str,
            key: &str,
            value: Value<'_>,
        ) -> zbus::Result<()>;
    }

    async fn service(
        bus: &Bus,
        version: u32,
        fail_version: bool,
        race: bool,
    ) -> (Connection, Settings) {
        let settings = Settings {
            value: Arc::new(AtomicU32::new(if race { 1 } else { 2 })),
            version,
            fail_version,
            legacy_reads: Arc::new(AtomicUsize::new(0)),
            modern_reads: Arc::new(AtomicUsize::new(0)),
            race: Arc::new(AtomicBool::new(race)),
        };
        let connection = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .serve_at(PATH, settings.clone())
            .unwrap()
            .build()
            .await
            .unwrap();
        (connection, settings)
    }

    async fn wait_for(watcher: &Watcher, expected: Option<Theme>) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while watcher.theme() != expected {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("portal theme {:?}, expected {expected:?}", watcher.theme()));
    }

    async fn emit(connection: &Connection, namespace: &str, key: &str, value: u32) {
        connection
            .emit_signal(
                None::<&str>,
                PATH,
                INTERFACE,
                "SettingChanged",
                &(namespace, key, Value::U32(value)),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn portal_wire_v1_v2_failed_version_signals_and_reconnect() {
        for (version, failed_version) in [(1, false), (2, false), (2, true)] {
            let bus = Bus::new();
            let wakeups = Arc::new(AtomicUsize::new(0));
            let wake = wakeups.clone();
            // Start unavailable, then let the actual private service appear.
            let watcher = Watcher::start_at(Some(bus.address.clone()), move || {
                wake.fetch_add(1, Ordering::SeqCst);
            });
            assert_eq!(watcher.theme(), None);
            let (connection, settings) = service(&bus, version, failed_version, false).await;
            wait_for(&watcher, Some(Theme::Light)).await;
            if version == 1 || failed_version {
                assert!(settings.legacy_reads.load(Ordering::SeqCst) > 0);
                assert_eq!(settings.modern_reads.load(Ordering::SeqCst), 0);
            } else {
                assert!(settings.modern_reads.load(Ordering::SeqCst) > 0);
                assert_eq!(settings.legacy_reads.load(Ordering::SeqCst), 0);
            }
            for (value, expected) in [
                (1, Some(Theme::Dark)),
                (2, Some(Theme::Light)),
                (0, None),
                (99, None),
                (2, Some(Theme::Light)),
            ] {
                // Keep Read/ReadOne fixed at Light: polling cannot deliver Dark
                // or unknown. Every signal transition must arrive before a reread.
                let reads = settings.legacy_reads.load(Ordering::SeqCst)
                    + settings.modern_reads.load(Ordering::SeqCst);
                let wakes = wakeups.load(Ordering::SeqCst);
                let previous = watcher.theme();
                emit(&connection, NAMESPACE, KEY, value).await;
                tokio::time::timeout(Duration::from_millis(750), wait_for(&watcher, expected))
                    .await
                    .expect("live signal delivery, not periodic reread");
                assert_eq!(
                    settings.legacy_reads.load(Ordering::SeqCst)
                        + settings.modern_reads.load(Ordering::SeqCst),
                    reads
                );
                if previous != expected {
                    assert!(
                        wakeups.load(Ordering::SeqCst) > wakes,
                        "signal must request a UI repaint"
                    );
                }
            }
            emit(&connection, "unrelated", KEY, 1).await;
            emit(&connection, NAMESPACE, "unrelated", 1).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(watcher.theme(), Some(Theme::Light));
            assert!(
                wakeups.load(Ordering::SeqCst) >= 5,
                "source changes must wake the UI"
            );
            connection.close().await.unwrap();
            wait_for(&watcher, None).await;
            let (_replacement, settings) = service(&bus, 2, false, false).await;
            settings.value.store(1, Ordering::SeqCst);
            wait_for(&watcher, Some(Theme::Dark)).await;
        }
    }

    #[tokio::test]
    async fn portal_subscription_precedes_initial_read_and_receives_racing_signal() {
        let bus = Bus::new();
        let (_connection, _) = service(&bus, 2, false, true).await;
        let watcher = Watcher::start_at(Some(bus.address.clone()), || {});
        // The read returns old Dark while emitting Light before its reply.
        tokio::time::timeout(Duration::from_secs(1), async {
            while watcher.theme() != Some(Theme::Light) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("queued SettingChanged must win before the periodic read");
    }
    #[tokio::test]
    async fn stopping_owner_ends_worker_even_with_deferred_viewport_clone() {
        let bus = Bus::new();
        let watcher = Arc::new(Watcher::start_at(Some(bus.address.clone()), || {}));
        let deferred = watcher.clone();
        watcher.stop();
        drop(watcher);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !deferred.finished.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("UI owner must cancel watcher without releasing callback clones");
    }
}
