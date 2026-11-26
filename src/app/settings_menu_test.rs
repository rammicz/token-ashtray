use super::*;
use std::cell::RefCell;

#[derive(Clone, Debug)]
struct ControlProbe {
    label: String,
    id: egui::Id,
    rect: egui::Rect,
    interact_rect: egui::Rect,
    focused: bool,
}

thread_local! {
    static CONTROL_PROBES: RefCell<Vec<ControlProbe>> = const { RefCell::new(Vec::new()) };
    static SCROLL_PROBE: RefCell<Option<MenuScrollProbe>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug)]
struct MenuScrollProbe {
    content_height: f32,
    viewport_height: f32,
    offset: f32,
}

pub(super) fn record_scroll(content_height: f32, viewport_height: f32, offset: f32) {
    SCROLL_PROBE.with_borrow_mut(|probe| {
        *probe = Some(MenuScrollProbe {
            content_height,
            viewport_height,
            offset,
        });
    });
}

pub(super) fn record_control(label: &str, response: &egui::Response) {
    CONTROL_PROBES.with_borrow_mut(|probes| {
        probes.push(ControlProbe {
            label: label.into(),
            id: response.id,
            rect: response.rect,
            interact_rect: response.interact_rect,
            focused: response.has_focus(),
        })
    });
}

struct MenuFrame {
    output: egui::FullOutput,
    action: BoardAction,
    board_offset: f32,
    controls: Vec<ControlProbe>,
    scroll: Option<MenuScrollProbe>,
}

impl MenuFrame {
    fn control(&self, label: &str) -> &ControlProbe {
        self.controls
            .iter()
            .find(|control| control.label == label)
            .unwrap_or_else(|| panic!("missing menu control {label}"))
    }
}

struct MenuHarness {
    ctx: Context,
    cards: Vec<ProviderCard>,
    prefs: BoardPreferences,
    update_status: UpdateStatus,
    pinned: bool,
    compact: bool,
    size: egui::Vec2,
    scale: f32,
}

impl MenuHarness {
    fn new(size: egui::Vec2, scale: f32, long_names: bool) -> Self {
        let ctx = Context::default();
        configure_visuals(&ctx);
        ctx.enable_accesskit();
        let mut cards = cards();
        if long_names {
            for (index, card) in cards.iter_mut().enumerate() {
                card.short_name = format!("Provider {index} with a deliberately long visible name");
            }
        }
        Self {
            ctx,
            cards,
            prefs: BoardPreferences::default(),
            update_status: UpdateStatus::Off,
            pinned: false,
            compact: false,
            size,
            scale,
        }
    }

    fn render(&mut self, events: Vec<egui::Event>) -> MenuFrame {
        CONTROL_PROBES.with_borrow_mut(Vec::clear);
        SCROLL_PROBE.with_borrow_mut(|probe| *probe = None);
        let mut input = egui::RawInput::default();

        input
            .viewports
            .get_mut(&input.viewport_id)
            .unwrap()
            .native_pixels_per_point = Some(self.scale);
        input.screen_rect = Some(egui::Rect::from_min_size(Pos2::ZERO, self.size));
        input.events = events;
        let mut action = BoardAction::default();
        let mut board_offset = 0.0;
        let output = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let board = egui::ScrollArea::vertical()
                    .id_salt("settings-menu-test-board")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        action = draw_board_with_updates(
                            ui,
                            "Token Ashtray",
                            &self.cards,
                            &self.prefs,
                            &self.update_status,
                            true,
                            &mut self.pinned,
                            &mut self.compact,
                        );
                    });
                board_offset = board.state.offset.y;
            });
        });
        if action.changed {
            apply_board_preferences(&mut self.prefs, &action, |_| {});
        }
        let controls = CONTROL_PROBES.with_borrow(|probes| probes.clone());
        let scroll = SCROLL_PROBE.with_borrow(|probe| *probe);
        MenuFrame {
            output,
            action,
            board_offset,
            controls,
            scroll,
        }
    }

    fn open(&mut self) -> MenuFrame {
        let _ = self.render(vec![]);
        let closed = self.render(vec![]);
        let button = closed
            .output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Window settings"))
            .expect("settings button in accessibility tree")
            .0;
        let _ = self.render(vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target: button,
                data: None,
            },
        )]);
        let _ = self.render(vec![]);
        self.render(vec![])
    }

    fn wheel(&mut self, point: Pos2, dy: f32) -> MenuFrame {
        let _ = self.render(vec![egui::Event::PointerMoved(point)]);
        let _ = self.render(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, dy),
            modifiers: egui::Modifiers::NONE,
        }]);
        self.render(vec![])
    }

    fn click(&mut self, point: Pos2) -> MenuFrame {
        let _ = self.render(vec![egui::Event::PointerMoved(point)]);
        let _ = self.render(vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        self.render(vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }])
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> MenuFrame {
        let pressed = self.render(vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]);
        let _released = self.render(vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        }]);
        let mut settled = self.render(vec![]);
        if pressed.action.changed {
            settled.action = pressed.action;
        }
        settled
    }
}

