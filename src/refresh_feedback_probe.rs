//! Credential-free native rendering probe. Run only on an isolated QA desktop.
#![allow(dead_code)]
extern crate self as token_ashtray;
extern crate token_ashtray as product;
pub use product::{config, model, platform, providers, refresh, updates};
mod app;
mod host;
mod ipc;
pub mod poll;
mod provider_logos;
use async_trait::async_trait;
use eframe::egui;
use providers::{Credentials, Descriptor, Provider, ProviderError, Usage};
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(serde::Serialize, serde::Deserialize)]
struct Options {
    output: PathBuf,
    reduced_motion: bool,
    standalone: bool,
    config: PathBuf,
}
struct Synthetic {
    id: &'static str,
    delay: Duration,
    calls: AtomicUsize,
    log: PathBuf,
}
#[async_trait]
impl Provider for Synthetic {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            id: self.id,
            name: self.id,
            link: "",
            login: "Synthetic probe; no account access",
        }
    }
    fn discover(&self) -> providers::Result<Credentials> {
        Ok(Credentials::token("synthetic".into()))
    }
    async fn fetch(
        &self,
        _: &reqwest::Client,
        _: &mut Credentials,
    ) -> providers::Result<serde_json::Value> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .unwrap();
        writeln!(log, "{} attempt {call}", self.id).unwrap();
        tokio::time::sleep(self.delay).await;
        if self.id == "claude" && (call == 2 || call == 4) {
            return Err(ProviderError {
                status: model::Status::Error,
                message: "Synthetic temporary failure; cached quota retained".into(),
                retry_after_secs: Some(6),
            });
        }
        Ok(serde_json::Value::Null)
    }
    fn parse(&self, _: &serde_json::Value) -> providers::Result<Usage> {
        Ok(Usage {
            meters: vec![model::Meter {
                label: "Weekly".into(),
                used_percent: 25.0,
                resets_at: None,
            }],
            ..Default::default()
        })
    }
}
fn options_path() -> PathBuf {
    std::env::temp_dir().join("refresh-probe-options.json")
}
fn options() -> Options {
    serde_json::from_slice(&std::fs::read(options_path()).unwrap()).unwrap()
}
fn start(
    board: poll::SharedBoard,
    mut settings: poll::PollSettings,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Result<poll::Poller, String> {
    let output = options().output;
    settings.interval_secs = 10;
    poll::Poller::with_providers(
        board,
        settings,
        wake,
        vec![
            Arc::new(Synthetic {
                id: "codex",
                delay: Duration::from_millis(600),
                calls: AtomicUsize::new(0),
                log: output.join("codex-calls.txt"),
            }),
            Arc::new(Synthetic {
                id: "claude",
                delay: Duration::from_millis(2500),
                calls: AtomicUsize::new(0),
                log: output.join("claude-calls.txt"),
            }),
        ],
        Duration::from_secs(5),
        Duration::from_secs(4),
    )
}
struct Capture {
    app: app::AshtrayApp,
    options: Options,
    began: Instant,
    manual: usize,
    capture: usize,
}
impl eframe::App for Capture {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let elapsed = self.began.elapsed().as_secs_f32();
        for event in ctx.input(|input| input.events.clone()) {
            if let egui::Event::Screenshot {
                user_data, image, ..
            } = event
            {
                let label = user_data
                    .data
                    .as_ref()
                    .and_then(|value| value.downcast_ref::<String>())
                    .unwrap();
                let mut file =
                    std::fs::File::create(self.options.output.join(format!("{label}.ppm")))
                        .unwrap();
                write!(file, "P6\n{} {}\n255\n", image.size[0], image.size[1]).unwrap();
                for pixel in &image.pixels {
                    file.write_all(&pixel.to_array()[..3]).unwrap();
                }
            }
        }
        // Exercise the same request method used by toolbar mouse and keyboard.
        let requests = [5.0, 5.1, 9.0];
        if self.manual < requests.len() && elapsed >= requests[self.manual] {
            self.app.request_refresh();
            self.manual += 1;
        }
        self.app.update(ctx, frame);
        let captures = [
            (0.2, "startup-running"),
            (1.0, "startup-fast-finished"),
            (3.0, "startup-complete"),
            (5.3, "manual-running"),
            (6.0, "manual-fast-finished"),
            (8.0, "manual-partial-failure"),
            (9.3, "manual-backoff-waiting"),
            (10.0, "manual-fast-done-slow-waiting"),
            (14.0, "manual-slow-running"),
            (16.5, "manual-complete"),
            (21.0, "scheduled-quiet"),
            (27.0, "scheduled-failure-quiet"),
            (34.5, "scheduled-retry-quiet"),
            (37.0, "scheduled-complete-quiet"),
        ];
        if self.capture < captures.len() && elapsed >= captures[self.capture].0 {
            let mut states = std::collections::BTreeMap::new();
            for id in ["codex", "claude"] {
                let state = ctx.accesskit_node_builder(egui::Id::new(("refresh-card-state", id)), |node| {
                    serde_json::json!({ "text": node.label().or(node.value()), "live": format!("{:?}", node.live()) })
                });
                states.insert(id, state);
            }
            std::fs::write(
                self.options
                    .output
                    .join(format!("{}.json", captures[self.capture].1)),
                serde_json::to_vec_pretty(&states).unwrap(),
            )
            .unwrap();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                captures[self.capture].1.to_owned(),
            )));
            self.capture += 1;
        }
        if elapsed >= 39.0 {
            self.app.send_closed(true);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(25));
    }
}
fn main() -> eframe::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let child = args.iter().any(|arg| arg == "--ui-child");
    if !child && !args.iter().any(|arg| arg == "--probe-session") {
        let output = args
            .windows(2)
            .find(|pair| pair[0] == "--output-dir")
            .map(|pair| PathBuf::from(&pair[1]))
            .expect("--output-dir is required");
        std::fs::create_dir_all(&output).unwrap();
        let output = std::fs::canonicalize(output).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let options = Options {
            output: output.clone(),
            reduced_motion: args.iter().any(|arg| arg == "--reduced-motion"),
            standalone: args.iter().any(|arg| arg == "--standalone"),
            config: temporary.path().join("probe-config.toml"),
        };
        std::fs::write(
            temporary.path().join("refresh-probe-options.json"),
            serde_json::to_vec(&options).unwrap(),
        )
        .unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("--probe-session")
            .env("TEMP", temporary.path())
            .env("TMP", temporary.path())
            .env("TMPDIR", temporary.path());
        for name in [
            "APPDATA",
            "LOCALAPPDATA",
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
        ] {
            let path = temporary.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            command.env(name, path);
        }
        let log = std::fs::File::create(output.join("native-probe.log")).unwrap();
        let status = command.stderr(log).status().unwrap();
        assert!(status.success(), "native probe exited: {status}");
        return Ok(());
    }
    let options = options();
    if !child {
        let mut config = config::ConfigStore::load_from(options.config.clone());
        config.preferences_mut().board.hidden = providers::all()
            .iter()
            .map(|provider| provider.descriptor().id.to_owned())
            .filter(|id| id != "codex" && id != "claude")
            .collect();
        config.preferences_mut().window.width = 720.0;
        config.preferences_mut().window.height = 680.0;
        config.save().unwrap();
        if !options.standalone {
            match host::run_with_poller(
                false,
                config::ConfigStore::load_from(options.config.clone()),
                start,
                None,
                None,
            )
            .unwrap()
            {
                None => return Ok(()),
                Some(error) => panic!("tray-hosted probe requires a native tray: {error}"),
            }
        }
    }
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Token Ashtray refresh probe")
            .with_inner_size([720.0, 680.0])
            .with_visible(!child),
        ..Default::default()
    };
    eframe::run_native(
        "Token Ashtray",
        native,
        Box::new(move |creation| {
            let app = if child {
                app::AshtrayApp::new_remote_with_config(
                    creation,
                    ipc::UiConnection::from_stdio(),
                    config::ConfigStore::load_from(options.config.clone()),
                )
            } else {
                app::AshtrayApp::new_with_poller(
                    creation,
                    false,
                    false,
                    None,
                    config::ConfigStore::load_from(options.config.clone()),
                    start,
                )
            }
            .map_err(std::io::Error::other)?;
            creation.egui_ctx.enable_accesskit();
            if options.reduced_motion {
                creation
                    .egui_ctx
                    .all_styles_mut(|style| style.animation_time = 0.0);
            }
            Ok(Box::new(Capture {
                app,
                options,
                began: Instant::now(),
                manual: 0,
                capture: 0,
            }))
        }),
    )
}
