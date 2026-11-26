use super::*;

fn snapshot(viewport: ViewportId, scale: f32, zoom: f32) -> egui::RawInput {
    let mut input = egui::RawInput {
        viewport_id: viewport,
        ..Default::default()
    };
    // Each native scale is applied to the main board snapshot.
    input
        .viewports
        .get_mut(&ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(1.0);
    let pixels_per_point = scale * zoom;
    input.viewports.insert(
        viewport,
        egui::ViewportInfo {
            native_pixels_per_point: Some(scale),
            inner_rect: Some(egui::Rect::from_min_size(
                Pos2::new(-1904.0, 62.0) / pixels_per_point,
                egui::vec2(880.0, 1040.0) / pixels_per_point,
            )),
            outer_rect: Some(egui::Rect::from_min_size(
                Pos2::new(-1912.0, 0.0) / pixels_per_point,
                egui::vec2(896.0, 1118.0) / pixels_per_point,
            )),
            ..Default::default()
        },
    );
    input
}

fn work_area() -> Vec<platform::MonitorRect> {
    vec![platform::MonitorRect {
        x: -1920.0,
        y: 0.0,
        width: 1920.0,
        height: 984.0,
    }]
}

fn settled(window: &WindowPreferences, scale: f32) -> PositionRestore {
    let frame = Pos2::new(window.x.unwrap(), window.y.unwrap());
    PositionRestore::Settled {
        saved_anchor: frame,
        observed_anchor: frame + egui::vec2(8.0, 62.0) / scale,
        egui_frame_seen: true,
        unknown_first_frame: false,
        candidate: None,
    }
}

fn geometry_commands(commands: &[ViewportCommand]) -> Vec<&ViewportCommand> {
    commands
        .iter()
        .filter(|c| {
            matches!(
                c,
                ViewportCommand::InnerSize(_)
                    | ViewportCommand::OuterPosition(_)
                    | ViewportCommand::MinInnerSize(_)
            )
        })
        .collect()
}

/// Outer frame minus client area in `normal_snapshot`, in physical pixels.
const FRAME: egui::Vec2 = egui::vec2(16.0, 78.0);

fn normal_snapshot(
    viewport: ViewportId,
    scale: f32,
    zoom: f32,
    origin: Pos2,
    size: egui::Vec2,
) -> egui::RawInput {
    let mut input = snapshot(viewport, scale, zoom);
    let info = input.viewports.get_mut(&viewport).unwrap();
    info.maximized = Some(false);
    info.minimized = Some(false);
    info.outer_rect = Some(egui::Rect::from_min_size(origin, size) / (scale * zoom));
    info.inner_rect = Some(
        egui::Rect::from_min_size(origin + egui::vec2(8.0, 62.0), size - FRAME) / (scale * zoom),
    );
    input
}

#[test]
fn normal_moves_and_resizes_persist_after_maximize_minimize_and_restore() {
    let viewport = ViewportId::ROOT;
    for scale in [1.0, 1.5, 2.0] {
        for zoom in [1.0, 1.1] {
            let ctx = Context::default();
            ctx.set_zoom_factor(zoom);
            let mut window = WindowPreferences {
                x: Some(-1800.0 / scale),
                y: Some(100.0 / scale),
                position_scale: Some(scale),
                width: 600.0 / scale,
                height: 500.0 / scale,
                ..Default::default()
            };
            let original = serde_json::to_value(&window).unwrap();
            let mut restore = settled(&window, scale);
            let start = Instant::now();
            for (maximized, minimized) in [(true, false), (false, true), (false, false)] {
                let mut input = normal_snapshot(
                    viewport,
                    scale,
                    zoom,
                    Pos2::new(-1800.0, 100.0),
                    egui::vec2(616.0, 578.0),
                );
                let info = input.viewports.get_mut(&viewport).unwrap();
                info.maximized = Some(maximized);
                info.minimized = Some(minimized);
                if maximized || minimized {
                    let rect = egui::Rect::from_min_size(
                        if minimized {
                            Pos2::new(-32000.0, -32000.0)
                        } else {
                            Pos2::new(-1920.0, 0.0)
                        },
                        if minimized {
                            egui::vec2(1.0, 1.0)
                        } else {
                            egui::vec2(1920.0, 984.0)
                        },
                    ) / (scale * zoom);
                    info.inner_rect = Some(rect);
                    info.outer_rect = Some(rect);
                }
                let output = ctx.run(input, |ctx| {
                    assert!(!fit_live_viewport_in_work_areas(
                        ctx,
                        &mut window,
                        &mut restore,
                        "owned viewport",
                        work_area()
                    ));
                    restore.capture_viewport(ctx, &mut window, "owned viewport", start);
                });
                assert_eq!(serde_json::to_value(&window).unwrap(), original);
                assert!(geometry_commands(&output.viewport_output[&viewport].commands).is_empty());
                assert!(matches!(restore, PositionRestore::Settled { .. }));
                assert!(
                    !restore.needs_repaint(),
                    "native restore to normal bounds must stay settled"
                );
            }
            // A deliberately partly off-screen normal frame remains movable
            // and resizable after the native state transitions, including
            // after the debounced position has become the saved position.
            for elapsed in [1000, 1250, 1500, 1750] {
                let output = ctx.run(
                    normal_snapshot(
                        viewport,
                        scale,
                        zoom,
                        Pos2::new(-2100.0, 40.0),
                        egui::vec2(716.0, 678.0),
                    ),
                    |ctx| {
                        assert!(!fit_live_viewport_in_work_areas(
                            ctx,
                            &mut window,
                            &mut restore,
                            "owned viewport",
                            work_area()
                        ));
                        restore.capture_viewport(
                            ctx,
                            &mut window,
                            "owned viewport",
                            start + Duration::from_millis(elapsed),
                        );
                    },
                );
                assert!(geometry_commands(&output.viewport_output[&viewport].commands).is_empty());
            }
            for (actual, expected) in [
                (window.x.unwrap(), -2100.0 / scale),
                (window.y.unwrap(), 40.0 / scale),
                (window.width, 700.0 / scale),
                (window.height, 600.0 / scale),
            ] {
                assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
            }
            assert!(
                !restore.needs_repaint(),
                "saved partly off-screen move must remain settled"
            );
        }
    }
}

#[test]
fn inaccessible_normal_frames_recover_from_observed_bounds_at_current_dpi() {
    let viewport = ViewportId::ROOT;
    for scale in [1.0, 1.5, 2.0] {
        for zoom in [1.0, 1.1] {
            for origin in [
                Pos2::new(-6000.0, 1800.0),
                Pos2::new(-1800.0, -100.0),
                Pos2::new(-1800.0, -62.0),
            ] {
                let ctx = Context::default();
                ctx.set_zoom_factor(zoom);
                // Old saved bounds are reachable. Recovery must be based
                // on the current native frame after a display/DPI change.
                let mut window = WindowPreferences {
                    x: Some(-900.0),
                    y: Some(50.0),
                    ..Default::default()
                };
                let mut restore = settled(&window, scale);
                let output = ctx.run(
                    normal_snapshot(viewport, scale, zoom, origin, egui::vec2(2816.0, 1878.0)),
                    |ctx| {
                        assert!(fit_live_viewport_in_work_areas(
                            ctx,
                            &mut window,
                            &mut restore,
                            "owned viewport",
                            work_area()
                        ));
                    },
                );
                for (actual, expected) in [
                    (window.x.unwrap(), -1920.0 / scale),
                    (window.y.unwrap(), 0.0),
                    (window.width, 1904.0 / scale),
                    (window.height, 906.0 / scale),
                ] {
                    assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
                }
                let commands = &output.viewport_output[&viewport].commands;
                assert!(commands.contains(&ViewportCommand::InnerSize(
                    egui::vec2(window.width, window.height) / zoom
                )));
                assert!(commands.contains(&ViewportCommand::OuterPosition(
                    Pos2::new(window.x.unwrap(), window.y.unwrap()) / zoom
                )));
                assert!(commands.contains(&ViewportCommand::MinInnerSize(
                    egui::vec2(window.width.min(360.0), window.height.min(360.0)) / zoom
                )));
                assert!(matches!(restore, PositionRestore::Requested { .. }));
            }
        }
    }
}

#[test]
fn mixed_monitor_move_uses_observed_viewport_scale_and_accessible_top_strip() {
    let areas = vec![
        platform::MonitorRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 984.0,
        },
        platform::MonitorRect {
            x: -2560.0,
            y: -400.0,
            width: 2560.0,
            height: 1400.0,
        },
    ];
    for scale in [1.0, 1.5, 2.0] {
        for compact in [false, true] {
            let ctx = Context::default();
            let mut window = WindowPreferences {
                x: Some(100.0),
                y: Some(100.0),
                compact,
                ..Default::default()
            };
            let mut restore = settled(&window, scale);
            let output = ctx.run(
                normal_snapshot(
                    ViewportId::ROOT,
                    scale,
                    1.0,
                    Pos2::new(-2600.0, -390.0),
                    egui::vec2(716.0, 678.0),
                ),
                |ctx| {
                    assert!(!fit_live_viewport_in_work_areas(
                        ctx,
                        &mut window,
                        &mut restore,
                        "board",
                        areas.clone()
                    ));
                },
            );
            assert!(
                geometry_commands(&output.viewport_output[&ViewportId::ROOT].commands).is_empty()
            );
            assert!(matches!(restore, PositionRestore::Settled { .. }));
        }
    }
}

