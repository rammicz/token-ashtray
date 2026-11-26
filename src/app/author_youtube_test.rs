use super::*;
use eframe::App;

const LABEL: &str = "Watch the author’s latest video";

fn input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(800.0, 700.0),
        )),
        ..Default::default()
    }
}

#[test]
fn main_board_exposes_latest_video_header_action() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = std::sync::mpsc::sync_channel(64);
    let mut app = about_test::app(dir.path().join("config.toml"), tx);
    app.main_visible = true;
    let ctx = Context::default();
    ctx.enable_accesskit();
    ctx.set_embed_viewports(false);
    configure_visuals(&ctx);
    let _ = ctx.run(input(), |ctx| {
        app.update(ctx, &mut eframe::Frame::_new_kittest())
    });
    let output = ctx.run(input(), |ctx| {
        app.update(ctx, &mut eframe::Frame::_new_kittest())
    });
    let (_, header) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(LABEL))
        .expect("main board must expose the YouTube header action");
    assert!(matches!(
        header.role(),
        egui::accesskit::Role::Button | egui::accesskit::Role::Link
    ));
    assert!(header.supports_action(egui::accesskit::Action::Click));
}

fn node(
    output: &egui::FullOutput,
    label: &str,
) -> Option<(egui::accesskit::NodeId, egui::accesskit::Node)> {
    output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label) || node.value() == Some(label))
        .cloned()
}
fn access(id: egui::accesskit::NodeId, action: egui::accesskit::Action) -> egui::Event {
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action,
        target: id,
        data: None,
    })
}
fn flattened(output: &egui::FullOutput) -> Vec<&egui::Shape> {
    fn walk<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::Shape>) {
        if let egui::Shape::Vec(shapes) = shape {
            for shape in shapes {
                walk(shape, out);
            }
        } else {
            out.push(shape);
        }
    }
    let mut result = Vec::new();
    for shape in &output.shapes {
        walk(&shape.shape, &mut result);
    }
    result
}
fn visible_label(output: &egui::FullOutput) -> bool {
    flattened(output)
        .iter()
        .any(|s| matches!(s,egui::Shape::Text(t) if t.galley.job.text == LABEL))
}
fn dot(output: &egui::FullOutput) -> bool {
    flattened(output)
        .iter()
        .any(|s| matches!(s,egui::Shape::Circle(c) if c.radius == 2.0 && c.fill.b() > c.fill.r()))
}
fn app_input(size: egui::Vec2, scale: f32, events: Vec<egui::Event>) -> egui::RawInput {
    let mut i = input();
    i.screen_rect = Some(egui::Rect::from_min_size(Pos2::ZERO, size));
    i.events = events;
    i.viewports
        .get_mut(&ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(scale);
    i
}
fn render(
    app: &mut AshtrayApp,
    ctx: &Context,
    size: egui::Vec2,
    scale: f32,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(app_input(size, scale, events), |ctx| {
        app.update(ctx, &mut eframe::Frame::_new_kittest())
    })
}
fn fixture_card() -> ProviderCard {
    ProviderCard {
        id: "codex".into(),
        short_name: "Fixture quota".into(),
        plan: None,
        price: None,
        meters: vec![token_ashtray::model::Meter {
            label: "Weekly".into(),
            used_percent: 25.0,
            resets_at: None,
        }],
        usage_breakdown: vec![],
        status: Status::Live,
        detail: String::new(),
        link: String::new(),
        fetched_at: None,
    }
}
fn card_rects(ctx: &Context) -> Vec<egui::Rect> {
    ctx.data(|d| {
        d.get_temp::<Vec<DragSlot>>(Id::new(("card-drag-layout", ViewportId::ROOT)))
            .unwrap()
    })
    .iter()
    .map(|s| s.rect)
    .collect()
}

#[test]
fn header_layout_hover_focus_hidden_board_and_about_are_independent() {
    for size in [egui::vec2(360.0, 360.0), egui::vec2(1000.0, 760.0)] {
        for scale in [1.0, 2.0] {
            for appearance in [ThemePreference::Light, ThemePreference::Dark] {
                let dir = tempfile::tempdir().unwrap();
                let (tx, rx) = std::sync::mpsc::sync_channel(64);
                let mut app = about_test::app(dir.path().join("config.toml"), tx);
                app.main_visible = true;
                app.board_preferences.write().unwrap().theme_preference = appearance;
                let card = fixture_card();
                app.state
                    .board
                    .write()
                    .unwrap()
                    .insert(card.id.clone(), card);
                let ctx = Context::default();
                ctx.enable_accesskit();
                ctx.set_embed_viewports(false);
                configure_visuals(&ctx);
                ctx.set_zoom_factor(1.0);
                ctx.style_mut(|s| s.interaction.tooltip_delay = 0.0);
                let _ = render(&mut app, &ctx, size, scale, vec![]);
                let closed = render(&mut app, &ctx, size, scale, vec![]);
                assert_eq!(ctx.pixels_per_point(), scale);
                let (id, control) = node(&closed, LABEL).expect("collapsed accessible header");
                let bounds = control.bounds().unwrap();
                assert!(bounds.width() <= 32.0, "compact header target: {bounds:?}");
                assert!(bounds.height() <= 32.0);
                let controls = ["Refresh now", "Window settings"]
                    .map(|label| node(&closed, label).unwrap().1.bounds().unwrap());
                assert!(
                    bounds.x1 <= controls[0].x0,
                    "YouTube immediately before Refresh"
                );
                for control in controls {
                    assert!(((control.y0 + control.y1) - (bounds.y0 + bounds.y1)).abs() < 1.0);
                    assert!(control.x1 <= f64::from(size.x));
                }
                assert!(bounds.y1 <= f64::from(size.y));
                assert!(!visible_label(&closed));
                assert!(dot(&closed));
                let cards = card_rects(&ctx);
                assert_eq!(cards.len(), 1);
                let point = egui::pos2(
                    ((bounds.x0 + bounds.x1) / 2.0) as f32,
                    ((bounds.y0 + bounds.y1) / 2.0) as f32,
                );
                let _ = render(
                    &mut app,
                    &ctx,
                    size,
                    scale,
                    vec![egui::Event::PointerMoved(point)],
                );
                let _ = render(&mut app, &ctx, size, scale, vec![]);
                let hover = render(&mut app, &ctx, size, scale, vec![]);
                assert!(visible_label(&hover));
                assert_eq!(card_rects(&ctx), cards);
                for (label, expected) in
                    ["Refresh now", "Window settings"].into_iter().zip(controls)
                {
                    assert_eq!(node(&hover, label).unwrap().1.bounds().unwrap(), expected);
                }
                assert!(dot(&hover));
                let _ = render(&mut app, &ctx, size, scale, vec![egui::Event::PointerGone]);
                let _ = render(
                    &mut app,
                    &ctx,
                    size,
                    scale,
                    vec![access(id, egui::accesskit::Action::Focus)],
                );
                let focus = render(&mut app, &ctx, size, scale, vec![]);
                assert!(visible_label(&focus));
                assert_eq!(card_rects(&ctx), cards);
                assert!(dot(&focus));
                for (label, expected) in
                    ["Refresh now", "Window settings"].into_iter().zip(controls)
                {
                    assert_eq!(node(&focus, label).unwrap().1.bounds().unwrap(), expected);
                }

                assert!(!app.board_preferences.read().unwrap().author_youtube_visited);
                assert!(rx.try_recv().is_err());
                app.board_preferences
                    .write()
                    .unwrap()
                    .author_youtube_visited = true;
                let visited = render(&mut app, &ctx, size, scale, vec![]);
                assert!(
                    !dot(&visited),
                    "legacy visited preference suppresses the indicator"
                );
                app.board_preferences
                    .write()
                    .unwrap()
                    .author_youtube_visited = false;
                app.main_visible = false;
                let hidden = render(&mut app, &ctx, size, scale, vec![]);
                assert!(node(&hidden, LABEL).is_none());
                app.main_visible = true;
                app.about = true;
                let _ = render(&mut app, &ctx, size, scale, vec![]);
                let about = render(&mut app, &ctx, size, scale, vec![]);
                let youtube = about
                    .platform_output
                    .accesskit_update
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .find(|(_, n)| {
                        n.label() == Some("YouTube")
                            && n.supports_action(egui::accesskit::Action::Click)
                    })
                    .unwrap()
                    .0;
                let _ = render(
                    &mut app,
                    &ctx,
                    size,
                    scale,
                    vec![access(youtube, egui::accesskit::Action::Focus)],
                );
                let _ = render(&mut app, &ctx, size, scale, vec![]);
                let opened = render(
                    &mut app,
                    &ctx,
                    size,
                    scale,
                    vec![egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
                assert!(opened.platform_output.commands.iter().any(|c|matches!(c,egui::OutputCommand::OpenUrl(u) if u.url=="https://www.youtube.com/@jirkahernik")), "size={size:?}, scale={scale}, commands={:?}, focus={:?}", opened.platform_output.commands, ctx.memory(|m| m.focused()));
                assert!(!app.board_preferences.read().unwrap().author_youtube_visited);
                // Geometry may finish its debounce during this multi-frame test.
                // About activation must not mark the header link or poll providers.
                assert!(rx.try_iter().all(|message| matches!(message, ChildMessage::SavePreferences(p) if !p.board.author_youtube_visited)));
            }
        }
    }
}

#[test]
fn header_actual_mouse_and_keyboard_activation_mark_immediately_and_open_once_on_failure() {
    use author_youtube::tests::Server;
    use std::sync::atomic::Ordering;
    for keyboard in [false, true] {
        let server = Server::new("invalid feed".into());
        server.reply.lock().unwrap().delay = Duration::from_millis(180);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        let mut app = about_test::app(path.clone(), tx);
        app.main_visible = true;
        app.author_video = author_youtube::LatestVideo::with_endpoint(
            server.endpoint.clone(),
            Duration::from_secs(5),
        );
        let ctx = Context::default();
        ctx.enable_accesskit();
        ctx.set_embed_viewports(false);
        configure_visuals(&ctx);
        let size = egui::vec2(800.0, 700.0);
        let _ = render(&mut app, &ctx, size, 1.0, vec![]);
        let out = render(&mut app, &ctx, size, 1.0, vec![]);
        let (id, n) = node(&out, LABEL).unwrap();
        let activated = if keyboard {
            for _ in 0..12 {
                if ctx.memory(|m| m.has_focus(Id::new("author-youtube-action"))) {
                    break;
                }
                let _ = render(
                    &mut app,
                    &ctx,
                    size,
                    1.0,
                    vec![egui::Event::Key {
                        key: egui::Key::Tab,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            assert!(
                ctx.memory(|m| m.has_focus(Id::new("author-youtube-action"))),
                "Tab must reach YouTube"
            );
            render(
                &mut app,
                &ctx,
                size,
                1.0,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            )
        } else {
            let b = n.bounds().unwrap();
            let p = egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
            let _ = render(
                &mut app,
                &ctx,
                size,
                1.0,
                vec![
                    egui::Event::PointerMoved(p),
                    egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            render(
                &mut app,
                &ctx,
                size,
                1.0,
                vec![egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            )
        };
        assert!(app.board_preferences.read().unwrap().author_youtube_visited);
        assert!(!dot(&activated));
        let messages = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(messages.len(), 1);
        assert!(
            matches!(&messages[0],ChildMessage::SavePreferences(p) if p.board.author_youtube_visited)
        );
        assert!(
            activated.platform_output.commands.is_empty(),
            "no early browser open before lookup completes"
        );
        let mut urls = Vec::new();
        for _ in 0..3 {
            let out = render(
                &mut app,
                &ctx,
                size,
                1.0,
                vec![access(id, egui::accesskit::Action::Click)],
            );
            assert!(!dot(&out));
            assert!(out.platform_output.commands.is_empty());
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while urls.is_empty() && Instant::now() < deadline {
            let start = Instant::now();
            let out = render(&mut app, &ctx, size, 1.0, vec![]);
            assert!(start.elapsed() < Duration::from_millis(100));
            urls.extend(out.platform_output.commands.iter().filter_map(|c| match c {
                egui::OutputCommand::OpenUrl(u) => Some(u.url.clone()),
                _ => None,
            }));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(urls, [author_youtube::FALLBACK]);
        assert_eq!(server.requests.load(Ordering::Acquire), 1);
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn header_visit_save_retries_a_full_host_channel_without_polling() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    tx.try_send(ChildMessage::Refresh { request: 1 }).unwrap();
    let mut app = about_test::app(dir.path().join("config.toml"), tx);
    app.main_visible = true;
    app.apply(BoardAction {
        author_youtube_visited: true,
        changed: true,
        ..Default::default()
    });
    assert!(app.preferences_save_pending);
    assert!(app.board_preferences.read().unwrap().author_youtube_visited);
    assert!(matches!(
        rx.try_recv().unwrap(),
        ChildMessage::Refresh { .. }
    ));
    let ctx = Context::default();
    ctx.set_embed_viewports(false);
    let _ = render(&mut app, &ctx, egui::vec2(800.0, 700.0), 1.0, vec![]);
    assert!(!app.preferences_save_pending);
    assert!(
        matches!(rx.try_recv().unwrap(),ChildMessage::SavePreferences(p) if p.board.author_youtube_visited)
    );
    assert!(rx.try_recv().is_err());
}