fn assert_fully_interactive(frame: &MenuFrame, label: &str, size: egui::Vec2) -> Pos2 {
    let control = frame.control(label);
    assert!(control.rect.height() > 0.0);
    assert!(
        control.interact_rect.width() > 0.0 && control.interact_rect.height() > 0.0,
        "{label} has no visible hit area: {control:?}"
    );
    assert!(
        control.interact_rect.height() + 1.0 >= control.rect.height(),
        "{label} remains clipped: {control:?}"
    );
    assert!(
        control.interact_rect.width() + 1.0 >= control.rect.width(),
        "{label} remains horizontally clipped: {control:?}"
    );
    let viewport = egui::Rect::from_min_size(Pos2::ZERO, size);
    assert!(
        viewport.contains(control.rect.min) && viewport.contains(control.rect.max),
        "{label} full hit area is outside viewport: {control:?}"
    );
    control.interact_rect.center()
}

fn cards() -> Vec<ProviderCard> {
    [
        "Antigravity",
        "Codex",
        "Claude",
        "Copilot",
        "Cursor",
        "Grok",
        "Grok Bot",
        "Kimi",
    ]
    .iter()
    .enumerate()
    .map(|(i, name)| ProviderCard {
        id: format!("provider-{i}"),
        short_name: (*name).into(),
        plan: None,
        price: None,
        meters: vec![],
        usage_breakdown: vec![],
        status: Status::Reconnect,
        detail: String::new(),
        link: String::new(),
        fetched_at: None,
    })
    .collect()
}

fn render(ctx: &Context, events: Vec<egui::Event>) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(440.0, 300.0),
        )),
        events,
        ..Default::default()
    };
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            draw_board(
                ui,
                "Token Ashtray",
                &cards(),
                &BoardPreferences::default(),
                false,
                &mut false,
                &mut false,
            );
        });
    })
}

fn row_y(output: &egui::FullOutput, label: &str) -> f32 {
    fn find(shape: &egui::Shape, label: &str) -> Option<f32> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == label => Some(text.pos.y),
            egui::Shape::Vec(children) => children.iter().find_map(|child| find(child, label)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|s| find(&s.shape, label))
        .or_else(|| {
            output
                .platform_output
                .accesskit_update
                .as_ref()?
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label))
                .and_then(|(_, node)| node.bounds().map(|rect| rect.y0 as f32))
        })
        .unwrap_or_else(|| panic!("no painted or accessibility geometry for {label}"))
}

#[test]
fn small_screen_settings_menu_reaches_clipped_controls_by_wheel() {
    use egui::{Event, Modifiers, MouseWheelUnit, accesskit};
    let ctx = Context::default();
    ctx.enable_accesskit();
    let _ = render(&ctx, vec![]);
    let closed = render(&ctx, vec![]);
    let button = closed
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Window settings"))
        .expect("settings button in accessibility tree")
        .0;
    let click = Event::AccessKitActionRequest(accesskit::ActionRequest {
        action: accesskit::Action::Click,
        target: button,
        data: None,
    });
    let _ = render(&ctx, vec![click]);
    let open = render(&ctx, vec![]);
    let first_y = row_y(&open, "Always on top");
    let last_y = row_y(&open, "Show Kimi");
    let (label, delta, before) = if first_y < 0.0 {
        ("Always on top", 600.0, first_y)
    } else {
        assert!(
            last_y + 20.0 > 300.0,
            "fixture must clip some menu content; first={first_y}, last={last_y}"
        );
        ("Show Kimi", -600.0, last_y)
    };
    let pointer = egui::pos2(300.0, 200.0);
    let _ = render(&ctx, vec![Event::PointerMoved(pointer)]);
    let _wheel_frame = render(
        &ctx,
        vec![Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta),
            modifiers: Modifiers::NONE,
        }],
    );
    // egui applies the wheel offset after laying out this pass; inspect the
    // next painted pass, when the newly exposed row can actually be hit.
    let settled = render(&ctx, vec![]);
    let after_y = row_y(&settled, label);
    assert!(
        after_y >= 0.0 && after_y + 20.0 <= 300.0,
        "wheel in open settings must reveal clipped {label} row; before={before}, after={after_y}"
    );
}

