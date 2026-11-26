//! Egui and custom painters use the same two complete palettes.
use crate::config::ThemePreference;
use eframe::egui::{self, Color32, Context, Theme};

#[cfg(target_os = "linux")]
mod portal;

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub canvas: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub outline: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub blue: Color32,
    pub action: Color32,
    pub hovered: Color32,
    pub faint: Color32,
    pub extreme: Color32,
    pub link: Color32,
}

pub(super) const DARK: Palette = Palette {
    canvas: Color32::from_rgb(16, 19, 24),
    surface: Color32::from_rgb(25, 30, 38),
    raised: Color32::from_rgb(36, 44, 55),
    outline: Color32::from_rgb(54, 66, 81),
    text: Color32::from_rgb(238, 242, 247),
    secondary: Color32::from_rgb(174, 187, 201),
    blue: Color32::from_rgb(56, 139, 235),
    action: Color32::from_rgb(0, 92, 170),
    hovered: Color32::from_rgb(45, 58, 74),
    faint: Color32::from_rgb(15, 24, 38),
    extreme: Color32::from_rgb(7, 12, 22),
    link: Color32::from_rgb(96, 165, 250),
};

pub(super) const LIGHT: Palette = Palette {
    canvas: Color32::from_rgb(237, 241, 246),
    surface: Color32::from_rgb(255, 255, 255),
    raised: Color32::from_rgb(228, 234, 242),
    outline: Color32::from_rgb(151, 166, 184),
    text: Color32::from_rgb(25, 36, 51),
    secondary: Color32::from_rgb(75, 91, 111),
    blue: Color32::from_rgb(0, 96, 181),
    action: Color32::from_rgb(0, 92, 170),
    hovered: Color32::from_rgb(213, 227, 243),
    faint: Color32::from_rgb(233, 239, 247),
    extreme: Color32::from_rgb(255, 255, 255),
    link: Color32::from_rgb(0, 85, 161),
};

pub(super) fn palette(ctx: &Context) -> Palette {
    if ctx.style().visuals.dark_mode {
        DARK
    } else {
        LIGHT
    }
}

pub(super) fn configure(ctx: &Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        for (text, size) in [
            (egui::TextStyle::Body, 14.0),
            (egui::TextStyle::Button, 14.0),
            (egui::TextStyle::Small, 12.0),
            (egui::TextStyle::Heading, 16.0),
        ] {
            style
                .text_styles
                .insert(text, egui::FontId::proportional(size));
        }
    });
    for (theme, p) in [(Theme::Dark, DARK), (Theme::Light, LIGHT)] {
        let mut v = match theme {
            Theme::Dark => egui::Visuals::dark(),
            Theme::Light => egui::Visuals::light(),
        };
        v.panel_fill = p.canvas;
        v.window_fill = p.surface;
        v.faint_bg_color = p.faint;
        v.extreme_bg_color = p.extreme;
        // Let each widget supply its foreground: a global override would put
        // dark text on the blue selected controls in the light theme.
        v.override_text_color = None;
        v.hyperlink_color = p.link;
        v.selection.bg_fill = p.action;
        v.selection.stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
        v.window_corner_radius = egui::CornerRadius::same(8);
        v.window_stroke = egui::Stroke::new(1.0_f32, p.outline);
        v.button_frame = true;
        v.collapsing_header_frame = true;
        for (widget, fill, stroke, text) in [
            (&mut v.widgets.noninteractive, p.surface, p.outline, p.text),
            (&mut v.widgets.inactive, p.raised, p.outline, p.text),
            (&mut v.widgets.hovered, p.hovered, p.blue, p.text),
            // Egui also uses active text for RichText::strong, including
            // headings outside buttons. Keep it readable on the page surface.
            (&mut v.widgets.active, p.hovered, p.blue, p.text),
            (&mut v.widgets.open, p.hovered, p.blue, p.text),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = egui::Stroke::new(1.0_f32, stroke);
            widget.fg_stroke = egui::Stroke::new(1.0_f32, text);
            widget.corner_radius = egui::CornerRadius::same(6);
        }
        ctx.set_visuals_of(theme, v);
    }
}

/// The watcher belongs to the UI lifetime, never to a provider or poll scheduler.
#[derive(Clone, Default)]
pub(super) struct Appearance {
    #[cfg(target_os = "linux")]
    portal: Option<std::sync::Arc<portal::Watcher>>,
}

impl Appearance {
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        #[cfg(target_os = "linux")]
        {
            Self {
                portal: Some(std::sync::Arc::new(portal::Watcher::start(wake))),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = wake;
            Self::default()
        }
    }

    pub fn stop(&self) {
        #[cfg(target_os = "linux")]
        if let Some(portal) = &self.portal {
            portal.stop();
        }
    }

    pub fn apply(&self, ctx: &Context, preference: ThemePreference) {
        #[cfg(target_os = "linux")]
        if let Some(portal) = &self.portal {
            // Egui's system-theme slot is private and winit reports None on
            // Linux. Resolve the display here; the persisted choice stays System.
            apply_system(ctx, preference, portal.theme());
            return;
        }
        ctx.set_theme(match preference {
            ThemePreference::System => egui::ThemePreference::System,
            ThemePreference::Light => egui::ThemePreference::Light,
            ThemePreference::Dark => egui::ThemePreference::Dark,
        });
    }
}

#[cfg(target_os = "linux")]
pub(super) fn apply_system(ctx: &Context, preference: ThemePreference, system: Option<Theme>) {
    ctx.set_theme(match preference {
        ThemePreference::System => system.unwrap_or(Theme::Dark),
        ThemePreference::Light => Theme::Light,
        ThemePreference::Dark => Theme::Dark,
    });
}
