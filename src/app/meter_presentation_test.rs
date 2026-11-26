// Decorative black passes share the one foreground galley and never create
// another semantic value. Keep their glyph coverage separate in shape checks.
fn is_value_outline(text: &egui::epaint::TextShape) -> bool {
    text.override_text_color == Some(egui::Color32::BLACK)
        && text.galley.job.text.ends_with('%')
        && text
            .galley
            .job
            .sections
            .iter()
            .all(|s| s.format.color == egui::Color32::WHITE)
}
struct QuotaClockGuard(Option<OffsetDateTime>);
impl Drop for QuotaClockGuard {
    fn drop(&mut self) {
        QUOTA_TEST_NOW.with(|clock| clock.set(self.0));
    }
}
fn frozen_quota_clock() -> QuotaClockGuard {
    QuotaClockGuard(QUOTA_TEST_NOW.with(|clock| {
        clock.replace(Some(
            OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
        ))
    }))
}
fn presentation_cards() -> Vec<ProviderCard> {
    serde_json::from_str(include_str!("../../tests/fixtures/meter-presentation.json")).unwrap()
}
fn contrast_ratio(a: egui::Color32, b: egui::Color32) -> f32 {
    let luminance = |c: egui::Color32| {
        let linear = |v: u8| {
            let s = v as f32 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    };
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
#[test]
fn meter_overlay_has_one_centered_value_and_contrast_through_the_fill_boundary() {
    let _clock = frozen_quota_clock();
    for theme in [ThemePreference::Light, ThemePreference::Dark] {
        for used in [0.0, 0.4, 23.0, 49.0, 50.0, 51.0, 100.0] {
            let ctx = Context::default();
            configure_visuals(&ctx);
            theme::Appearance::default().apply(&ctx, theme);
            ctx.enable_accesskit();
            let mut meter = card(used).meters.remove(0);
            meter.resets_at =
                Some(quota_now() + time::Duration::days(6) + time::Duration::hours(23));
            let output = ctx.run(accessibility_input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let layout = quota_row_layout(ui, &meter, Status::Live, 320.0);
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(20.0, 20.0),
                        egui::vec2(320.0, layout.height(layout.header_height)),
                    );
                    draw_quota_meter(
                        ui,
                        &meter,
                        Status::Live,
                        "",
                        Id::new("presentation-meter"),
                        rect,
                        0,
                        QuotaDisplayMode::Used,
                    );
                });
            });
            let all = shapes(&output);
            let palette = palette(&ctx);
            let tracks: Vec<_> = all
                .iter()
                .filter_map(|s| match s {
                    egui::Shape::Rect(r)
                        if r.fill == palette.outline && r.rect.height() == QUOTA_TRACK_HEIGHT =>
                    {
                        Some(r.rect)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(tracks.len(), 1);
            let track = tracks[0];
            let expected = QuotaDisplayMode::Used.percent_label(used);
            let numeric_foreground: Vec<_> = all
                .iter()
                .filter_map(|s| match s {
                    egui::Shape::Text(t)
                        if !is_value_outline(t)
                            && (t.galley.job.text == "0" || t.galley.job.text.ends_with('%')) =>
                    {
                        Some(t.galley.job.text.as_str())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                numeric_foreground,
                vec![expected.as_str()],
                "no fixed 0/100% references in any position"
            );
            let values: Vec<_> = all
                .iter()
                .filter_map(|s| match s {
                    egui::Shape::Text(t)
                        if t.galley.job.text == expected && !is_value_outline(t) =>
                    {
                        Some(t)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                values.len(),
                1,
                "one foreground percentage, separate from its glyph outline"
            );
            let value = values[0];
            assert!(((value.pos + value.galley.size() / 2.0) - track.center()).length() < 0.01);
            assert!(
                !all.iter()
                    .any(|s| matches!(s, egui::Shape::LineSegment { .. }))
            );
            assert!(!all.iter().any(|s|matches!(s,egui::Shape::Text(t) if t.pos.y>=track.bottom() && (t.galley.job.text=="0" || t.galley.job.text.ends_with('%')))));
            let outlines: Vec<_> = all
                .iter()
                .filter_map(|s| match s {
                    egui::Shape::Text(t) if is_value_outline(t) => Some(t),
                    _ => None,
                })
                .collect();
            assert_eq!(
                outlines.len(),
                8,
                "decorative glyph edge, not extra foreground values or an opaque badge"
            );
            assert!(
                outlines
                    .iter()
                    .all(|t| Arc::ptr_eq(&t.galley, &value.galley)
                        && (t.pos - value.pos).length() <= 1.01)
            );
            let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
            let black_vertices: Vec<_> = primitives
                .iter()
                .filter_map(|p| match &p.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .flat_map(|m| m.vertices.iter())
                .filter(|v| v.color == egui::Color32::BLACK)
                .collect();
            assert!(!black_vertices.is_empty());
            assert!(
                black_vertices.iter().all(|v| track.contains(v.pos)),
                "tessellated outline stays inside the track, including fractional values"
            );
            assert!(
                black_vertices
                    .iter()
                    .all(|v| (0.0..=1.0).contains(&v.uv.x) && (0.0..=1.0).contains(&v.uv.y)),
                "outline glyphs must sample normalized atlas coordinates in actual tessellated output"
            );
            assert!(
                value
                    .galley
                    .job
                    .sections
                    .iter()
                    .all(|s| s.format.color == egui::Color32::WHITE)
            );
            for background in [
                palette.outline,
                meter_color(
                    100.0 - used,
                    Status::Live,
                    0,
                    theme == ThemePreference::Dark,
                ),
            ] {
                assert!(
                    contrast_ratio(egui::Color32::WHITE, background)
                        .max(contrast_ratio(egui::Color32::BLACK, background))
                        >= 4.5,
                    "white ink or its black edge remains legible on both regions"
                );
            }
            if (49.0..=51.0).contains(&used) {
                let edge = track.left() + track.width() * used / 100.0;
                assert!(
                    (value.pos.x..=value.pos.x + value.galley.size().x).contains(&edge),
                    "fill boundary actually crosses the number in this fixture"
                );
            }
            let nodes = accessibility_nodes(&output);
            let gauges: Vec<_> = nodes
                .iter()
                .filter(|(_, n)| accessibility_text(n).starts_with("Weekly:"))
                .collect();
            assert_eq!(gauges.len(), 1);
            assert_eq!(gauges[0].1.numeric_value(), Some(f64::from(used)));
            if used == 100.0 {
                assert_eq!(text(&output).lines().filter(|s| *s == "100%").count(), 1);
                assert!(text(&output).contains("Allowance exhausted"));
            }
        }
    }
}
#[test]
fn frozen_countdown_shares_title_row_and_missing_reset_reserves_nothing() {
    let _clock = frozen_quota_clock();
    for width in [160.0, 320.0] {
        for missing in [false, true] {
            let ctx = Context::default();
            configure_visuals(&ctx);
            ctx.enable_accesskit();
            let mut meter = card(25.0).meters.remove(0);
            meter.label = if width < 200.0 {
                "ResearchModelIdentifierWithoutWordBreaks custom rolling allowance".into()
            } else {
                "Weekly".into()
            };
            meter.resets_at = (!missing)
                .then(|| quota_now() + time::Duration::days(6) + time::Duration::hours(23));
            let height = std::cell::Cell::new(0.0);
            let output = ctx.run(accessibility_input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let layout = quota_row_layout(ui, &meter, Status::Live, width);
                    height.set(layout.height(layout.header_height));
                    if missing {
                        assert!(layout.countdown.job.text.is_empty());
                        assert_eq!(
                            height.get(),
                            layout.label.size().y + 4.0 + QUOTA_TRACK_HEIGHT
                        );
                    }
                    draw_quota_meter(
                        ui,
                        &meter,
                        Status::Live,
                        "https://example.invalid/usage",
                        Id::new("countdown-meter"),
                        egui::Rect::from_min_size(
                            egui::pos2(20.0, 20.0),
                            egui::vec2(width, height.get()),
                        ),
                        0,
                        QuotaDisplayMode::Remaining,
                    );
                });
            });
            let visible = text(&output);
            assert!(!visible.contains("Resets in") && !visible.contains("Reset unavailable"));
            let all = shapes(&output);
            let title = all
                .iter()
                .find_map(|s| match s {
                    egui::Shape::Text(t) if t.galley.job.text == meter.label => Some(t),
                    _ => None,
                })
                .unwrap();
            let actual: String = title
                .galley
                .rows
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .map(|g| g.chr)
                .filter(|c| !c.is_whitespace())
                .collect();
            assert_eq!(
                actual,
                meter
                    .label
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
            );
            if !missing {
                let countdown = all
                    .iter()
                    .find_map(|s| match s {
                        egui::Shape::Text(t) if t.galley.job.text == "6d 23h" => Some(t),
                        _ => None,
                    })
                    .expect("duration only at frozen clock");
                assert!(
                    (countdown.pos.x + countdown.galley.size().x - (20.0 + width)).abs() < 0.01
                );
                assert!(countdown.pos.x > title.pos.x);
                if width == 320.0 {
                    assert!((countdown.pos.y - title.pos.y).abs() < 2.0);
                    assert_eq!(height.get(), 40.0);
                }
            }
            let nodes = accessibility_nodes(&output);
            let gauge = nodes
                .iter()
                .find(|(_, n)| accessibility_text(n).starts_with(&meter.label))
                .unwrap()
                .1;
            assert!(accessibility_text(gauge).contains(if missing {
                "Reset unavailable"
            } else {
                "Resets in 6d 23h"
            }));
        }
    }
}
#[test]
fn presentation_board_keep_status_freshness_attribution_and_modes() {
    let _clock = frozen_quota_clock();
    {
        for mode in [QuotaDisplayMode::Used, QuotaDisplayMode::Remaining] {
            let ctx = Context::default();
            configure_visuals(&ctx);
            ctx.enable_accesskit();
            let providers = presentation_cards();
            let output = ctx.run(accessibility_input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_board(
                        ui,
                        "Token Ashtray",
                        &providers,
                        &BoardPreferences {
                            quota_display_mode: mode,
                            ..Default::default()
                        },
                        false,
                        &mut false,
                        &mut false,
                    );
                });
            });
            let visible = text(&output);
            assert!(
                visible.contains("6d 23h")
                    && visible.contains("Allowance exhausted")
                    && visible.contains("Last updated 7m ago")
                    && visible.contains("Stale cached quota")
                    && visible.contains("Shared quota used")
            );
            assert!(
                !visible.contains("Resets in")
                    && !visible.contains("Quota Used")
                    && !visible.contains("Quota Remaining")
            );
            assert_eq!(
                visible.lines().filter(|s| *s == "Used").count(),
                2,
                "only the two shared-attribution products retain a standalone Used label"
            );
            assert!(!visible.lines().any(|s| s == "Remaining"));
            if mode == QuotaDisplayMode::Used {
                assert_eq!(
                    visible.lines().filter(|s| *s == "100%").count(),
                    1,
                    "fully used Claude has one value, with no duplicate endpoint reference"
                );
            }
            let nodes = accessibility_nodes(&output);
            let gauge = nodes
                .iter()
                .find(|(_, n)| accessibility_text(n).starts_with("Gemini Models · Weekly:"))
                .unwrap()
                .1;
            assert!(
                accessibility_text(gauge).contains(if mode == QuotaDisplayMode::Used {
                    "25% Used"
                } else {
                    "75% Remaining"
                })
            );
            assert!(accessibility_text(gauge).contains("Resets in 6d 23h"));
            assert!(nodes.iter().any(|(_, n)| {
                accessibility_text(n).contains("Foreground requests: 12% of the shared quota used")
            }));
        }
    }
}

