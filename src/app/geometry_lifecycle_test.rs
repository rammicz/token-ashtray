use super::*;

fn app(path: std::path::PathBuf) -> AshtrayApp {
    let config = ConfigStore::load_from(path);
    let root = config.preferences().window.clone();
    AshtrayApp {
        appearance: theme::Appearance::default(),
        author_video: author_youtube::LatestVideo::default(),
        preferences_save_pending: false,
        state: Arc::new(AppState::remote(Arc::new(RwLock::new(BTreeMap::new())))),
        poller: None,
        updater: None,
        tray: None,
        activations: None,
        connection: None,
        preferences_sender: None,
        tray_snapshot: Arc::new(RwLock::new(TraySnapshot::default())),
        board_preferences: Arc::new(RwLock::new(config.preferences().board.clone())),
        config,

        root,

        // Measured settled state immediately before the native SNI hide:
        // frame20,20, client24,49; X11 supplied the trustworthy frame.
        root_restore: PositionRestore::Settled {
            saved_anchor: Pos2::new(20.0, 20.0),
            observed_anchor: Pos2::new(24.0, 49.0),
            egui_frame_seen: false,
            unknown_first_frame: false,
            candidate: None,
        },
        main_visible: true,

        about: false,
        message: None,
        startup_visibility_applied: true,
        geometry_last_change: None,
        monitor_rects: vec![],
        remote: false,
        received_first_action: true,
        closed_message_sent: false,
        refresh_feedback: refresh_feedback::Feedback::default(),
    }
}

fn capture(app: &mut AshtrayApp, ctx: &Context, origin: Pos2, size: egui::Vec2) {
    let rect = egui::Rect::from_min_size(origin, size);
    let mut input = egui::RawInput::default();
    let view = input.viewports.get_mut(&ViewportId::ROOT).unwrap();
    view.inner_rect = Some(rect);
    view.outer_rect = Some(rect);
    let _ = ctx.run(input, |ctx| {
        app.capture_root_geometry(ctx);
    });
}

fn settle_shown_frame(app: &mut AshtrayApp, frame: Pos2, client: Pos2, size: egui::Vec2) {
    let start = Instant::now();
    for elapsed in [400, 550] {
        app.root_restore.capture_with_frame(
            &mut app.root,
            egui::Rect::from_min_size(frame, size + (client - frame)),
            egui::Rect::from_min_size(client, size),
            Some(frame),
            start + Duration::from_millis(elapsed),
        );
    }
}

#[test]
fn native_state_reports_preserve_normal_bounds_across_capture_hide_and_show() {
    for (maximized, minimized) in [(true, false), (false, true)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[window]\nx = 20.0\ny = 20.0\nwidth = 800.0\nheight = 760.0\n",
        )
        .unwrap();
        let mut app = app(path.clone());
        // A legacy position is tagged with the scale it is first read at.
        let original = WindowPreferences {
            position_scale: Some(1.0),
            ..app.root.clone()
        };
        app.startup_visibility_applied = false;
        let ctx = Context::default();
        for _ in 0..4 {
            let mut input = egui::RawInput::default();
            let info = input.viewports.get_mut(&ViewportId::ROOT).unwrap();
            info.maximized = Some(maximized);
            info.minimized = Some(minimized);
            let rect = egui::Rect::from_min_size(
                if minimized {
                    Pos2::new(-32000.0, -32000.0)
                } else {
                    Pos2::ZERO
                },
                if minimized {
                    egui::vec2(1.0, 1.0)
                } else {
                    egui::vec2(1920.0, 1080.0)
                },
            );
            info.inner_rect = Some(rect);
            info.outer_rect = Some(rect);
            let output = ctx.run(input, |ctx| {
                eframe::App::update(&mut app, ctx, &mut eframe::Frame::_new_kittest());
                app.capture_root_geometry(ctx);
                app.hide_main(ctx);
                app.show_main(ctx);
            });
            assert_eq!(
                serde_json::to_value(&app.root).unwrap(),
                serde_json::to_value(&original).unwrap(),
                "maximized/minimized reports must never replace saved normal bounds"
            );
            assert!(
                !output.viewport_output[&ViewportId::ROOT]
                    .commands
                    .iter()
                    .any(|c| matches!(
                        c,
                        ViewportCommand::InnerSize(_)
                            | ViewportCommand::OuterPosition(_)
                            | ViewportCommand::MinInnerSize(_)
                    )),
                "reopening a native state must not unmaximize through geometry commands"
            );
        }
        app.persist();
        assert_eq!(
            serde_json::to_value(&ConfigStore::load_from(path).preferences().window).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
    }
}

