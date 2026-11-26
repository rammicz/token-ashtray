// Shared by headless assertions and the actual native IPC driver.
fn responsive_cards() -> Vec<ProviderCard> {
    serde_json::from_str(include_str!("../../tests/fixtures/responsive-layout.json")).unwrap()
}
fn responsive_render(
    ctx: &Context,
    width: f32,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut input = accessibility_input();
    input.screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        egui::vec2(width, 1400.0),
    ));
    input.events = events;
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    draw_board(
                        ui,
                        "Token Ashtray",
                        &responsive_cards(),
                        &BoardPreferences::default(),
                        false,
                        &mut false,
                        &mut false,);
                });
        });
        let geometry = responsive_capture_geometry(ctx,);
        ctx.data_mut(|data| data.insert_temp(Id::new("responsive-layout-receipt"), geometry));
    })
}
fn responsive_geometry(ctx: &Context) -> serde_json::Value {
    ctx.data(|data| data.get_temp::<serde_json::Value>(Id::new("responsive-layout-receipt")))
        .unwrap()
}
fn responsive_capture_geometry(ctx: &Context) -> serde_json::Value {
    let viewport = accessibility_input().viewport_id;
    let slots = ctx
        .data(|d| d.get_temp::<Vec<DragSlot>>(Id::new(("card-drag-layout", viewport))))
        .expect("actual card slots");
    let rect_json = |r: egui::Rect| serde_json::json!({"left":r.left(),"top":r.top(),"right":r.right(),"bottom":r.bottom(),"width":r.width(),"height":r.height()});
    serde_json::json!({"cards":responsive_cards().iter().map(|card| {
        let outer=slots.iter().find(|s|s.id==card.id).unwrap().rect;
        serde_json::json!({"id":card.id,"rect":rect_json(outer),"meters":card.meters.iter().enumerate().map(|(index,meter)| {
            let rect=ctx.read_response(Id::new(("meter-gauge",&card.id,index))).expect("real meter response").rect;
            serde_json::json!({"label":meter.label,"rect":rect_json(rect)})
        }).collect::<Vec<_>>()})
    }).collect::<Vec<_>>()})
}
fn fixture_rect(value: &serde_json::Value) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            value["left"].as_f64().unwrap() as f32,
            value["top"].as_f64().unwrap() as f32,
        ),
        egui::pos2(
            value["right"].as_f64().unwrap() as f32,
            value["bottom"].as_f64().unwrap() as f32,
        ),
    )
}
#[test]
fn responsive_reference_board_has_two_equal_cards_and_weekly_meter_cells() {

        let ctx = Context::default();
        configure_visuals(&ctx);
        let _ = responsive_render(&ctx, 1000.0, vec![],);
        let _ = responsive_render(&ctx, 1000.0, vec![],);
        let geometry = responsive_geometry(&ctx,);
        let cards = geometry["cards"].as_array().unwrap();
        let outer: Vec<_> = cards
            .iter()
            .map(|card| fixture_rect(&card["rect"]))
            .collect();
        assert!((outer[0].width() - outer[1].width()).abs() < 0.1);
        assert_eq!(outer[0].top(), outer[1].top());
        assert!(
            outer[2].top() >= outer[0].bottom().max(outer[1].bottom()),
            "1000-point board needs two cards in its first row: {geometry}"
        );
        let meters = cards[0]["meters"].as_array().unwrap();
        let a = fixture_rect(&meters[0]["rect"]);
        let b = fixture_rect(&meters[1]["rect"]);
        assert_eq!(
            a.top(),
            b.top(),
            "Antigravity weekly windows must be side by side: {geometry}"
        );
        assert!((a.width() - b.width()).abs() < 0.1 && a.right() < b.left());
        let single = fixture_rect(&cards[1]["meters"][0]["rect"]);
        assert!(
            (single.width() - (outer[1].width() - 24.0)).abs() < 0.1,
            "a single meter fills its card: {geometry}"
        );

}
#[test]
#[ignore = "writes offline geometry receipts for the native fixture"]
fn write_responsive_geometry_receipts() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("TOKEN_ASHTRAY_LAYOUT_RECEIPTS").expect("explicit evidence directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    {
        for width in [360.0, 1000.0] {
            let ctx = Context::default();
            configure_visuals(&ctx);
            let _ = responsive_render(&ctx, width, vec![],);
            let _ = responsive_render(&ctx, width, vec![],);
            let mut geometry = responsive_geometry(&ctx,);
            geometry["viewport_width"] = serde_json::json!(width);
            let cards = geometry["cards"].as_array().unwrap();
            let top = cards[0]["rect"]["top"].as_f64().unwrap();
            let bottom = cards
                .iter()
                .filter(|c| (c["rect"]["top"].as_f64().unwrap() - top).abs() < 0.1)
                .map(|c| c["rect"]["bottom"].as_f64().unwrap())
                .fold(top, f64::max);
            geometry["first_row_height"] = serde_json::json!(bottom - top);
            let name = format!("{}-{width}.json", { "board" });
            std::fs::write(
                directory.join(name),
                serde_json::to_vec_pretty(&geometry).unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
fn responsive_cells_keep_order_bounds_and_every_title_detail_glyph() {

        for width in [360.0, 1000.0] {
            let ctx = Context::default();
            configure_visuals(&ctx);
            ctx.enable_accesskit();
            let _ = responsive_render(&ctx, width, vec![],);
            let output = responsive_render(&ctx, width, vec![],);
            let geometry = responsive_geometry(&ctx,);
            let nodes = accessibility_nodes(&output);
            for (card, actual) in responsive_cards()
                .iter()
                .zip(geometry["cards"].as_array().unwrap())
            {
                let outer = fixture_rect(&actual["rect"]);
                assert!(
                    outer.left() >= 0.0 && outer.right() <= width,
                    "no horizontal overflow: {geometry}"
                );
                let rects: Vec<_> = actual["meters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| fixture_rect(&m["rect"]))
                    .collect();
                for (index, (meter, rect)) in card.meters.iter().zip(&rects).enumerate() {
                    assert!(outer.contains_rect(*rect), "meter remains in its card");
                    for previous in &rects[..index] {
                        assert!(!previous.intersects(*rect), "meter cells must not overlap");
                    }
                    if index > 0 {
                        let previous = rects[index - 1];
                        assert!(
                            rect.top() >= previous.bottom()
                                || (rect.top() == previous.top() && rect.left() > previous.right()),
                            "row-major meter order"
                        );
                    }
                    let mut visible = vec![meter.label.clone()];
                    if let Some(reset) = meter.resets_at {
                        visible.push(reset_label(reset));
                    }
                    let cue = quota_status(meter, card.status);
                    if !cue.is_empty() {
                        visible.push(cue.to_owned());
                    }
                    for expected in visible {
                        let (painted, clip) = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text)
                                    if text.galley.job.text == expected
                                        && rect.contains(text.pos) =>
                                {
                                    Some((text, shape.clip_rect))
                                }
                                _ => None,
                            })
                            .expect("complete title/detail is actually painted in its cell");
                        let glyphs: String = painted
                            .galley
                            .rows
                            .iter()
                            .flat_map(|row| row.glyphs.iter())
                            .map(|g| g.chr)
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        assert_eq!(
                            glyphs,
                            expected
                                .chars()
                                .filter(|c| !c.is_whitespace())
                                .collect::<String>(),
                            "no ellipsis or missing text"
                        );
                        for row in &painted.galley.rows {
                            for vertex in
                                &row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()]
                            {
                                let point = painted.pos + vertex.pos.to_vec2();
                                assert!(
                                    rect.expand(0.1).contains(point) && clip.contains(point),
                                    "glyph {point:?} must fit cell {rect:?} and visible clip {clip:?}"
                                );
                            }
                        }
                    }
                    let (_, node) = nodes
                        .iter()
                        .find(|(_, n)| {
                            accessibility_text(n).starts_with(&format!("{}:", meter.label))
                        })
                        .expect("complete meter accessibility identity");
                    assert!(accessibility_text(node).contains(&quota_detail(meter, card.status)));
                    assert!(node.supports_action(egui::accesskit::Action::Click));
                }
                if width == 360.0 && rects.len() > 1 {
                    assert!(
                        rects[1].top() >= rects[0].bottom(),
                        "narrow cards wrap their meters"
                    );
                }
            }
        }

}

