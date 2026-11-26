//! Private inherited pipes carry normalized views and controls, never credentials.
use crate::platform::TrayAction;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};
use token_ashtray::{
    model::ProviderCard, poll::SharedBoard, refresh::RefreshSnapshot, updates::UpdateStatus,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum ParentMessage {
    Snapshot(BTreeMap<String, ProviderCard>),
    RefreshState {
        snapshot: RefreshSnapshot,
        baseline: bool,
    },
    RefreshAccepted {
        request: u64,
        result: Result<u64, String>,
    },
    Action(TrayAction),
    Shutdown,
    CloseAcknowledged,
    UpdateStatus(UpdateStatus),
    /// The host could not write the preferences the child sent.
    PreferencesSaveFailed(String),
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum ChildMessage {
    Refresh { request: u64 },
    SavePreferences(crate::config::Preferences),
    Closed { quit: bool },
}

type Wake = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;
fn wake_ui(wake: &Wake) {
    let callback = wake.lock().ok().and_then(|value| value.clone());
    if let Some(callback) = callback {
        callback();
    }
}

pub struct UiConnection {
    pub board: SharedBoard,
    pub feedback: Arc<RwLock<RefreshSnapshot>>,
    /// The host updater's status; the child never checks for updates itself.
    pub update_status: Arc<RwLock<UpdateStatus>>,
    /// The host's last preferences save failure, until the board shows it.
    pub save_failure: Arc<Mutex<Option<String>>>,
    pub suppressed_through: Arc<AtomicU64>,
    replies: mpsc::Receiver<(u64, Result<u64, String>)>,
    actions: mpsc::Receiver<TrayAction>,
    outgoing: mpsc::SyncSender<ChildMessage>,
    disconnected: Arc<AtomicBool>,
    close_acknowledged: Arc<AtomicBool>,
    wake: Wake,
}
impl UiConnection {
    #[cfg(test)]
    pub(crate) fn for_test() -> (Self, mpsc::Sender<TrayAction>, mpsc::Receiver<ChildMessage>) {
        let (sender, actions) = mpsc::channel();
        let (outgoing, messages) = mpsc::sync_channel(64);
        let (_, replies) = mpsc::channel();
        (
            Self {
                board: Arc::new(RwLock::new(BTreeMap::new())),
                feedback: Arc::new(RwLock::new(RefreshSnapshot::default())),
                update_status: Arc::new(RwLock::new(UpdateStatus::Off)),
                save_failure: Arc::new(Mutex::new(None)),
                suppressed_through: Arc::new(AtomicU64::new(0)),
                replies,
                actions,
                outgoing,
                disconnected: Arc::new(AtomicBool::new(false)),
                close_acknowledged: Arc::new(AtomicBool::new(true)),
                wake: Arc::new(Mutex::new(None)),
            },
            sender,
            messages,
        )
    }

    pub fn from_stdio() -> Self {
        let board = Arc::new(RwLock::new(BTreeMap::new()));
        let feedback = Arc::new(RwLock::new(RefreshSnapshot::default()));
        let input_feedback = feedback.clone();
        let update_status = Arc::new(RwLock::new(UpdateStatus::Off));
        let input_update_status = update_status.clone();
        let save_failure = Arc::new(Mutex::new(None));
        let input_save_failure = save_failure.clone();
        let suppressed_through = Arc::new(AtomicU64::new(0));
        let input_suppressed = suppressed_through.clone();
        let (replies_tx, replies) = mpsc::channel();
        let (actions_tx, actions) = mpsc::channel();
        let (outgoing, messages) = mpsc::sync_channel::<ChildMessage>(64);
        let disconnected = Arc::new(AtomicBool::new(false));
        let close_acknowledged = Arc::new(AtomicBool::new(false));
        let input_ack = close_acknowledged.clone();
        let wake: Wake = Arc::new(Mutex::new(None));
        let input_board = board.clone();
        let input_disconnected = disconnected.clone();
        let input_wake = wake.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(message) = serde_json::from_str::<ParentMessage>(&line) else {
                    break;
                };
                match message {
                    ParentMessage::Snapshot(snapshot) => {
                        if let Ok(mut board) = input_board.write() {
                            *board = snapshot;
                        }
                    }
                    ParentMessage::RefreshState { snapshot, baseline } => {
                        if baseline {
                            input_suppressed.store(snapshot.generation, Ordering::Release);
                        }
                        *input_feedback.write().expect("refresh feedback lock") = snapshot;
                    }
                    ParentMessage::RefreshAccepted { request, result } => {
                        let _ = replies_tx.send((request, result));
                    }
                    ParentMessage::Action(action) => {
                        let _ = actions_tx.send(action);
                    }
                    ParentMessage::Shutdown => {
                        let _ = actions_tx.send(TrayAction::Quit);
                    }
                    ParentMessage::CloseAcknowledged => {
                        input_ack.store(true, Ordering::Release);
                    }
                    ParentMessage::UpdateStatus(status) => {
                        *input_update_status.write().expect("update status lock") = status;
                    }
                    ParentMessage::PreferencesSaveFailed(error) => {
                        *input_save_failure.lock().expect("save failure lock") = Some(error);
                    }
                }
                wake_ui(&input_wake);
            }
            input_disconnected.store(true, Ordering::Release);
            wake_ui(&input_wake);
        });
        let output_disconnected = disconnected.clone();
        let output_wake = wake.clone();
        std::thread::spawn(move || {
            let mut output = std::io::stdout().lock();
            for message in messages {
                if write_message(&mut output, &message).is_err() {
                    output_disconnected.store(true, Ordering::Release);
                    wake_ui(&output_wake);
                    break;
                }
            }
        });
        Self {
            board,
            feedback,
            update_status,
            save_failure,
            suppressed_through,
            replies,
            actions,
            outgoing,
            disconnected,
            close_acknowledged,
            wake,
        }
    }
    pub fn set_wake(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.wake.lock().expect("IPC wake lock") = Some(callback);
        wake_ui(&self.wake);
    }
    pub fn sender(&self) -> mpsc::SyncSender<ChildMessage> {
        self.outgoing.clone()
    }
    pub fn action(&self) -> Option<TrayAction> {
        self.actions.try_recv().ok()
    }
    pub fn refresh_reply(&self) -> Option<(u64, Result<u64, String>)> {
        self.replies.try_recv().ok()
    }
    pub fn try_send(&self, message: ChildMessage) -> Result<(), String> {
        if self.disconnected() {
            return Err("Refresh unavailable: tray connection lost".into());
        }
        self.outgoing
            .try_send(message)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "Refresh not sent: tray connection is busy".into(),
                mpsc::TrySendError::Disconnected(_) => {
                    "Refresh not sent: tray connection lost".into()
                }
            })
    }
    pub fn send(&self, message: ChildMessage) {
        let closing = matches!(message, ChildMessage::Closed { .. });
        if self.outgoing.try_send(message).is_err() {
            return;
        }
        if closing {
            // Wait for the parent's decision, not just a flushed stdout buffer.
            // This prevents process exit racing the parent's message reader.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while !self.close_acknowledged.load(Ordering::Acquire)
                && !self.disconnected()
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    pub fn disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }
}

