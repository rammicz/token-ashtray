use super::*;
use eframe::App;
use egui::{Color32, Shape, Theme};
use token_ashtray::model::{Meter, ProductUsage};

fn input(viewport: ViewportId, scale: f32, system: Option<Theme>) -> egui::RawInput {
    let mut input = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(800.0, 700.0),
        )),
        system_theme: system,
        ..Default::default()
    };
    input.viewports.insert(
        viewport,
        egui::ViewportInfo {
            parent: (viewport != ViewportId::ROOT).then_some(ViewportId::ROOT),
            native_pixels_per_point: Some(scale),
            ..Default::default()
        },
    );
    input
}
fn shapes(output: &egui::FullOutput) -> Vec<&Shape> {
    fn walk<'a>(shape: &'a Shape, into: &mut Vec<&'a Shape>) {
        if let Shape::Vec(children) = shape {
            for child in children {
                walk(child, into);
            }
        } else {
            into.push(shape);
        }
    }
    let mut result = Vec::new();
    for shape in &output.shapes {
        walk(&shape.shape, &mut result);
    }
    result
}
fn contrast(a: Color32, b: Color32) -> f32 {
    fn luminance(c: Color32) -> f32 {
        let channel = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn provider() -> ProviderCard {
    ProviderCard {
        id: "codex".into(),
        short_name: "Codex".into(),
        plan: Some("Pro".into()),
        price: Some("$20/month".into()),
        meters: vec![Meter {
            label: "Weekly".into(),
            used_percent: 25.0,
            resets_at: None,
        }],
        usage_breakdown: vec![ProductUsage {
            label: "CLI".into(),
            used_percent: 25.0,
        }],
        status: Status::Live,
        detail: String::new(),
        link: String::new(),
        fetched_at: None,
    }
}

#[test]
fn appearance_system_sequence_manual_overrides_and_two_complete_styles() {
    let ctx = Context::default();
    configure_visuals(&ctx);
    let appearance = theme::Appearance::default();
    for (preference, os, expected) in [
        (ThemePreference::System, Some(Theme::Light), Theme::Light),
        (ThemePreference::System, Some(Theme::Dark), Theme::Dark),
        (ThemePreference::Light, Some(Theme::Dark), Theme::Light),
        (ThemePreference::Dark, Some(Theme::Light), Theme::Dark),
        (ThemePreference::System, Some(Theme::Light), Theme::Light),
    ] {
        let _ = ctx.run(input(ViewportId::ROOT, 1.0, os), |ctx| {
            appearance.apply(ctx, preference);
            assert_eq!(ctx.theme(), expected);
            assert_eq!(ctx.style().visuals.dark_mode, expected == Theme::Dark);
            assert_eq!(ctx.style().spacing.interact_size.y, 28.0);
            assert_eq!(ctx.style().text_styles[&egui::TextStyle::Body].size, 14.0);
        });
    }
    #[cfg(target_os = "linux")]
    for preference in [
        ThemePreference::System,
        ThemePreference::Light,
        ThemePreference::Dark,
    ] {
        for system in [Some(Theme::Light), Some(Theme::Dark), None] {
            theme::apply_system(&ctx, preference, system);
            assert_eq!(
                ctx.theme(),
                match preference {
                    ThemePreference::System => system.unwrap_or(Theme::Dark),
                    ThemePreference::Light => Theme::Light,
                    ThemePreference::Dark => Theme::Dark,
                }
            );
        }
    }
}

#[test]
fn appearance_real_board_shapes_have_readable_light_dark_cards_meters_and_focus() {
    for (preference, dark) in [
        (ThemePreference::Light, false),
        (ThemePreference::Dark, true),
    ] {
        for scale in [1.0, 2.0] {
            for mode in [QuotaDisplayMode::Remaining, QuotaDisplayMode::Used] {
                let ctx = Context::default();
                configure_visuals(&ctx);
                theme::Appearance::default().apply(&ctx, preference);
                let view = ViewportId::ROOT;
                let prefs = BoardPreferences {
                    theme_preference: preference,
                    quota_display_mode: mode,
                    primary: Some("codex".into()),
                    ..Default::default()
                };
                let focus = Id::new(("provider-primary", "codex"));
                let mut output = None;
                for _ in 0..2 {
                    ctx.memory_mut(|m| m.request_focus(focus));
                    output = Some(ctx.run(input(view, scale, None), |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            draw_board(
                                ui,
                                "Token Ashtray",
                                &[provider()],
                                &prefs,
                                false,
                                &mut false,
                                &mut false,
                            );
                        });
                    }));
                }
                let output = output.unwrap();
                let painted = shapes(&output);
                let card = painted
                    .iter()
                    .filter_map(|shape| {
                        if let Shape::Rect(rect) = shape {
                            Some(rect)
                        } else {
                            None
                        }
                    })
                    .find(|rect| rect.corner_radius.nw == 8 && rect.rect.width() > 300.0)
                    .expect("real painted provider card");
                assert_eq!(
                    card.fill.r() > 200,
                    !dark,
                    "custom card must follow active style"
                );
                let text = painted
                    .iter()
                    .filter_map(|shape| {
                        if let Shape::Text(text) = shape {
                            Some(text)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                for label in ["Weekly", "Used", "CLI"] {
                    let shape = text
                        .iter()
                        .find(|shape| shape.galley.job.text == label)
                        .unwrap_or_else(|| panic!("missing real label {label}"));
                    assert!(
                        contrast(shape.galley.job.sections[0].format.color, card.fill) >= 4.5,
                        "unreadable {label} in {preference:?}"
                    );
                }
                let gauge = painted
                    .iter()
                    .filter_map(|shape| {
                        if let Shape::Rect(rect) = shape {
                            Some(rect)
                        } else {
                            None
                        }
                    })
                    .filter(|rect| rect.rect.height() == QUOTA_TRACK_HEIGHT)
                    .collect::<Vec<_>>();
                assert_eq!(
                    gauge.len(),
                    2,
                    "track and actual quota fill must both be visited"
                );
                let (track, fill) = (gauge[0], gauge[1]);
                let expected_value = mode.percent_label(25.0);
                let values = text
                    .iter()
                    .filter(|shape| {
                        shape.override_text_color != Some(Color32::BLACK)
                            && track.rect.contains(shape.pos)
                            && (shape.galley.job.text == "0"
                                || shape.galley.job.text.ends_with('%'))
                    })
                    .collect::<Vec<_>>();
                assert_eq!(values.len(), 1, "one foreground value, no endpoint labels");
                assert!(text.iter().all(|shape| {
                    shape.galley.job.text != "0" && shape.galley.job.text != "100%"
                }));
                let value = values[0];
                assert_eq!(value.galley.job.text, expected_value);
                assert!(
                    ((value.pos + value.galley.size() / 2.0) - track.rect.center()).length() < 0.01,
                    "selected percentage must stay centered on the track"
                );
                let outlines = text
                    .iter()
                    .filter(|shape| {
                        shape.override_text_color == Some(Color32::BLACK)
                            && Arc::ptr_eq(&shape.galley, &value.galley)
                    })
                    .collect::<Vec<_>>();
                assert!(
                    !outlines.is_empty(),
                    "value needs its contrasting glyph edge"
                );
                for background in [track.fill, fill.fill] {
                    let foreground = value.galley.job.sections[0].format.color;
                    assert!(
                        contrast(foreground, background).max(contrast(Color32::BLACK, background))
                            >= 4.5,
                        "outlined value must read over filled and unfilled track in {preference:?}"
                    );
                }
                assert!(
                    (fill.rect.width() / track.rect.width() - mode.percent(25.0) / 100.0).abs()
                        < 0.001
                );
                assert!(
                    contrast(fill.fill, card.fill) >= 3.0,
                    "quota color must read on its card"
                );
                assert!(painted.iter().any(|shape| matches!(shape, Shape::Rect(rect) if rect.stroke.color == palette(&ctx).blue && rect.stroke.width == 1.0)), "real selected/focused primary outline missing");
                assert!(painted.iter().any(|shape| matches!(shape, Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default())), "provider logo texture must survive");
            }
        }
    }
}

fn node(output: &egui::FullOutput, label: &str) -> egui::accesskit::NodeId {
    output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label) || node.value() == Some(label))
        .unwrap_or_else(|| panic!("missing {label}"))
        .0
}
fn click(id: egui::accesskit::NodeId) -> egui::Event {
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target: id,
        data: None,
    })
}