#[test]
fn settled_reachable_frames_leave_native_placement_and_resize_alone() {
    let viewport = ViewportId::ROOT;
    for scale in [1.0, 1.5, 2.0] {
        for zoom in [1.0, 1.1] {
            // Partly off either edge, partly below the taskbar, and native Snap.
            for (origin, size) in [
                (egui::pos2(-2100.0, 40.0), egui::vec2(600.0, 500.0)),
                (egui::pos2(-100.0, 40.0), egui::vec2(600.0, 500.0)),
                (egui::pos2(-1700.0, 950.0), egui::vec2(600.0, 500.0)),
                (egui::pos2(-1920.0, 0.0), egui::vec2(960.0, 984.0)),
                // Even a single physical pixel of the top strip is reachable.
                (egui::pos2(-2519.0, 40.0), egui::vec2(600.0, 500.0)),
                (egui::pos2(-1.0, 40.0), egui::vec2(600.0, 500.0)),
                (egui::pos2(-1700.0, -61.0), egui::vec2(600.0, 500.0)),
                (egui::pos2(-1700.0, 983.0), egui::vec2(600.0, 500.0)),
            ] {
                let ctx = Context::default();
                ctx.set_zoom_factor(zoom);
                let mut window = WindowPreferences {
                    x: Some(origin.x / scale),
                    y: Some(origin.y / scale),
                    position_scale: Some(scale),
                    width: 440.0,
                    height: 400.0,
                    ..Default::default()
                };
                let original = serde_json::to_value(&window).unwrap();
                let mut restore = settled(&window, scale);
                for _ in 0..4 {
                    let mut input = snapshot(viewport, scale, zoom);
                    let info = input.viewports.get_mut(&viewport).unwrap();
                    info.outer_rect =
                        Some(egui::Rect::from_min_size(origin, size) / (scale * zoom));
                    info.inner_rect = Some(
                        egui::Rect::from_min_size(
                            origin + egui::vec2(8.0, 62.0),
                            size - egui::vec2(16.0, 78.0),
                        ) / (scale * zoom),
                    );
                    let output = ctx.run(input, |ctx| {
                        assert!(
                            !fit_live_viewport_in_work_areas(
                                ctx,
                                &mut window,
                                &mut restore,
                                "owned viewport",
                                work_area()
                            ),
                            "settled reachable frames must not reopen restore"
                        );
                    });
                    assert!(
                        geometry_commands(&output.viewport_output[&viewport].commands).is_empty(),
                        "reachable frames must emit no geometry commands"
                    );
                    assert_eq!(serde_json::to_value(&window).unwrap(), original);
                    assert!(matches!(restore, PositionRestore::Settled { .. }));
                }
            }
        }
    }
}

