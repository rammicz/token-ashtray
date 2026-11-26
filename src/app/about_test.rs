use super::*;
use eframe::App;
use egui::accesskit::{Action, ActionRequest, NodeId};

fn input(view: ViewportId, events: Vec<egui::Event>, size: egui::Vec2) -> egui::RawInput {
    let mut input = egui::RawInput {
        viewport_id: view,
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, size)),
        events,
        ..Default::default()
    };
    input.viewports.insert(
        view,
        egui::ViewportInfo {
            parent: (view != ViewportId::ROOT).then_some(ViewportId::ROOT),
            native_pixels_per_point: Some(1.0),
            ..Default::default()
        },
    );
    input
}

fn node(output: &egui::FullOutput, label: &str) -> Option<(NodeId, egui::accesskit::Node)> {
    output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some(label) || n.value() == Some(label))
        .cloned()
}
fn click(id: NodeId) -> egui::Event {
    egui::Event::AccessKitActionRequest(ActionRequest {
        action: Action::Click,
        target: id,
        data: None,
    })
}
fn focus(id: NodeId) -> egui::Event {
    egui::Event::AccessKitActionRequest(ActionRequest {
        action: Action::Focus,
        target: id,
        data: None,
    })
}
fn enter() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
pub(super) fn app(path: std::path::PathBuf, sender: SyncSender<ChildMessage>) -> AshtrayApp {
    let config = ConfigStore::load_from(path);
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
        preferences_sender: Some(sender),
        tray_snapshot: Arc::new(RwLock::new(TraySnapshot::default())),
        board_preferences: Arc::new(RwLock::new(config.preferences().board.clone())),
        config,
        root: WindowPreferences::default(),

        root_restore: PositionRestore::default(),
        main_visible: false,

        about: false,
        message: None,
        startup_visibility_applied: true,
        geometry_last_change: None,
        monitor_rects: vec![],
        remote: true,
        received_first_action: true,
        closed_message_sent: false,
        refresh_feedback: refresh_feedback::Feedback::default(),
    }
}

#[test]
fn about_settings_and_tray_routes_open_main_board_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    let mut app = app(path.clone(), tx);
    let ctx = Context::default();
    ctx.enable_accesskit();
    configure_visuals(&ctx);
    let size = egui::vec2(800.0, 700.0);
    app.main_visible = true;
    let mut root = |events| {
        ctx.run(input(ViewportId::ROOT, events, size), |ctx| {
            app.update(ctx, &mut eframe::Frame::_new_kittest())
        })
    };
    let closed = root(vec![]);
    let _ = root(vec![click(node(&closed, "Window settings").unwrap().0)]);
    let menu = root(vec![]);
    // A coincident tray request must be consumed even when the settings action fires.
    app.about = true;
    let clicked = ctx.run(
        input(
            ViewportId::ROOT,
            vec![click(node(&menu, "About Token Ashtray").unwrap().0)],
            size,
        ),
        |ctx| app.update(ctx, &mut eframe::Frame::_new_kittest()),
    );
    assert!(
        !app.about,
        "pending tray request must be consumed unconditionally"
    );
    assert!(
        node(&clicked, "Close About").is_some(),
        "board settings must render About in root"
    );
    assert!(!path.exists());
    assert!(
        rx.try_recv().is_err(),
        "About must not save preferences or request refresh"
    );
}