#[test]
fn hidden_root_reports_cannot_corrupt_saved_geometry_but_shown_moves_still_persist() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "future_setting = 'keep me'\n[board]\nprimary = 'claude'\nquota_display_mode = 'used'\n[[panels]]\nid = 0\nx = 20.0\ny = 20.0\nwidth = 800.0\nheight = 760.0\npinned = true\nhidden = ['codex']\n",
    )
    .unwrap();
    let mut app = app(path.clone());
    // A legacy position is tagged with the scale it is first read at.
    let original = WindowPreferences {
        position_scale: Some(1.0),
        ..app.root.clone()
    };
    let board = app.board_preferences.read().unwrap().clone();
    let ctx = Context::default();
    capture(
        &mut app,
        &ctx,
        Pos2::new(24.0, 49.0),
        egui::vec2(800.0, 760.0),
    );

    // Replay the observed post-hide reports, beyond the real candidate debounce.
    // The native frame is absent in this headless test, as it is after unmap.
    app.main_visible = false;
    capture(
        &mut app,
        &ctx,
        Pos2::new(20.0, 20.0),
        egui::vec2(800.0, 760.0),
    );
    std::thread::sleep(Duration::from_millis(220));
    capture(
        &mut app,
        &ctx,
        Pos2::new(20.0, 20.0),
        egui::vec2(800.0, 760.0),
    );
    app.persist();
    let reloaded = ConfigStore::load_from(path.clone());
    assert_eq!(
        serde_json::to_value(&reloaded.preferences().window).unwrap(),
        serde_json::to_value(&original).unwrap(),
        "hidden root must not save unmap coordinates"
    );

    // A transient hidden size must not replace the desired restored size either.
    capture(&mut app, &ctx, Pos2::ZERO, egui::vec2(1.0, 1.0));
    assert_eq!(
        serde_json::to_value(&app.root).unwrap(),
        serde_json::to_value(&original).unwrap()
    );

    let _ = ctx.run(egui::RawInput::default(), |ctx| app.show_main(ctx));
    // Available native work areas reassert the saved frame on remap. Observe
    // its settlement before replaying a later user move.
    settle_shown_frame(
        &mut app,
        Pos2::new(20.0, 20.0),
        Pos2::new(24.0, 49.0),
        egui::vec2(800.0, 760.0),
    );
    // A genuine mapped move/resize after showing must still traverse capture,
    // debounce and ConfigStore. Freezing all geometry would fail this control.
    capture(
        &mut app,
        &ctx,
        Pos2::new(304.0, 249.0),
        egui::vec2(640.0, 480.0),
    );
    std::thread::sleep(Duration::from_millis(220));
    capture(
        &mut app,
        &ctx,
        Pos2::new(304.0, 249.0),
        egui::vec2(640.0, 480.0),
    );
    app.persist();
    let saved = ConfigStore::load_from(path.clone());
    let expected = WindowPreferences {
        x: Some(300.0),
        y: Some(220.0),
        width: 640.0,
        height: 480.0,
        ..original
    };
    assert_eq!(
        serde_json::to_value(&saved.preferences().window).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(saved.preferences().board, board);
    assert!(std::fs::read_to_string(path).unwrap().contains("keep me"));
}