#[test]
fn native_maximized_and_minimized_frames_emit_no_fit_commands() {
    let viewport = ViewportId::ROOT;
    for scale in [1.0, 1.5, 2.0] {
        for (maximized, minimized) in [(true, false), (false, true)] {
            for restoring in [false, true] {
                let ctx = Context::default();
                let mut window = WindowPreferences {
                    x: Some(-2100.0),
                    y: Some(900.0),
                    position_scale: Some(scale),
                    width: 1400.0,
                    height: 900.0,
                    ..Default::default()
                };
                let original = serde_json::to_value(&window).unwrap();
                let mut restore = if restoring {
                    PositionRestore::default()
                } else {
                    settled(&window, scale)
                };
                let mut input = snapshot(viewport, scale, 1.0);
                let info = input.viewports.get_mut(&viewport).unwrap();
                info.maximized = Some(maximized);
                info.minimized = Some(minimized);
                let output = ctx.run(input, |ctx| {
                    assert!(!fit_live_viewport_in_work_areas(
                        ctx,
                        &mut window,
                        &mut restore,
                        "owned viewport",
                        work_area()
                    ));
                });
                assert!(geometry_commands(&output.viewport_output[&viewport].commands).is_empty());
                assert_eq!(
                    serde_json::to_value(&window).unwrap(),
                    original,
                    "native state cannot replace normal bounds"
                );
            }
        }
    }
}

