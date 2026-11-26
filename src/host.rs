//! Persistent tray owner. The only scheduler and credential access live here.
use crate::{
    config::{BoardPreferences, ConfigStore},
    ipc::{ChildMessage, ChildProcess, ParentMessage},
    platform::{self, TrayAction, TrayProvider, TraySnapshot, instance::Listener},
};
use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use token_ashtray::{
    model::{ProviderCard, Status, duration_label},
    poll::{PollSettings, Poller, SharedBoard, initial_board},
    updates::Updater,
};

fn settings(preferences: &BoardPreferences) -> PollSettings {
    PollSettings {
        interval_secs: preferences.refresh_secs.max(60),
        hidden: preferences.hidden.iter().cloned().collect(),
    }
}
fn snapshot(board: &SharedBoard, preferences: &BoardPreferences) -> TraySnapshot {
    let board = board.read().expect("board lock");
    let used = |card: &ProviderCard| {
        (!preferences.hidden.contains(&card.id)
            && card.effective_status(preferences.refresh_secs.max(60)) == Status::Live)
            .then(|| card.used())
            .flatten()
    };
    let primary = preferences
        .primary
        .as_ref()
        .and_then(|id| board.get(id))
        .or_else(|| board.values().find(|card| used(card).is_some()));
    TraySnapshot {
        providers: board
            .values()
            .map(|card| TrayProvider {
                name: card.short_name.clone(),
                used: used(card),
                reset: card
                    .tightest_meter()
                    .and_then(|meter| meter.resets_at)
                    .map(|at| {
                        duration_label((at - time::OffsetDateTime::now_utc()).whole_seconds())
                    }),
            })
            .collect(),
        quota_display_mode: preferences.quota_display_mode,
        primary_remaining: primary.and_then(used).map(|used| 100.0 - used),
        any_readable: board.values().any(|card| used(card).is_some()),
    }
}

/// Child saves update the cached tray, without waking provider loops
/// unless a scheduling preference changed. The update opt-in reaches the
/// host's updater here; its trust floors stay in the updater's own store.
/// A failed save is returned for the child to show.
fn apply_preferences(
    message: ChildMessage,
    preferences: &mut BoardPreferences,
    store: &mut ConfigStore,
    dirty: &AtomicBool,
    updater: Option<&Updater>,
    update_polling: impl FnOnce(&BoardPreferences),
) -> std::io::Result<()> {
    let ChildMessage::SavePreferences(next) = message else {
        unreachable!("only preference messages are handled here");
    };
    preferences.replace(next.board.clone(), update_polling);
    if let Some(updater) = updater {
        updater.set_enabled(preferences.updates_enabled);
    }
    *store.preferences_mut() = next;
    dirty.store(true, Ordering::Release);
    store.save()
}

// A successful UI exit is terminal only without the close-to-tray handshake.
fn child_exit_quits_host(closing_to_tray: bool, success: bool) -> bool {
    !closing_to_tray && success
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn acknowledged_close_to_tray_keeps_host_alive_after_child_exit() {
        assert!(!child_exit_quits_host(true, true));
        assert!(!child_exit_quits_host(true, false));
        assert!(child_exit_quits_host(false, true));
        assert!(!child_exit_quits_host(false, false));
    }
}

/// Returns the startup error when no tray host exists, so main opens the standalone UI.
/// `updater` belongs to the primary instance; main applies its result after Quit.
pub fn run(
    start_minimized: bool,
    activation: Option<&Listener>,
    updater: &Updater,
) -> Result<Option<String>, String> {
    run_with_poller(
        start_minimized,
        ConfigStore::load(),
        Poller::start,
        activation,
        Some(updater),
    )
}

