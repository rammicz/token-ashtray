//! Shared About content; opening it is local to the initiating native viewport.
use eframe::egui::{self, Context, Id};

const YOUTUBE: &str = "https://www.youtube.com/@jirkahernik";
const PATREON: &str = "https://www.patreon.com/JirkaHernik";

pub(super) fn show(ctx: &Context, requested: bool) {
    let state = Id::new(("about-open", ctx.viewport_id()));
    let was_open = ctx.data(|data| data.get_temp::<bool>(state).unwrap_or(false));
    let mut open = was_open || requested;
    if !open {
        return;
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        ctx.data_mut(|data| data.insert_temp(state, false));
        return;
    }
    let screen = ctx.screen_rect();
    let width = (screen.width() - 48.0).clamp(160.0, 560.0);
    let height = (screen.height() - 48.0).clamp(120.0, 640.0);
    egui::Window::new("About Token Ashtray")
        .id(Id::new(("about-dialog", ctx.viewport_id())))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .constrain_to(screen.shrink(8.0))
        .fixed_size(egui::vec2(width, height))
        .show(ctx, |ui| {
            // Closing stays outside the scrolling content, including at 360×360.
            ui.horizontal(|ui| {
                ui.strong("About Token Ashtray");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = ui.button("Close").on_hover_text("Close About");
                    close.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close About")
                    });
                    if requested && !was_open {
                        close.request_focus();
                    }
                    if close.clicked() {
                        open = false;
                    }
                });
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("about-content")
                .auto_shrink([false, false])
                .max_height((height - 42.0).max(60.0))
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.heading("Token Ashtray");
                    ui.label(concat!(env!("CARGO_PKG_NAME"), " · ", env!("CARGO_PKG_VERSION")));
                    let copy = ui.button("Copy version");
                    super::reveal_focused_control(&copy);
                    if copy.clicked() {
                        ui.ctx().copy_text(env!("CARGO_PKG_VERSION").to_owned());
                    }
                    ui.add_space(12.0);
                    ui.strong("All your limits. One tray.");
                    ui.label("It reads existing CLI logins locally. No account, telemetry, or browser automation.");
                    ui.add_space(12.0);
                    ui.label("Jirka Herník / @jirkahernik");
                    ui.horizontal_wrapped(|ui| {
                        super::reveal_focused_control(&ui.hyperlink_to("YouTube", YOUTUBE));
                        ui.add_space(8.0);
                        super::reveal_focused_control(&ui.hyperlink_to("Support on Patreon", PATREON));
                    });
                    ui.add_space(16.0);
                    ui.separator();
                    ui.strong("Licenses and notices");
                    notice(ui, "Application license", include_str!("../../LICENSE"));
                    notice(ui, "Third-party libraries", include_str!("../../THIRD-PARTY.md"));
                    notice(ui, "Provider logo notices", include_str!("../../assets/providers/NOTICE.md"));
                    notice(ui, "Author link logo notice", include_str!("../../assets/author/NOTICE.md"));
                });
        });
    ctx.data_mut(|data| data.insert_temp(state, open));
}

fn notice(ui: &mut egui::Ui, title: &str, text: &str) {
    let section = egui::CollapsingHeader::new(title).show(ui, |ui| {
        ui.add(egui::Label::new(text).selectable(true).wrap());
    });
    super::reveal_focused_control(&section.header_response);
}