#[test]
fn windows_live_snapshot_fits_default_and_saved_outer_frames_at_board_dpi() {
    let viewport = ViewportId::ROOT;
    for scale in [1.0, 1.5, 2.0] {
        for zoom in [1.0, 1.1] {
            for saved in [false, true] {
                let ctx = Context::default();
                ctx.set_zoom_factor(zoom);
                let mut window = WindowPreferences {
                    width: if saved { 1400.0 } else { 440.0 },
                    height: if saved { 900.0 } else { 520.0 },
                    x: saved.then_some(-4000.0),
                    y: saved.then_some(900.0),
                    ..Default::default()
                };
                let mut restore = PositionRestore::default();
                let output = ctx.run(snapshot(viewport, scale, zoom), |ctx| {
                    assert!(fit_live_viewport_in_work_areas(
                        ctx,
                        &mut window,
                        &mut restore,
                        "owned viewport",
                        work_area()
                    ));
                });
                let x = window.x.unwrap() * scale;
                let y = window.y.unwrap() * scale;
                assert!(x >= -1920.01 && y >= -0.01);
                assert!(
                    x + window.width * scale + 16.0 <= 0.01,
                    "outer width must include measured frame: {window:?}, scale={scale}, zoom={zoom}"
                );
                assert!(
                    y + window.height * scale + 78.0 <= 984.01,
                    "outer height must fit rcWork: {window:?}, scale={scale}, zoom={zoom}"
                );
                let commands = &output.viewport_output[&viewport].commands;
                assert!(commands.contains(&ViewportCommand::InnerSize(
                    egui::vec2(window.width, window.height) / zoom
                )));
                assert!(commands.contains(&ViewportCommand::OuterPosition(
                    Pos2::new(window.x.unwrap(), window.y.unwrap()) / zoom
                )));
                assert!(matches!(restore, PositionRestore::Requested { .. }));
            }
        }
    }
}

#[test]
fn missing_work_area_preserves_saved_geometry_without_display_bounds_fallback() {
    let ctx = Context::default();
    let mut window = WindowPreferences {
        x: Some(-4000.0),
        y: Some(900.0),
        position_scale: Some(2.0),
        width: 1400.0,
        height: 900.0,
        ..Default::default()
    };
    let original = serde_json::to_value(&window).unwrap();
    let mut restore = PositionRestore::default();
    let output = ctx.run(snapshot(ViewportId::ROOT, 2.0, 1.0), |ctx| {
        assert!(!fit_live_viewport_in_work_areas(
            ctx,
            &mut window,
            &mut restore,
            "board",
            vec![]
        ));
    });
    assert_eq!(serde_json::to_value(&window).unwrap(), original);
    assert!(
        !output.viewport_output[&ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(
                command,
                ViewportCommand::InnerSize(_) | ViewportCommand::OuterPosition(_)
            ))
    );
}