pub fn write_message(writer: &mut impl Write, message: &impl Serialize) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, message).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn allowed_ui_environment(name: &std::ffi::OsStr) -> bool {
    const ALLOWED: &[&str] = &[
        "HOME",
        "USER",
        "LOGNAME",
        "USERNAME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
        "TMPDIR",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATH",
        "PATHEXT",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_DATA_DIRS",
        "XDG_CACHE_HOME",
        "DBUS_SESSION_BUS_ADDRESS",
        "GDK_BACKEND",
        "WINIT_UNIX_BACKEND",
        "WINIT_X11_SCALE_FACTOR",
        "GDK_SCALE",
        "GDK_DPI_SCALE",
        "QT_SCALE_FACTOR",
        "LIBGL_ALWAYS_SOFTWARE",
        "GALLIUM_DRIVER",
        "MESA_LOADER_DRIVER_OVERRIDE",
        "MESA_GL_VERSION_OVERRIDE",
        "MESA_GLSL_VERSION_OVERRIDE",
        "MESA_SHADER_CACHE_DIR",
        "__GLX_VENDOR_LIBRARY_NAME",
        "__NV_PRIME_RENDER_OFFLOAD",
        "DRI_PRIME",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "LC_CTYPE",
        "LC_MESSAGES",
        "LC_NUMERIC",
        "LC_TIME",
        "LC_COLLATE",
        "LC_MONETARY",
        "RUST_BACKTRACE",
    ];
    ALLOWED.contains(&name.to_string_lossy().to_ascii_uppercase().as_str())
}

