//! Each native event loop owns its process's main thread. Only the tray parent polls.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod app;
use token_ashtray::config;
mod host;
mod ipc;
// Compile the vendored scheduler regression in the ordinary application tests.
#[cfg(test)]
#[path = "../vendor/eframe/src/native/hidden_repaint.rs"]
mod hidden_window_repaint;
#[cfg(test)]
mod windows_first_show {
    use eframe::egui;
    include!("../vendor/eframe/src/native/first_show.rs");
}
mod provider_logos;
// Exercise the actual Windows cursor-entry dispatch contract in ordinary tests.
#[cfg(test)]
#[path = "../vendor/winit/src/platform_impl/windows/cursor_movement.rs"]
mod windows_cursor_movement;
// Exercise the Windows appearance-change refresh order in ordinary tests.
#[cfg(test)]
#[path = "../vendor/winit/src/platform_impl/windows/theme_refresh.rs"]
mod windows_theme_refresh;
use std::sync::{Arc, OnceLock};
pub use token_ashtray::platform;
use token_ashtray::platform::instance::{self, Instance, Primary};
use token_ashtray::updates::{Updater, UpdaterConfig, apply::platform_installer};

#[derive(Debug, PartialEq, Eq)]
enum Startup<P> {
    HostExited,
    Standalone {
        notice: Option<String>,
        primary: Option<P>,
    },
    /// The running instance accepted the open-board request.
    Activated,
    /// A login autostart found the app already running.
    AlreadyRunning,
}

/// What a launch found: its own guard, or a running instance plus a hand-off
/// that returns the guard when that instance exits first.
enum Launch<P, H> {
    Primary(P),
    Running(H),
}

fn decide_startup<P, H: FnOnce() -> Result<Option<P>, String>>(
    remote_ui: bool,
    no_tray: bool,
    autostart: bool,
    launch: impl FnOnce() -> Launch<P, H>,
    run_host: impl FnOnce(bool, &P) -> Result<Option<String>, String>,
) -> Result<Startup<P>, String> {
    // The host's UI child belongs to the instance that started it.
    if remote_ui {
        return Ok(Startup::Standalone {
            notice: None,
            primary: None,
        });
    }
    let primary = match launch() {
        Launch::Primary(primary) => primary,
        Launch::Running(_) if autostart => return Ok(Startup::AlreadyRunning),
        Launch::Running(hand_off) => match hand_off()? {
            None => return Ok(Startup::Activated),
            Some(primary) => primary,
        },
    };
    if no_tray {
        return Ok(Startup::Standalone {
            notice: None,
            primary: Some(primary),
        });
    }
    Ok(match run_host(autostart, &primary)? {
        None => Startup::HostExited,
        Some(_) => Startup::Standalone {
            notice: Some(app::TRAY_UNAVAILABLE_NOTICE.to_owned()),
            primary: Some(primary),
        },
    })
}

fn launch() -> Launch<Primary, impl FnOnce() -> Result<Option<Primary>, String>> {
    let Some(scope) = instance::Scope::user_session() else {
        return Launch::Primary(Primary::unguarded());
    };
    match instance::acquire(&scope) {
        Ok(Instance::Primary(primary)) => Launch::Primary(primary),
        Ok(Instance::Running(running)) => Launch::Running(move || running.activate()),
        Err(error) => {
            eprintln!("Token Ashtray: {error}; starting without the single-instance guard.");
            Launch::Primary(Primary::unguarded())
        }
    }
}

/// Only the primary instance owns an updater; a UI child never does.
fn start_updater(primary: &Primary, notify: Arc<dyn Fn() + Send + Sync>) -> Updater {
    let enabled = config::ConfigStore::load()
        .preferences()
        .board
        .updates_enabled;
    Updater::start(primary, UpdaterConfig::production(), enabled, notify)
}

/// After a normal Quit, hands a verified staged update to the installer.
fn apply_after_exit(updater: Updater) {
    let Some(installer) = platform_installer() else {
        return;
    };
    let result = std::env::current_exe()
        .and_then(|running| updater.after_exit(installer.as_ref(), &running));
    if let Err(error) = result {
        eprintln!("Token Ashtray: the downloaded update was not started: {error}");
    }
}

fn run_host(start_minimized: bool, primary: &Primary) -> Result<Option<String>, String> {
    // The host loop polls the status, so the updater needs no wake.
    let updater = start_updater(primary, Arc::new(|| {}));
    let result = host::run(start_minimized, primary.listener(), &updater);
    if matches!(result, Ok(None)) {
        apply_after_exit(updater);
    }
    result
}

