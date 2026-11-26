//! Root-only author link. No feed request exists until this control is activated.
use eframe::egui::{self, Context, Id, TextureHandle};
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(super) const LABEL: &str = "Watch the author’s latest video";
const CHANNEL: &str = "UC9xyobUju3jFwbwT-Kbo-IA";
const FEED: &str = "https://www.youtube.com/feeds/videos.xml?channel_id=UC9xyobUju3jFwbwT-Kbo-IA";
pub(super) const FALLBACK: &str = "https://www.youtube.com/@jirkahernik/videos";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FEED_BYTES: usize = 256 * 1024;
const ATOM: &[u8] = b"http://www.w3.org/2005/Atom";
const YOUTUBE: &[u8] = b"http://www.youtube.com/xml/schemas/2015";

pub(super) struct LatestVideo {
    pending: Option<mpsc::Receiver<String>>,
    endpoint: String,
    timeout: Duration,
}
impl Default for LatestVideo {
    fn default() -> Self {
        Self {
            pending: None,
            endpoint: FEED.into(),
            timeout: TIMEOUT,
        }
    }
}
impl LatestVideo {
    pub fn activate(&mut self, ctx: &Context) {
        if self.pending.is_some() {
            return;
        }
        let deadline = Instant::now() + self.timeout;
        let endpoint = self.endpoint.clone();
        let wake = ctx.clone();
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        // Client/runtime construction, DNS and all network work stay off the UI
        // and provider runtimes. The deadline includes worker startup and body.
        let _ = std::thread::Builder::new()
            .name("author-video".into())
            .spawn(move || {
                let result = resolve_before_deadline(lookup(&endpoint), deadline);
                let _ = sender.send(result);
                wake.request_repaint_of(egui::ViewportId::ROOT);
            });
    }

    pub fn open_ready(&mut self, ctx: &Context) {
        let result = self
            .pending
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(url) => Some(url),
                Err(mpsc::TryRecvError::Disconnected) => Some(FALLBACK.into()),
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some(url) = result {
            self.pending = None;
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }

    #[cfg(test)]
    pub(super) fn with_endpoint(endpoint: String, timeout: Duration) -> Self {
        Self {
            pending: None,
            endpoint,
            timeout,
        }
    }
}

// Kept separate so the actual worker timeout/teardown path can be exercised
// with slow resolver-like blocking work as well as real HTTP responses.
fn resolve_before_deadline(
    lookup: impl std::future::Future<Output = Option<String>>,
    deadline: Instant,
) -> String {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
        .and_then(|runtime| {
            let result = runtime.block_on(async {
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), lookup)
                    .await
                    .ok()
                    .flatten()
            });
            // DNS uses blocking OS work that timeout cannot cancel. Runtime's
            // normal Drop joins it; publish the result without that extra wait.
            runtime.shutdown_background();
            result
        })
        .filter(|_| Instant::now() <= deadline)
        .unwrap_or_else(|| FALLBACK.into())
}