#[test]
fn late_frame_measurement_reconciles_provisional_fit_and_reopen_uses_current_work_area() {
    let ctx = Context::default();
    let mut window = WindowPreferences {
        pinned: true,
        ..Default::default()
    };
    let mut restore = PositionRestore::default();
    let mut input = snapshot(ViewportId::ROOT, 2.0, 1.0);
    input
        .viewports
        .get_mut(&ViewportId::ROOT)
        .unwrap()
        .outer_rect = None;
    let _ = ctx.run(input, |ctx| {
        fit_live_viewport_in_work_areas(ctx, &mut window, &mut restore, "board", work_area());
    });
    assert_eq!(
        window.height, 492.0,
        "unmapped frame is only a provisional fit"
    );
    let _ = ctx.run(snapshot(ViewportId::ROOT, 2.0, 1.0), |ctx| {
        assert!(fit_live_viewport_in_work_areas(
            ctx,
            &mut window,
            &mut restore,
            "board",
            work_area()
        ));
    });
    assert_eq!(window.height, 453.0, "mapped outer frame must now fit");

    // Restart consumes persisted fitted values through the same live path.
    let saved = serde_json::to_value(&window).unwrap();
    window = serde_json::from_value(saved.clone()).unwrap();
    restore = PositionRestore::default();
    let _ = ctx.run(snapshot(ViewportId::ROOT, 2.0, 1.0), |ctx| {
        fit_live_viewport_in_work_areas(ctx, &mut window, &mut restore, "board", work_area());
    });
    assert_eq!(serde_json::to_value(&window).unwrap(), saved);

    // Hidden-board show refreshes work areas before calling the live fitter.
    restore.prepare_show(Instant::now());
    let mut smaller = work_area();
    smaller[0].height = 784.0;
    let _ = ctx.run(snapshot(ViewportId::ROOT, 2.0, 1.0), |ctx| {
        assert!(fit_live_viewport_in_work_areas(
            ctx,
            &mut window,
            &mut restore,
            "board",
            smaller.clone()
        ));
    });
    assert_eq!(window.height, 353.0);
    assert_eq!(window.y, Some(0.0));
    assert!(window.pinned);
}

#[test]
fn compact_viewport_ignores_previous_decorated_frame_until_decorations_return() {
    let ctx = Context::default();
    let mut window = WindowPreferences {
        compact: true,
        ..Default::default()
    };
    let mut restore = PositionRestore::default();
    let _ = ctx.run(snapshot(ViewportId::ROOT, 2.0, 1.0), |ctx| {
        fit_live_viewport_in_work_areas(ctx, &mut window, &mut restore, "board", work_area());
    });
    assert_eq!(window.height, 492.0);
    window.compact = false;
    let _ = ctx.run(snapshot(ViewportId::ROOT, 2.0, 1.0), |ctx| {
        fit_live_viewport_in_work_areas(ctx, &mut window, &mut restore, "board", work_area());
    });
    assert_eq!(window.height, 453.0);
}

#[test]
fn native_first_show_applies_live_board_geometry_before_visibility() {
    use crate::windows_first_show::FirstShow;
    for scale in [1.0, 2.0] {
        let viewport = ViewportId::ROOT;
        for compact in [false, true] {
            let zoom = 1.1;
            let ctx = Context::default();
            ctx.set_zoom_factor(zoom);
            let mut window = WindowPreferences {
                width: 1600.0,
                height: 1080.0,
                compact,
                x: Some(-1800.0),
                y: Some(78.0),
                ..Default::default()
            };
            let desired_builder = board_viewport_builder(&window, false, None, zoom);
            let mut gate = FirstShow::new(desired_builder.visible);
            assert_eq!(gate.creation_builder(&desired_builder).visible, Some(false));
            let mut restore = PositionRestore::default();
            let output = ctx.run(snapshot(viewport, scale, zoom), |ctx| {
                fit_live_viewport_in_work_areas(
                    ctx,
                    &mut window,
                    &mut restore,
                    "owned viewport",
                    work_area(),
                );
            });
            let mut commands = vec![ViewportCommand::Visible(true), ViewportCommand::Focus];
            commands.extend(output.viewport_output[&viewport].commands.clone());
            let mut actual_outer =
                egui::Rect::from_min_size(egui::pos2(-1800.0, 78.0), egui::vec2(1616.0, 1119.0));
            let frame = if compact {
                egui::Vec2::ZERO
            } else {
                egui::vec2(16.0, 78.0)
            };
            let mut shows = 0;
            gate.painted(); // The HWND's own callback/render/swap has completed.
            gate.apply_commands(commands, |commands| {
                for command in commands {
                    match command {
                        ViewportCommand::InnerSize(size) => {
                            actual_outer = egui::Rect::from_min_size(
                                actual_outer.min,
                                size * scale * zoom + frame,
                            )
                        }
                        ViewportCommand::OuterPosition(position) => {
                            actual_outer = egui::Rect::from_min_size(
                                position * scale * zoom,
                                actual_outer.size(),
                            )
                        }
                        ViewportCommand::Visible(true) => {
                            shows += 1;
                            assert!(
                                actual_outer.left() >= -1920.01
                                    && actual_outer.top() >= -0.01
                                    && actual_outer.right() <= 0.01
                                    && actual_outer.bottom() <= 984.01,
                                "native exposure precedes fitting: {actual_outer:?}"
                            );
                        }
                        _ => {}
                    }
                }
            });
            assert_eq!(shows, 1);
        }
    }
}