fn main() -> eframe::Result<()> {
    let arguments = std::env::args().collect::<Vec<_>>();
    let remote_ui = arguments.iter().any(|arg| arg == "--ui-child");
    let no_tray = arguments.iter().any(|arg| arg == "--no-tray");
    let autostart = arguments.iter().any(|arg| arg == "--autostart");
    // The Windows UI child keeps the guard object alive without owning it.
    #[cfg(windows)]
    let _app_process = remote_ui
        .then(instance::Scope::user_session)
        .flatten()
        .map(|scope| scope.mark_app_process());
    let startup = decide_startup(remote_ui, no_tray, autostart, launch, run_host)
        .map_err(|error| eframe::Error::AppCreation(Box::new(std::io::Error::other(error))))?;
    let Startup::Standalone {
        notice: startup_notice,
        primary: mut guard,
    } = startup
    else {
        return Ok(());
    };
    // The guard stays on this thread until the board exits.
    let activation = guard.as_mut().and_then(Primary::take_listener);
    let repaint = Arc::new(OnceLock::<eframe::egui::Context>::new());
    let updater = guard.as_ref().map(|primary| {
        let repaint = Arc::clone(&repaint);
        Arc::new(start_updater(
            primary,
            Arc::new(move || {
                if let Some(ctx) = repaint.get() {
                    ctx.request_repaint();
                }
            }),
        ))
    });
    let board_updater = updater.clone();
    let options = eframe::NativeOptions {
        viewport: app::initial_viewport(remote_ui),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Token Ashtray",
        options,
        Box::new(move |creation| {
            let result = if remote_ui {
                app::AshtrayApp::new_remote(creation, ipc::UiConnection::from_stdio())
            } else {
                let _ = repaint.set(creation.egui_ctx.clone());
                app::AshtrayApp::new(creation, false, false, startup_notice).map(|app| {
                    app.with_activation(activation, &creation.egui_ctx)
                        .with_updater(board_updater)
                })
            };
            result
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::other(error))
                })
        }),
    );
    // The board has dropped its handle, so this is the only one left.
    if result.is_ok()
        && let Some(updater) = updater.and_then(|updater| Arc::try_unwrap(updater).ok())
    {
        apply_after_exit(updater);
    }
    result
}

/// Polls `condition` for up to ten seconds in tests of background work.
#[cfg(test)]
mod test_wait {
    use std::time::{Duration, Instant};

    pub(crate) fn until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod startup_tests {
    use super::{Launch, Startup, decide_startup};
    use std::cell::Cell;

    /// A stand-in guard; the real one is tested in `platform::instance`.
    #[derive(Debug, PartialEq, Eq)]
    struct Guard;

    type HandOff = fn() -> Result<Option<Guard>, String>;

    fn first() -> Launch<Guard, HandOff> {
        Launch::Primary(Guard)
    }

    #[test]
    fn tray_failure_opens_board_with_plain_notice_in_regular_and_autostart_modes() {
        for autostart in [false, true] {
            let startup = decide_startup(false, false, autostart, first, |start_minimized, _| {
                assert_eq!(start_minimized, autostart);
                Ok(Some(
                    "No usable StatusNotifier tray host: org.freedesktop.DBus.Error.ServiceUnknown at /run/user/1000/bus: os error 2".into(),
                ))
            })
            .unwrap();
            assert_eq!(
                startup,
                Startup::Standalone {
                    notice: Some(
                        "Tray support is unavailable; the board is open without a tray icon."
                            .into()
                    ),
                    primary: Some(Guard),
                }
            );
        }
    }

    #[test]
    fn working_tray_autostart_does_not_open_standalone_board() {
        assert_eq!(
            decide_startup(false, false, true, first, |_, _| Ok(None)).unwrap(),
            Startup::HostExited
        );
    }

    #[test]
    fn explicit_no_tray_skips_tray_and_has_no_failure_notice() {
        assert_eq!(
            decide_startup(false, true, false, first, |_, _| panic!(
                "tray must not start"
            ))
            .unwrap(),
            Startup::Standalone {
                notice: None,
                primary: Some(Guard),
            }
        );
    }

    #[test]
    fn second_launch_activates_running_instance_without_starting_host() {
        for no_tray in [false, true] {
            let activations = Cell::new(0);
            let startup = decide_startup(
                false,
                no_tray,
                false,
                || {
                    Launch::Running(|| {
                        activations.set(activations.get() + 1);
                        Ok(None)
                    })
                },
                |_, _: &Guard| panic!("a second launch must not start a host"),
            )
            .unwrap();
            assert_eq!(startup, Startup::Activated, "no_tray={no_tray}");
            assert_eq!(activations.get(), 1);
        }
    }

    #[test]
    fn autostart_second_launch_is_silent() {
        let startup = decide_startup(
            false,
            false,
            true,
            || Launch::Running(|| panic!("autostart must not open the running board")),
            |_, _: &Guard| panic!("a second launch must not start a host"),
        )
        .unwrap();
        assert_eq!(startup, Startup::AlreadyRunning);
    }

    #[test]
    fn ui_child_never_consults_guard() {
        assert_eq!(
            decide_startup(
                true,
                false,
                false,
                || -> Launch<Guard, HandOff> { panic!("the UI child must not take the guard") },
                |_, _| panic!("the UI child must not start a host"),
            )
            .unwrap(),
            Startup::Standalone {
                notice: None,
                primary: None,
            }
        );
    }

    #[test]
    fn launch_during_quit_starts_with_the_released_guard() {
        let mut hosted = false;
        let startup = decide_startup(
            false,
            false,
            false,
            || Launch::Running(|| Ok(Some(Guard))),
            |start_minimized, guard| {
                assert!(!start_minimized);
                assert_eq!(guard, &Guard);
                hosted = true;
                Ok(None)
            },
        )
        .unwrap();
        assert_eq!(startup, Startup::HostExited);
        assert!(hosted, "the relaunch must start its own host");
    }

    #[test]
    fn unanswered_hand_off_exits_without_a_second_instance() {
        let error = decide_startup(
            false,
            false,
            false,
            || Launch::Running(|| Err("did not respond".into())),
            |_, _: &Guard| panic!("a second launch must not start a host"),
        )
        .unwrap_err();
        assert_eq!(error, "did not respond");
    }
}