#[test]
fn hiding_during_mapped_move_debounce_saves_last_mapped_position_and_finishes_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[panels]]\nid = 0\nx = 20.0\ny = 20.0\nwidth = 800.0\nheight = 760.0\n",
    )
    .unwrap();
    let mut app = app(path.clone());
    let ctx = Context::default();
    capture(
        &mut app,
        &ctx,
        Pos2::new(104.0, 129.0),
        egui::vec2(640.0, 480.0),
    );
    assert!(
        app.root_restore.needs_repaint(),
        "a mapped move is still awaiting debounce"
    );
    assert_eq!((app.root.x, app.root.y), (Some(20.0), Some(20.0)));

    let output = ctx.run(egui::RawInput::default(), |ctx| app.hide_main(ctx));
    assert!(!app.main_visible);
    assert!(
        output.viewport_output[&ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, ViewportCommand::Visible(false)))
    );
    // A quit in the same pass must save the last mapped move, even before any
    // later hidden callback has a chance to finish the candidate.
    app.persist();
    capture(&mut app, &ctx, Pos2::ZERO, egui::vec2(1.0, 1.0));
    let saved = ConfigStore::load_from(path);
    let root = &saved.preferences().window;
    assert_eq!(
        (root.x, root.y, root.width, root.height),
        (Some(100.0), Some(100.0), 640.0, 480.0),
        "hide must finish the last mapped observation, not discard it or consume hidden noise"
    );
    assert!(
        !app.root_restore.needs_repaint(),
        "a hidden move candidate must not request endless repaints"
    );

    // Finishing the candidate must advance both anchors. Otherwise an equal-rect
    // mapped observation after show double-counts the previous displacement.
    let _ = ctx.run(egui::RawInput::default(), |ctx| app.show_main(ctx));
    settle_shown_frame(
        &mut app,
        Pos2::new(100.0, 100.0),
        Pos2::new(104.0, 129.0),
        egui::vec2(640.0, 480.0),
    );
    capture(
        &mut app,
        &ctx,
        Pos2::new(404.0, 329.0),
        egui::vec2(640.0, 480.0),
    );
    std::thread::sleep(Duration::from_millis(220));
    capture(
        &mut app,
        &ctx,
        Pos2::new(404.0, 329.0),
        egui::vec2(640.0, 480.0),
    );
    assert_eq!((app.root.x, app.root.y), (Some(400.0), Some(300.0)));
}

#[test]
fn an_initially_hidden_restore_gets_a_fresh_settlement_window_on_show() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[panels]]\nid = 0\nx = 20.0\ny = 20.0\nwidth = 800.0\nheight = 760.0\n",
    )
    .unwrap();
    let mut app = app(path);
    app.main_visible = false;
    app.root_restore = PositionRestore::default();
    let monitors = [platform::MonitorRect {
        x: 0.0,
        y: 0.0,
        width: 4000.0,
        height: 2600.0,
    }];
    app.root_restore.requested_position(
        &app.root,
        &monitors,
        true,
        Instant::now() - Duration::from_secs(60),
    );
    let ctx = Context::default();
    capture(&mut app, &ctx, Pos2::ZERO, egui::vec2(800.0, 760.0));
    let _ = ctx.run(egui::RawInput::default(), |ctx| app.show_main(ctx));
    let start = Instant::now();
    let transient = egui::Rect::from_min_size(Pos2::new(1000.0, 800.0), egui::vec2(800.0, 760.0));
    assert_eq!(
        app.root_restore
            .capture_with_frame(
                &mut app.root,
                transient,
                transient,
                None,
                start + Duration::from_millis(10)
            )
            .1,
        None,
        "time spent hidden must not age the first shown-frame retry"
    );
    let client = egui::Rect::from_min_size(Pos2::new(24.0, 49.0), egui::vec2(800.0, 760.0));
    for elapsed in [400, 550] {
        app.root_restore.capture_with_frame(
            &mut app.root,
            client,
            client,
            Some(Pos2::new(20.0, 20.0)),
            start + Duration::from_millis(elapsed),
        );
    }
    assert_eq!((app.root.x, app.root.y), (Some(20.0), Some(20.0)));
    assert!(!app.root_restore.needs_repaint());
}

#[test]
fn slow_mapped_motion_does_not_restart_the_stationary_candidate_reference_each_frame() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[panels]]\nid = 0\nx = 20.0\ny = 20.0\nwidth = 800.0\nheight = 760.0\n",
    )
    .unwrap();
    let mut app = app(path);
    let start = Instant::now();
    for (x, millis) in [(100.0, 0), (101.0, 100), (102.0, 210)] {
        let frame = Pos2::new(x, 100.0);
        let client =
            egui::Rect::from_min_size(frame + egui::vec2(4.0, 29.0), egui::vec2(800.0, 760.0));
        app.root_restore.capture_with_frame(
            &mut app.root,
            client,
            client,
            Some(frame),
            start + Duration::from_millis(millis),
        );
    }
    assert_eq!(
        (app.root.x, app.root.y),
        (Some(20.0), Some(20.0)),
        "continuous movement is not a stationary 200ms candidate"
    );
    let frame = Pos2::new(102.0, 100.0);
    let client = egui::Rect::from_min_size(frame + egui::vec2(4.0, 29.0), egui::vec2(800.0, 760.0));
    app.root_restore.capture_with_frame(
        &mut app.root,
        client,
        client,
        Some(frame),
        start + Duration::from_millis(420),
    );
    assert_eq!((app.root.x, app.root.y), (Some(102.0), Some(100.0)));
}