#[test]
fn about_copy_and_links_use_exact_metadata_and_targets_with_pointer_and_keyboard() {
    for keyboard in [false, true] {
        let ctx = Context::default();
        ctx.enable_accesskit();
        configure_visuals(&ctx);
        let render = |requested, events| {
            ctx.run(
                input(ViewportId::ROOT, events, egui::vec2(800.0, 800.0)),
                |ctx| about::show(ctx, requested),
            )
        };
        let _ = render(true, vec![]);
        let out = render(false, vec![]);
        assert!(
            node(
                &out,
                concat!(env!("CARGO_PKG_NAME"), " · ", env!("CARGO_PKG_VERSION"))
            )
            .is_some()
        );
        assert!(node(&out, "Jirka Herník / @jirkahernik").is_some());
        for (label, expected) in [
            ("Copy version", env!("CARGO_PKG_VERSION")),
            ("YouTube", "https://www.youtube.com/@jirkahernik"),
            ("Support on Patreon", "https://www.patreon.com/JirkaHernik"),
        ] {
            let current = render(false, vec![]);
            let (id, n) = current
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|(_, n)| n.label() == Some(label) && n.supports_action(Action::Click))
                .cloned()
                .unwrap();
            let activated = if keyboard {
                let _ = render(false, vec![focus(id)]);
                render(false, vec![enter()])
            } else {
                let b = n.bounds().unwrap();
                let p = egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
                let _ = render(false, vec![egui::Event::PointerMoved(p)]);
                let _ = render(
                    false,
                    vec![egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
                render(
                    false,
                    vec![egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                )
            };
            assert!(
                activated.platform_output.commands.iter().any(|c| match c {
                    egui::OutputCommand::CopyText(text) =>
                        label == "Copy version" && text == expected,
                    egui::OutputCommand::OpenUrl(url) =>
                        label != "Copy version" && url.url == expected,
                    _ => false,
                }),
                "{label}, keyboard={keyboard}: {:?}",
                activated.platform_output.commands
            );
            let _ = render(
                false,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
    }
}

#[test]
fn about_close_remains_painted_and_interactive_in_small_and_large_viewports() {
    for size in [egui::vec2(360.0, 360.0), egui::vec2(1000.0, 800.0)] {
        for scale in [1.0, 2.0] {
            let ctx = Context::default();
            ctx.enable_accesskit();
            configure_visuals(&ctx);
            let render = |requested, events| {
                let mut raw = input(ViewportId::ROOT, events, size);
                raw.viewports
                    .get_mut(&ViewportId::ROOT)
                    .unwrap()
                    .native_pixels_per_point = Some(scale);
                ctx.run(raw, |ctx| about::show(ctx, requested))
            };
            let _ = render(true, vec![]);
            let _ = render(false, vec![]);
            let out = render(false, vec![]);
            let close = node(&out, "Close About").expect("reachable close control");
            assert!(close.1.supports_action(Action::Click));
            let viewport = egui::Rect::from_min_size(Pos2::ZERO, size);
            let close_text = out
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == "Close" => Some((
                        shape.clip_rect,
                        text.galley.rect.translate(text.pos.to_vec2()),
                    )),
                    _ => None,
                })
                .expect("actual Close glyphs must be painted");
            assert!(
                viewport.contains_rect(close_text.1),
                "Close escaped viewport at {size:?}/{scale}"
            );
            assert!(
                close_text.0.contains_rect(close_text.1),
                "Close glyphs clipped"
            );
            let _ = render(false, vec![click(close.0)]);
            assert!(node(&render(false, vec![]), "Close About").is_none());
        }
    }
}

#[test]
fn about_legal_sections_retain_exact_application_library_and_provider_texts() {
    for (title, expected) in [
        ("Application license", include_str!("../../LICENSE")),
        (
            "Third-party libraries",
            include_str!("../../THIRD-PARTY.md"),
        ),
        (
            "Provider logo notices",
            include_str!("../../assets/providers/NOTICE.md"),
        ),
    ] {
        let ctx = Context::default();
        ctx.enable_accesskit();
        configure_visuals(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        let render = |requested, events| {
            ctx.run(
                input(ViewportId::ROOT, events, egui::vec2(1000.0, 900.0)),
                |ctx| about::show(ctx, requested),
            )
        };
        let _ = render(true, vec![]);
        let _ = render(false, vec![]);
        let closed = render(false, vec![]);
        let section = closed
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some(title) && n.supports_action(Action::Click))
            .unwrap()
            .0;
        let _ = render(false, vec![click(section)]);
        let out = render(false, vec![]);
        assert!(out.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.job.text==expected)),"{title} must lay out the exact embedded legal text");
        assert!(node(&out, "Close About").is_some());
    }
}

#[test]
fn about_keyboard_focus_reveals_legal_controls_in_small_viewport() {
    let ctx = Context::default();
    ctx.enable_accesskit();
    configure_visuals(&ctx);
    let render = |requested, events| {
        ctx.run(
            input(ViewportId::ROOT, events, egui::vec2(360.0, 360.0)),
            |ctx| about::show(ctx, requested),
        )
    };
    let _ = render(true, vec![]);
    let _ = render(false, vec![]);
    let out = render(false, vec![]);
    let section = out
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, n)| {
            n.label() == Some("Provider logo notices") && n.supports_action(Action::Focus)
        })
        .unwrap()
        .0;
    let _ = render(false, vec![focus(section)]);
    let mut out = render(false, vec![]);
    for _ in 0..20 {
        out = render(false, vec![]);
    }
    let painted = out
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "Provider logo notices" => Some((
                shape.clip_rect,
                text.galley.rect.translate(text.pos.to_vec2()),
            )),
            _ => None,
        })
        .expect("focused legal control must be painted");
    assert!(
        painted.0.contains_rect(painted.1),
        "focused legal control must be fully visible"
    );
}
