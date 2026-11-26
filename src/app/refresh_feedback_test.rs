use super::refresh_feedback::{Feedback, Presentation};
use super::*;
use egui::accesskit::{Action, ActionRequest, Node, NodeId};
use token_ashtray::refresh::{RefreshPhase as Phase, RefreshSnapshot};

fn snapshot(generation: u64, fast: Phase, slow: Phase) -> RefreshSnapshot {
    RefreshSnapshot {
        generation,
        startup: generation == 1,
        providers: [("fast".into(), fast), ("slow".into(), slow)].into(),
    }
}
fn cards() -> Vec<ProviderCard> {
    ["fast", "slow"]
        .into_iter()
        .map(|id| {
            let mut card = token_ashtray::providers::Descriptor {
                id,
                name: id,
                link: "",
                login: "synthetic",
            }
            .empty();
            if id == "fast" {
                card.status = Status::Live;
                card.meters = vec![token_ashtray::model::Meter {
                    label: "Weekly".into(),
                    used_percent: 25.0,
                    resets_at: None,
                }];
                card.detail.clear();
            } else {
                card.detail = "Synthetic provider failure detail".into();
            }
            card
        })
        .collect()
}
fn input(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(720.0, 700.0),
        )),
        events,
        ..Default::default()
    }
}
fn draw(
    ctx: &Context,
    presentation: Presentation,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, BoardAction) {
    refresh_feedback::install(ctx, presentation);
    let mut action = BoardAction::default();
    let output = ctx.run(input(events), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = draw_board(
                ui,
                "Token Ashtray",
                &cards(),
                &BoardPreferences::default(),
                true,
                &mut false,
                &mut false,
            );
        });
    });
    (output, action)
}
fn nodes(output: &egui::FullOutput) -> &Vec<(NodeId, Node)> {
    &output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
}
fn refresh_node(output: &egui::FullOutput) -> &(NodeId, Node) {
    nodes(output)
        .iter()
        .find(|(_, node)| {
            node.label()
                .is_some_and(|text| text.starts_with("Refresh now"))
        })
        .unwrap()
}
#[test]
fn immediate_coalesced_unchanged_outcomes_hold_once_and_cards_expire_independently() {
    let start = Instant::now();
    let mut feedback = Feedback::default();
    let mixed = snapshot(1, Phase::Succeeded, Phase::Running);
    let presentation = feedback.update(mixed.clone(), 0, start);
    assert_eq!(presentation.cards["fast"], Phase::Succeeded);
    assert_eq!(presentation.cards["slow"], Phase::Running);
    let end = snapshot(1, Phase::Succeeded, Phase::Failed);
    let presentation = feedback.update(end.clone(), 0, start + Duration::from_secs(3));
    assert_eq!(presentation.cards["fast"], Phase::Succeeded);
    assert_eq!(presentation.cards["slow"], Phase::Failed);
    let presentation = feedback.update(end.clone(), 0, start + Duration::from_secs(5));
    assert!(!presentation.cards.contains_key("fast"));
    assert_eq!(presentation.cards["slow"], Phase::Failed);
    assert_eq!(
        presentation.toolbar.as_deref(),
        Some("Refresh finished with errors")
    );
    let presentation = feedback.update(end.clone(), 0, start + Duration::from_secs(8));
    assert!(presentation.cards.is_empty());
    assert!(presentation.toolbar.is_none());
    // Scheduled updates retain the same generation and cannot replay a completion.
    assert!(
        feedback
            .update(end, 0, start + Duration::from_secs(10))
            .cards
            .is_empty()
    );
    let request = feedback.request(start + Duration::from_secs(11)).unwrap();
    feedback.accepted(request, Ok(2), start + Duration::from_secs(11));
    let presentation = feedback.update(
        snapshot(2, Phase::Succeeded, Phase::Succeeded),
        0,
        start + Duration::from_secs(11),
    );
    assert_eq!(presentation.cards.len(), 2);
    assert_eq!(presentation.toolbar.as_deref(), Some("Refresh complete"));
}
#[test]
fn reopened_board_suppresses_previous_generation_but_manual_adopts_it() {
    let now = Instant::now();
    let mut feedback = Feedback::default();
    let active = snapshot(1, Phase::Succeeded, Phase::Running);
    assert!(feedback.update(active.clone(), 1, now).cards.is_empty());
    assert!(
        feedback
            .update(snapshot(1, Phase::Succeeded, Phase::Succeeded), 1, now)
            .cards
            .is_empty()
    );
    let request = feedback.request(now).unwrap();
    feedback.accepted(request, Ok(1), now);
    assert_eq!(feedback.update(active, 1, now).cards.len(), 2);
}
#[test]
fn wrong_or_missing_ack_and_dropped_requests_never_infer_success() {
    let now = Instant::now();
    let mut feedback = Feedback::default();
    let request = feedback.request(now).unwrap();
    assert!(feedback.request(now).is_none());
    feedback.accepted(request + 1, Ok(7), now);
    let unrelated = snapshot(1, Phase::Succeeded, Phase::Succeeded);
    let waiting = feedback.update(unrelated.clone(), 0, now);
    assert!(waiting.toolbar.unwrap().contains("waiting"));
    assert!(waiting.cards.is_empty());
    let timed_out = feedback.update(unrelated.clone(), 0, now + Duration::from_secs(6));
    assert!(timed_out.toolbar.unwrap().contains("not confirmed"));
    assert!(timed_out.cards.is_empty());
    assert!(
        feedback
            .update(unrelated.clone(), 0, now + Duration::from_secs(11))
            .toolbar
            .is_none()
    );
    let request = feedback.request(now + Duration::from_secs(12)).unwrap();
    feedback.accepted(
        request,
        Err("Refresh not sent: tray connection is busy".into()),
        now + Duration::from_secs(12),
    );
    let failed = feedback.update(unrelated, 0, now + Duration::from_secs(12));
    assert!(failed.toolbar.unwrap().contains("not sent"));
    assert!(failed.cards.is_empty());
}
#[test]
fn waiting_running_success_failure_are_accessible_on_both_card_types_and_keep_toolbar_bounds() {
    let ctx = Context::default();
    configure_visuals(&ctx);
    ctx.enable_accesskit();
    let (initial, _) = draw(&ctx, Presentation::default(), vec![]);
    let bounds = refresh_node(&initial).1.bounds().unwrap();
    for phase in [
        Phase::Waiting,
        Phase::Running,
        Phase::Succeeded,
        Phase::Failed,
    ] {
        let mut feedback = Feedback::default();
        let presentation = feedback.update(snapshot(1, phase, phase), 0, Instant::now());
        let (output, _) = draw(&ctx, presentation, vec![]);
        assert_eq!(refresh_node(&output).1.bounds().unwrap(), bounds);
        for provider in ["fast", "slow"] {
            let label = format!("{provider}: {}", phase.label());
            let node = nodes(&output)
                .iter()
                .find(|(_, node)| {
                    node.label() == Some(label.as_str()) || node.value() == Some(label.as_str())
                })
                .expect("accessible refresh state on both card types");
            assert_eq!(node.1.live(), Some(egui::accesskit::Live::Polite));
        }
        assert!(
            nodes(&output).iter().any(|(_, node)| node
                .label()
                .is_some_and(|text| text.contains("75%"))
                || node.value().is_some_and(|text| text.contains("75%"))),
            "quota stays accessible"
        );
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == "Synthetic provider failure detail")), "failure details stay visible");
    }
}
#[test]
fn toolbar_mouse_enter_and_space_activate_and_pending_feedback_is_immediate() {
    for key in [None, Some(egui::Key::Enter), Some(egui::Key::Space)] {
        let ctx = Context::default();
        configure_visuals(&ctx);
        ctx.enable_accesskit();
        let (output, _) = draw(&ctx, Presentation::default(), vec![]);
        let (id, node) = refresh_node(&output);
        let id = *id;
        let action = if let Some(key) = key {
            let _ = draw(
                &ctx,
                Presentation::default(),
                vec![egui::Event::AccessKitActionRequest(ActionRequest {
                    action: Action::Focus,
                    target: id,
                    data: None,
                })],
            );
            let (_, pressed) = draw(
                &ctx,
                Presentation::default(),
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            let (_, released) = draw(
                &ctx,
                Presentation::default(),
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            pressed.refresh || released.refresh
        } else {
            let rect = node.bounds().unwrap();
            let point = egui::pos2(
                ((rect.x0 + rect.x1) / 2.0) as f32,
                ((rect.y0 + rect.y1) / 2.0) as f32,
            );
            let _ = draw(
                &ctx,
                Presentation::default(),
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            draw(
                &ctx,
                Presentation::default(),
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            )
            .1
            .refresh
        };
        assert!(action, "toolbar activation: {key:?}");
        let mut feedback = Feedback::default();
        let now = Instant::now();
        feedback.request(now).unwrap();
        let presentation = feedback.update(RefreshSnapshot::default(), 0, now);
        let (output, _) = draw(&ctx, presentation, vec![]);
        assert!(
            refresh_node(&output)
                .1
                .label()
                .unwrap()
                .contains("requested")
        );
    }
}
#[test]
fn reduced_motion_is_static_and_request_failure_preserves_readable_board() {
    let ctx = Context::default();
    configure_visuals(&ctx);
    ctx.style_mut(|style| style.animation_time = 0.0);
    refresh_feedback::install(
        &ctx,
        Presentation {
            toolbar: Some("Refreshing providers".into()),
            cards: Arc::new([("fast".into(), Phase::Running)].into()),
        },
    );
    let _ = ctx.run(input(vec![]), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert_eq!(refresh_feedback::opacity(ui, "fast"), 1.0);
        });
    });
    let dir = tempfile::tempdir().unwrap();
    let (tx, _) = std::sync::mpsc::sync_channel(1);
    let mut app = about_test::app(dir.path().join("config.toml"), tx);
    let (connection, _, messages) = UiConnection::for_test();
    for request in 0..64 {
        connection
            .try_send(ChildMessage::Refresh { request })
            .unwrap();
    }
    app.connection = Some(connection);
    app.request_refresh();
    app.update_refresh_feedback(&ctx);
    assert!(
        refresh_feedback::presentation(&ctx)
            .toolbar
            .unwrap()
            .contains("busy")
    );
    drop(messages);
    app.request_refresh();
    app.update_refresh_feedback(&ctx);
    assert!(
        refresh_feedback::presentation(&ctx)
            .toolbar
            .unwrap()
            .contains("lost")
    );
    assert!(!dir.path().join("config.toml").exists());
}