fn tray_app() -> (
    tempfile::TempDir,
    AshtrayApp,
    Context,
    std::sync::mpsc::Sender<TrayAction>,
    std::sync::mpsc::Receiver<ChildMessage>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[panels]]\nid = 0\nx = 20.0\ny = 20.0\nwidth = 800.0\nheight = 760.0\n",
    )
    .unwrap();
    let mut app = app(path);
    app.remote = true;
    app.main_visible = false;
    let (connection, sender, messages) = UiConnection::for_test();
    app.connection = Some(connection);
    (dir, app, Context::default(), sender, messages)
}

#[test]
fn host_save_failure_is_shown_in_the_board_message() {
    let (_dir, mut app, ctx, _sender, _messages) = tray_app();
    let failure = app.connection.as_ref().unwrap().save_failure.clone();
    *failure.lock().unwrap() = Some("Access is denied.".into());
    let _ = ctx.run(Default::default(), |ctx| app.handle_action(ctx));
    assert_eq!(
        app.message.as_deref(),
        Some("Could not save board preferences: Access is denied.")
    );
    assert_eq!(*failure.lock().unwrap(), None, "each failure is shown once");
}

#[test]
fn tray_open_preserves_saved_geometry() {
    let (_dir, mut app, ctx, sender, _messages) = tray_app();
    sender.send(TrayAction::OpenBoard).unwrap();
    let output = ctx.run(Default::default(), |ctx| app.handle_action(ctx));
    assert!(app.main_visible);
    let commands = &output.viewport_output[&ViewportId::ROOT].commands;
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, ViewportCommand::Visible(true)))
    );
    assert!(commands.iter().any(|c| matches!(c, ViewportCommand::Focus)));
    assert!(
        commands.iter().all(|c| match c {
            ViewportCommand::OuterPosition(position) => *position == Pos2::new(20.0, 20.0),
            _ => true,
        }),
        "settled saved geometry must not move to the tray anchor: {commands:?}"
    );
    assert_eq!((app.root.x, app.root.y), (Some(20.0), Some(20.0)));
}

fn minimized_input() -> egui::RawInput {
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&ViewportId::ROOT)
        .unwrap()
        .minimized = Some(true);
    input
}

/// Activation shows, restores, then focuses the board without moving or resizing it.
fn assert_restores_minimized_board(commands: &[ViewportCommand], activation: &str) {
    let visible = commands
        .iter()
        .position(|c| matches!(c, ViewportCommand::Visible(true)))
        .expect("activation must show the board");
    let focus = commands
        .iter()
        .position(|c| matches!(c, ViewportCommand::Focus))
        .expect("activation must request focus");
    assert!(
        commands[visible + 1..focus]
            .iter()
            .any(|c| matches!(c, ViewportCommand::Minimized(false))),
        "{activation} must restore the minimized board after showing and before requesting focus: {commands:?}"
    );
    assert!(commands.iter().all(|c| !matches!(
        c,
        ViewportCommand::InnerSize(_)
            | ViewportCommand::OuterPosition(_)
            | ViewportCommand::MinInnerSize(_)
            | ViewportCommand::Maximized(_)
    )));
}

fn assert_tray_restores_minimized_board(action: TrayAction) {
    let (_dir, mut app, ctx, sender, _messages) = tray_app();
    app.main_visible = true;
    let original = serde_json::to_value(&app.root).unwrap();
    sender.send(action).unwrap();

    let output = ctx.run(minimized_input(), |ctx| app.handle_action(ctx));
    if action == TrayAction::About {
        assert!(app.about, "About must request the dialog");
    }
    assert_restores_minimized_board(
        &output.viewport_output[&ViewportId::ROOT].commands,
        &format!("{action:?}"),
    );
    assert_eq!(serde_json::to_value(&app.root).unwrap(), original);
}