#[test]
fn appearance_board_action_repaints_about_without_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(64);
    let mut app = about_test::app(dir.path().join("config.toml"), tx);
    app.main_visible = true;
    let ctx = Context::default();
    ctx.enable_accesskit();
    configure_visuals(&ctx);
    let mut render = |events| {
        let mut raw = input(ViewportId::ROOT, 1.0, None);
        raw.events = events;
        ctx.run(raw, |ctx| {
            app.update(ctx, &mut eframe::Frame::_new_kittest())
        })
    };
    let _ = render(vec![]);
    let board = render(vec![]);
    let _ = render(vec![click(node(&board, "Window settings"))]);
    let menu = render(vec![]);
    let _ = render(vec![click(node(&menu, "Light"))]);
    let _ = render(vec![]);
    assert_eq!(
        app.board_preferences.read().unwrap().theme_preference,
        ThemePreference::Light
    );
    let messages = rx.try_iter().collect::<Vec<_>>();
    assert!(messages.iter().any(|m| matches!(m, ChildMessage::SavePreferences(p) if p.board.theme_preference == ThemePreference::Light)));
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m, ChildMessage::Refresh { .. }))
    );
    app.about = true;
    let _ = ctx.run(input(ViewportId::ROOT, 1.0, None), |ctx| {
        app.update(ctx, &mut eframe::Frame::_new_kittest())
    });
    assert!(!ctx.style().visuals.dark_mode);
    let about = ctx.run(input(ViewportId::ROOT, 1.0, None), |ctx| {
        app.update(ctx, &mut eframe::Frame::_new_kittest())
    });
    assert!(
        shapes(&about).iter().any(
            |s| matches!(s, Shape::Text(t) if t.galley.job.text.contains("About Token Ashtray"))
        )
    );
}

