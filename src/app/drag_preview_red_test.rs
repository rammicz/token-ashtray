use super::*;
use token_ashtray::model::Meter;

fn card(id: &str, name: &str) -> ProviderCard {
    ProviderCard {
        id: id.into(),
        short_name: name.into(),
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
        fetched_at: Some(OffsetDateTime::now_utc()),
    }
}

fn render(ctx: &Context, events: Vec<egui::Event>) -> (egui::FullOutput, BoardAction) {
    let cards = [card("alpha", "Alpha"), card("beta", "Beta")];
    render_with(ctx, &cards, &BoardPreferences::default(), 440.0, events)
}

fn render_with(
    ctx: &Context,
    cards: &[ProviderCard],
    prefs: &BoardPreferences,
    width: f32,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, BoardAction) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(width, 1200.0),
        )),
        events,
        ..Default::default()
    };
    let mut action = None;
    let output = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = Some(draw_board(
                ui,
                "Token Ashtray",
                cards,
                prefs,
                false,
                &mut false,
                &mut false,
            ));
        });
    });
    (output, action.unwrap())
}

fn slot(ctx: &Context, id: &str) -> DragSlot {
    let layout_id = Id::new(("card-drag-layout", ctx.viewport_id()));
    ctx.data(|data| data.get_temp::<Vec<DragSlot>>(layout_id))
        .unwrap()
        .into_iter()
        .find(|slot| slot.id == id)
        .unwrap()
}

fn pointer_button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn begin_drag(
    ctx: &Context,
    cards: &[ProviderCard],
    prefs: &BoardPreferences,
    width: f32,
    source: &str,
) -> Pos2 {
    let source_slot = slot(ctx, source);
    let grip = source_slot.rect.min + egui::vec2(19.0, 22.0);
    let _ = render_with(
        ctx,
        cards,
        prefs,
        width,
        vec![egui::Event::PointerMoved(grip)],
    );
    let _ = render_with(ctx, cards, prefs, width, vec![pointer_button(grip, true)]);
    grip
}

fn text_y(output: &egui::FullOutput, name: &str) -> f32 {
    fn find(shape: &egui::Shape, name: &str) -> Option<f32> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == name => Some(text.pos.y),
            egui::Shape::Vec(children) => children.iter().find_map(|child| find(child, name)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, name))
        .unwrap()
}

fn text_positions(output: &egui::FullOutput, name: &str) -> Vec<Pos2> {
    fn find(shape: &egui::Shape, name: &str, found: &mut Vec<Pos2>) {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == name => found.push(text.pos),
            egui::Shape::Vec(children) => {
                for child in children {
                    find(child, name, found);
                }
            }
            _ => {}
        }
    }
    let mut positions = Vec::new();
    for shape in &output.shapes {
        find(&shape.shape, name, &mut positions);
    }
    positions
}