#[test]
fn menu_wheel_reaches_real_hit_targets_without_scrolling_board() {
    let size = egui::vec2(440.0, 300.0);
    {
        let mut harness = MenuHarness::new(size, 1.0, false);
        let open = harness.open();
        let menu_point = open.control("Compact window").interact_rect.center();
        assert_fully_interactive(&open, "Always on top", size);
        assert!(
            open.control("Show Kimi").interact_rect.height()
                < open.control("Show Kimi").rect.height() - 1.0,
            "fixture must have an initially clipped last checkbox"
        );
        let original_board = open.board_offset;
        let mut bottom = harness.wheel(menu_point, -600.0);
        for _ in 0..7 {
            if bottom.control("Show Kimi").interact_rect.height()
                >= bottom.control("Show Kimi").rect.height() - 1.0
            {
                break;
            }
            bottom = harness.wheel(menu_point, -600.0);
        }
        let last = assert_fully_interactive(&bottom, "Show Kimi", size);
        assert_eq!(
            bottom.board_offset, original_board,
            "menu wheel moved board"
        );
        let at_bottom = harness.wheel(menu_point, -600.0);
        assert_eq!(
            at_bottom.board_offset, original_board,
            "wheel at menu end moved board"
        );
        let toggled = harness.click(last);

        assert!(
            toggled
                .action
                .hidden
                .as_ref()
                .is_some_and(|hidden| hidden.contains("provider-7"))
        );
        assert!(harness.prefs.hidden.contains("provider-7"));

        let mut top = harness.wheel(menu_point, 600.0);
        for _ in 0..7 {
            if top.control("Always on top").interact_rect.height()
                >= top.control("Always on top").rect.height() - 1.0
            {
                break;
            }
            top = harness.wheel(menu_point, 600.0);
        }
        let first = assert_fully_interactive(&top, "Always on top", size);
        assert_eq!(top.board_offset, original_board);
        let at_top = harness.wheel(menu_point, 600.0);
        assert_eq!(
            at_top.board_offset, original_board,
            "wheel at menu start moved board"
        );
        let pin = harness.click(first);
        assert!(harness.pinned);
        assert!(pin.action.changed);
    }
}

#[test]
fn keyboard_traversal_reveals_and_toggles_first_and_last_settings() {
    let size = egui::vec2(440.0, 300.0);
    {
        let mut harness = MenuHarness::new(size, 1.0, false);
        let open = harness.open();
        let first_id = open.control("Always on top").id;
        harness
            .ctx
            .memory_mut(|memory| memory.request_focus(first_id));
        let focused_first = harness.render(vec![]);
        assert!(focused_first.control("Always on top").focused);
        assert_fully_interactive(&focused_first, "Always on top", size);
        let pin = harness.key(egui::Key::Space, egui::Modifiers::NONE);
        assert!(harness.pinned, "Space must toggle focused pin checkbox");
        assert!(pin.action.changed);

        let mut visited = Vec::new();
        let mut last = None;
        for _ in 0..24 {
            let frame = harness.key(egui::Key::Tab, egui::Modifiers::NONE);
            if let Some(control) = frame.controls.iter().find(|control| control.focused) {
                visited.push(control.label.clone());
                if control.label == "Show Kimi" {
                    last = Some(frame);
                    break;
                }
            }
        }
        let last = last.unwrap_or_else(|| panic!("Tab did not reach final provider: {visited:?}"));
        assert_fully_interactive(&last, "Show Kimi", size);
        let toggle = harness.key(egui::Key::Space, egui::Modifiers::NONE);

        assert!(
            toggle
                .action
                .hidden
                .as_ref()
                .is_some_and(|hidden| hidden.contains("provider-7"))
        );

        let mut returned = false;
        for _ in 0..24 {
            let frame = harness.key(egui::Key::Tab, egui::Modifiers::SHIFT);
            if frame.control("Always on top").focused {
                assert_fully_interactive(&frame, "Always on top", size);
                returned = true;
                break;
            }
        }
        assert!(returned, "Shift+Tab must return to visible first choice");
    }
}