async fn lookup(endpoint: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .user_agent(concat!("TokenAshtray/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let mut response = client
        .get(endpoint)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_FEED_BYTES as u64)
    {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_FEED_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    latest_published(std::str::from_utf8(&body).ok()?)
}

#[derive(Default)]
struct Entry {
    video: Option<String>,
    channel: Option<String>,
    published: Option<String>,
    invalid: bool,
}

fn latest_published(xml: &str) -> Option<String> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut entry = None::<Entry>;
    let mut field = None::<u8>;
    let mut content = String::new();
    let mut newest = None::<(OffsetDateTime, String)>;
    loop {
        let (namespace, event) = reader.read_resolved_event().ok()?;
        let namespace = match namespace {
            ResolveResult::Bound(ns) => Some(ns.0),
            ResolveResult::Unbound => None,
            ResolveResult::Unknown(_) => return None,
        };
        match event {
            Event::Start(tag) => {
                if tag.attributes().any(|attribute| attribute.is_err()) {
                    return None;
                }
                depth += 1;
                if depth == 1 {
                    if root_seen || namespace != Some(ATOM) || tag.local_name().as_ref() != b"feed"
                    {
                        return None;
                    }
                    root_seen = true;
                } else if depth == 2
                    && namespace == Some(ATOM)
                    && tag.local_name().as_ref() == b"entry"
                {
                    entry = Some(Entry::default());
                } else if depth == 3 && entry.is_some() {
                    field = match (namespace, tag.local_name().as_ref()) {
                        (Some(YOUTUBE), b"videoId") => Some(0),
                        (Some(YOUTUBE), b"channelId") => Some(1),
                        (Some(ATOM), b"published") => Some(2),
                        _ => None,
                    };
                    content.clear();
                } else if field.is_some() {
                    entry.as_mut()?.invalid = true;
                }
            }
            Event::Text(text) if field.is_some() => content.push_str(&text.decode().ok()?),
            Event::CData(text) if field.is_some() => content.push_str(&text.decode().ok()?),
            Event::GeneralRef(reference) if field.is_some() => {
                if let Some(ch) = reference.resolve_char_ref().ok()? {
                    content.push(ch);
                } else {
                    content.push_str(match &reference[..] {
                        b"amp" => "&",
                        b"lt" => "<",
                        b"gt" => ">",
                        b"quot" => "\"",
                        b"apos" => "'",
                        _ => return None,
                    });
                }
            }
            Event::End(_) => {
                if depth == 3
                    && let Some(field) = field.take()
                {
                    let entry = entry.as_mut()?;
                    let slot = match field {
                        0 => &mut entry.video,
                        1 => &mut entry.channel,
                        _ => &mut entry.published,
                    };
                    if slot.replace(content.trim().into()).is_some() {
                        entry.invalid = true;
                    }
                }
                if depth == 2
                    && let Some(entry) = entry.take()
                    && !entry.invalid
                    && entry.channel.as_deref() == Some(CHANNEL)
                    && let (Some(video), Some(published)) = (entry.video, entry.published)
                    && video.len() == 11
                    && video
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                    && let Ok(published) = OffsetDateTime::parse(&published, &Rfc3339)
                    && newest
                        .as_ref()
                        .is_none_or(|(latest, _)| published > *latest)
                {
                    newest = Some((published, video));
                }
                depth = depth.checked_sub(1)?;
            }
            Event::Text(text) if depth == 0 && !text.decode().ok()?.trim().is_empty() => {
                return None;
            }
            Event::DocType(_) => return None,
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen || depth != 0 {
        return None;
    }
    newest.map(|(_, id)| format!("https://www.youtube.com/watch?v={id}"))
}

pub(super) fn show(ui: &mut egui::Ui, visited: bool) -> bool {
    let ctx = ui.ctx().clone();
    let colors = super::palette(&ctx);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
    let response = ui.interact(rect, Id::new("author-youtube-action"), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, LABEL));
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, 4.0, colors.faint);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0_f32, colors.blue),
            egui::StrokeKind::Inside,
        );
        response.show_tooltip_text(LABEL);
    }
    let key = Id::new("author-youtube-logo");
    let texture = ctx
        .data_mut(|d| d.get_temp::<TextureHandle>(key))
        .unwrap_or_else(|| {
            let image = eframe::icon_data::from_png_bytes(include_bytes!(
                "../../assets/author/youtube-icon-ui.png"
            ))
            .expect("official embedded YouTube PNG");
            let texture = ctx.load_texture(
                "YouTube",
                egui::ColorImage::from_rgba_unmultiplied(
                    [image.width as usize, image.height as usize],
                    &image.rgba,
                ),
                egui::TextureOptions::LINEAR,
            );
            ctx.data_mut(|d| d.insert_temp(key, texture.clone()));
            texture
        });
    // The official asset includes transparent clear space: its visible mark is
    // 171px wide in a 251px canvas. Keep that mark 16pt wide and preserve the asset.
    let logo = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(16.0 * 251.0 / 171.0, 16.0 * 215.0 / 171.0),
    );
    ui.painter().image(
        texture.id(),
        logo,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    if !visited && !response.clicked() {
        ui.painter()
            .circle_filled(rect.center() + egui::vec2(9.0, -7.0), 2.0, colors.blue);
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(LABEL)
        .clicked()
}

#[cfg(test)]
pub(super) mod tests;