#[test]
fn tray_open_restores_minimized_board_before_focusing() {
    assert_tray_restores_minimized_board(TrayAction::OpenBoard);
}

#[test]
fn tray_about_restores_minimized_board_before_focusing() {
    assert_tray_restores_minimized_board(TrayAction::About);
}

#[test]
fn later_launch_restores_minimized_standalone_board_before_it_exits() {
    use crate::platform::instance::{self, Instance, Scope};
    let dir = tempfile::tempdir().unwrap();
    let ctx = Context::default();
    // Only this test uses the name, and each test process has its own id.
    let scope = Scope::isolated(&format!("standalone-board-{}", std::process::id()));
    let Ok(Instance::Primary(mut primary)) = instance::acquire(&scope) else {
        panic!("an unused scope starts as primary");
    };
    let mut app =
        app(dir.path().join("config.toml")).with_activation(primary.take_listener(), &ctx);
    // A second launch from another thread, which does not own the guard.
    let launch = std::thread::spawn(move || match instance::acquire(&scope) {
        Ok(Instance::Running(running)) => running.activate().map(|own| own.is_none()),
        _ => Err("a second launch must find the standalone board".to_owned()),
    });
    // The launch thread ends on its own hand-off deadline if never answered.
    let commands = loop {
        let output = ctx.run(minimized_input(), |ctx| app.handle_action(ctx));
        let commands = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .map(|viewport| viewport.commands.clone())
            .unwrap_or_default();
        if !commands.is_empty() {
            break commands;
        }
        assert!(!launch.is_finished(), "the board never saw the launch");
        std::thread::sleep(Duration::from_millis(30));
    };
    assert_eq!(
        launch.join().unwrap(),
        Ok(true),
        "the later launch must exit once the board answers"
    );
    assert_restores_minimized_board(&commands, "A later launch");
}

#[test]
fn tray_activation_preserves_visible_normal_and_maximized_geometry() {
    for action in [TrayAction::OpenBoard, TrayAction::About] {
        for maximized in [false, true] {
            let (_dir, mut app, ctx, sender, _messages) = tray_app();
            app.main_visible = true;
            let original = serde_json::to_value(&app.root).unwrap();
            for _ in 0..3 {
                let mut input = egui::RawInput::default();
                let info = input.viewports.get_mut(&ViewportId::ROOT).unwrap();
                info.minimized = Some(false);
                info.maximized = Some(maximized);
                sender.send(action).unwrap();
                let output = ctx.run(input, |ctx| app.handle_action(ctx));
                let commands = &output.viewport_output[&ViewportId::ROOT].commands;
                assert!(app.main_visible);
                assert!(commands.iter().any(|c| matches!(c, ViewportCommand::Focus)));
                assert!(commands.iter().all(|c| !matches!(
                    c,
                    ViewportCommand::InnerSize(_)
                        | ViewportCommand::OuterPosition(_)
                        | ViewportCommand::MinInnerSize(_)
                        | ViewportCommand::Maximized(_)
                )));
                assert_eq!(serde_json::to_value(&app.root).unwrap(), original);
            }
        }
    }
}

#[test]
fn tray_open_survives_focus_loss_after_focused_frame() {
    use eframe::App;
    let (_dir, mut app, ctx, sender, _messages) = tray_app();
    sender.send(TrayAction::OpenBoard).unwrap();
    for focused in [true, false] {
        let mut input = egui::RawInput::default();
        input.viewports.get_mut(&ViewportId::ROOT).unwrap().focused = Some(focused);
        let _ = ctx.run(input, |ctx| {
            app.update(ctx, &mut eframe::Frame::_new_kittest())
        });
        assert!(
            app.main_visible,
            "board must remain visible with focused={focused}"
        );
    }
}