// The native probe exercises this host with synthetic providers and private config.
pub(crate) fn run_with_poller(
    start_minimized: bool,
    mut store: ConfigStore,
    start_poller: impl FnOnce(
        SharedBoard,
        PollSettings,
        Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Poller, String>,
    activation: Option<&Listener>,
    updater: Option<&Updater>,
) -> Result<Option<String>, String> {
    let shared_tray = Arc::new(RwLock::new(TraySnapshot::default()));
    let tray = match platform::Tray::new(eframe::egui::Context::default(), shared_tray.clone()) {
        Ok(tray) => tray,
        Err(error) => {
            eprintln!("Token Ashtray: {error}; opening the board without a tray icon.");
            return Ok(Some(error));
        }
    };
    let mut preferences = store.preferences().board.clone();
    let board = initial_board();
    let dirty = Arc::new(AtomicBool::new(true));
    let wake = dirty.clone();
    let poller = start_poller(
        board.clone(),
        settings(&preferences),
        Arc::new(move || {
            wake.store(true, Ordering::Release);
        }),
    )?;
    // The status last delivered to the current UI child.
    let mut sent_status = None;
    let mut child = None::<ChildProcess>;
    let mut allow_startup_cues = !start_minimized;
    let mut pending = (!start_minimized).then_some(TrayAction::OpenBoard);
    let mut quitting = false;
    let mut last_tooltip = Instant::now();
    while !quitting {
        // Windows tray HWNDs belong to this main thread, even with no UI child.
        platform::pump_events();
        if let Some(ui) = child.as_mut() {
            while let Some(message) = ui.receive() {
                match message {
                    ChildMessage::Refresh { request } => {
                        let result = poller.refresh();
                        if !ui.send(ParentMessage::RefreshAccepted { request, result }) {
                            eprintln!("Token Ashtray: refresh acknowledgment could not be sent");
                        }
                    }
                    message @ ChildMessage::SavePreferences(_) => {
                        if let Err(error) = apply_preferences(
                            message,
                            &mut preferences,
                            &mut store,
                            &dirty,
                            updater,
                            |next| poller.set_settings(settings(next)),
                        ) {
                            eprintln!("Token Ashtray: could not save preferences: {error}");
                            ui.send(ParentMessage::PreferencesSaveFailed(error.to_string()));
                        }
                    }
                    ChildMessage::Closed { quit } => {
                        ui.closing_to_tray = !quit;
                        quitting |= quit;
                        ui.send(ParentMessage::CloseAcknowledged);
                    }
                }
            }
            if let Some(status) = ui.process.try_wait().map_err(|error| error.to_string())? {
                if child_exit_quits_host(ui.closing_to_tray, status.success()) {
                    quitting = true;
                }
                if !status.success() {
                    eprintln!(
                        "Token Ashtray: native UI exited unexpectedly; use Open board to reopen it."
                    );
                }
                child = None;
            }
        }
        let changed = dirty.swap(false, Ordering::AcqRel);
        if changed || last_tooltip.elapsed() >= Duration::from_secs(1) {
            *shared_tray.write().expect("tray snapshot lock") = snapshot(&board, &preferences);
            tray.sync();
            last_tooltip = Instant::now();
        }
        if let (Some(ui), Some(updater)) = (&child, updater) {
            let status = updater.status();
            if sent_status.as_ref() != Some(&status)
                && ui.send(ParentMessage::UpdateStatus(status.clone()))
            {
                sent_status = Some(status);
            }
        }
        if changed && let Some(ui) = &child {
            let feedback_sent = ui.send(ParentMessage::RefreshState {
                snapshot: poller.feedback(),
                baseline: false,
            });
            let board_sent = ui.send(ParentMessage::Snapshot(
                board.read().expect("board lock").clone(),
            ));
            if !feedback_sent || !board_sent {
                dirty.store(true, Ordering::Release);
            }
        }
        loop {
            let action = tray.poll();
            if action == TrayAction::None {
                break;
            }
            match action {
                TrayAction::Refresh => {
                    if let Err(error) = poller.refresh() {
                        eprintln!("Token Ashtray: {error}");
                    }
                }
                TrayAction::Quit => {
                    quitting = true;
                    break;
                }
                TrayAction::ToggleAutostart => {
                    if let Err(error) = platform::set_autostart(!platform::autostart_enabled()) {
                        eprintln!("Token Ashtray: could not change autostart: {error}");
                    }
                    tray.sync();
                }
                action => pending = Some(action),
            }
        }
        if quitting {
            break;
        }
        // A later launch opens the board like the tray does. The acknowledgement
        // waits for this loop, so a launch during Quit is not swallowed.
        while let Some(request) = activation.and_then(Listener::try_next) {
            request.acknowledge();
            pending = Some(TrayAction::OpenBoard);
        }
        if let Some(action) = pending.take() {
            match child.as_ref() {
                Some(ui) if ui.closing_to_tray => pending = Some(action),
                Some(ui) => {
                    ui.send(ParentMessage::Action(action));
                }
                None => {
                    match ChildProcess::spawn(
                        board.read().expect("board lock").clone(),
                        action,
                        poller.feedback(),
                        allow_startup_cues,
                    ) {
                        Ok(ui) => {
                            child = Some(ui);
                            allow_startup_cues = false;
                            sent_status = None;
                        }
                        Err(error) => {
                            eprintln!("Token Ashtray: could not open native board: {error}")
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    if let Some(mut ui) = child {
        ui.stop();
    }
    drop(poller); // Bounded OAuth refresh persistence completes before exit.
    Ok(None)
}

#[cfg(test)]
mod quota_display_tests {
    use super::*;
    use crate::config::{Preferences, QuotaDisplayMode};
    use token_ashtray::model::Meter;
    use token_ashtray::{platform::instance::Primary, updates::UpdaterConfig};

    fn cached_board() -> SharedBoard {
        let card = ProviderCard {
            id: "fixture".into(),
            short_name: "Fixture".into(),
            plan: None,
            price: None,
            meters: vec![Meter {
                label: "Weekly".into(),
                used_percent: 25.0,
                resets_at: None,
            }],
            usage_breakdown: vec![],
            status: Status::Live,
            detail: String::new(),
            link: String::new(),
            fetched_at: Some(time::OffsetDateTime::now_utc()),
        };
        Arc::new(RwLock::new([(card.id.clone(), card)].into()))
    }

    #[test]
    fn child_saved_update_opt_in_reaches_the_host_updater_but_no_floors() {
        use crate::test_wait::until;
        use token_ashtray::updates::{Unavailable, UpdateStatus};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::load_from(path.clone());
        let mut preferences = BoardPreferences::default();
        // An inert updater reports why it cannot run and stays silent.
        let config = UpdaterConfig::inert();
        let updater = Updater::start(&Primary::unguarded(), config, false, Arc::new(|| {}));
        until("the updater starts off", || {
            updater.status() == UpdateStatus::Off
        });
        for (enabled, expected) in [
            (true, UpdateStatus::Unavailable(Unavailable::NoTrustRoot)),
            (false, UpdateStatus::Off),
        ] {
            let message = ChildMessage::SavePreferences(Preferences {
                board: BoardPreferences {
                    updates_enabled: enabled,
                    ..preferences.clone()
                },
                ..Default::default()
            });
            let wire = serde_json::to_vec(&message).unwrap();
            apply_preferences(
                serde_json::from_slice(&wire).unwrap(),
                &mut preferences,
                &mut store,
                &AtomicBool::new(false),
                Some(&updater),
                |_| panic!("the update opt-in must not reschedule providers"),
            )
            .unwrap();
            until("the updater follows the opt-in", || {
                updater.status() == expected
            });
            let saved = ConfigStore::load_from(path.clone());
            assert_eq!(saved.preferences().board.updates_enabled, enabled);
            let text = std::fs::read_to_string(&path).unwrap();
            for floor in ["root", "sequence", "digest", "pending"] {
                assert!(!text.contains(floor), "{floor} leaked into config: {text}");
            }
        }
    }

    #[test]
    fn failed_child_save_is_returned_for_the_child_to_show() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // An unreadable file blocks saves, like any other write failure.
        std::fs::write(&path, "[board\n").unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        let mut preferences = BoardPreferences::default();
        let dirty = AtomicBool::new(false);
        let message = ChildMessage::SavePreferences(Preferences {
            board: BoardPreferences {
                quota_display_mode: QuotaDisplayMode::Used,
                ..preferences.clone()
            },
            ..Default::default()
        });
        let error = apply_preferences(message, &mut preferences, &mut store, &dirty, None, |_| {})
            .unwrap_err();
        assert!(error.to_string().contains("Settings could not be loaded"));
        // The running host still follows the choice; the file is untouched.
        assert_eq!(preferences.quota_display_mode, QuotaDisplayMode::Used);
        assert!(dirty.load(Ordering::Acquire));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[board\n");
    }

    #[test]
    fn child_save_persist_mode_and_dirty_cached_tray_without_poll_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::load_from(path.clone());
        let mut preferences = BoardPreferences::default();
        let board = cached_board();
        let before = snapshot(&board, &preferences);
        assert_eq!(before.description(), "Fixture 75% Remaining");
        let dirty = AtomicBool::new(false);
        let mut poll_events = 0;
        let next = BoardPreferences {
            quota_display_mode: QuotaDisplayMode::Used,
            ..preferences.clone()
        };
        let message = ChildMessage::SavePreferences(Preferences {
            board: next,
            ..Default::default()
        });
        // Exercise the serialized child protocol, not just assignment to host RAM.
        let wire = serde_json::to_vec(&message).unwrap();
        apply_preferences(
            serde_json::from_slice(&wire).unwrap(),
            &mut preferences,
            &mut store,
            &dirty,
            None,
            |_| poll_events += 1,
        )
        .unwrap();
        assert!(
            dirty.load(Ordering::Acquire),
            "mode-only save must invalidate tray"
        );
        assert_eq!(
            poll_events, 0,
            "presentation change must not reschedule polling"
        );
        assert_eq!(
            ConfigStore::load_from(path)
                .preferences()
                .board
                .quota_display_mode,
            QuotaDisplayMode::Used
        );
        let after = snapshot(&board, &preferences);
        assert_eq!(after.description(), "Fixture 25% Used");
        assert_eq!(after.primary_remaining, Some(75.0));
        assert_eq!(
            platform::ashtray_rgba(32, before.primary_remaining),
            platform::ashtray_rgba(32, after.primary_remaining)
        );
        assert_eq!(
            board.read().unwrap()["fixture"].meters[0].used_percent,
            25.0
        );
        let scheduling_change = BoardPreferences {
            refresh_secs: 600,
            ..preferences.clone()
        };
        apply_preferences(
            ChildMessage::SavePreferences(Preferences {
                board: scheduling_change,
                ..Default::default()
            }),
            &mut preferences,
            &mut store,
            &dirty,
            None,
            |_| poll_events += 1,
        )
        .unwrap();
        assert_eq!(
            poll_events, 1,
            "positive control: scheduling edits must notify poller"
        );
    }

    #[test]
    fn appearance_child_save_survive_reopen_and_update_without_polling() {
        use crate::config::ThemePreference;
        {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let mut store = ConfigStore::load_from(path.clone());
            let mut preferences = BoardPreferences::default();
            let board = cached_board();
            let before = serde_json::to_value(&*board.read().unwrap()).unwrap();
            let dirty = AtomicBool::new(false);
            for theme in [
                ThemePreference::Light,
                ThemePreference::Dark,
                ThemePreference::System,
            ] {
                let next = BoardPreferences {
                    theme_preference: theme,
                    ..preferences.clone()
                };
                let message = ChildMessage::SavePreferences(Preferences {
                    board: next,
                    ..Default::default()
                });
                let wire = serde_json::to_vec(&message).unwrap();
                apply_preferences(
                    serde_json::from_slice(&wire).unwrap(),
                    &mut preferences,
                    &mut store,
                    &dirty,
                    None,
                    |_| panic!("appearance must never notify provider scheduler"),
                )
                .unwrap();
                let reloaded = ConfigStore::load_from(path.clone());
                assert_eq!(reloaded.preferences().board.theme_preference, theme);
                // Reopened/replaced UI binaries deserialize the same saved file.
                let reopened: Preferences =
                    serde_json::from_slice(&serde_json::to_vec(reloaded.preferences()).unwrap())
                        .unwrap();
                assert_eq!(reopened.board.theme_preference, theme);
                assert_eq!(
                    snapshot(&board, &preferences).description(),
                    "Fixture 75% Remaining"
                );
                assert_eq!(
                    serde_json::to_value(&*board.read().unwrap()).unwrap(),
                    before
                );
            }
        }
    }

    #[test]
    fn author_visit_serialized_child_save_persist_without_provider_polling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "future = 9\n[board]\nrefresh_secs=720\nfuture_board='keep'\n",
        )
        .unwrap();
        let mut store = ConfigStore::load_from(path.clone());
        let mut preferences = store.preferences().board.clone();
        let next = BoardPreferences {
            author_youtube_visited: true,
            ..preferences.clone()
        };
        let message = ChildMessage::SavePreferences(Preferences {
            board: next,
            ..Default::default()
        });
        let wire = serde_json::to_vec(&message).unwrap();
        apply_preferences(
            serde_json::from_slice(&wire).unwrap(),
            &mut preferences,
            &mut store,
            &AtomicBool::new(false),
            None,
            |_| panic!("author visit must not reschedule providers"),
        )
        .unwrap();
        let reopened = ConfigStore::load_from(path.clone());
        assert!(reopened.preferences().board.author_youtube_visited);
        assert_eq!(reopened.preferences().board.refresh_secs, 720);
        let updated: Preferences =
            serde_json::from_slice(&serde_json::to_vec(reopened.preferences()).unwrap()).unwrap();
        assert!(updated.board.author_youtube_visited);
        let source = std::fs::read_to_string(path).unwrap();
        assert!(source.contains("future = 9"));
        assert_eq!(
            source.parse::<toml_edit::DocumentMut>().unwrap()["board"]["future_board"].as_str(),
            Some("keep")
        );
    }

    #[test]
    fn tray_tooltip_shows_the_same_number_as_the_meter_near_a_tie() {
        let board = cached_board();
        for used in [0.499_999_9_f32, 12.499_998, 50.5, 99.6] {
            board.write().unwrap().get_mut("fixture").unwrap().meters[0].used_percent = used;
            for mode in [QuotaDisplayMode::Used, QuotaDisplayMode::Remaining] {
                let preferences = BoardPreferences {
                    quota_display_mode: mode,
                    ..Default::default()
                };
                let meter = format!("Fixture {} {}", mode.percent_label(used), mode.label());
                assert_eq!(
                    snapshot(&board, &preferences).description(),
                    meter,
                    "{used} {mode:?}"
                );
            }
        }
    }

    #[test]
    fn host_tray_keeps_stale_error_missing_and_hidden_quota_unavailable() {
        let board = cached_board();
        let mut preferences = BoardPreferences {
            quota_display_mode: QuotaDisplayMode::Used,
            ..Default::default()
        };
        for status in [Status::Stale, Status::Error, Status::Reconnect] {
            board.write().unwrap().get_mut("fixture").unwrap().status = status;
            let view = snapshot(&board, &preferences);
            assert_eq!(view.primary_remaining, None);
            assert_eq!(view.providers[0].used, None);
            assert!(!view.description().contains('%'));
        }
        {
            let mut cards = board.write().unwrap();
            let card = cards.get_mut("fixture").unwrap();
            card.status = Status::Live;
            card.fetched_at = Some(time::OffsetDateTime::now_utc() - time::Duration::hours(1));
        }
        assert_eq!(snapshot(&board, &preferences).providers[0].used, None);
        board
            .write()
            .unwrap()
            .get_mut("fixture")
            .unwrap()
            .fetched_at = None;
        preferences.hidden.insert("fixture".into());
        assert_eq!(snapshot(&board, &preferences).providers[0].used, None);
        preferences.hidden.clear();
        board
            .write()
            .unwrap()
            .get_mut("fixture")
            .unwrap()
            .meters
            .clear();
        assert_eq!(snapshot(&board, &preferences).providers[0].used, None);
    }
}