#[test]
fn card_order_previews_before_pointer_release() {
    use egui::{Event, Modifiers, PointerButton};
    let ctx = Context::default();
    let (before, _) = render(&ctx, vec![]);
    let original_beta_y = text_y(&before, "Beta");
    let grip = slot(&ctx, "alpha").rect.min + egui::vec2(19.0, 22.0);
    let target = egui::pos2(200.0, original_beta_y + 8.0);
    let _ = render(&ctx, vec![Event::PointerMoved(grip)]);
    let _ = render(
        &ctx,
        vec![Event::PointerButton {
            pos: grip,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    let (held, action) = render(&ctx, vec![Event::PointerMoved(target)]);
    assert!(
        ctx.is_being_dragged(Id::new(("provider-card", "alpha"))),
        "test must hold the Alpha drag source"
    );
    assert!(
        action.order.is_none(),
        "committed preference must wait for release"
    );
    assert!(
        text_y(&held, "Beta") < original_beta_y - 50.0,
        "Beta should fill Alpha's slot while Alpha is held over Beta; before={} held={}",
        original_beta_y,
        text_y(&held, "Beta")
    );
}

#[test]
fn held_drag_moves_card_name_and_quota_content_with_cursor_once() {
    use egui::{Event, Modifiers, PointerButton};
    let ctx = Context::default();
    let (before, _) = render(&ctx, vec![]);
    let original_beta_y = text_y(&before, "Beta");
    let grip = slot(&ctx, "alpha").rect.min + egui::vec2(19.0, 22.0);
    let target = egui::pos2(200.0, original_beta_y + 8.0);
    let _ = render(&ctx, vec![Event::PointerMoved(grip)]);
    let _ = render(
        &ctx,
        vec![Event::PointerButton {
            pos: grip,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    let (held, _) = render(&ctx, vec![Event::PointerMoved(target)]);
    assert!(ctx.is_being_dragged(Id::new(("provider-card", "alpha"))));
    for text in ["Alpha", "Weekly"] {
        let before_pos = text_positions(&before, text);
        let held_pos = text_positions(&held, text);
        let count = if text == "Alpha" { 1 } else { 2 };
        assert_eq!(before_pos.len(), count, "{text} count before drag");
        assert_eq!(held_pos.len(), count, "{text} count while held");
        assert!(
            before_pos.iter().any(|before| held_pos.iter().any(|held| {
                let delta = *held - *before;
                (delta.x - (target.x - grip.x)).abs() < 2.0
                    && (delta.y - (target.y - grip.y)).abs() < 2.0
            })),
            "{text} must follow the cursor with its card"
        );
    }
}

#[test]
fn held_reflow_is_stable_in_both_directions_and_across_grid_rows() {
    for width in [440.0, 1050.0] {
        for (source, target) in [("alpha", "delta"), ("delta", "alpha")] {
            let ctx = Context::default();
            let mut cards = [
                card("alpha", "Alpha"),
                card("beta", "Beta"),
                card("gamma", "Gamma"),
                card("delta", "Delta"),
            ];
            for (index, provider) in cards.iter_mut().enumerate() {
                provider.meters = (0..=index)
                    .map(|meter| Meter {
                        label: format!("Meter {meter}"),
                        used_percent: 25.0,
                        resets_at: None,
                    })
                    .collect();
            }
            let prefs = BoardPreferences::default();
            let (before, _) = render_with(&ctx, &cards, &prefs, width, vec![]);
            let target_pos = slot(&ctx, target).rect.center();
            let grip = begin_drag(&ctx, &cards, &prefs, width, source);
            let (held, action) = render_with(
                &ctx,
                &cards,
                &prefs,
                width,
                vec![egui::Event::PointerMoved(target_pos)],
            );
            assert!(
                ctx.is_being_dragged(Id::new(("provider-card", source))),
                "real pointer drag required: {source} at width {width}, grip {grip:?}"
            );
            assert!(action.order.is_none());
            let moved_peers = ["Alpha", "Beta", "Gamma", "Delta"]
                .into_iter()
                .filter(|name| !name.eq_ignore_ascii_case(source))
                .filter(|name| text_positions(&before, name) != text_positions(&held, name))
                .count();
            assert!(moved_peers > 0, "peers must reflow before release");
            for name in ["Alpha", "Beta", "Gamma", "Delta"] {
                assert_eq!(text_positions(&held, name).len(), 1, "no duplicate {name}");
            }
            let (steady, steady_action) = render_with(&ctx, &cards, &prefs, width, vec![]);
            assert!(steady_action.order.is_none());
            for name in ["Alpha", "Beta", "Gamma", "Delta"] {
                assert_eq!(
                    text_positions(&held, name),
                    text_positions(&steady, name),
                    "stationary pointer changed {name} at width {width}"
                );
            }
            let (released, committed) = render_with(
                &ctx,
                &cards,
                &prefs,
                width,
                vec![pointer_button(target_pos, false)],
            );
            let expected: Vec<String> = match source {
                "alpha" => vec![
                    "beta".into(),
                    "gamma".into(),
                    "delta".into(),
                    "alpha".into(),
                ],
                "delta" => vec![
                    "delta".into(),
                    "alpha".into(),
                    "beta".into(),
                    "gamma".into(),
                ],
                _ => unreachable!("the fixture only drags alpha or delta"),
            };
            assert_eq!(committed.order, Some(expected));
            assert!(committed.changed);
            for name in ["Alpha", "Beta", "Gamma", "Delta"] {
                if !name.eq_ignore_ascii_case(source) {
                    assert_eq!(
                        text_positions(&steady, name),
                        text_positions(&released, name),
                        "release must confirm displayed peer order"
                    );
                }
            }
        }
    }
}

#[test]
fn unavailable_preview_has_full_row_content_and_cross_section_drop_is_invalid() {
    let ctx = Context::default();
    let mut cards = [
        card("alpha", "Alpha"),
        card("beta", "Beta"),
        card("gamma", "Gamma"),
        card("delta", "Delta"),
    ];
    for provider in &mut cards[2..] {
        provider.status = Status::Error;
        provider.detail = format!("{} offline detail", provider.short_name);
    }
    let prefs = BoardPreferences::default();
    let (before, _) = render_with(&ctx, &cards, &prefs, 440.0, vec![]);
    let gamma = slot(&ctx, "gamma");
    let delta = slot(&ctx, "delta");
    let grip = begin_drag(&ctx, &cards, &prefs, 440.0, "gamma");
    let (held, action) = render_with(
        &ctx,
        &cards,
        &prefs,
        440.0,
        vec![egui::Event::PointerMoved(delta.rect.center())],
    );
    assert!(ctx.is_being_dragged(Id::new(("provider-row", "gamma"))));
    assert!(action.order.is_none());
    assert!(text_y(&held, "Delta") < text_y(&before, "Delta") - 30.0);
    for text in ["Gamma", "Gamma offline detail"] {
        let start = text_positions(&before, text);
        let end = text_positions(&held, text);
        assert_eq!(start.len(), 1);
        assert_eq!(end.len(), 1);
        let delta_pos = end[0] - start[0];
        let pointer_delta = delta.rect.center() - grip;
        assert!((delta_pos.x - pointer_delta.x).abs() < 2.0);
        assert!((delta_pos.y - pointer_delta.y).abs() < 2.0);
    }
    let (_, committed) = render_with(
        &ctx,
        &cards,
        &prefs,
        440.0,
        vec![pointer_button(delta.rect.center(), false)],
    );
    assert_eq!(
        committed.order,
        Some(vec![
            "alpha".into(),
            "beta".into(),
            "delta".into(),
            "gamma".into(),
        ])
    );

    let ctx = Context::default();
    let (before, _) = render_with(&ctx, &cards, &prefs, 440.0, vec![]);
    let near_handle = egui::pos2(
        slot(&ctx, "delta").rect.left() + 22.0,
        slot(&ctx, "delta").rect.center().y,
    );
    let _ = begin_drag(&ctx, &cards, &prefs, 440.0, "gamma");
    let (held, _) = render_with(
        &ctx,
        &cards,
        &prefs,
        440.0,
        vec![egui::Event::PointerMoved(near_handle)],
    );
    let (steady, _) = render_with(&ctx, &cards, &prefs, 440.0, vec![]);
    assert!(text_y(&held, "Delta") < text_y(&before, "Delta") - 30.0);
    assert_eq!(
        text_positions(&held, "Delta"),
        text_positions(&steady, "Delta"),
        "unavailable row order must remain stable near the handle"
    );

    let ctx = Context::default();
    let _ = render_with(&ctx, &cards, &prefs, 440.0, vec![]);
    let _ = begin_drag(&ctx, &cards, &prefs, 440.0, "alpha");
    let (held, _) = render_with(
        &ctx,
        &cards,
        &prefs,
        440.0,
        vec![egui::Event::PointerMoved(gamma.rect.center())],
    );
    assert_eq!(
        text_positions(&held, "Beta"),
        text_positions(&before, "Beta"),
        "cross-section hover must not promise a reorder that the renderer cannot display"
    );
    let (_, invalid) = render_with(
        &ctx,
        &cards,
        &prefs,
        440.0,
        vec![pointer_button(gamma.rect.center(), false)],
    );
    assert!(invalid.order.is_none());
    assert!(!invalid.changed);

    let ctx = Context::default();
    let mut changing = [card("alpha", "Alpha"), card("beta", "Beta")];
    let _ = render_with(&ctx, &changing, &prefs, 440.0, vec![]);
    let beta = slot(&ctx, "beta");
    let _ = begin_drag(&ctx, &changing, &prefs, 440.0, "alpha");
    let _ = render_with(
        &ctx,
        &changing,
        &prefs,
        440.0,
        vec![egui::Event::PointerMoved(beta.rect.center())],
    );
    changing[1].status = Status::Error;
    let (_, pending) = render_with(&ctx, &changing, &prefs, 440.0, vec![]);
    assert!(pending.order.is_none());
    let (_, invalid) = render_with(
        &ctx,
        &changing,
        &prefs,
        440.0,
        vec![pointer_button(beta.rect.center(), false)],
    );
    assert!(
        invalid.order.is_none(),
        "status change may not make a cross-section drop valid"
    );
    assert!(!invalid.changed);
}

#[test]
fn outside_release_and_escape_restore_original_order_without_save() {
    let cards = [card("alpha", "Alpha"), card("beta", "Beta")];
    let prefs = BoardPreferences::default();
    for escape in [false, true] {
        let ctx = Context::default();
        let (before, _) = render_with(&ctx, &cards, &prefs, 440.0, vec![]);
        let beta = slot(&ctx, "beta");
        let _ = begin_drag(&ctx, &cards, &prefs, 440.0, "alpha");
        let (held, action) = render_with(
            &ctx,
            &cards,
            &prefs,
            440.0,
            vec![egui::Event::PointerMoved(beta.rect.center())],
        );
        assert!(text_y(&held, "Beta") < text_y(&before, "Beta") - 50.0);
        assert!(action.order.is_none());
        if escape {
            let (cancelled, action) = render_with(
                &ctx,
                &cards,
                &prefs,
                440.0,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(action.order.is_none());
            assert_eq!(
                text_positions(&cancelled, "Beta"),
                text_positions(&before, "Beta")
            );
        }
        let release_at = if escape {
            beta.rect.center()
        } else {
            egui::pos2(-50.0, -50.0)
        };
        let (released, action) = render_with(
            &ctx,
            &cards,
            &prefs,
            440.0,
            vec![pointer_button(release_at, false)],
        );
        assert!(action.order.is_none(), "cancelled drag must not commit");
        assert!(!action.changed, "cancelled drag must not save");
        assert_eq!(
            text_positions(&released, "Beta"),
            text_positions(&before, "Beta")
        );
    }
    let ctx = Context::default();
    let _ = render_with(&ctx, &cards, &prefs, 440.0, vec![]);
    let beta = slot(&ctx, "beta");
    let _ = begin_drag(&ctx, &cards, &prefs, 440.0, "alpha");
    let _ = render_with(
        &ctx,
        &cards,
        &prefs,
        440.0,
        vec![egui::Event::PointerMoved(beta.rect.center())],
    );
    let (_, action) = render_with(
        &ctx,
        &cards,
        &prefs,
        440.0,
        vec![
            egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            pointer_button(beta.rect.center(), false),
        ],
    );
    assert!(
        action.order.is_none(),
        "Escape and release in one frame must cancel"
    );
    assert!(!action.changed);
}

#[test]
fn valid_drop_persists_and_reappears_after_reload_without_losing_hidden_ids() {
    let cards = [
        card("alpha", "Alpha"),
        card("hidden", "Hidden"),
        card("beta", "Beta"),
    ];
    let mut prefs = BoardPreferences::default();
    prefs.hidden.insert("hidden".into());
    {
        let ctx = Context::default();
        let (before, _) = render_with(&ctx, &cards, &prefs, 440.0, vec![]);
        let beta = slot(&ctx, "beta");
        let _ = begin_drag(&ctx, &cards, &prefs, 440.0, "alpha");
        let (held, pending) = render_with(
            &ctx,
            &cards,
            &prefs,
            440.0,
            vec![egui::Event::PointerMoved(beta.rect.center())],
        );
        assert!(pending.order.is_none());
        assert!(!pending.changed);
        assert_eq!(
            prefs.order,
            Vec::<String>::new(),
            "preview cannot mutate shared preferences"
        );
        assert!(text_y(&held, "Beta") < text_y(&before, "Beta") - 50.0);
        let (_, committed) = render_with(
            &ctx,
            &cards,
            &prefs,
            440.0,
            vec![pointer_button(beta.rect.center(), false)],
        );
        assert_eq!(
            committed.order.as_deref(),
            Some(&["hidden".into(), "beta".into(), "alpha".into()][..])
        );
        assert!(committed.changed);
        let mut next = prefs.clone();
        apply_board_preferences(&mut next, &committed, |_| {});
        assert_eq!(next.hidden, prefs.hidden);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        let mut store = ConfigStore::load_from(path.clone());
        store.preferences_mut().board = next.clone();
        store.save().unwrap();
        let restarted = ConfigStore::load_from(path).preferences().board.clone();
        assert_eq!(restarted.order, next.order);
        assert_eq!(restarted.hidden, prefs.hidden);
        let ordered = restarted
            .order
            .iter()
            .filter_map(|id| cards.iter().find(|card| &card.id == id))
            .cloned()
            .collect::<Vec<_>>();
        let second_window = Context::default();
        let (display, _) = render_with(&second_window, &ordered, &restarted, 440.0, vec![]);
        assert!(text_y(&display, "Beta") < text_y(&display, "Alpha"));
        assert!(text_positions(&display, "Hidden").is_empty());
    }
}
