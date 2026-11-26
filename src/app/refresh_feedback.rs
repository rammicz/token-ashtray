//! UI lifetimes start when observed, retaining fast unchanged outcomes.
use super::*;
use token_ashtray::refresh::{RefreshPhase, RefreshSnapshot};
const HOLD: Duration = Duration::from_secs(4);
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Clone, Default)]
pub(super) struct Presentation {
    pub toolbar: Option<String>,
    pub cards: Arc<BTreeMap<String, RefreshPhase>>,
}
#[derive(Default)]
pub(super) struct Feedback {
    next_request: u64,
    pending: Option<(u64, Instant)>,
    expected_generation: Option<u64>,
    snapshot: RefreshSnapshot,
    observed: BTreeMap<String, (RefreshPhase, Instant)>,
    completed: Option<Instant>,
    error: Option<(String, Instant)>,
    suppress_after_failure: bool,
}
impl Feedback {
    pub fn request(&mut self, now: Instant) -> Option<u64> {
        if self.pending.is_some() || self.snapshot.active() {
            return None;
        }
        self.next_request = self.next_request.wrapping_add(1).max(1);
        self.pending = Some((self.next_request, now));
        self.error = None;
        self.suppress_after_failure = false;
        self.observed.clear();
        Some(self.next_request)
    }
    pub fn accepted(&mut self, request: u64, result: Result<u64, String>, now: Instant) {
        if self.pending.is_some_and(|(pending, _)| pending == request) {
            self.pending = None;
            match result {
                Ok(generation) => self.expected_generation = Some(generation),
                Err(error) => self.fail(error, now),
            }
        }
    }
    pub fn fail(&mut self, error: String, now: Instant) {
        self.pending = None;
        self.expected_generation = None;
        self.observed.clear();
        self.error = Some((error, now));
        self.suppress_after_failure = true;
    }
    pub fn update(
        &mut self,
        snapshot: RefreshSnapshot,
        suppressed: u64,
        now: Instant,
    ) -> Presentation {
        if self
            .pending
            .is_some_and(|(_, since)| now.duration_since(since) >= ACK_TIMEOUT)
        {
            self.fail(
                "Refresh not confirmed: tray did not acknowledge the request".into(),
                now,
            );
        }
        if let Some((error, since)) = &self.error {
            if now.duration_since(*since) < HOLD {
                return Presentation {
                    toolbar: Some(error.clone()),
                    cards: Arc::default(),
                };
            }
            self.error = None;
        }
        if self.suppress_after_failure {
            return Presentation::default();
        }
        if self.pending.is_some() {
            return Presentation {
                toolbar: Some("Refresh requested; waiting for tray".into()),
                cards: Arc::default(),
            };
        }
        let expected = self.expected_generation;
        if snapshot.generation == 0
            || (snapshot.generation <= suppressed && expected != Some(snapshot.generation))
        {
            return Presentation::default();
        }
        if expected.is_some_and(|generation| snapshot.generation < generation) {
            return Presentation {
                toolbar: Some("Refresh accepted; waiting for provider state".into()),
                cards: Arc::default(),
            };
        }
        if snapshot.generation != self.snapshot.generation {
            self.observed.clear();
            self.completed = None;
        }
        for (id, phase) in &snapshot.providers {
            if self
                .observed
                .get(id)
                .is_none_or(|(previous, _)| previous != phase)
            {
                self.observed.insert(id.clone(), (*phase, now));
            }
        }
        if !snapshot.active() && self.completed.is_none() {
            self.completed = Some(now);
        }
        self.snapshot = snapshot;
        let cards = self
            .observed
            .iter()
            .filter(|(_, (phase, since))| phase.active() || now.duration_since(*since) < HOLD)
            .map(|(id, (phase, _))| (id.clone(), *phase))
            .collect();
        let toolbar = (self.snapshot.active()
            || self
                .completed
                .is_some_and(|since| now.duration_since(since) < HOLD))
        .then(|| self.snapshot.summary().into());
        Presentation {
            toolbar,
            cards: Arc::new(cards),
        }
    }
}
fn key() -> Id {
    Id::new("refresh-presentation")
}
pub(super) fn install(ctx: &Context, presentation: Presentation) {
    ctx.data_mut(|data| data.insert_temp(key(), presentation));
}
#[cfg(test)]
pub(super) fn presentation(ctx: &Context) -> Presentation {
    ctx.data(|data| data.get_temp(key())).unwrap_or_default()
}
pub(super) fn toolbar_status(ctx: &Context) -> Option<String> {
    ctx.data(|data| {
        data.get_temp::<Presentation>(key())
            .and_then(|view| view.toolbar)
    })
}
fn phase(ctx: &Context, id: &str) -> Option<RefreshPhase> {
    ctx.data(|data| {
        data.get_temp::<Presentation>(key())
            .and_then(|view| view.cards.get(id).copied())
    })
}
pub(super) fn toolbar(ui: &mut egui::Ui) -> bool {
    let status = toolbar_status(ui.ctx());
    let label = status
        .as_ref()
        .map(|text| format!("Refresh now: {text}"))
        .unwrap_or_else(|| "Refresh now".into());
    let response = ui_icons::button(ui, Icon::Refresh, &label, false);
    if let Some(status) = status {
        let (glyph, color) = if status.contains("failed")
            || status.contains("errors")
            || status.contains("not ")
            || status.contains("lost")
            || status.contains("unavailable")
        {
            ("!", status_label(Status::Error).1)
        } else if status.contains("complete") || status.contains("finished") {
            ("+", palette(ui.ctx()).blue)
        } else {
            (".", palette(ui.ctx()).blue)
        };
        let center = response.rect.right_bottom() - egui::vec2(6.0, 6.0);
        ui.painter()
            .circle_filled(center, 6.0, palette(ui.ctx()).surface);
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(12.0),
            color,
        );
    }
    response.clicked()
}
/// Reserve one compact strip so refresh transitions do not move card controls.
pub(super) fn card_cue(ui: &mut egui::Ui, card: &ProviderCard) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 16.0), egui::Sense::hover());
    let response = ui.interact(
        rect,
        Id::new(("refresh-card-state", &card.id)),
        egui::Sense::hover(),
    );
    if let Some(phase) = phase(ui.ctx(), &card.id) {
        let text = format!("{}: {}", card.short_name, phase.label());
        let color = if phase == RefreshPhase::Failed {
            status_label(Status::Error).1
        } else {
            palette(ui.ctx()).secondary
        };
        paint_left_truncated(
            ui.painter(),
            rect,
            phase.label(),
            egui::FontId::proportional(11.0),
            color,
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &text));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_live(egui::accesskit::Live::Polite)
        });
        response.on_hover_text(text);
    }
}
pub(super) fn opacity(ui: &egui::Ui, id: &str) -> f32 {
    if ui.is_visible()
        && phase(ui.ctx(), id) == Some(RefreshPhase::Running)
        && ui.style().animation_time > 0.0
        && !platform::reduced_motion_requested()
    {
        ui.ctx().request_repaint_after(Duration::from_millis(50));
        let time = ui.input(|input| input.time) as f32;
        0.96 + 0.04 * (time * 2.5).cos()
    } else {
        1.0
    }
}
pub(super) fn highlight(ui: &egui::Ui, id: &str, rect: egui::Rect) {
    if let Some(phase) = phase(ui.ctx(), id) {
        let color = match phase {
            RefreshPhase::Succeeded => palette(ui.ctx()).blue,
            RefreshPhase::Failed => status_label(Status::Error).1,
            _ => palette(ui.ctx()).outline,
        };
        ui.painter().rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.0_f32, color),
            egui::StrokeKind::Inside,
        );
    }
}

pub(super) fn short_status(status: &str) -> &'static str {
    if status.contains("not confirmed") {
        "Unconfirmed"
    } else if status.contains("unavailable") || status.contains("lost") {
        "Offline"
    } else if status.contains("not sent") {
        "Not sent"
    } else if status.contains("errors") {
        "Errors"
    } else if status.contains("No enabled") {
        "No providers"
    } else if status.contains("complete") || status.contains("finished") {
        "Done"
    } else if status.contains("waiting") {
        "Waiting"
    } else {
        "Refreshing"
    }
}