#[test]
fn repeated_tray_open_raises_visible_board() {
    let (_dir, mut app, ctx, sender, _messages) = tray_app();
    for action in [
        TrayAction::OpenBoard,
        TrayAction::OpenBoard,
        TrayAction::OpenBoard,
    ] {
        sender.send(action).unwrap();
        let output = ctx.run(Default::default(), |ctx| app.handle_action(ctx));
        assert!(
            app.main_visible,
            "repeat activation must not hide the board"
        );
        assert!(
            output.viewport_output[&ViewportId::ROOT]
                .commands
                .iter()
                .any(|c| matches!(c, ViewportCommand::Focus))
        );
    }
}

fn root_close(app: &mut AshtrayApp, ctx: &Context) -> egui::FullOutput {
    use eframe::App;
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    ctx.run(input, |ctx| {
        app.update(ctx, &mut eframe::Frame::_new_kittest())
    })
}

#[test]
fn root_close_to_tray_reports_nonquit_and_exits() {
    let (_dir, mut app, ctx, _sender, messages) = tray_app();
    app.main_visible = true;
    let output = root_close(&mut app, &ctx);
    let closed: Vec<_> = messages
        .try_iter()
        .filter_map(|m| match m {
            ChildMessage::Closed { quit } => Some(quit),
            _ => None,
        })
        .collect();
    assert_eq!(
        closed,
        vec![false],
        "root cross must release only the UI child"
    );
    assert!(!app.main_visible);
    let commands = &output.viewport_output[&ViewportId::ROOT].commands;
    assert!(matches!(commands.last(), Some(ViewportCommand::Close)));
}

#[test]
fn root_close_does_not_intercept_standalone_or_explicit_quit() {
    for remote in [false, true] {
        let (_dir, mut app, ctx, sender, messages) = tray_app();
        app.main_visible = true;
        app.remote = remote;
        if remote {
            sender.send(TrayAction::Quit).unwrap();
        }
        let output = root_close(&mut app, &ctx);
        assert!(
            !output.viewport_output[&ViewportId::ROOT]
                .commands
                .iter()
                .any(|c| matches!(c, ViewportCommand::CancelClose))
        );
        let closed: Vec<_> = messages
            .try_iter()
            .filter_map(|m| match m {
                ChildMessage::Closed { quit } => Some(quit),
                _ => None,
            })
            .collect();
        assert_eq!(closed, if remote { vec![true] } else { vec![] });
    }
}

#[test]
fn reopening_hidden_board_reasserts_saved_frame_after_native_remap() {
    let (_dir, mut app, ctx, _sender, _messages) = tray_app();
    app.monitor_rects = vec![platform::MonitorRect {
        x: 0.0,
        y: 0.0,
        width: 4000.0,
        height: 2600.0,
    }];
    let output = ctx.run(Default::default(), |ctx| app.show_main(ctx));
    assert!(output.viewport_output[&ViewportId::ROOT].commands.iter().any(|command|
        matches!(command, ViewportCommand::OuterPosition(position) if *position == Pos2::new(20.0, 20.0))
    ), "native remapping must restore the saved frame, not adopt the WM placement");
}

#[test]
fn only_a_settled_resize_replaces_the_saved_and_preferred_size() {
    let now = Instant::now();
    let outer = egui::Rect::from_min_size(Pos2::new(100.0, 100.0), egui::vec2(616.0, 978.0));
    let inner = egui::Rect::from_min_size(Pos2::new(108.0, 131.0), egui::vec2(600.0, 939.0));
    for (mut restore, keeps) in [
        (
            PositionRestore::Requested {
                target: outer.min,
                requested_at: now,
                last_seen: None,
                retries: 0,
            },
            true,
        ),
        (
            PositionRestore::Settled {
                saved_anchor: outer.min,
                observed_anchor: inner.min,
                egui_frame_seen: true,
                unknown_first_frame: false,
                candidate: None,
            },
            false,
        ),
    ] {
        let mut saved = WindowPreferences {
            x: Some(100.0),
            y: Some(100.0),
            width: 592.0,
            height: 921.0,
            preferred_size: Some([1100.0, 1500.0]),
            ..Default::default()
        };
        restore.capture(&mut saved, outer, inner, now);
        let expected = if keeps {
            (592.0, 921.0)
        } else {
            (600.0, 939.0)
        };
        assert_eq!((saved.width, saved.height), expected);
        assert_eq!(
            saved.preferred_size.is_some(),
            keeps,
            "placement-time reports keep it; a settled resize replaces it"
        );
    }
}