#[test]
fn responsive_meter_focus_and_usage_actions_survive_wrapping() {

        let ctx = Context::default();
        configure_visuals(&ctx);
        ctx.enable_accesskit();
        let _ = responsive_render(&ctx, 1000.0, vec![],);
        let _ = responsive_render(&ctx, 1000.0, vec![],);
        for card in responsive_cards() {
            for index in 0..card.meters.len() {
                let id = Id::new(("meter-gauge", &card.id, index));
                ctx.memory_mut(|memory| memory.request_focus(id));
                let _ = responsive_render(&ctx, 360.0, vec![],);
                let mut input = egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                };
                let output = responsive_render(&ctx, 360.0, vec![input.clone()],);
                let opened: Vec<_> = output
                    .platform_output
                    .commands
                    .iter()
                    .filter_map(|c| match c {
                        egui::OutputCommand::OpenUrl(open) => Some(&open.url),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    opened,
                    vec![&card.link],
                    "focus identity and keyboard usage after resize"
                );
                if let egui::Event::Key { pressed, .. } = &mut input {
                    *pressed = false;
                }
                let _ = responsive_render(&ctx, 360.0, vec![input],);
                let wide = responsive_render(&ctx, 1000.0, vec![],);
                let (node, _) = accessibility_nodes(&wide)
                    .into_iter()
                    .find(|(_, n)| {
                        accessibility_text(n).starts_with(&format!("{}:", card.meters[index].label))
                    })
                    .unwrap();
                let output = responsive_render(&ctx, 1000.0, vec![accessibility_click(node)],);
                assert!(
                    output.platform_output.commands.iter().any(
                        |c| matches!(c,egui::OutputCommand::OpenUrl(open) if open.url==card.link)
                    ),
                    "usage activation after expanding back to columns"
                );
            }
        }

}
