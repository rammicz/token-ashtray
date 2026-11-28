//! Offline provider identity assets. Decoding and texture ownership stay in the UI process.
use eframe::egui::{self, Color32, ColorImage, Id, Rect, TextureHandle, TextureOptions, Ui};

const SIZE: usize = 96;

fn logo_asset(provider_id: &str) -> Option<(&'static str, &'static [u8])> {
    match provider_id {
        "claude" => Some((
            "claude",
            include_bytes!("../assets/providers/claude.rgba").as_slice(),
        )),
        "codex" => Some((
            "codex",
            include_bytes!("../assets/providers/codex.rgba").as_slice(),
        )),
        "grok" | "grok-bot" => Some((
            "grok",
            include_bytes!("../assets/providers/grok.rgba").as_slice(),
        )),
        "cursor" => Some((
            "cursor",
            include_bytes!("../assets/providers/cursor.rgba").as_slice(),
        )),
        "copilot" => Some((
            "copilot",
            include_bytes!("../assets/providers/copilot.rgba").as_slice(),
        )),
        "kimi" => Some((
            "kimi",
            include_bytes!("../assets/providers/kimi.rgba").as_slice(),
        )),
        "antigravity" => Some((
            "antigravity",
            include_bytes!("../assets/providers/antigravity.rgba").as_slice(),
        )),
        "vibe" => Some((
            "vibe",
            include_bytes!("../assets/providers/vibe.rgba").as_slice(),
        )),
        _ => None,
    }
}

pub fn paint_provider_logo(ui: &mut Ui, provider_id: &str, rect: Rect) {
    let asset = logo_asset(provider_id);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 9.0, Color32::from_rgb(8, 13, 22));
    painter.rect_stroke(
        rect,
        9.0,
        (1.0, Color32::from_rgb(48, 65, 86)),
        egui::StrokeKind::Inside,
    );
    if let Some((name, bytes)) = asset {
        let key = Id::new(("provider-logo", name));
        let cached = ui
            .ctx()
            .data_mut(|data| data.get_temp::<TextureHandle>(key));
        let texture = cached.unwrap_or_else(|| {
            let texture = ui.ctx().load_texture(
                name,
                ColorImage::from_rgba_unmultiplied([SIZE, SIZE], bytes),
                TextureOptions::LINEAR,
            );
            ui.ctx()
                .data_mut(|data| data.insert_temp(key, texture.clone()));
            texture
        });
        painter.image(
            texture.id(),
            rect.shrink(1.5),
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        painter.circle_stroke(rect.center(), rect.width() * 0.24, (1.5, Color32::GRAY));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_provider_has_a_96px_rgba_logo_asset() {
        let expected_bytes = SIZE * SIZE * 4;
        for provider in token_ashtray::providers::all() {
            let id = provider.descriptor().id;
            let (name, bytes) = logo_asset(id)
                .unwrap_or_else(|| panic!("provider {id} is missing an embedded logo"));
            assert!(!name.is_empty());
            assert_eq!(bytes.len(), expected_bytes, "provider {id} RGBA size");
        }
        assert!(logo_asset("vibe").is_some());
        assert!(logo_asset("unknown-provider").is_none());
    }
}