pub struct ChildProcess {
    pub process: std::process::Child,
    pub closing_to_tray: bool,
    outgoing: mpsc::SyncSender<ParentMessage>,
    incoming: mpsc::Receiver<ChildMessage>,
}
impl ChildProcess {
    pub fn spawn(
        snapshot: BTreeMap<String, ProviderCard>,
        action: TrayAction,
        feedback: RefreshSnapshot,
        allow_cues: bool,
    ) -> std::io::Result<Self> {
        let mut command = std::process::Command::new(std::env::current_exe()?);
        // Inherit only desktop/configuration context. Authentication variables
        // such as GITHUB_TOKEN must remain in the polling parent, not the UI.
        command
            .env_clear()
            .envs(std::env::vars_os().filter(|(key, _)| allowed_ui_environment(key)));
        command
            .arg("--ui-child")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW, including startup errors.
        }
        let mut process = command.spawn()?;
        let mut input = process.stdin.take().expect("piped child stdin");
        let output = process.stdout.take().expect("piped child stdout");
        let (outgoing, messages) = mpsc::sync_channel::<ParentMessage>(32);
        let (results, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            for message in messages {
                if write_message(&mut input, &message).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(message) = serde_json::from_str::<ChildMessage>(&line) else {
                    break;
                };
                if results.send(message).is_err() {
                    break;
                }
            }
        });
        let child = Self {
            process,
            closing_to_tray: false,
            outgoing,
            incoming,
        };
        child.send(ParentMessage::RefreshState {
            snapshot: feedback,
            baseline: !allow_cues,
        });
        child.send(ParentMessage::Snapshot(snapshot));
        child.send(ParentMessage::Action(action));
        Ok(child)
    }
    pub fn send(&self, message: ParentMessage) -> bool {
        self.outgoing.try_send(message).is_ok()
    }
    pub fn receive(&self) -> Option<ChildMessage> {
        self.incoming.try_recv().ok()
    }
    pub fn stop(&mut self) {
        self.send(ParentMessage::Shutdown);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if self.process.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn preferences_save_failure_reaches_the_child_over_the_wire() {
        let message = ParentMessage::PreferencesSaveFailed("Access is denied.".into());
        let wire = serde_json::to_string(&message).unwrap();
        let ParentMessage::PreferencesSaveFailed(error) = serde_json::from_str(&wire).unwrap()
        else {
            panic!("expected a save failure message");
        };
        assert_eq!(error, "Access is denied.");
    }

    #[test]
    fn update_status_reaches_the_child_over_the_wire() {
        use token_ashtray::updates::{Failure, Unavailable, UpdateStatus};
        for status in [
            UpdateStatus::Off,
            UpdateStatus::Unavailable(Unavailable::NoTrustRoot),
            UpdateStatus::Ready("0.2.2".into()),
            UpdateStatus::Failed(Failure::Network),
            UpdateStatus::InstallFailed {
                version: "0.2.2".into(),
                attempt: 2,
            },
        ] {
            let wire = serde_json::to_string(&ParentMessage::UpdateStatus(status.clone())).unwrap();
            let ParentMessage::UpdateStatus(received) = serde_json::from_str(&wire).unwrap() else {
                panic!("expected an update status message");
            };
            assert_eq!(received, status);
        }
    }

    use super::*;
    #[test]
    fn refresh_send_reports_full_and_disconnected_channels() {
        let (connection, _, messages) = UiConnection::for_test();
        for request in 0..64 {
            connection
                .try_send(ChildMessage::Refresh { request })
                .unwrap();
        }
        assert!(
            connection
                .try_send(ChildMessage::Refresh { request: 65 })
                .unwrap_err()
                .contains("busy")
        );
        drop(messages);
        assert!(
            connection
                .try_send(ChildMessage::Refresh { request: 66 })
                .unwrap_err()
                .contains("lost")
        );
        connection.disconnected.store(true, Ordering::Release);
        assert!(
            connection
                .try_send(ChildMessage::Refresh { request: 67 })
                .unwrap_err()
                .contains("lost")
        );
    }
    #[test]
    fn refresh_lifecycle_and_ack_round_trip_without_quota_changes() {
        let snapshot = RefreshSnapshot {
            generation: 42,
            startup: false,
            providers: [
                (
                    "fast".into(),
                    token_ashtray::refresh::RefreshPhase::Succeeded,
                ),
                ("slow".into(), token_ashtray::refresh::RefreshPhase::Running),
            ]
            .into(),
        };
        let encoded = serde_json::to_string(&ParentMessage::RefreshState {
            snapshot: snapshot.clone(),
            baseline: false,
        })
        .unwrap();
        let ParentMessage::RefreshState {
            snapshot: decoded,
            baseline,
        } = serde_json::from_str(&encoded).unwrap()
        else {
            panic!("refresh snapshot");
        };
        assert_eq!(decoded, snapshot);
        assert!(!baseline);
        let ack = serde_json::to_string(&ParentMessage::RefreshAccepted {
            request: 7,
            result: Ok(42),
        })
        .unwrap();
        assert!(matches!(
            serde_json::from_str::<ParentMessage>(&ack).unwrap(),
            ParentMessage::RefreshAccepted {
                request: 7,
                result: Ok(42)
            }
        ));
    }
    #[test]
    fn child_environment_excludes_credentials_and_retains_desktop_context() {
        for key in [
            "GITHUB_TOKEN",
            "GITHUB_COPILOT_API_TOKEN",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_SESSION_TOKEN",
            "UNKNOWN_SECRET",
        ] {
            assert!(!allowed_ui_environment(std::ffi::OsStr::new(key)), "{key}");
        }
        for key in [
            "DISPLAY",
            "XDG_CONFIG_HOME",
            "DBUS_SESSION_BUS_ADDRESS",
            "Path",
            "SystemRoot",
            "LC_MESSAGES",
        ] {
            assert!(allowed_ui_environment(std::ffi::OsStr::new(key)), "{key}");
        }
    }
    #[test]
    fn normalized_snapshots_round_trip_without_authentication_material() {
        let message =
            ParentMessage::Snapshot(token_ashtray::poll::initial_board().read().unwrap().clone());
        let encoded = serde_json::to_string(&message).unwrap();
        let ParentMessage::Snapshot(cards) = serde_json::from_str(&encoded).unwrap() else {
            panic!("snapshot")
        };
        assert_eq!(cards.len(), 9);
        assert!(cards.contains_key("antigravity"));
        assert!(cards.contains_key("vibe"));
        assert!(!encoded.contains("access_token"));
        assert!(!encoded.contains("refresh_token"));
    }
}