/// Primary D1 at 100% plus a portrait D2 whose scale changes while the board
/// is hidden. Rectangles are physical virtual-desktop pixels.
fn mixed_dpi_work_areas() -> Vec<platform::MonitorRect> {
    vec![
        platform::MonitorRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1040.0,
        },
        platform::MonitorRect {
            x: 3440.0,
            y: 0.0,
            width: 1200.0,
            height: 1920.0,
        },
    ]
}

/// Reopen a hidden board as a new HWND at `scale` and return the physical
/// outer frame its first fit requests.
fn reopen_at_scale(window: &mut WindowPreferences, scale: f32, origin: Pos2) -> egui::Rect {
    // Each reopen reads the preferences the previous session persisted.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut config = ConfigStore::load_from(path.clone());
    config.preferences_mut().window = window.clone();
    config.save().unwrap();
    *window = ConfigStore::load_from(path).preferences().window.clone();
    let ctx = Context::default();
    let mut restore = PositionRestore::Pending;
    let output = ctx.run(
        normal_snapshot(
            ViewportId::ROOT,
            scale,
            1.0,
            origin,
            egui::vec2(456.0, 598.0),
        ),
        |ctx| {
            fit_live_viewport_in_work_areas(
                ctx,
                window,
                &mut restore,
                "board",
                mixed_dpi_work_areas(),
            );
        },
    );
    let mut position = None;
    let mut size = None;
    for command in &output.viewport_output[&ViewportId::ROOT].commands {
        match command {
            ViewportCommand::OuterPosition(p) => position = Some(*p * scale),
            ViewportCommand::InnerSize(s) => size = Some(*s * scale),
            _ => {}
        }
    }
    let (position, size) = (
        position.expect("reopen must place the board"),
        size.expect("reopen must size the board"),
    );
    egui::Rect::from_min_size(position, size + FRAME)
}

#[test]
fn hidden_scale_change_keeps_board_on_its_monitor_and_its_saved_size() {
    let d2 = mixed_dpi_work_areas()[1];
    for scale in [1.25, 1.5, 1.75, 2.0] {
        let mut window = WindowPreferences {
            x: Some(3500.0),
            y: Some(200.0),
            width: 1100.0,
            height: 1500.0,
            ..Default::default()
        };
        let before = reopen_at_scale(&mut window, 1.0, Pos2::new(3500.0, 200.0));
        assert!((before.left() - 3500.0).abs() < 1.0, "{before:?}");

        // D2 changed scale while the board was hidden.
        let scaled = reopen_at_scale(&mut window, scale, Pos2::new(7000.0, 400.0));
        assert!(
            scaled.left() >= d2.x - 0.5
                && scaled.right() <= d2.x + d2.width + 0.5
                && scaled.top() >= d2.y - 0.5
                && scaled.bottom() <= d2.y + d2.height + 0.5,
            "scale {scale}: the board must stay on D2: {scaled:?}"
        );

        // D2 is back at 100%: the temporary fit must not have replaced the
        // saved size or moved the board to another monitor.
        let restored = reopen_at_scale(&mut window, 1.0, Pos2::new(64.0, 0.0));
        assert!(
            (restored.width() - FRAME.x - 1100.0).abs() <= 1.0
                && (restored.height() - FRAME.y - 1500.0).abs() <= 1.0,
            "scale {scale}: saved size must return: {restored:?}, {window:?}"
        );
        assert!(
            restored.left() >= d2.x - 0.5 && restored.right() <= d2.x + d2.width + 0.5,
            "scale {scale}: the board must return on D2: {restored:?}"
        );
    }
}

