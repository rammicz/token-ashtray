//! Native interface marks on a shared 16-point grid; provider brands live elsewhere.
use eframe::egui::{self, Color32, Painter, Rect, Stroke};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Refresh,
    Menu,
    Columns,
    Pin,
    Primary,
    Compact,
    Drag,
}

impl Icon {
    pub(super) fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let origin = rect.center() - egui::vec2(8.0, 8.0);
        let point = |x, y| origin + egui::vec2(x, y);
        let stroke = Stroke::new(1.5_f32, color);
        let line = |a: (f32, f32), b: (f32, f32)| {
            painter.line_segment([point(a.0, a.1), point(b.0, b.1)], stroke);
        };
        match self {
            Self::Refresh => {
                let points = (0..=24)
                    .map(|i| {
                        let angle = 0.45 + i as f32 / 24.0 * 5.2;
                        point(8.0 + 5.0 * angle.cos(), 8.0 + 5.0 * angle.sin())
                    })
                    .collect();
                painter.add(egui::Shape::line(points, stroke));
                line((12.0, 2.0), (12.0, 5.0));
                line((9.0, 5.0), (12.0, 5.0));
            }
            Self::Menu => {
                for x in [3.0, 8.0, 13.0] {
                    painter.circle_filled(point(x, 8.0), 1.1, color);
                }
            }
            Self::Columns => {
                for x in [3.0, 8.0, 13.0] {
                    painter.rect_filled(
                        Rect::from_min_max(point(x, 3.0), point(x + 2.6, 13.0)),
                        1.0,
                        color,
                    );
                }
            }
            Self::Compact => {
                painter.rect_stroke(
                    Rect::from_min_max(point(2.0, 3.0), point(14.0, 13.0)),
                    2.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
                line((5.0, 6.0), (11.0, 6.0));
            }
            Self::Pin => {
                painter.add(egui::Shape::line(
                    [
                        (5.0, 2.0),
                        (11.0, 2.0),
                        (10.0, 7.0),
                        (12.0, 10.0),
                        (4.0, 10.0),
                        (6.0, 7.0),
                        (5.0, 2.0),
                    ]
                    .map(|(x, y)| point(x, y))
                    .to_vec(),
                    stroke,
                ));
                line((8.0, 10.0), (8.0, 14.0));
            }
            Self::Primary => {
                let points = (0..10)
                    .map(|i| {
                        let angle =
                            -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                        let radius = if i % 2 == 0 { 6.0 } else { 2.8 };
                        point(8.0 + radius * angle.cos(), 8.0 + radius * angle.sin())
                    })
                    .collect();
                painter.add(egui::Shape::closed_line(points, stroke));
            }
            Self::Drag => {
                for x in [5.0, 11.0] {
                    for y in [3.0, 8.0, 13.0] {
                        painter.circle_filled(point(x, y), 0.9, color);
                    }
                }
            }
        }
    }
}

pub(super) fn button(ui: &mut egui::Ui, icon: Icon, label: &str, text: bool) -> egui::Response {
    let response = ui.add(
        egui::Button::new(if text {
            format!("       {label}")
        } else {
            String::new()
        })
        .min_size(egui::vec2(32.0, 32.0)),
    );
    let rect = if text {
        Rect::from_center_size(
            egui::pos2(response.rect.left() + 16.0, response.rect.center().y),
            egui::vec2(16.0, 16.0),
        )
    } else {
        response.rect
    };
    icon.paint(
        ui.painter(),
        rect,
        ui.style().interact(&response).fg_stroke.color,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response.on_hover_text(label)
}

pub(super) fn label(ui: &mut egui::Ui, icon: Icon) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 20.0), egui::Sense::hover());
    icon.paint(ui.painter(), rect, ui.visuals().weak_text_color());
}