#[test]
fn appearance_strong_and_selected_native_labels_keep_text_contrast() {
    for preference in [ThemePreference::Light, ThemePreference::Dark] {
        let ctx = Context::default();
        configure_visuals(&ctx);
        theme::Appearance::default().apply(&ctx, preference);
        let output = ctx.run(input(ViewportId::ROOT, 1.0, None), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.strong("Appearance heading");
                let _ = ui.selectable_label(true, "Selected choice");
            });
        });
        let painted = shapes(&output);
        let text = |label| {
            painted
                .iter()
                .find_map(|s| match s {
                    Shape::Text(t) if t.galley.job.text == label => Some(t),
                    _ => None,
                })
                .unwrap()
        };
        let heading = text("Appearance heading");
        assert!(
            contrast(
                heading.galley.job.sections[0].format.color,
                ctx.style().visuals.panel_fill
            ) >= 4.5,
            "strong text must remain visible on {preference:?} surfaces"
        );
        let selected = text("Selected choice");
        let color = if selected.galley.job.sections[0].format.color == Color32::PLACEHOLDER {
            selected.fallback_color
        } else {
            selected.galley.job.sections[0].format.color
        };
        assert!(
            contrast(color, ctx.style().visuals.selection.bg_fill) >= 4.5,
            "selected label must remain readable on {preference:?} selection"
        );
    }
}

#[test]
fn appearance_status_labels_remain_readable_in_both_palettes() {
    for preference in [ThemePreference::Light, ThemePreference::Dark] {
        let ctx = Context::default();
        configure_visuals(&ctx);
        theme::Appearance::default().apply(&ctx, preference);
        let cards = [
            Status::Live,
            Status::Stale,
            Status::Reconnect,
            Status::Error,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, status)| {
            let mut card = provider();
            card.id = format!("status-{index}");
            card.short_name = format!("Provider {index}");
            card.status = status;
            card.detail = "Synthetic status detail".into();
            card
        })
        .collect::<Vec<_>>();
        let output = ctx.run(input(ViewportId::ROOT, 1.0, None), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_board(
                    ui,
                    "Token Ashtray",
                    &cards,
                    &BoardPreferences::default(),
                    false,
                    &mut false,
                    &mut false,
                );
            });
        });
        let painted = shapes(&output);
        for label in ["Stale", "Reconnect", "Error"] {
            let text = painted
                .iter()
                .find_map(|shape| match shape {
                    Shape::Text(text) if text.galley.job.text == label => Some(text),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {label} status meaning"));
            assert!(
                contrast(
                    text.galley.job.sections[0].format.color,
                    palette(&ctx).surface
                ) >= 4.5,
                "unreadable {label} in {preference:?}"
            );
        }
    }
}

#[test]
fn appearance_severity_boundaries_match_chart_contract_in_both_palettes() {
    // docs/CHART-DESIGN.md: remaining <=10% is critical, <=25% warns.
    // Appearance may change hues, never these severity bands.
    for dark in [false, true] {
        for accent in 0..3 {
            let critical = meter_color(0.0, Status::Live, accent, dark);
            let warning = meter_color(20.0, Status::Live, accent, dark);
            let normal = meter_color(100.0, Status::Live, accent, dark);
            let muted = meter_color(100.0, Status::Stale, accent, dark);
            assert_ne!(critical, warning, "severity bands must remain distinct");
            assert_ne!(warning, normal);
            assert_ne!(critical, normal);
            assert!(![critical, warning, normal].contains(&muted));
            for remaining in [0.0, 9.999, 10.0] {
                assert_eq!(
                    meter_color(remaining, Status::Live, accent, dark),
                    critical,
                    "<=10% must be critical: remaining={remaining}, dark={dark}"
                );
            }
            for remaining in [10.001, 12.0, 25.0] {
                assert_eq!(
                    meter_color(remaining, Status::Live, accent, dark),
                    warning,
                    ">10% and <=25% must warn: remaining={remaining}, dark={dark}"
                );
            }
            for remaining in [25.001, 30.0, 100.0] {
                assert_eq!(
                    meter_color(remaining, Status::Live, accent, dark),
                    normal,
                    ">25% must retain normal accent: remaining={remaining}, dark={dark}"
                );
            }
            for remaining in [0.0, 10.0, 10.001, 25.0, 25.001, 100.0] {
                for status in [Status::Stale, Status::Reconnect, Status::Error] {
                    assert_eq!(
                        meter_color(remaining, status, accent, dark),
                        muted,
                        "non-live quota must be muted independently of severity"
                    );
                }
            }
        }
    }
}