#[test]
fn position_saved_at_one_scale_restores_the_same_physical_frame_at_another() {
    for (saved_scale, restored_scale) in [(2.0, 1.0), (1.0, 1.5), (1.75, 1.25)] {
        let mut window = WindowPreferences {
            x: Some(3600.0 / saved_scale),
            y: Some(100.0 / saved_scale),
            width: 300.0,
            height: 400.0,
            ..Default::default()
        };
        let saved = reopen_at_scale(&mut window, saved_scale, Pos2::new(3600.0, 100.0));
        let restored = reopen_at_scale(&mut window, restored_scale, saved.min);
        assert!(
            (restored.left() - saved.left()).abs() <= 1.0
                && (restored.top() - saved.top()).abs() <= 1.0,
            "{saved_scale} -> {restored_scale}: {saved:?} became {restored:?}"
        );
    }
}

/// One `update` geometry pass: fit, then capture only when nothing was fitted.
fn geometry_frame(
    window: &mut WindowPreferences,
    restore: &mut PositionRestore,
    scale: f32,
    origin: Pos2,
    client: egui::Vec2,
) -> Vec<ViewportCommand> {
    let ctx = Context::default();
    let output = ctx.run(
        normal_snapshot(ViewportId::ROOT, scale, 1.0, origin, client * scale + FRAME),
        |ctx| {
            if !fit_live_viewport_in_work_areas(
                ctx,
                window,
                restore,
                "board",
                mixed_dpi_work_areas(),
            ) {
                restore.capture_viewport(ctx, window, "board", Instant::now());
            }
        },
    );
    output.viewport_output[&ViewportId::ROOT].commands.clone()
}

#[test]
fn restored_preferred_size_survives_reports_that_predate_its_resize() {
    // Saved while D2 was at 200%; launched with D2 back at 100%.
    let mut window = WindowPreferences {
        x: Some(1720.0),
        y: Some(28.0),
        position_scale: Some(2.0),
        width: 587.0,
        height: 877.0,
        preferred_size: Some([1100.0, 1500.0]),
        ..Default::default()
    };
    let mut restore = PositionRestore::Pending;
    let origin = Pos2::new(3440.0, 56.0);
    let first = geometry_frame(
        &mut window,
        &mut restore,
        1.0,
        origin,
        egui::vec2(587.0, 877.0),
    );
    assert!(first.contains(&ViewportCommand::InnerSize(egui::vec2(1100.0, 1500.0))));
    // Native reports still show the creation size until the resize lands.
    for _ in 0..3 {
        let later = geometry_frame(
            &mut window,
            &mut restore,
            1.0,
            origin,
            egui::vec2(587.0, 877.0),
        );
        assert!(
            !later
                .iter()
                .any(|c| matches!(c, ViewportCommand::InnerSize(s) if s.x < 1099.0)),
            "a stale report must not shrink the restored size: {later:?}"
        );
        assert_eq!((window.width, window.height), (1100.0, 1500.0));
    }
}

#[test]
fn os_clamped_creation_size_is_not_taken_as_the_preferred_size() {
    // Saved at 100% on D2; D2 is now 200%. Windows created the hidden HWND on
    // D1 and clamped its client height to 1440 before the first fit.
    let mut window = WindowPreferences {
        x: Some(3500.0),
        y: Some(200.0),
        position_scale: Some(1.0),
        width: 1100.0,
        height: 1500.0,
        ..Default::default()
    };
    let mut restore = PositionRestore::Pending;
    geometry_frame(
        &mut window,
        &mut restore,
        1.0,
        Pos2::new(26.0, 26.0),
        egui::vec2(1100.0, 1440.0),
    );
    geometry_frame(
        &mut window,
        &mut restore,
        1.0,
        Pos2::new(26.0, 26.0),
        egui::vec2(1100.0, 1440.0),
    );
    // The move reaches D2, whose 200% scale shrinks the logical budget.
    geometry_frame(
        &mut window,
        &mut restore,
        2.0,
        Pos2::new(3440.0, 56.0),
        egui::vec2(1100.0, 1440.0),
    );
    assert_eq!(window.preferred_size, Some([1100.0, 1500.0]), "{window:?}");
    assert!(window.width < 600.0 && window.height < 961.0, "{window:?}");
}