#[test]
fn grok_credit_response_keeps_product_attribution_used_in_both_modes() {
    let adapter = token_ashtray::providers::all()
        .into_iter()
        .find(|provider| provider.descriptor().id == "grok")
        .unwrap();
    let payload =
        serde_json::from_str(include_str!("../../tests/fixtures/grok-products.json")).unwrap();
    let usage = adapter.parse(&payload).unwrap();
    let mut provider = adapter.descriptor().empty();
    provider.meters = usage.meters;
    provider.usage_breakdown = usage.usage_breakdown;
    provider.plan = usage.plan;
    provider.status = Status::Live;
    assert_eq!(provider.meters.len(), 1);
    for mode in [QuotaDisplayMode::Used, QuotaDisplayMode::Remaining] {
        for width in [1000.0, 360.0] {
            for theme in [ThemePreference::Light, ThemePreference::Dark] {
                let ctx = Context::default();
                configure_visuals(&ctx);
                ctx.enable_accesskit();
                theme::Appearance::default().apply(&ctx, theme);
                let mut input = accessibility_input();
                input.screen_rect = Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 950.0),
                ));
                let preferences = BoardPreferences {
                    quota_display_mode: mode,
                    ..Default::default()
                };
                let output = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_board(
                            ui,
                            "Token Ashtray",
                            &[provider.clone()],
                            &preferences,
                            false,
                            &mut false,
                            &mut false,
                        );
                    });
                });
                let nodes = accessibility_nodes(&output);
                let gauges: Vec<_> = nodes
                    .iter()
                    .filter(|(_, n)| accessibility_text(n).starts_with("Weekly shared credits:"))
                    .collect();
                assert_eq!(gauges.len(), 1);
                assert!(accessibility_text(gauges[0].1).contains(
                    if mode == QuotaDisplayMode::Used {
                        "72% Used"
                    } else {
                        "28% Remaining"
                    }
                ));
                for (name, value) in [("Grok Build", 70), ("Grok Chat", 1), ("Grok Imagine", 1)] {
                    assert!(nodes.iter().any(|(_, n)| accessibility_text(n) == format!(
                        "{name}: {value}% of the shared quota used. This is usage attribution, not a separate allowance."
                    )));
                }
                let visible = text(&output);
                assert!(visible.contains("Shared quota used"));
                assert!(visible.contains("Grok Build") && visible.contains("Grok Chat"));
                assert_eq!(visible.lines().filter(|line| *line == "Used").count(), 3);
                let bars: Vec<_> = shapes(&output)
                    .into_iter()
                    .filter_map(|shape| match shape {
                        egui::Shape::Rect(rect) if rect.rect.height() == 3.0 => Some(rect),
                        _ => None,
                    })
                    .collect();
                assert_eq!(bars.len(), 6);
                for (pair, fraction) in bars.chunks_exact(2).zip([0.70, 0.01, 0.01]) {
                    assert!((pair[1].rect.width() / pair[0].rect.width() - fraction).abs() < 0.001);
                }
            }
        }
    }
}
#[test]
fn meter_value_and_label_round_ties_on_the_used_share_in_both_modes() {
    for (mode, value, expected) in [
        (QuotaDisplayMode::Used, "51%", "51% Used"),
        (QuotaDisplayMode::Remaining, "49%", "49% Remaining"),
    ] {
        let ctx = Context::default();
        configure_visuals(&ctx);
        ctx.enable_accesskit();
        let meter = card(50.5).meters.remove(0);
        let output = ctx.run(accessibility_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let layout = quota_row_layout(ui, &meter, Status::Live, 320.0);
                let rect = egui::Rect::from_min_size(
                    egui::pos2(20.0, 20.0),
                    egui::vec2(320.0, layout.height(layout.header_height)),
                );
                draw_quota_meter(
                    ui,
                    &meter,
                    Status::Live,
                    "",
                    Id::new("rounding-meter"),
                    rect,
                    0,
                    mode,
                );
            });
        });
        let painted: Vec<_> = shapes(&output)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) if !is_value_outline(t) && t.galley.job.text.ends_with('%') => {
                    Some(t.galley.job.text.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(painted, vec![value.to_owned()], "{mode:?}");
        let nodes = accessibility_nodes(&output);
        let gauge = nodes
            .iter()
            .find(|(_, n)| accessibility_text(n).starts_with("Weekly:"))
            .unwrap()
            .1;
        assert!(
            accessibility_text(gauge).contains(expected),
            "{mode:?}: {}",
            accessibility_text(gauge)
        );
    }
}
#[test]
fn standalone_tray_takes_the_reset_from_the_tightest_meter() {
    let _clock = frozen_quota_clock();
    let mut provider = card(10.0);
    provider.meters[0].resets_at = Some(quota_now() + time::Duration::days(6));
    provider.meters.push(Meter {
        label: "Daily".into(),
        used_percent: 80.0,
        resets_at: Some(quota_now() + time::Duration::hours(2)),
    });
    let snapshot = local_tray_snapshot(&[provider], &BoardPreferences::default());
    assert_eq!(snapshot.providers[0].used, Some(80.0));
    assert_eq!(snapshot.providers[0].reset.as_deref(), Some("2h 0m"));
}
