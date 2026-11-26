use super::*;

fn render(
    ctx: &Context,
    width: f32,
    scale: f32,
    compact: bool,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(width, 600.0),
        )),
        events,
        ..Default::default()
    };
    let viewport = input.viewports.get_mut(&ViewportId::ROOT).unwrap();
    viewport.title = Some("Token Ashtray".into());
    viewport.native_pixels_per_point = Some(scale);
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = draw_board(
                ui,
                "Token Ashtray",
                &[],
                &BoardPreferences::default(),
                true,
                &mut false,
                &mut compact.clone(),
            );
        });
    })
}

#[test]
fn toolbar_identity_and_actions_fit_native_window_budget() {
    for width in [360.0, 440.0] {
        for scale in [1.0, 2.0] {
            for dark in [false, true] {
                for compact in [false, true] {
                    let ctx = Context::default();
                    configure_visuals(&ctx);
                    ctx.enable_accesskit();
                    ctx.style_mut(|s| s.interaction.tooltip_delay = 0.0);
                    ctx.set_visuals(if dark {
                        egui::Visuals::dark()
                    } else {
                        egui::Visuals::light()
                    });
                    let _ = render(&ctx, width, scale, compact, vec![]);
                    let out = render(&ctx, width, scale, compact, vec![]);
                    assert_eq!(ctx.pixels_per_point(), scale);
                    assert!(out.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.rect.top() == 8.0 && rect.rect.height() == 40.0)), "rendered toolbar frame must occupy 40 logical points");
                    let nodes = &out.platform_output.accesskit_update.as_ref().unwrap().nodes;
                    assert!(
                        nodes.iter().all(|(_, node)| node.label() != Some("Panel")),
                        "toolbar must expose only main-board actions"
                    );
                    assert_eq!(
                        out.viewport_output.len(),
                        1,
                        "toolbar renders one quota window"
                    );
                    let title = String::from("Token Ashtray");
                    let identity = nodes.iter().find(|(_, n)| {
                        n.label() == Some(title.as_str()) || n.value() == Some(title.as_str())
                    });
                    assert_eq!(
                        identity.is_some(),
                        compact,
                        "identity must follow decorations: compact={compact}"
                    );
                    if compact {
                        let painted = out
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if text.galley.job.text == title => {
                                    Some(text)
                                }
                                _ => None,
                            })
                            .expect("painted Compact identity");
                        for section in &painted.galley.job.sections {
                            assert_eq!(section.format.font_id.size, 13.0);
                            assert_eq!(
                                section.format.font_id.family,
                                egui::FontFamily::Name("Compact identity".into()),
                                "Compact must paint with the embedded semibold face"
                            );
                            assert_eq!(section.format.color, palette(&ctx).text);
                        }
                    }

                    let labels = vec![
                        "Watch the author’s latest video",
                        "Refresh now",
                        "Window settings",
                    ];
                    let controls: Vec<_> = labels
                        .iter()
                        .map(|label| {
                            nodes
                                .iter()
                                .find(|(_, n)| n.label() == Some(label))
                                .unwrap()
                                .1
                                .bounds()
                                .unwrap()
                        })
                        .collect();
                    for r in &controls {
                        assert!(r.width() >= 32.0 && r.height() >= 32.0, "hit target {r:?}");
                        assert!(r.x0 >= 0.0 && r.x1 <= f64::from(width), "clipped {r:?}");
                        assert!(r.y0 >= 12.0 && r.y1 <= 44.0, "40pt toolbar budget: {r:?}");
                    }
                    for pair in controls.windows(2) {
                        assert!(pair[0].x1 <= pair[1].x0);
                    }
                    if let Some((_, identity)) = identity {
                        let r = identity.bounds().unwrap();
                        assert!(r.x1 <= controls[0].x0, "identity overlaps actions: {r:?}");
                        assert!(r.y0 >= 12.0 && r.y1 <= 44.0);
                        let point =
                            egui::pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32);
                        let _ = render(
                            &ctx,
                            width,
                            scale,
                            compact,
                            vec![egui::Event::PointerMoved(point)],
                        );
                        let _ = render(&ctx, width, scale, compact, vec![]);
                        let _hover = render(&ctx, width, scale, compact, vec![]);
                    }
                }
            }
        }
    }
}