#[test]
fn wheel_can_leave_a_previously_focused_menu_row() {
    let size = egui::vec2(440.0, 300.0);
    let mut harness = MenuHarness::new(size, 1.0, false);
    let open = harness.open();
    harness
        .ctx
        .memory_mut(|memory| memory.request_focus(open.control("Always on top").id));
    let focused = harness.render(vec![]);
    assert!(focused.control("Always on top").focused);
    let pointer = focused.control("Compact window").interact_rect.center();
    let mut bottom = harness.wheel(pointer, -600.0);
    for _ in 0..7 {
        if bottom.control("Show Kimi").interact_rect.height()
            >= bottom.control("Show Kimi").rect.height() - 1.0
        {
            break;
        }
        bottom = harness.wheel(pointer, -600.0);
    }
    assert_fully_interactive(&bottom, "Show Kimi", size);
    let mut next = harness.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert!(next.control("Compact window").focused);
    for _ in 0..10 {
        if next.control("Compact window").interact_rect.height()
            >= next.control("Compact window").rect.height() - 1.0
        {
            break;
        }
        next = harness.render(vec![]);
    }
    assert_fully_interactive(&next, "Compact window", size);
}

#[test]
fn resize_and_scale_keep_long_named_settings_reachable_and_preserve_preferences() {
    for scale in [1.0, 2.0] {
        let mut harness = MenuHarness::new(egui::vec2(800.0, 600.0), scale, true);
        harness.prefs.order = vec!["provider-4".into(), "provider-0".into()];
        harness.prefs.primary = Some("provider-4".into());
        harness.prefs.refresh_secs = 900;
        harness.prefs.hidden.insert("provider-2".into());
        let before = harness.prefs.clone();
        let large = harness.open();
        assert_eq!(large.output.pixels_per_point, scale);
        let size = if scale == 1.0 {
            egui::vec2(440.0, 300.0)
        } else {
            egui::vec2(320.0, 240.0)
        };
        harness.size = size;
        let _ = harness.render(vec![]);
        let mut small = harness.render(vec![]);
        if small.scroll.is_none() {
            small = harness.open();
        }
        assert_eq!(small.output.pixels_per_point, scale);
        let scroll = small
            .scroll
            .expect("settings menu can be reopened after resize");
        assert!(
            scroll.content_height > scroll.viewport_height + 1.0,
            "long-name fixture must overflow at scale {scale}: {scroll:?}"
        );
        assert!(scroll.viewport_height <= size.y - 10.0);
        let menu_point = assert_fully_interactive(&small, "Compact window", size);
        let last_name = format!("Show {}", harness.cards.last().unwrap().short_name);
        let mut bottom = harness.wheel(menu_point, -600.0);
        for _ in 0..12 {
            if bottom.control(&last_name).interact_rect.height()
                >= bottom.control(&last_name).rect.height() - 1.0
            {
                break;
            }
            bottom = harness.wheel(menu_point, -600.0);
        }
        let last = assert_fully_interactive(&bottom, &last_name, size);
        assert!(bottom.scroll.unwrap().offset > 0.0);
        let toggled = harness.click(last);

        assert!(
            toggled
                .action
                .hidden
                .as_ref()
                .is_some_and(|hidden| hidden.contains("provider-7"))
        );
        assert!(harness.prefs.hidden.contains("provider-7"));

        let mut top = harness.wheel(menu_point, 600.0);
        for _ in 0..12 {
            if top.control("Compact window").interact_rect.height()
                >= top.control("Compact window").rect.height() - 1.0
            {
                break;
            }
            top = harness.wheel(menu_point, 600.0);
        }
        let compact = assert_fully_interactive(&top, "Compact window", size);
        let changed = harness.click(compact);
        assert!(harness.compact);
        assert!(changed.action.changed);
        assert_eq!(harness.prefs.order, before.order);
        assert_eq!(harness.prefs.primary, before.primary);
        assert_eq!(harness.prefs.refresh_secs, before.refresh_secs);
        assert_eq!(harness.prefs.quota_display_mode, before.quota_display_mode);
        assert!(harness.prefs.hidden.contains("provider-2"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = ConfigStore::load_from(path.clone());
        config.preferences_mut().board = harness.prefs.clone();
        let window = WindowPreferences {
            pinned: harness.pinned,
            compact: harness.compact,
            ..WindowPreferences::default()
        };
        config.preferences_mut().window = window;
        config.save().unwrap();
        let saved = ConfigStore::load_from(path);
        assert_eq!(saved.preferences().board.order, before.order);
        assert_eq!(saved.preferences().board.primary, before.primary);
        assert_eq!(saved.preferences().board.refresh_secs, before.refresh_secs);
        assert_eq!(
            saved.preferences().board.quota_display_mode,
            before.quota_display_mode
        );
        assert!(saved.preferences().board.hidden.contains("provider-7"));
        assert!(saved.preferences().board.hidden.contains("provider-2"));
        assert!(saved.preferences().window.compact);
    }
}

#[test]
fn normal_800_by_800_menu_has_no_overflow_and_keeps_both_ends_interactive() {
    // The About action and the update opt-in add menu rows; use a viewport
    // that fits all window controls.
    let size = egui::vec2(800.0, 800.0);
    {
        let mut harness = MenuHarness::new(size, 1.0, false);
        let open = harness.open();
        let scroll = open.scroll.unwrap();
        assert!(
            scroll.content_height <= scroll.viewport_height + 1.0,
            "800x680 control unexpectedly overflows: {scroll:?}"
        );
        assert_fully_interactive(&open, "Always on top", size);
        assert_fully_interactive(&open, "Show Kimi", size);
        let point = open.control("Compact window").interact_rect.center();
        let after = harness.wheel(point, -600.0);
        assert_eq!(after.scroll.unwrap().offset, 0.0);
        assert_eq!(after.board_offset, open.board_offset);
    }
}

#[test]
fn appearance_choices_are_accessible_and_keyboard_reachable_in_small_board() {
    for scale in [1.0, 2.0] {
        let size = egui::vec2(360.0, 300.0);
        let mut harness = MenuHarness::new(size, scale, false);
        let open = harness.open();
        assert!(
            open.output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Appearance"))
        );
        for option in [
            ThemePreference::Light,
            ThemePreference::Dark,
            ThemePreference::System,
        ] {
            let frame = harness.render(vec![]);
            let id = frame.control(option.label()).id;
            harness.ctx.memory_mut(|m| m.request_focus(id));
            let _ = harness.render(vec![]);
            let focused = harness.render(vec![]);
            assert!(focused.control(option.label()).focused);
            assert_fully_interactive(&focused, option.label(), size);
            let chosen = harness.key(egui::Key::Space, egui::Modifiers::NONE);
            assert_eq!(chosen.action.theme_preference, Some(option));
            assert_eq!(harness.prefs.theme_preference, option);
            assert!(!chosen.action.refresh);
        }
    }
}

#[test]
fn update_opt_in_is_off_by_default_explained_and_toggles_by_keyboard() {
    let size = egui::vec2(800.0, 800.0);
    let mut harness = MenuHarness::new(size, 1.0, false);
    harness.update_status = UpdateStatus::Unavailable(Unavailable::NoTrustRoot);
    let open = harness.open();
    assert!(!harness.prefs.updates_enabled);
    // The explanation and status are painted below the opt-in.
    let checkbox = row_y(&open.output, UPDATES_CONTROL);
    let explanation = row_y(&open.output, UPDATE_EXPLANATION);
    let status = row_y(
        &open.output,
        "Updates are unavailable: this build has no release signing key.",
    );
    assert!(checkbox < explanation && explanation < status);
    for expected in [true, false] {
        let id = harness.render(vec![]).control(UPDATES_CONTROL).id;
        harness.ctx.memory_mut(|memory| memory.request_focus(id));
        let _ = harness.render(vec![]);
        let focused = harness.render(vec![]);
        assert!(focused.control(UPDATES_CONTROL).focused);
        assert_fully_interactive(&focused, UPDATES_CONTROL, size);
        let toggled = harness.key(egui::Key::Space, egui::Modifiers::NONE);
        assert_eq!(toggled.action.updates_enabled, Some(expected));
        assert!(toggled.action.changed);
        assert_eq!(harness.prefs.updates_enabled, expected);
    }
}

#[test]
fn the_last_installer_start_is_explained_in_plain_sentences() {
    assert_eq!(
        update_status_text(&UpdateStatus::Installed("0.2.2".into())),
        "Token Ashtray was updated to 0.2.2."
    );
    assert_eq!(
        update_status_text(&UpdateStatus::InstallFailed {
            version: "0.2.2".into(),
            attempt: 1,
        }),
        "Installing version 0.2.2 failed (attempt 1 of 3). It is tried again after the next Quit."
    );
    assert_eq!(
        update_status_text(&UpdateStatus::InstallStopped("0.2.2".into())),
        "Version 0.2.2 could not be installed automatically. Install it with the setup from the project's GitHub releases."
    );
}
