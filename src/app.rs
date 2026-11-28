mod about;
mod author_youtube;
mod refresh_feedback;
mod theme;
use theme::palette;
#[path = "ui_icons.rs"]
mod ui_icons;
use ui_icons::Icon;

use std::{
    collections::BTreeMap,
    sync::{
        Arc, OnceLock, RwLock,
        mpsc::{Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Context, Frame, IconData, Id, Pos2, ViewportBuilder, ViewportCommand, ViewportId,
    WindowLevel,
};
use time::OffsetDateTime;

use crate::{
    config::{
        BoardPreferences, ColumnLayout, ConfigStore, QuotaDisplayMode, ThemePreference,
        WindowPreferences,
    },
    ipc::{ChildMessage, UiConnection},
    platform::{
        self, SharedTraySnapshot, TrayAction, TrayProvider, TraySnapshot,
        instance::{Listener, Request},
    },
};
use token_ashtray::{
    model::{ProviderCard, Status, duration_label, reset_countdown_label, reset_detail_label},
    poll::{PollSettings, Poller, SharedBoard, initial_board},
    updates::{Failure, Unavailable, UpdateStatus, Updater, apply::MAX_ATTEMPTS},
};

/// Shown with the update setting, so opting in is informed.
const UPDATES_CONTROL: &str = "Check for updates automatically";
const UPDATE_EXPLANATION: &str = "When on, Token Ashtray checks the project's GitHub releases once a day and downloads verified updates. GitHub sees your IP address, app version and platform. Nothing else is sent.";

/// One plain sentence for the settings menu.
fn update_status_text(status: &UpdateStatus) -> String {
    let failed = |reason: &str| format!("The last update check failed: {reason}.");
    match status {
        UpdateStatus::Off => "Automatic updates are off.".into(),
        UpdateStatus::Unavailable(reason) => match reason {
            Unavailable::NoTrustRoot => {
                "Updates are unavailable: this build has no release signing key."
            }
            Unavailable::UnsupportedPlatform => "Updates are unavailable on this platform.",
            Unavailable::NoStateDirectory => {
                "Updates are unavailable: no folder is available for update data."
            }
            Unavailable::CorruptState => {
                "Updates are unavailable: the saved update data is damaged."
            }
            Unavailable::LockedElsewhere => {
                "Updates are unavailable: another Token Ashtray process is updating."
            }
        }
        .into(),
        UpdateStatus::Checking => "Checking for updates…".into(),
        UpdateStatus::UpToDate => "No newer verified release was found.".into(),
        UpdateStatus::Ready(version) => format!("Version {version} is downloaded and verified."),
        UpdateStatus::Failed(Failure::Connection) => {
            failed("the secure connection could not be set up")
        }
        UpdateStatus::Failed(Failure::Storage) => failed("the update data could not be saved"),
        UpdateStatus::Failed(Failure::Network) => {
            failed("the release channel could not be reached")
        }
        UpdateStatus::Failed(Failure::Rejected) => failed("the release data did not verify"),
        UpdateStatus::Installed(version) => format!("Token Ashtray was updated to {version}."),
        UpdateStatus::InstallFailed { version, attempt } => format!(
            "Installing version {version} failed (attempt {attempt} of {MAX_ATTEMPTS}). It is tried again after the next Quit."
        ),
        UpdateStatus::InstallStopped(version) => format!(
            "Version {version} could not be installed automatically. Install it with the setup from the project's GitHub releases."
        ),
    }
}

pub(crate) const TRAY_UNAVAILABLE_NOTICE: &str =
    "Tray support is unavailable; the board is open without a tray icon.";

pub struct AppState {
    pub board: SharedBoard,
}

impl AppState {
    fn local() -> Self {
        Self {
            board: initial_board(),
        }
    }
    fn remote(board: SharedBoard) -> Self {
        Self { board }
    }
    fn request_repaint(&self, ctx: &Context) {
        ctx.request_repaint_of(ViewportId::ROOT);
    }
}

struct PositionCandidate {
    frame: Pos2,
    client: Pos2,
    since: Instant,
}

#[derive(Default)]
enum PositionRestore {
    #[default]
    Pending,
    Requested {
        target: Pos2,
        requested_at: Instant,
        last_seen: Option<(Pos2, Instant)>,
        retries: u8,
    },
    Settled {
        saved_anchor: Pos2,
        observed_anchor: Pos2,
        egui_frame_seen: bool,
        unknown_first_frame: bool,
        candidate: Option<PositionCandidate>,
    },
    Unpositioned {
        since: Instant,
        last_seen: Option<(Pos2, Instant)>,
    },
    Unsupported,
}

impl PositionRestore {
    fn capture_viewport(
        &mut self,
        ctx: &Context,
        saved: &mut WindowPreferences,
        title: &str,
        now: Instant,
    ) -> bool {
        position_at_native_scale(saved, native_scale(ctx));
        if !viewport_has_normal_geometry(ctx) {
            return false;
        }
        let Some(zoom) = valid_geometry_zoom(ctx.zoom_factor()) else {
            return false;
        };
        let geometry = ctx.input(|i| i.viewport().outer_rect.zip(i.viewport().inner_rect));
        let Some((outer, inner)) = geometry else {
            return false;
        };
        let native_frame = native_frame_origin(ctx, title, inner);
        let (changed, command) =
            self.capture_with_zoom(saved, outer, inner, native_frame, zoom, now);
        if let Some(position) = command {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(viewport_position(
                position, zoom,
            )));
        }
        changed
    }

    fn requested_position(
        &mut self,
        saved: &WindowPreferences,
        monitors: &[platform::MonitorRect],
        supported: bool,
        now: Instant,
    ) -> (Option<Pos2>, bool) {
        if !supported {
            *self = Self::Unsupported;
            return (None, false);
        }
        if matches!(self, Self::Pending) && !monitors.is_empty() {
            if let Some(target) = restore_position(saved, monitors) {
                *self = Self::Requested {
                    target,
                    requested_at: now,
                    last_seen: None,
                    retries: 0,
                };
                return (Some(target), true);
            }
            *self = Self::Unpositioned {
                since: now,
                last_seen: None,
            };
        }
        let position = match self {
            Self::Requested { target, .. } => Some(*target),
            _ => None,
        };
        (position, false)
    }

    fn prepare_show(&mut self, now: Instant) {
        match self {
            Self::Requested {
                requested_at,
                last_seen,
                retries,
                ..
            } => {
                *requested_at = now;
                *last_seen = None;
                *retries = 0;
            }
            Self::Unpositioned { since, last_seen } => {
                *since = now;
                *last_seen = None;
            }
            _ => {}
        }
    }

    #[cfg(test)]
    fn capture(
        &mut self,
        saved: &mut WindowPreferences,
        outer: egui::Rect,
        inner: egui::Rect,
        now: Instant,
    ) -> (bool, Option<Pos2>) {
        self.capture_with_frame(saved, outer, inner, None, now)
    }

    #[cfg(test)]
    fn capture_with_frame(
        &mut self,
        saved: &mut WindowPreferences,
        outer: egui::Rect,
        inner: egui::Rect,
        native_frame: Option<Pos2>,
        now: Instant,
    ) -> (bool, Option<Pos2>) {
        self.capture_with_zoom(saved, outer, inner, native_frame, 1.0, now)
    }

    fn capture_with_zoom(
        &mut self,
        saved: &mut WindowPreferences,
        outer: egui::Rect,
        inner: egui::Rect,
        native_frame: Option<Pos2>,
        zoom: f32,
        now: Instant,
    ) -> (bool, Option<Pos2>) {
        let Some(zoom) = valid_geometry_zoom(zoom) else {
            return (false, None);
        };
        // Persist native-DPI logical geometry. egui viewport coordinates and
        // the verified X11 frame are divided by native DPI * UI zoom, while
        // monitor rectangles are divided by native DPI alone.
        let outer = durable_rect(outer, zoom);
        let inner = durable_rect(inner, zoom);
        let native_frame = native_frame.map(|frame| frame * zoom);
        // While a requested placement is in flight, size reports can predate
        // its resize or show the OS clamping the new window. Only sizes after
        // placement settles are the user's.
        let changed = !matches!(self, Self::Requested { .. }) && capture_size(saved, inner);
        if changed && matches!(self, Self::Settled { .. }) {
            // A resize after placement settled is the user's new size.
            saved.preferred_size = None;
        }
        // The first viewport callbacks can report the client origin as both
        // rectangles. Later callbacks distinguish the frame and client. Track
        // the client consistently; use a reported frame when it is available.
        let observed = inner.min;
        let frame =
            native_frame.or_else(|| (!same_position(outer.min, inner.min)).then_some(outer.min));
        match self {
            Self::Pending | Self::Unsupported => (changed, None),
            Self::Requested {
                target,
                requested_at,
                last_seen,
                retries,
            } => {
                let mut command = None;
                let mut settle_anchor = None;

                let near_target =
                    (observed.x - target.x).abs() <= 64.0 && (observed.y - target.y).abs() <= 80.0;
                if now.duration_since(*requested_at) >= Duration::from_millis(350) {
                    if near_target
                        && last_seen.is_some_and(|(previous, since)| {
                            same_position(previous, observed)
                                && now.duration_since(since) >= Duration::from_millis(100)
                        })
                    {
                        settle_anchor = Some(*target);
                    } else if !near_target
                        && now.duration_since(*requested_at) >= Duration::from_millis(500)
                    {
                        if *retries < 3 {
                            *retries += 1;
                            *requested_at = now;
                            command = Some(*target);
                        } else {
                            // The compositor did not honor placement. Keep the saved
                            // coordinates until a later real movement is observed.
                            settle_anchor = Some(observed);
                        }
                    }
                }
                if !last_seen.is_some_and(|(previous, _)| same_position(previous, observed)) {
                    *last_seen = Some((observed, now));
                }
                if let Some(saved_anchor) = settle_anchor {
                    *self = Self::Settled {
                        saved_anchor,
                        observed_anchor: observed,
                        egui_frame_seen: native_frame.is_none() && frame.is_some(),
                        unknown_first_frame: false,
                        candidate: None,
                    };
                    return (changed, command);
                }
                (changed, command)
            }
            Self::Unpositioned { since, last_seen } => {
                if now.duration_since(*since) >= Duration::from_millis(500)
                    && last_seen.is_some_and(|(previous, first)| {
                        same_position(previous, observed)
                            && now.duration_since(first) >= Duration::from_millis(100)
                    })
                {
                    *self = Self::Settled {
                        saved_anchor: frame.unwrap_or(observed),
                        observed_anchor: observed,
                        egui_frame_seen: native_frame.is_none() && frame.is_some(),
                        // Only the X11 path has a separate native frame lookup.
                        // Keep other platforms' existing equal-rect behavior.
                        unknown_first_frame: cfg!(target_os = "linux") && frame.is_none(),
                        candidate: None,
                    };
                } else if !last_seen.is_some_and(|(previous, _)| same_position(previous, observed))
                {
                    *last_seen = Some((observed, now));
                }
                (changed, None)
            }
            Self::Settled {
                saved_anchor,
                observed_anchor,
                egui_frame_seen,
                unknown_first_frame,
                candidate,
            } => {
                if *unknown_first_frame {
                    if let Some(frame_origin) = frame {
                        let moved = !same_position(*observed_anchor, observed);
                        if !moved {
                            *saved_anchor = frame_origin;
                        }
                        *observed_anchor = observed;
                        *egui_frame_seen = native_frame.is_none();
                        *unknown_first_frame = false;
                        *candidate = moved.then_some(PositionCandidate {
                            frame: frame_origin,
                            client: observed,
                            since: now,
                        });
                    }
                    // The initial client coordinate is not a frame coordinate.
                    // Keep size and other preferences independent while it is unresolved.
                    return (changed, None);
                }
                if frame.is_some() {
                    *egui_frame_seen = native_frame.is_none();
                }
                if cfg!(target_os = "linux") && *egui_frame_seen && frame.is_none() {
                    // A distinct egui frame observed earlier does not make a
                    // later equal client rectangle a frame. Wait for a fresh
                    // egui or X11 frame before committing another position.
                    *candidate = None;
                    return (changed, None);
                }
                let corrected = if let Some(frame_origin) = frame {
                    frame_origin
                } else if *egui_frame_seen {
                    outer.min
                } else {
                    // A later equal-rect callback can still be a client origin,
                    // even after one trustworthy frame observation.
                    *saved_anchor + (observed - *observed_anchor)
                };
                if same_position(*observed_anchor, observed)
                    && same_position(*saved_anchor, corrected)
                {
                    *candidate = None;
                    return (changed, None);
                }
                if let Some(previous) = candidate
                    && same_position(previous.frame, corrected)
                    && now.duration_since(previous.since) >= Duration::from_millis(200)
                {
                    *saved_anchor = corrected;
                    *observed_anchor = observed;
                    *candidate = None;
                    let position_changed =
                        saved.x != Some(corrected.x) || saved.y != Some(corrected.y);
                    saved.x = Some(corrected.x);
                    saved.y = Some(corrected.y);
                    (changed || position_changed, None)
                } else {
                    if !candidate
                        .as_ref()
                        .is_some_and(|previous| same_position(previous.frame, corrected))
                    {
                        *candidate = Some(PositionCandidate {
                            frame: corrected,
                            client: observed,
                            since: now,
                        });
                    }
                    (changed, None)
                }
            }
        }
    }

    /// Hiding ends the stream of trustworthy mapped observations. Finish its
    /// last pending movement without reading the newly unmapped client bounds.
    fn finish_pending_capture(&mut self, saved: &mut WindowPreferences) -> bool {
        let Self::Settled {
            saved_anchor,
            observed_anchor,
            candidate,
            ..
        } = self
        else {
            return false;
        };
        let Some(position) = candidate.take() else {
            return false;
        };
        *saved_anchor = position.frame;
        *observed_anchor = position.client;
        let changed = saved.x != Some(position.frame.x) || saved.y != Some(position.frame.y);
        saved.x = Some(position.frame.x);
        saved.y = Some(position.frame.y);
        changed
    }

    fn needs_repaint(&self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Requested { .. } | Self::Unpositioned { .. }
        ) || matches!(
            self,
            Self::Settled {
                candidate: Some(_),
                ..
            }
        )
    }
}

fn same_position(a: Pos2, b: Pos2) -> bool {
    (a.x - b.x).abs() <= 1.0 && (a.y - b.y).abs() <= 1.0
}

fn valid_geometry_zoom(zoom: f32) -> Option<f32> {
    (zoom.is_finite() && zoom > 0.0).then_some(zoom)
}

fn geometry_zoom(ctx: &Context) -> f32 {
    valid_geometry_zoom(ctx.zoom_factor()).unwrap_or(1.0)
}

/// The current viewport's native display scale, without UI zoom.
fn native_scale(ctx: &Context) -> f32 {
    ctx.input(|i| i.viewport().native_pixels_per_point)
        .and_then(valid_geometry_zoom)
        .unwrap_or(1.0)
}

/// Saved positions are physical pixels divided by their `position_scale`.
/// Express them at `scale`, the board's native scale, which its work areas
/// and position commands use. Positions without a scale predate it and were
/// saved at the board's scale.
fn position_at_native_scale(window: &mut WindowPreferences, scale: f32) {
    if window.position_scale == Some(scale) || (window.x.is_none() && window.y.is_none()) {
        return;
    }
    if let Some(saved) = window.position_scale.and_then(valid_geometry_zoom) {
        let factor = saved / scale;
        window.x = window.x.map(|x| x * factor);
        window.y = window.y.map(|y| y * factor);
    }
    window.position_scale = Some(scale);
}

fn viewport_has_normal_geometry(ctx: &Context) -> bool {
    ctx.input(|i| {
        let viewport = i.viewport();
        viewport.maximized != Some(true) && viewport.minimized != Some(true)
    })
}

fn durable_rect(rect: egui::Rect, zoom: f32) -> egui::Rect {
    egui::Rect::from_min_max(rect.min * zoom, rect.max * zoom)
}

fn viewport_position(position: Pos2, zoom: f32) -> Pos2 {
    position / zoom
}

fn viewport_size(width: f32, height: f32, zoom: f32) -> egui::Vec2 {
    egui::vec2(width / zoom, height / zoom)
}

fn capture_size(saved: &mut WindowPreferences, inner: egui::Rect) -> bool {
    let changed =
        (saved.width - inner.width()).abs() > 1.0 || (saved.height - inner.height()).abs() > 1.0;
    if changed {
        saved.width = inner.width();
        saved.height = inner.height();
    }
    changed
}

#[derive(Default)]
struct BoardAction {
    about: bool,
    refresh: bool,
    changed: bool,
    compact_changed: bool,
    order: Option<Vec<String>>,
    hidden: Option<std::collections::BTreeSet<String>>,
    primary: Option<Option<String>>,
    refresh_secs: Option<u64>,
    quota_display_mode: Option<QuotaDisplayMode>,
    columns: Option<ColumnLayout>,
    theme_preference: Option<ThemePreference>,
    author_youtube_visited: bool,
    updates_enabled: Option<bool>,
}

#[derive(Clone)]
struct DragSlot {
    id: String,
    rect: egui::Rect,
    available: bool,
}

#[derive(Clone)]
struct CardDragPreview {
    source: String,
    available: bool,
    grab_offset: egui::Vec2,
    slots: Vec<DragSlot>,
    target: Option<String>,
}

impl CardDragPreview {
    fn target_at(&self, pointer: Pos2) -> Option<String> {
        self.slots
            .iter()
            .find(|slot| slot.available == self.available && slot.rect.contains(pointer))
            .map(|slot| slot.id.clone())
    }
}

fn move_card_to_target<'a>(
    cards: &[&'a ProviderCard],
    source: &str,
    target: &str,
) -> Vec<&'a ProviderCard> {
    let mut sorted = cards.to_vec();
    if let (Some(from), Some(to)) = (
        sorted.iter().position(|card| card.id == source),
        sorted.iter().position(|card| card.id == target),
    ) {
        let card = sorted.remove(from);
        sorted.insert(to, card);
    }
    sorted
}

fn moved_order(cards: &[ProviderCard], source: &str, target: &str) -> Option<Vec<String>> {
    let mut order = cards.iter().map(|card| card.id.clone()).collect::<Vec<_>>();
    let (Some(from), Some(to)) = (
        order.iter().position(|id| id == source),
        order.iter().position(|id| id == target),
    ) else {
        return None;
    };
    let id = order.remove(from);
    order.insert(to, id);
    Some(order)
}

pub struct AshtrayApp {
    state: Arc<AppState>,
    poller: Option<Arc<Poller>>,
    // Only a standalone primary board has one; a UI child shows the host's.
    updater: Option<Arc<Updater>>,
    tray: Option<platform::Tray>,
    // Open-board requests from later launches while the board runs standalone.
    activations: Option<Receiver<Request>>,
    connection: Option<UiConnection>,
    preferences_sender: Option<SyncSender<ChildMessage>>,
    tray_snapshot: SharedTraySnapshot,
    config: ConfigStore,
    board_preferences: Arc<RwLock<BoardPreferences>>,
    appearance: theme::Appearance,
    author_video: author_youtube::LatestVideo,
    preferences_save_pending: bool,
    root: WindowPreferences,
    root_restore: PositionRestore,
    main_visible: bool,
    about: bool,
    message: Option<String>,
    startup_visibility_applied: bool,
    geometry_last_change: Option<Instant>,
    monitor_rects: Vec<platform::MonitorRect>,
    remote: bool,
    received_first_action: bool,
    closed_message_sent: bool,
    refresh_feedback: refresh_feedback::Feedback,
}

fn save_failure_message(error: &dyn std::fmt::Display) -> String {
    format!("Could not save board preferences: {error}")
}

fn application_icon() -> Arc<IconData> {
    static ICON: OnceLock<Arc<IconData>> = OnceLock::new();
    Arc::clone(ICON.get_or_init(|| {
        Arc::new(
            eframe::icon_data::from_png_bytes(include_bytes!(
                "../assets/application/token-ashtray-64.png"
            ))
            .expect("embedded application icon is a valid PNG"),
        )
    }))
}

pub fn initial_viewport(start_minimized: bool) -> ViewportBuilder {
    let config = ConfigStore::load();
    let root = &config.preferences().window;
    board_viewport_builder(
        root,
        platform::pin_supported(),
        restore_position(root, &[]),
        1.0,
    )
    .with_visible(!start_minimized)
}

fn board_viewport_builder(
    initial: &WindowPreferences,
    pin_supported: bool,
    position: Option<Pos2>,
    zoom: f32,
) -> ViewportBuilder {
    let zoom = valid_geometry_zoom(zoom).unwrap_or(1.0);
    let mut builder = ViewportBuilder::default()
        .with_title("Token Ashtray")
        .with_icon(application_icon())
        .with_inner_size(viewport_size(initial.width, initial.height, zoom))
        .with_min_inner_size(viewport_size(
            initial.width.min(360.0),
            initial.height.min(360.0),
            zoom,
        ))
        .with_decorations(!initial.compact);
    if initial.pinned && pin_supported {
        builder = builder.with_always_on_top();
    }
    if let Some(position) = position {
        builder = builder.with_position(viewport_position(position, zoom));
    }
    builder
}

#[cfg(test)]
fn fitted_board_viewport_builder(
    initial: &WindowPreferences,
    areas: &[platform::MonitorRect],
    pin: bool,
    placement: bool,
) -> ViewportBuilder {
    let fitted = fitted_preferences(initial, areas, egui::Vec2::ZERO);
    let position = placement
        .then(|| fit_geometry(&fitted, areas, egui::Vec2::ZERO).map(|fit| fit.1))
        .flatten();
    board_viewport_builder(&fitted, pin, position, 1.0)
}

fn native_frame_origin(ctx: &Context, title: &str, inner: egui::Rect) -> Option<Pos2> {
    #[cfg(target_os = "linux")]
    {
        if platform::placement_supported() {
            return platform::verified_frame_origin(title, inner, ctx.pixels_per_point());
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (ctx, title, inner);
    None
}

fn displayed_pin(pinned: bool, pin_supported: bool) -> bool {
    pinned && pin_supported
}

impl AshtrayApp {
    pub fn new(
        creation: &eframe::CreationContext<'_>,
        use_tray: bool,
        start_minimized: bool,
        startup_notice: Option<String>,
    ) -> Result<Self, String> {
        Self::new_with_poller(
            creation,
            use_tray,
            start_minimized,
            startup_notice,
            ConfigStore::load(),
            Poller::start,
        )
    }

    // The native probe supplies synthetic providers while using this same UI path.
    pub(crate) fn new_with_poller(
        creation: &eframe::CreationContext<'_>,
        use_tray: bool,
        start_minimized: bool,
        startup_notice: Option<String>,
        config: ConfigStore,
        start_poller: impl FnOnce(
            SharedBoard,
            PollSettings,
            Arc<dyn Fn() + Send + Sync>,
        ) -> Result<Poller, String>,
    ) -> Result<Self, String> {
        configure_visuals(&creation.egui_ctx);
        let root = config.preferences().window.clone();
        let state = Arc::new(AppState::local());
        let theme_state = Arc::clone(&state);
        let theme_context = creation.egui_ctx.clone();
        let appearance =
            theme::Appearance::start(move || theme_state.request_repaint(&theme_context));
        appearance.apply(
            &creation.egui_ctx,
            config.preferences().board.theme_preference,
        );
        let repaint_state = Arc::clone(&state);
        let repaint_context = creation.egui_ctx.clone();
        let poller = start_poller(
            Arc::clone(&state.board),
            poll_settings(&config),
            Arc::new(move || repaint_state.request_repaint(&repaint_context)),
        )?;
        let tray_snapshot = Arc::new(RwLock::new(TraySnapshot::default()));
        let mut message = startup_notice;
        let tray = if use_tray {
            match platform::Tray::new(creation.egui_ctx.clone(), Arc::clone(&tray_snapshot)) {
                Ok(tray) => Some(tray),
                Err(error) => {
                    eprintln!("Token Ashtray: {error}; opening the board without a tray icon.");
                    message = Some(TRAY_UNAVAILABLE_NOTICE.to_owned());
                    None
                }
            }
        } else {
            None
        };
        let main_visible = !start_minimized || !use_tray || tray.is_none();
        Ok(Self {
            state,
            poller: Some(Arc::new(poller)),
            updater: None,
            tray,
            activations: None,
            connection: None,
            preferences_sender: None,
            tray_snapshot,
            board_preferences: Arc::new(RwLock::new(config.preferences().board.clone())),
            appearance,
            author_video: author_youtube::LatestVideo::default(),
            preferences_save_pending: false,
            config,
            root,
            root_restore: PositionRestore::default(),
            main_visible,
            about: false,
            message,
            startup_visibility_applied: false,
            geometry_last_change: None,
            monitor_rects: Vec::new(),
            remote: false,
            received_first_action: true,
            closed_message_sent: false,
            refresh_feedback: refresh_feedback::Feedback::default(),
        })
    }

    /// Lets later launches bring this standalone board forward.
    pub fn with_activation(mut self, listener: Option<Listener>, ctx: &Context) -> Self {
        let ctx = ctx.clone();
        self.activations = listener.map(|listener| listener.spawn(move || ctx.request_repaint()));
        self
    }

    /// Gives a standalone primary board the instance's updater.
    pub fn with_updater(mut self, updater: Option<Arc<Updater>>) -> Self {
        self.updater = updater;
        self
    }

    fn update_status(&self) -> UpdateStatus {
        if let Some(updater) = &self.updater {
            updater.status()
        } else if let Some(connection) = &self.connection {
            connection
                .update_status
                .read()
                .expect("update status lock")
                .clone()
        } else {
            UpdateStatus::Off
        }
    }

    pub fn new_remote(
        creation: &eframe::CreationContext<'_>,
        connection: UiConnection,
    ) -> Result<Self, String> {
        Self::new_remote_with_config(creation, connection, ConfigStore::load())
    }
    pub(crate) fn new_remote_with_config(
        creation: &eframe::CreationContext<'_>,
        connection: UiConnection,
        config: ConfigStore,
    ) -> Result<Self, String> {
        configure_visuals(&creation.egui_ctx);
        let root = config.preferences().window.clone();
        let state = Arc::new(AppState::remote(Arc::clone(&connection.board)));
        let theme_state = Arc::clone(&state);
        let theme_context = creation.egui_ctx.clone();
        let appearance =
            theme::Appearance::start(move || theme_state.request_repaint(&theme_context));
        appearance.apply(
            &creation.egui_ctx,
            config.preferences().board.theme_preference,
        );
        let wake_state = Arc::clone(&state);
        let wake_context = creation.egui_ctx.clone();
        connection.set_wake(Arc::new(move || wake_state.request_repaint(&wake_context)));
        Ok(Self {
            state,
            poller: None,
            updater: None,
            tray: None,
            activations: None,
            preferences_sender: Some(connection.sender()),
            connection: Some(connection),
            tray_snapshot: Arc::new(RwLock::new(TraySnapshot::default())),
            board_preferences: Arc::new(RwLock::new(config.preferences().board.clone())),
            appearance,
            author_video: author_youtube::LatestVideo::default(),
            preferences_save_pending: false,
            config,
            root,
            root_restore: PositionRestore::default(),
            main_visible: false,
            about: false,
            message: None,
            startup_visibility_applied: false,
            geometry_last_change: None,
            monitor_rects: Vec::new(),
            remote: true,
            received_first_action: false,
            closed_message_sent: false,
            refresh_feedback: refresh_feedback::Feedback::default(),
        })
    }

    fn persist(&mut self) {
        self.config.preferences_mut().board = self
            .board_preferences
            .read()
            .expect("board preferences lock")
            .clone();
        self.config.preferences_mut().window = self.root.clone();
        if let Some(sender) = &self.preferences_sender {
            self.preferences_save_pending = match sender.try_send(ChildMessage::SavePreferences(
                self.config.preferences().clone(),
            )) {
                Ok(()) => false,
                Err(std::sync::mpsc::TrySendError::Full(_)) => true,
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => false,
            };
        } else if let Err(error) = self.config.save() {
            self.message = Some(save_failure_message(&error));
        }
    }
    fn apply(&mut self, action: BoardAction) {
        let next = {
            let mut preferences = self
                .board_preferences
                .write()
                .expect("board preferences lock");
            apply_board_preferences(&mut preferences, &action, |next| {
                if let Some(poller) = &self.poller {
                    poller.set_settings(poll_settings_from_preferences(next));
                }
            });
            preferences.clone()
        };
        if let (Some(enabled), Some(updater)) = (action.updates_enabled, &self.updater) {
            updater.set_enabled(enabled);
        }
        self.config.preferences_mut().board = next;
        if action.refresh {
            self.request_refresh();
        }
        if action.changed {
            self.persist();
        }
    }
    pub(crate) fn request_refresh(&mut self) {
        let now = Instant::now();
        let Some(request) = self.refresh_feedback.request(now) else {
            return;
        };
        if let Some(connection) = &self.connection {
            if let Err(error) = connection.try_send(ChildMessage::Refresh { request }) {
                self.refresh_feedback.accepted(request, Err(error), now);
            }
        } else {
            let result = self
                .poller
                .as_ref()
                .ok_or_else(|| "Refresh unavailable: polling is disconnected".into())
                .and_then(|poller| poller.refresh());
            self.refresh_feedback.accepted(request, result, now);
        }
    }
    fn update_refresh_feedback(&mut self, ctx: &Context) {
        let now = Instant::now();
        let (snapshot, suppressed) = if let Some(connection) = &self.connection {
            while let Some((request, result)) = connection.refresh_reply() {
                self.refresh_feedback.accepted(request, result, now);
            }
            if connection.disconnected() {
                self.refresh_feedback
                    .fail("Refresh unavailable: tray connection lost".into(), now);
            }
            (
                connection
                    .feedback
                    .read()
                    .expect("refresh feedback lock")
                    .clone(),
                connection
                    .suppressed_through
                    .load(std::sync::atomic::Ordering::Acquire),
            )
        } else {
            (
                self.poller
                    .as_ref()
                    .map(|poller| poller.feedback())
                    .unwrap_or_default(),
                0,
            )
        };
        let presentation = self.refresh_feedback.update(snapshot, suppressed, now);
        if presentation.toolbar.is_some() || !presentation.cards.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        refresh_feedback::install(ctx, presentation);
    }
    fn update_tray_snapshot(&self, cards: &[ProviderCard]) {
        let preferences = self
            .board_preferences
            .read()
            .expect("board preferences lock")
            .clone();
        if let Ok(mut snapshot) = self.tray_snapshot.write() {
            *snapshot = local_tray_snapshot(cards, &preferences);
        }
        if let Some(tray) = &self.tray {
            tray.sync();
        }
    }
    fn handle_action(&mut self, ctx: &Context) {
        if let Some(error) = self
            .connection
            .as_ref()
            .and_then(|connection| connection.save_failure.lock().ok()?.take())
        {
            self.message = Some(save_failure_message(&error));
        }
        let action = if let Some(connection) = &self.connection {
            connection.action().unwrap_or(TrayAction::None)
        } else if let Some(request) = self
            .activations
            .as_ref()
            .and_then(|activations| activations.try_recv().ok())
        {
            // Acknowledged from the UI thread, so a closed board cannot accept it.
            request.acknowledge();
            TrayAction::OpenBoard
        } else {
            self.tray
                .as_ref()
                .map(platform::Tray::poll)
                .unwrap_or(TrayAction::None)
        };
        if action != TrayAction::None && self.remote {
            self.received_first_action = true;
        }
        match action {
            TrayAction::None => {}
            TrayAction::Refresh => {
                self.request_refresh();
            }
            TrayAction::OpenBoard => self.show_main(ctx),
            TrayAction::ToggleAutostart => {
                match platform::set_autostart(!platform::autostart_enabled()) {
                    Ok(()) => {
                        if let Some(tray) = &self.tray {
                            tray.sync();
                        }
                    }
                    Err(error) => {
                        self.message = Some(format!("Could not change autostart: {error}"))
                    }
                }
            }
            TrayAction::About => {
                self.show_main(ctx);
                self.about = true;
            }
            TrayAction::Quit => {
                self.send_closed(true);
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    pub(crate) fn send_closed(&mut self, quit: bool) {
        if self.remote && !self.closed_message_sent {
            self.persist();
            if let Some(connection) = &self.connection {
                connection.send(ChildMessage::Closed { quit });
            }
            self.closed_message_sent = true;
        }
    }
    fn show_main(&mut self, ctx: &Context) {
        // A window manager may place a remapped root as a new window. Restore
        // its saved frame and settle it before accepting new geometry reports.
        // Refresh before the hidden-to-visible restore: tray activation can
        // arrive before update refreshes the cached monitor list.
        let current_areas = viewport_work_areas(ctx);
        if !current_areas.is_empty() {
            self.monitor_rects = current_areas;
        }
        let restore_normal = !self.main_visible && viewport_has_normal_geometry(ctx);
        if restore_normal {
            let mut remap_restore = PositionRestore::Pending;
            if fit_live_viewport(ctx, &mut self.root, &mut remap_restore, "Token Ashtray") {
                if !matches!(remap_restore, PositionRestore::Pending) {
                    self.root_restore = remap_restore;
                }
                self.geometry_last_change = Some(Instant::now());
            }
            if let Some(target) = restore_position(&self.root, &self.monitor_rects) {
                self.root_restore = PositionRestore::Requested {
                    target,
                    requested_at: Instant::now(),
                    last_seen: None,
                    retries: 0,
                };
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(viewport_position(
                    target,
                    geometry_zoom(ctx),
                )));
            }
        }
        self.main_visible = true;
        show_root_viewport(ctx, self.root.pinned, platform::pin_supported());
        self.root_restore.prepare_show(Instant::now());
        // Windows and X11 cannot focus an iconic window. Windows winit leaves a
        // nonminimized window unchanged, X11 activates it, and Wayland ignores it.
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }
    fn hide_main(&mut self, ctx: &Context) {
        // Capture while the frame is still mapped, then finish its last move
        // before a hidden root can quit or receive unmap geometry callbacks.
        let changed = self.capture_root_geometry(ctx);
        self.main_visible = false;
        if self.root_restore.finish_pending_capture(&mut self.root) || changed {
            self.geometry_last_change = Some(Instant::now());
            ctx.request_repaint_after(Duration::from_millis(600));
        }
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }
    fn capture_root_geometry(&mut self, ctx: &Context) -> bool {
        // Unmapping can change client coordinates without a user move; those
        // reports must not become durable window geometry.
        if !self.main_visible {
            return self.root_restore.finish_pending_capture(&mut self.root);
        }
        self.root_restore
            .capture_viewport(ctx, &mut self.root, "Token Ashtray", Instant::now())
    }
}

impl eframe::App for AshtrayApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.update_refresh_feedback(ctx);
        if self.preferences_save_pending {
            self.persist();
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.author_video.open_ready(ctx);
        self.handle_action(ctx);
        if self.remote
            && !self.closed_message_sent
            && ctx.input(|input| input.viewport().close_requested())
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide_main(ctx);
        }
        self.monitor_rects = viewport_work_areas(ctx);
        position_at_native_scale(&mut self.root, native_scale(ctx));
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let scale = native_scale(ctx);
            self.monitor_rects = platform::monitor_rects()
                .into_iter()
                .map(|a| platform::MonitorRect {
                    x: a.x / scale,
                    y: a.y / scale,
                    width: a.width / scale,
                    height: a.height / scale,
                })
                .collect();
        }
        let fitted = self.main_visible
            && fit_live_viewport(ctx, &mut self.root, &mut self.root_restore, "Token Ashtray");
        if fitted {
            self.geometry_last_change = Some(Instant::now());
        }
        if !self.startup_visibility_applied {
            self.startup_visibility_applied = true;
            ctx.send_viewport_cmd(ViewportCommand::Visible(self.main_visible));
            if viewport_has_normal_geometry(ctx) {
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(viewport_size(
                    self.root.width,
                    self.root.height,
                    geometry_zoom(ctx),
                )));
            }
            ctx.send_viewport_cmd(ViewportCommand::Decorations(!self.root.compact));
            if platform::pin_supported() {
                set_pin(ctx, self.root.pinned);
            }
        }
        let (root_position, first_request) = if viewport_has_normal_geometry(ctx) {
            self.root_restore.requested_position(
                &self.root,
                &self.monitor_rects,
                platform::placement_supported(),
                Instant::now(),
            )
        } else {
            (None, false)
        };
        if first_request && let Some(position) = root_position {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(viewport_position(
                position,
                geometry_zoom(ctx),
            )));
        }
        // This input still describes the frame before queued corrections.
        if !fitted && self.capture_root_geometry(ctx) {
            self.geometry_last_change = Some(Instant::now());
            ctx.request_repaint_after(Duration::from_millis(600));
        }
        if self.main_visible && self.root_restore.needs_repaint() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.main_visible {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        let board_preferences = self
            .board_preferences
            .read()
            .expect("board preferences lock")
            .clone();
        self.appearance
            .apply(ctx, board_preferences.theme_preference);
        let cards = ordered_cards(&self.state.board, &board_preferences.order);
        self.update_tray_snapshot(&cards);
        let before = self.root.pinned;
        let update_status = self.update_status();
        let mut action = egui::CentralPanel::default()
            .show(ctx, |ui| {
                if !self.main_visible {
                    ui.set_invisible();
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        draw_board_with_updates(
                            ui,
                            "Token Ashtray",
                            &cards,
                            &board_preferences,
                            &update_status,
                            platform::pin_supported(),
                            &mut self.root.pinned,
                            &mut self.root.compact,
                        )
                    })
                    .inner
            })
            .inner;
        if action.author_youtube_visited {
            action.changed |= !board_preferences.author_youtube_visited;
            self.author_video.activate(ctx);
        }
        if before != self.root.pinned && platform::pin_supported() {
            set_pin(ctx, self.root.pinned);
            action_changed_repaint(ctx);
        }
        if action.compact_changed {
            ctx.send_viewport_cmd(ViewportCommand::Decorations(!self.root.compact));
        }
        let changed = action.changed || action.refresh;
        let tray_about = std::mem::take(&mut self.about);
        let open_about = action.about || tray_about;
        self.apply(action);
        if changed {
            self.state.request_repaint(ctx);
        }
        about::show(ctx, open_about);
        if let Some(message) = &self.message {
            let mut open = true;
            egui::Window::new("Token Ashtray")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(message);
                });
            if !open {
                self.message = None;
            }
        }
        if self
            .geometry_last_change
            .is_some_and(|changed| changed.elapsed() >= Duration::from_millis(500))
        {
            self.persist();
            self.geometry_last_change = None;
        }
        if self.remote && self.received_first_action && !self.main_visible {
            self.send_closed(false);
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

fn show_root_viewport(ctx: &Context, pinned: bool, pin_supported: bool) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    if pin_supported {
        set_pin(ctx, pinned);
    }
}

// Positions describe the decorated outer origin; sizes describe the client.
fn action_changed_repaint(ctx: &Context) {
    ctx.request_repaint();
}
fn set_pin(ctx: &Context, enabled: bool) {
    ctx.send_viewport_cmd(ViewportCommand::WindowLevel(if enabled {
        WindowLevel::AlwaysOnTop
    } else {
        WindowLevel::Normal
    }));
}
fn poll_settings(config: &ConfigStore) -> PollSettings {
    poll_settings_from_preferences(&config.preferences().board)
}
fn poll_settings_from_preferences(preferences: &BoardPreferences) -> PollSettings {
    PollSettings {
        interval_secs: preferences.refresh_secs.max(60),
        hidden: preferences.hidden.iter().cloned().collect(),
    }
}
fn apply_board_preferences(
    preferences: &mut crate::config::BoardPreferences,
    action: &BoardAction,
    update_polling: impl FnOnce(&BoardPreferences),
) {
    let mut next = preferences.clone();
    if let Some(order) = &action.order {
        next.order = order.clone();
    }
    if let Some(hidden) = &action.hidden {
        next.hidden = hidden.clone();
    }
    if let Some(primary) = &action.primary {
        next.primary = primary.clone();
    }
    if let Some(secs) = action.refresh_secs {
        next.refresh_secs = secs.max(60);
    }
    if let Some(mode) = action.quota_display_mode {
        next.quota_display_mode = mode;
    }
    if let Some(columns) = action.columns {
        next.columns = columns;
    }
    if let Some(theme) = action.theme_preference {
        next.theme_preference = theme;
    }
    next.author_youtube_visited |= action.author_youtube_visited;
    if let Some(enabled) = action.updates_enabled {
        next.updates_enabled = enabled;
    }
    preferences.replace(next, update_polling);
}
fn local_tray_snapshot(cards: &[ProviderCard], prefs: &BoardPreferences) -> TraySnapshot {
    let used = |card: &ProviderCard| {
        (!prefs.hidden.contains(&card.id)
            && card.effective_status(prefs.refresh_secs.max(60)) == Status::Live)
            .then(|| card.used())
            .flatten()
    };
    let primary = prefs
        .primary
        .as_deref()
        .and_then(|id| cards.iter().find(|card| card.id == id))
        .or_else(|| cards.iter().find(|card| used(card).is_some()));
    TraySnapshot {
        providers: cards
            .iter()
            .map(|card| TrayProvider {
                name: card.short_name.clone(),
                used: used(card),
                reset: card
                    .tightest_meter()
                    .and_then(|meter| meter.resets_at)
                    .map(reset_label),
            })
            .collect(),
        primary_remaining: primary.and_then(used).map(|used| 100.0 - used),
        any_readable: cards.iter().any(|card| used(card).is_some()),
        quota_display_mode: prefs.quota_display_mode,
    }
}

fn ordered_cards(board: &SharedBoard, order: &[String]) -> Vec<ProviderCard> {
    let cards: BTreeMap<_, _> = board.read().expect("board lock").clone();
    let mut values: Vec<_> = cards.into_values().collect();
    values.sort_by_key(|card| {
        order
            .iter()
            .position(|id| id == &card.id)
            .unwrap_or(usize::MAX)
    });
    values
}
#[cfg(test)]
thread_local! {
    static QUOTA_TEST_NOW: std::cell::Cell<Option<OffsetDateTime>> = const { std::cell::Cell::new(None) };
}
fn quota_now() -> OffsetDateTime {
    #[cfg(test)]
    if let Some(now) = QUOTA_TEST_NOW.with(std::cell::Cell::get) {
        return now;
    }
    OffsetDateTime::now_utc()
}
fn reset_label(time: OffsetDateTime) -> String {
    reset_countdown_label(time, quota_now())
}
fn restore_position(
    window: &WindowPreferences,
    monitors: &[platform::MonitorRect],
) -> Option<Pos2> {
    window.x.zip(window.y)?;
    fit_geometry(window, monitors, egui::Vec2::ZERO).map(|fit| fit.1)
}

/// Inputs and output are native-DPI logical pixels, independent of UI zoom.
/// Monitor rectangles must be usable areas, and decorations are a client budget.
fn fit_geometry(
    window: &WindowPreferences,
    areas: &[platform::MonitorRect],
    decorations: egui::Vec2,
) -> Option<(egui::Vec2, Pos2)> {
    let areas: Vec<_> = areas
        .iter()
        .copied()
        .filter(|a| {
            a.x.is_finite()
                && a.y.is_finite()
                && a.width > decorations.x
                && a.height > decorations.y
        })
        .collect();
    let primary = *areas.first()?;
    let saved = window.x.zip(window.y).map(|(x, y)| Pos2::new(x, y));
    let desired = egui::vec2(window.width, window.height);
    let area = saved
        .and_then(|pos| {
            areas
                .iter()
                .copied()
                .filter(|a| intersection_area(*a, pos, desired) > 0.0)
                .max_by(|a, b| {
                    intersection_area(*a, pos, desired)
                        .total_cmp(&intersection_area(*b, pos, desired))
                })
        })
        .unwrap_or(primary);
    let size = desired
        .min(egui::vec2(area.width, area.height) - decorations)
        .max(egui::vec2(1.0, 1.0));
    let pos = saved
        .filter(|pos| intersection_area(area, *pos, desired) > 0.0)
        .unwrap_or(Pos2::new(area.x + 32.0, area.y + 32.0));
    Some((
        size,
        Pos2::new(
            pos.x.clamp(
                area.x,
                (area.x + area.width - size.x - decorations.x).max(area.x),
            ),
            pos.y.clamp(
                area.y,
                (area.y + area.height - size.y - decorations.y).max(area.y),
            ),
        ),
    ))
}

#[cfg(test)]
fn fitted_preferences(
    window: &WindowPreferences,
    areas: &[platform::MonitorRect],
    decorations: egui::Vec2,
) -> WindowPreferences {
    let mut fitted = window.clone();
    if let Some((size, pos)) = fit_geometry(window, areas, decorations) {
        fitted.width = size.x;
        fitted.height = size.y;
        fitted.x = Some(pos.x);
        fitted.y = Some(pos.y);
    }
    fitted
}

fn viewport_work_areas(ctx: &Context) -> Vec<platform::MonitorRect> {
    viewport_work_areas_from_physical(ctx, platform::work_areas())
}

fn viewport_work_areas_from_physical(
    ctx: &Context,
    areas: Vec<platform::MonitorRect>,
) -> Vec<platform::MonitorRect> {
    // Keep every signed virtual-desktop origin in physical pixels until this
    // viewport's transform, including native DPI and the current zoom.
    let scale = native_scale(ctx);
    areas
        .into_iter()
        .map(|a| platform::MonitorRect {
            x: a.x / scale,
            y: a.y / scale,
            width: a.width / scale,
            height: a.height / scale,
        })
        .collect()
}

/// Fit startup/remap geometry and recover inaccessible normal frames. Once a
/// reachable frame settles, native movement, resizing and Snap own its bounds.
fn fit_live_viewport(
    ctx: &Context,
    window: &mut WindowPreferences,
    restore: &mut PositionRestore,
    title: &str,
) -> bool {
    fit_live_viewport_in_work_areas(ctx, window, restore, title, platform::work_areas())
}

fn fit_live_viewport_in_work_areas(
    ctx: &Context,
    window: &mut WindowPreferences,
    restore: &mut PositionRestore,
    title: &str,
    physical_areas: Vec<platform::MonitorRect>,
) -> bool {
    position_at_native_scale(window, native_scale(ctx));
    let areas = viewport_work_areas_from_physical(ctx, physical_areas);
    let decorations = if window.compact {
        egui::Vec2::ZERO
    } else {
        viewport_frame_size(ctx, title)
    };
    fit_viewport_in_area(
        ctx,
        window,
        restore,
        &areas,
        decorations,
        platform::placement_supported(),
    )
}

fn viewport_frame_size(ctx: &Context, title: &str) -> egui::Vec2 {
    #[cfg(target_os = "windows")]
    {
        let _ = title;
        // egui-winit supplies this HWND's GetWindowRect/GetClientRect sizes
        // divided by native DPI * UI zoom. Convert only the zoom back here:
        // the work areas and persisted client sizes are native-DPI logical.
        // Re-read on every mapped frame so late decoration/DPI changes fit too.
        ctx.input(|i| {
            let viewport = i.viewport();
            viewport.outer_rect.zip(viewport.inner_rect)
        })
        .map(|(outer, inner)| (outer.size() - inner.size()) * geometry_zoom(ctx))
        .filter(|size| size.is_finite() && size.x >= 0.0 && size.y >= 0.0)
        .unwrap_or_default()
    }
    #[cfg(not(target_os = "windows"))]
    {
        platform::frame_size(title).unwrap_or_default() / native_scale(ctx)
    }
}

fn fit_viewport_in_area(
    ctx: &Context,
    window: &mut WindowPreferences,
    restore: &mut PositionRestore,
    areas: &[platform::MonitorRect],
    decorations: egui::Vec2,
    placement_supported: bool,
) -> bool {
    if !viewport_has_normal_geometry(ctx) {
        return false;
    }
    let zoom = geometry_zoom(ctx);
    let observed = ctx.input(|i| i.viewport().inner_rect);
    let mut source = window.clone();
    if let Some([width, height]) = window.preferred_size {
        (source.width, source.height) = (width, height);
    }
    let recovering = matches!(restore, PositionRestore::Settled { .. });
    if recovering {
        let geometry = ctx.input(|i| i.viewport().outer_rect.zip(i.viewport().inner_rect));
        let Some((outer, inner)) = geometry else {
            return false;
        };
        let outer = durable_rect(outer, zoom);
        let inner = durable_rect(inner, zoom);
        // Any visible part of the draggable top strip remains reachable. Do
        // not require full containment or a minimum exposed width after a drag.
        let top_height = if window.compact {
            32.0
        } else {
            (inner.top() - outer.top()).max(1.0)
        };
        let handle = egui::vec2(outer.width(), top_height.min(outer.height()));
        if areas
            .iter()
            .any(|a| intersection_area(*a, outer.min, handle) > 0.0)
        {
            return false;
        }
        source.x = Some(outer.left());
        source.y = Some(outer.top());
        source.width = inner.width();
        source.height = inner.height();
    }
    let Some((size, position)) = fit_geometry(&source, areas, decorations) else {
        return false;
    };
    let mut desired = source;
    // Keep the size a temporary work area could not fit, not the reduction.
    let reduced = size.x < desired.width - 1.0 || size.y < desired.height - 1.0;
    desired.preferred_size = window
        .preferred_size
        .filter(|_| recovering)
        .or(reduced.then_some([desired.width, desired.height]));
    desired.width = size.x;
    desired.height = size.y;
    desired.x = Some(position.x);
    desired.y = Some(position.y);
    let oversize = observed.is_some_and(|r| {
        r.width() * zoom > desired.width + 1.0 || r.height() * zoom > desired.height + 1.0
    });
    let position_changed = placement_supported && (desired.x != window.x || desired.y != window.y);
    if recovering
        || position_changed
        || matches!(restore, PositionRestore::Pending)
        || desired.width < window.width
        || desired.height < window.height
        // Only a remembered preferred size can fit larger than the window.
        || (matches!(restore, PositionRestore::Requested { .. })
            && (desired.width > window.width + 1.0 || desired.height > window.height + 1.0))
        || (oversize
            && matches!(
                restore,
                PositionRestore::Pending | PositionRestore::Requested { .. }
            ))
    {
        let minimum = viewport_size(desired.width.min(360.0), desired.height.min(360.0), zoom);
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(minimum));
        let saved_position = (window.x, window.y);
        *window = desired;
        if !placement_supported {
            (window.x, window.y) = saved_position;
        }
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(viewport_size(
            window.width,
            window.height,
            zoom,
        )));
        if placement_supported
            && let Some(pos) = window.x.zip(window.y).map(|(x, y)| Pos2::new(x, y))
        {
            *restore = PositionRestore::Requested {
                target: pos,
                requested_at: Instant::now(),
                last_seen: None,
                retries: 0,
            };
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(viewport_position(pos, zoom)));
        }
        return true;
    }
    false
}

fn intersection_area(monitor: platform::MonitorRect, position: Pos2, size: egui::Vec2) -> f32 {
    let width = (position.x + size.x).min(monitor.x + monitor.width) - position.x.max(monitor.x);
    let height = (position.y + size.y).min(monitor.y + monitor.height) - position.y.max(monitor.y);
    width.max(0.0) * height.max(0.0)
}

impl Drop for AshtrayApp {
    fn drop(&mut self) {
        // Deferred viewport callbacks retain Appearance clones and Context.
        // End the worker explicitly before those callback owners are released.
        self.appearance.stop();
        self.persist();
        if self.remote && !self.closed_message_sent {
            if let Some(connection) = &self.connection {
                connection.send(ChildMessage::Closed { quit: false });
            }
            self.closed_message_sent = true;
        }
    }
}

#[cfg(test)]
const SURFACE: egui::Color32 = theme::DARK.surface;
#[cfg(test)]
const SURFACE_RAISED: egui::Color32 = theme::DARK.raised;
#[cfg(test)]
const OUTLINE: egui::Color32 = theme::DARK.outline;
#[cfg(test)]
const TEXT: egui::Color32 = theme::DARK.text;
#[cfg(test)]
const SECONDARY: egui::Color32 = theme::DARK.secondary;
#[cfg(test)]
const ACTION_BLUE: egui::Color32 = theme::DARK.action;

fn compact_identity_font() -> egui::FontId {
    egui::FontId::new(13.0, egui::FontFamily::Name("Compact identity".into()))
}

fn configure_visuals(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Open Sans SemiBold".into(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/OpenSans-SemiBold.ttf")).into(),
    );
    // Keep the default proportional fallback chain for any missing glyphs.
    let mut identity = fonts.families[&egui::FontFamily::Proportional].clone();
    identity.insert(0, "Open Sans SemiBold".into());
    fonts
        .families
        .insert(compact_identity_font().family, identity);
    ctx.set_fonts(fonts);
    theme::configure(ctx);
}

fn show_card<R>(
    ui: &mut egui::Ui,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    Frame::new()
        .fill(palette(ui.ctx()).surface)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 12))
        .show(ui, contents)
}

fn reveal_focused_control(response: &egui::Response) {
    let pending_id = Id::new((
        "settings-focus-reveal",
        response.ctx.viewport_id(),
        response.layer_id.id,
    ));
    if response.gained_focus() {
        response
            .ctx
            .data_mut(|data| data.insert_temp(pending_id, response.id));
    }
    if response.has_focus()
        && response.ctx.data(|data| data.get_temp::<Id>(pending_id)) == Some(response.id)
    {
        if response.interact_rect.contains(response.rect.min)
            && response.interact_rect.contains(response.rect.max)
        {
            response.ctx.data_mut(|data| data.remove::<Id>(pending_id));
        } else {
            response.scroll_to_me(None);
        }
    }
}

fn settings_control(response: &egui::Response, _label: &str) {
    reveal_focused_control(response);
    #[cfg(test)]
    settings_menu_test::record_control(_label, response);
}

/// The board with the update setting showing no updater activity.
#[cfg(test)]
fn draw_board(
    ui: &mut egui::Ui,
    window_title: &str,
    cards: &[ProviderCard],
    prefs: &crate::config::BoardPreferences,
    pin_supported: bool,
    pinned: &mut bool,
    compact: &mut bool,
) -> BoardAction {
    draw_board_with_updates(
        ui,
        window_title,
        cards,
        prefs,
        &UpdateStatus::Off,
        pin_supported,
        pinned,
        compact,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_board_with_updates(
    ui: &mut egui::Ui,
    window_title: &str,
    cards: &[ProviderCard],
    prefs: &crate::config::BoardPreferences,
    update_status: &UpdateStatus,
    pin_supported: bool,
    pinned: &mut bool,
    compact: &mut bool,
) -> BoardAction {
    let mut action = BoardAction::default();
    let shown = cards
        .iter()
        .filter(|card| !prefs.hidden.contains(&card.id))
        .collect::<Vec<_>>();
    let (live_count, attention_count) = board_summary(&shown, prefs);
    let ctx = ui.ctx().clone();
    let layout_id = Id::new(("card-drag-layout", ctx.viewport_id()));
    let preview_id = Id::new(("card-drag-preview", ctx.viewport_id()));
    let previous_slots = ctx.data(|data| data.get_temp::<Vec<DragSlot>>(layout_id));
    let mut preview = ctx.data(|data| data.get_temp::<CardDragPreview>(preview_id));
    let had_preview = preview.is_some();
    let cancelled_id = Id::new(("card-drag-cancelled", ctx.viewport_id()));
    let mut cancelled = ctx
        .data(|data| data.get_temp::<bool>(cancelled_id))
        .unwrap_or(false);
    let released = ctx.input(|input| input.pointer.any_released());
    if had_preview && !egui::DragAndDrop::has_payload_of_type::<String>(&ctx) {
        if !released {
            cancelled = true;
        }
        preview = None;
    }
    if released {
        cancelled = false;
    }
    let active = shown
        .iter()
        .find(|card| {
            let kind = if matches!(
                card.effective_status(prefs.refresh_secs),
                Status::Live | Status::Stale
            ) {
                "provider-card"
            } else {
                "provider-row"
            };
            ctx.input(|input| input.pointer.primary_down())
                && ctx.is_being_dragged(Id::new((kind, &card.id)))
        })
        .map(|card| card.id.clone());
    if preview.is_none()
        && !cancelled
        && let (Some(card), Some(slots), Some(press)) = (
            active.as_ref(),
            previous_slots,
            ctx.input(|input| input.pointer.press_origin()),
        )
        && let Some(source) = slots.iter().find(|slot| slot.id == *card)
    {
        preview = Some(CardDragPreview {
            source: card.clone(),
            available: source.available,
            grab_offset: press - source.rect.min,
            slots,
            target: None,
        });
    }
    if let Some(state) = preview.as_mut() {
        if active.is_none() && !released {
            preview = None;
        } else {
            state.target = ctx
                .pointer_interact_pos()
                .and_then(|pos| state.target_at(pos))
                .filter(|target| {
                    shown.iter().any(|card| {
                        card.id == *target
                            && matches!(
                                card.effective_status(prefs.refresh_secs),
                                Status::Live | Status::Stale
                            ) == state.available
                    })
                });
        }
    }
    ctx.data_mut(|data| data.insert_temp(cancelled_id, cancelled));
    let mut slots = Vec::new();
    Frame::new()
        .fill(ui.visuals().panel_fill)
        .inner_margin(egui::Margin::symmetric(4, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().interact_size.y = 32.0;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let settings_button = egui::Button::new("").min_size(egui::vec2(32.0, 32.0));
                    let settings = egui::menu::menu_custom_button(ui, settings_button, |ui| {
                        // Keep all global choices comfortable in a normal window
                        // while the existing scroll area handles smaller windows.
                        ui.spacing_mut().item_spacing.y = 6.0;
                        let screen = ui.ctx().screen_rect();
                        let menu_height =
                            (screen.height() - ui.spacing().menu_margin.sum().y - 8.0).max(64.0);
                        let menu_width = ui
                            .spacing()
                            .menu_width
                            .min(screen.width() - ui.spacing().menu_margin.sum().x - 8.0)
                            .max(64.0);
                        // Popup Areas begin with a 600x400 size hint. Give the scroll
                        // area the actual viewport budget, then let the scope shrink to
                        // its contents so a menu that fits keeps its natural height.
                        let menu_rect = egui::Rect::from_min_size(
                            ui.available_rect_before_wrap().min,
                            egui::vec2(menu_width, menu_height),
                        );
                        let menu_scroll = ui
                            .scope_builder(egui::UiBuilder::new().max_rect(menu_rect), |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("window-settings-scroll")
                                    .animated(false)
                                    .max_height(menu_height)
                                    .max_width(menu_width)
                                    .show(ui, |ui| {
                                        let about = ui.button("About Token Ashtray…");
                                        about.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::WidgetType::Button,
                                                true,
                                                "About Token Ashtray",
                                            )
                                        });
                                        settings_control(&about, "About Token Ashtray");
                                        if about.clicked() {
                                            action.about = true;
                                            ui.close_menu();
                                        }
                                        if pin_supported {
                                            let response = ui
                                                .horizontal(|ui| {
                                                    ui_icons::label(ui, Icon::Pin);
                                                    ui.checkbox(pinned, "Always on top")
                                                })
                                                .inner;
                                            settings_control(&response, "Always on top");
                                            if response.changed() {
                                                action.changed = true;
                                            }
                                        } else {
                                            let mut shown = displayed_pin(*pinned, pin_supported);
                                            let response = ui
                                                .horizontal(|ui| {
                                                    ui.add_enabled_ui(false, |ui| {
                                                        ui_icons::label(ui, Icon::Pin)
                                                    });
                                                    ui.add_enabled(
                                                        false,
                                                        egui::Checkbox::new(
                                                            &mut shown,
                                                            "Always on top",
                                                        ),
                                                    )
                                                })
                                                .inner;
                                            settings_control(&response, "Always on top");
                                            // egui suppresses on_disabled_hover_text inside an open menu.
                                            if response.contains_pointer() {
                                                response.show_tooltip_text(
                                                    platform::PIN_UNSUPPORTED_EXPLANATION,
                                                );
                                            }
                                        }
                                        let response = ui
                                            .horizontal(|ui| {
                                                ui_icons::label(ui, Icon::Compact);
                                                ui.checkbox(compact, "Compact window")
                                            })
                                            .inner;
                                        settings_control(&response, "Compact window");
                                        if response.changed() {
                                            action.compact_changed = true;
                                            action.changed = true;
                                        }
                                        ui.separator();
                                        ui.label(egui::RichText::new("Appearance").strong());
                                        let mut appearance = prefs.theme_preference;
                                        let group_id = ui.make_persistent_id("appearance-choices");
                                        let ctx = ui.ctx().clone();
                                        ctx.accesskit_node_builder(group_id, |_| {});
                                        let choices =
                                            ctx.with_accessibility_parent(group_id, || {
                                                ui.horizontal(|ui| {
                                                    [
                                                        ThemePreference::System,
                                                        ThemePreference::Light,
                                                        ThemePreference::Dark,
                                                    ]
                                                    .map(|option| {
                                                        let response = ui.selectable_value(
                                                            &mut appearance,
                                                            option,
                                                            option.label(),
                                                        );
                                                        if response.changed() {
                                                            action.theme_preference =
                                                                Some(appearance);
                                                            action.changed = true;
                                                        }
                                                        (option, response)
                                                    })
                                                })
                                            });
                                        for (option, response) in choices.inner {
                                            settings_control(&response, option.label());
                                            response.widget_info(|| {
                                                egui::WidgetInfo::selected(
                                                    egui::WidgetType::RadioButton,
                                                    ui.is_enabled(),
                                                    appearance == option,
                                                    option.label(),
                                                )
                                            });
                                        }
                                        ui.interact(
                                            choices.response.rect,
                                            group_id,
                                            egui::Sense::hover(),
                                        )
                                        .widget_info(
                                            || {
                                                egui::WidgetInfo::labeled(
                                                    egui::WidgetType::RadioGroup,
                                                    ui.is_enabled(),
                                                    "Appearance",
                                                )
                                            },
                                        );
                                        ui.separator();
                                        ui.label(egui::RichText::new("Quota display").strong());
                                        let mut mode = prefs.quota_display_mode;
                                        let group_id =
                                            ui.make_persistent_id("quota-display-choices");
                                        let ctx = ui.ctx().clone();
                                        ctx.accesskit_node_builder(group_id, |_| {});
                                        let choices =
                                            ctx.with_accessibility_parent(group_id, || {
                                                ui.horizontal(|ui| {
                                                    [
                                                        QuotaDisplayMode::Remaining,
                                                        QuotaDisplayMode::Used,
                                                    ]
                                                    .map(|option| {
                                                        let response = ui.selectable_value(
                                                            &mut mode,
                                                            option,
                                                            option.label(),
                                                        );
                                                        if response.changed() {
                                                            action.quota_display_mode = Some(mode);
                                                            action.changed = true;
                                                        }
                                                        (option, response)
                                                    })
                                                })
                                            });
                                        // Describe both choices after the click has settled the shared value.
                                        for (option, response) in choices.inner {
                                            settings_control(&response, option.label());
                                            response.widget_info(|| {
                                                egui::WidgetInfo::selected(
                                                    egui::WidgetType::RadioButton,
                                                    ui.is_enabled(),
                                                    mode == option,
                                                    option.label(),
                                                )
                                            });
                                        }
                                        ui.interact(
                                            choices.response.rect,
                                            group_id,
                                            egui::Sense::hover(),
                                        )
                                        .widget_info(
                                            || {
                                                egui::WidgetInfo::labeled(
                                                    egui::WidgetType::RadioGroup,
                                                    ui.is_enabled(),
                                                    "Quota display",
                                                )
                                            },
                                        );
                                        ui.separator();
                                        ui.label(egui::RichText::new("Polling").strong());
                                        let mut secs = prefs.refresh_secs.max(60);
                                        ui.horizontal(|ui| {
                                            ui.label("Every");
                                            let response = ui.add(
                                                egui::DragValue::new(&mut secs)
                                                    .range(60..=3600)
                                                    .suffix(" s"),
                                            );
                                            settings_control(&response, "Polling interval");
                                            if response.changed() {
                                                action.refresh_secs = Some(secs);
                                                action.changed = true;
                                            }
                                        });
                                        ui.separator();
                                        ui.label("Hidden providers are not polled.");
                                        for card in cards {
                                            let mut shown = !prefs.hidden.contains(&card.id);
                                            let label = format!("Show {}", card.short_name);
                                            let response = ui.checkbox(&mut shown, &label);
                                            settings_control(&response, &label);
                                            if response.changed() {
                                                let mut hidden = prefs.hidden.clone();
                                                if shown {
                                                    hidden.remove(&card.id);
                                                } else {
                                                    hidden.insert(card.id.clone());
                                                }
                                                action.hidden = Some(hidden);
                                                action.changed = true;
                                            }
                                        }
                                        ui.separator();
                                        ui.label(egui::RichText::new("Updates").strong());
                                        let mut updates = prefs.updates_enabled;
                                        let response = ui.checkbox(&mut updates, UPDATES_CONTROL);
                                        settings_control(&response, UPDATES_CONTROL);
                                        if response.changed() {
                                            action.updates_enabled = Some(updates);
                                            action.changed = true;
                                        }
                                        ui.label(egui::RichText::new(UPDATE_EXPLANATION).small());
                                        ui.label(update_status_text(update_status));
                                    })
                            })
                            .inner;
                        #[cfg(test)]
                        settings_menu_test::record_scroll(
                            menu_scroll.content_size.y,
                            menu_scroll.inner_rect.height(),
                            menu_scroll.state.offset.y,
                        );
                        // A wheel gesture over the popup belongs to it even at a scroll
                        // boundary; otherwise the board behind it starts moving.
                        if ui.rect_contains_pointer(ui.min_rect().union(menu_scroll.inner_rect)) {
                            ui.ctx()
                                .input_mut(|input| input.smooth_scroll_delta.y = 0.0);
                        }
                    });
                    Icon::Menu.paint(
                        ui.painter(),
                        settings.response.rect,
                        ui.style().interact(&settings.response).fg_stroke.color,
                    );
                    settings.response.clone().on_hover_text("Window settings");
                    settings.response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            "Window settings",
                        )
                    });
                    let columns_button = egui::Button::new("").min_size(egui::vec2(32.0, 32.0));
                    let columns_menu = egui::menu::menu_custom_button(ui, columns_button, |ui| {
                        for option in [
                            ColumnLayout::Auto,
                            ColumnLayout::One,
                            ColumnLayout::Two,
                            ColumnLayout::Three,
                        ] {
                            let mut columns = prefs.columns;
                            let response =
                                ui.selectable_value(&mut columns, option, option.label());
                            settings_control(&response, option.label());
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::RadioButton,
                                    ui.is_enabled(),
                                    prefs.columns == option,
                                    option.label(),
                                )
                            });
                            if response.changed() {
                                action.columns = Some(columns);
                                action.changed = true;
                                ui.close_menu();
                            }
                        }
                    });
                    Icon::Columns.paint(
                        ui.painter(),
                        columns_menu.response.rect,
                        ui.style().interact(&columns_menu.response).fg_stroke.color,
                    );
                    columns_menu
                        .response
                        .clone()
                        .on_hover_text(format!("Card columns: {}", prefs.columns.label()));
                    columns_menu.response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            "Card columns",
                        )
                    });
                    if refresh_feedback::toolbar(ui) {
                        action.refresh = true;
                    }
                    if ui.is_visible() && author_youtube::show(ui, prefs.author_youtube_visited) {
                        action.author_youtube_visited = true;
                    }
                    // Actions get their full hit targets before allocating identity/status.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let title = window_title.to_owned();
                        let identity_width = if *compact {
                            ui.painter()
                                .layout_no_wrap(
                                    title.clone(),
                                    compact_identity_font(),
                                    palette(ui.ctx()).text,
                                )
                                .size()
                                .x
                        } else {
                            0.0
                        };
                        let remaining =
                            ui.available_width() - identity_width - ui.spacing().item_spacing.x;
                        let refresh_status = refresh_feedback::toolbar_status(ui.ctx());
                        let summary = if let Some(status) = refresh_status {
                            vec![
                                status.clone(),
                                refresh_feedback::short_status(&status).into(),
                            ]
                        } else {
                            vec![
                                format!("{live_count} live · {attention_count} need attention"),
                                format!("{live_count} live"),
                            ]
                        }
                        .into_iter()
                        .find(|summary| {
                            ui.painter()
                                .layout_no_wrap(
                                    summary.clone(),
                                    egui::FontId::proportional(12.0),
                                    palette(ui.ctx()).secondary,
                                )
                                .size()
                                .x
                                <= remaining
                        });
                        if *compact {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&title)
                                        .font(compact_identity_font())
                                        .color(palette(ui.ctx()).text),
                                )
                                .truncate(),
                            );
                        }
                        if let Some(summary) = summary {
                            ui.label(
                                egui::RichText::new(summary)
                                    .size(12.0)
                                    .color(palette(ui.ctx()).secondary),
                            );
                        }
                    });
                });
            });
        });
    ui.add_space(2.0);
    let (mut available, mut unavailable): (Vec<_>, Vec<_>) = shown.into_iter().partition(|card| {
        matches!(
            card.effective_status(prefs.refresh_secs),
            Status::Live | Status::Stale
        )
    });
    if let Some(state) = &preview
        && let Some(target) = &state.target
    {
        if state.available {
            available = move_card_to_target(&available, &state.source, target);
        } else {
            unavailable = move_card_to_target(&unavailable, &state.source, target);
        }
    }
    draw_card_grid(
        ui,
        &available,
        prefs,
        &mut action,
        preview.as_ref().filter(|_| !released),
        &mut slots,
    );
    if !unavailable.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Unavailable providers")
                .size(12.0)
                .strong()
                .color(palette(ui.ctx()).secondary),
        );
        for card in unavailable {
            draw_unavailable_row(
                ui,
                card,
                prefs,
                &mut action,
                preview.as_ref().filter(|_| !released),
                &mut slots,
            );
            ui.add_space(3.0);
        }
    }
    if released {
        if let Some(state) = &preview
            && let Some(target) = &state.target
            && let Some(order) = moved_order(cards, &state.source, target)
            && order != cards.iter().map(|card| card.id.clone()).collect::<Vec<_>>()
        {
            action.order = Some(order);
            action.changed = true;
        }
        ctx.data_mut(|data| data.remove::<CardDragPreview>(preview_id));
    } else if let Some(state) = preview {
        ctx.data_mut(|data| data.insert_temp(preview_id, state));
    } else {
        ctx.data_mut(|data| data.remove::<CardDragPreview>(preview_id));
    }
    if active.is_none() || released {
        ctx.data_mut(|data| data.insert_temp(layout_id, slots));
    }
    action
}

fn draw_card_grid(
    ui: &mut egui::Ui,
    cards: &[&ProviderCard],
    prefs: &crate::config::BoardPreferences,
    action: &mut BoardAction,
    preview: Option<&CardDragPreview>,
    slots: &mut Vec<DragSlot>,
) {
    let columns = resolve_columns(prefs.columns, ui.available_width());
    let gap = GRID_GAP;
    let card_width =
        ((ui.available_width() - gap * (columns - 1) as f32) / columns as f32).max(1.0);
    for row in cards.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for card in row {
                ui.allocate_ui_with_layout(
                    egui::vec2(card_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        draw_card_slot(ui, card, prefs, action, preview, slots);
                    },
                );
            }
        });
        ui.add_space((gap - ui.spacing().item_spacing.y).max(0.0));
    }
}

fn draw_card_slot(
    ui: &mut egui::Ui,
    card: &ProviderCard,
    prefs: &crate::config::BoardPreferences,
    action: &mut BoardAction,
    preview: Option<&CardDragPreview>,
    slots: &mut Vec<DragSlot>,
) {
    let payload = card.id.clone();
    let dragging = preview.is_some_and(|state| state.source == card.id);
    let layer = egui::LayerId::new(
        egui::Order::Tooltip,
        Id::new(("card-preview-layer", &card.id)),
    );
    let response = ui
        .scope_builder(
            egui::UiBuilder::new().layer_id(if dragging { layer } else { ui.layer_id() }),
            |ui| {
                let opacity = refresh_feedback::opacity(ui, &card.id);
                ui.multiply_opacity(opacity);
                show_card(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    draw_card(
                        ui,
                        card,
                        prefs,
                        action,
                        Id::new(("provider-card", &card.id)),
                        payload.clone(),
                    );
                    refresh_feedback::card_cue(ui, card);
                })
            },
        )
        .inner;
    refresh_feedback::highlight(ui, &card.id, response.response.rect);
    slots.push(DragSlot {
        id: card.id.clone(),
        rect: response.response.rect,
        available: true,
    });
    if let Some(state) = preview.filter(|_| dragging)
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        ui.ctx().transform_layer_shapes(
            layer,
            egui::emath::TSTransform::from_translation(
                pos - state.grab_offset - response.response.rect.min,
            ),
        );
    }
}

fn draw_unavailable_row(
    ui: &mut egui::Ui,
    card: &ProviderCard,
    prefs: &crate::config::BoardPreferences,
    action: &mut BoardAction,
    preview: Option<&CardDragPreview>,
    slots: &mut Vec<DragSlot>,
) {
    let status = card.effective_status(prefs.refresh_secs);
    let payload = card.id.clone();
    let dragging = preview.is_some_and(|state| state.source == card.id);
    let layer = egui::LayerId::new(
        egui::Order::Tooltip,
        Id::new(("row-preview-layer", &card.id)),
    );
    let response = ui
        .scope_builder(
            egui::UiBuilder::new().layer_id(if dragging { layer } else { ui.layer_id() }),
            |ui| {
                let opacity = refresh_feedback::opacity(ui, &card.id);
                ui.multiply_opacity(opacity);
                show_card(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    let (row, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 54.0),
                        egui::Sense::hover(),
                    );
                    let handle = egui::Rect::from_min_size(
                        row.min + egui::vec2(0.0, 4.0),
                        egui::vec2(16.0, 20.0),
                    );
                    if dragging {
                        paint_drag_dots(ui.painter(), handle);
                    }
                    draw_drag_handle_at(
                        ui,
                        Id::new(("provider-row", &card.id)),
                        payload.clone(),
                        &card.short_name,
                        handle,
                    );
                    let logo = egui::Rect::from_min_size(
                        row.min + egui::vec2(24.0, 3.0),
                        egui::vec2(22.0, 22.0),
                    );
                    crate::provider_logos::paint_provider_logo(ui, &card.id, logo);
                    let left = logo.right() + 10.0;
                    let metadata = header_metadata(card);
                    let star = egui::Rect::from_min_size(
                        egui::pos2(row.right() - 28.0, row.top()),
                        egui::vec2(28.0, 28.0),
                    );
                    let selected = prefs.primary.as_deref() == Some(&card.id);
                    if primary_button(ui, star, selected, &card.id) {
                        action.primary = Some((!selected).then(|| card.id.clone()));
                        action.changed = true;
                    }
                    let (status_text, status_color) = status_label(status);
                    let status_width = pill_width(ui, status_text, 68.0);
                    let status_pill = egui::Rect::from_min_size(
                        egui::pos2(star.left() - 8.0 - status_width, row.top() + 4.0),
                        egui::vec2(status_width, 20.0),
                    );
                    paint_pill(ui.painter(), status_pill, status_text, status_color);
                    let name = egui::Rect::from_min_max(
                        egui::pos2(left, row.top()),
                        egui::pos2(status_pill.left() - 8.0, row.top() + 28.0),
                    );
                    let name_response = ui.interact(
                        name,
                        Id::new(("provider-row-name", &card.id)),
                        if card.link.is_empty() {
                            egui::Sense::hover()
                        } else {
                            egui::Sense::click()
                        },
                    );
                    name_response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            if card.link.is_empty() {
                                egui::WidgetType::Label
                            } else {
                                egui::WidgetType::Link
                            },
                            ui.is_enabled(),
                            if card.link.is_empty() {
                                card.short_name.clone()
                            } else {
                                format!("{} — open provider usage page", card.short_name)
                            },
                        )
                    });
                    if name_response.has_focus() {
                        ui.painter().rect_stroke(
                            name.expand(1.0),
                            4.0,
                            egui::Stroke::new(1.0_f32, palette(ui.ctx()).blue),
                            egui::StrokeKind::Outside,
                        );
                    }
                    paint_left_truncated(
                        ui.painter(),
                        name,
                        &friendly_label(&card.short_name),
                        egui::FontId::proportional(14.0),
                        palette(ui.ctx()).text,
                    );
                    if !card.link.is_empty() {
                        if name_response.clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(&card.link));
                        }
                        name_response.on_hover_text(format!(
                            "{}\n{}\nOpen provider usage page",
                            card.short_name, metadata
                        ));
                    } else {
                        name_response.on_hover_text(format!("{}\n{}", card.short_name, metadata));
                    }
                    let reason = egui::Rect::from_min_max(
                        egui::pos2(left, row.top() + 32.0),
                        egui::pos2(row.right(), row.bottom()),
                    );
                    let detail_response = ui.interact(
                        reason,
                        Id::new(("provider-row-detail", &card.id)),
                        egui::Sense::hover(),
                    );
                    paint_left_truncated(
                        ui.painter(),
                        reason,
                        &card.detail,
                        egui::FontId::proportional(12.0),
                        palette(ui.ctx()).secondary,
                    );
                    detail_response.on_hover_text(if card.detail.is_empty() {
                        "No detail reported"
                    } else {
                        &card.detail
                    });
                    refresh_feedback::card_cue(ui, card);
                })
            },
        )
        .inner;
    refresh_feedback::highlight(ui, &card.id, response.response.rect);
    slots.push(DragSlot {
        id: card.id.clone(),
        rect: response.response.rect,
        available: false,
    });
    if let Some(state) = preview.filter(|_| dragging)
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        ui.ctx().transform_layer_shapes(
            layer,
            egui::emath::TSTransform::from_translation(
                pos - state.grab_offset - response.response.rect.min,
            ),
        );
    }
}

fn draw_card(
    ui: &mut egui::Ui,
    card: &ProviderCard,
    prefs: &crate::config::BoardPreferences,
    action: &mut BoardAction,
    drag_id: Id,
    payload: String,
) {
    let status = card.effective_status(prefs.refresh_secs);
    let metadata = header_metadata(card);
    let (header, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
    let handle =
        egui::Rect::from_min_size(header.min + egui::vec2(0.0, 10.0), egui::vec2(16.0, 20.0));
    if ui.ctx().is_being_dragged(drag_id) {
        paint_drag_dots(ui.painter(), handle);
    }
    draw_drag_handle_at(ui, drag_id, payload, &card.short_name, handle);
    let logo =
        egui::Rect::from_min_size(header.min + egui::vec2(24.0, 4.0), egui::vec2(32.0, 32.0));
    crate::provider_logos::paint_provider_logo(ui, &card.id, logo);
    let left = logo.right() + 10.0;
    let star = egui::Rect::from_min_size(
        egui::pos2(header.right() - 28.0, header.top() + 6.0),
        egui::vec2(28.0, 28.0),
    );
    let selected = prefs.primary.as_deref() == Some(&card.id);
    if primary_button(ui, star, selected, &card.id) {
        action.primary = Some((!selected).then(|| card.id.clone()));
        action.changed = true;
    }
    let right = star.left() - 8.0;
    let name = egui::Rect::from_min_max(
        egui::pos2(left, header.top()),
        egui::pos2(right, header.top() + 21.0),
    );
    let name_response = ui.interact(
        name,
        Id::new(("provider-name", &card.id)),
        if card.meters.is_empty() && !card.link.is_empty() {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    paint_left_truncated(
        ui.painter(),
        name,
        &friendly_label(&card.short_name),
        egui::FontId::proportional(16.0),
        palette(ui.ctx()).text,
    );
    let mut metadata_right = right;
    if status != Status::Live {
        let (label, color) = status_label(status);
        let width = pill_width(ui, label, 68.0);
        let pill = egui::Rect::from_min_size(
            egui::pos2(right - width, header.top() + 22.0),
            egui::vec2(width, 18.0),
        );
        paint_pill(ui.painter(), pill, label, color);
        metadata_right = pill.left() - 6.0;
    }
    let metadata_rect = egui::Rect::from_min_max(
        egui::pos2(left, header.top() + 22.0),
        egui::pos2(metadata_right.max(left), header.bottom()),
    );
    paint_left_truncated(
        ui.painter(),
        metadata_rect,
        &metadata,
        egui::FontId::proportional(12.0),
        palette(ui.ctx()).secondary,
    );
    ui.interact(
        metadata_rect,
        Id::new(("card-metadata", &card.id)),
        egui::Sense::hover(),
    )
    .on_hover_text(&metadata);
    name_response.widget_info(|| {
        egui::WidgetInfo::labeled(
            if card.meters.is_empty() && !card.link.is_empty() {
                egui::WidgetType::Link
            } else {
                egui::WidgetType::Label
            },
            true,
            &card.short_name,
        )
    });
    if card.meters.is_empty() && !card.link.is_empty() {
        if name_response.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&card.link));
        }
        name_response.on_hover_text(format!("{}\nOpen provider usage page", card.short_name));
    } else {
        name_response.on_hover_text(&card.short_name);
    }
    if !card.meters.is_empty() {
        if let Some(updated) = last_updated_label(status, card.fetched_at) {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(updated)
                        .size(11.0)
                        .color(palette(ui.ctx()).secondary),
                )
                .wrap(),
            )
            .on_hover_text(if status != Status::Live && !card.detail.is_empty() {
                &card.detail
            } else {
                "Reported quota windows"
            });
        }
        draw_quota_grid(ui, card, status, prefs.quota_display_mode);
    } else {
        ui.label(
            egui::RichText::new("No quota windows reported")
                .size(12.0)
                .color(palette(ui.ctx()).secondary),
        );
    }
    if !card.usage_breakdown.is_empty() {
        draw_usage_breakdown(ui, card, status);
    }
}

fn draw_quota_grid(ui: &mut egui::Ui, card: &ProviderCard, status: Status, mode: QuotaDisplayMode) {
    let width = ui.available_width();
    let gap = 12.0;
    // Keep ordinary window names/details readable, then wrap whole cells.
    let columns = (((width + gap) / (210.0 + gap)).floor() as usize)
        .max(1)
        .min(card.meters.len());
    let cell_width = ((width - gap * (columns - 1) as f32) / columns as f32).max(1.0);
    for (row_index, row) in card.meters.chunks(columns).enumerate() {
        let layouts: Vec<_> = row
            .iter()
            .map(|meter| quota_row_layout(ui, meter, status, cell_width))
            .collect();
        let header_height = layouts
            .iter()
            .map(|layout| layout.header_height)
            .fold(0.0, f32::max);
        let height = layouts
            .iter()
            .map(|layout| layout.height(header_height))
            .fold(0.0, f32::max);
        let (row_rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        for (column, (meter, layout)) in row.iter().zip(layouts).enumerate() {
            let index = row_index * columns + column;
            let rect = egui::Rect::from_min_size(
                row_rect.min + egui::vec2(column as f32 * (cell_width + gap), 0.0),
                egui::vec2(cell_width, height),
            );
            draw_quota_meter_layout(
                ui,
                meter,
                status,
                &card.link,
                Id::new(("meter-gauge", &card.id, index)),
                rect,
                index,
                mode,
                layout,
                header_height,
            );
        }
        ui.add_space(4.0);
    }
}

fn quota_detail(meter: &token_ashtray::model::Meter, status: Status) -> String {
    let reset = meter
        .resets_at
        .map(|at| reset_detail_label(at, quota_now()))
        .unwrap_or_else(|| "Reset unavailable".into());
    let cue = quota_status(meter, status);
    if cue.is_empty() {
        reset
    } else {
        format!("{reset} · {cue}")
    }
}
fn quota_status(meter: &token_ashtray::model::Meter, status: Status) -> &'static str {
    if status == Status::Stale {
        "Stale cached quota"
    } else if meter.used_percent >= 100.0 {
        "Allowance exhausted"
    } else if meter.used_percent >= 90.0 {
        "Nearly exhausted"
    } else if meter.used_percent >= 75.0 {
        "High usage"
    } else {
        ""
    }
}
const QUOTA_TRACK_HEIGHT: f32 = 20.0;
struct QuotaRowLayout {
    label: Arc<egui::Galley>,
    countdown: Arc<egui::Galley>,
    status: Arc<egui::Galley>,
    header_height: f32,
    inline_countdown: bool,
}
impl QuotaRowLayout {
    fn height(&self, header_height: f32) -> f32 {
        header_height
            + 4.0
            + QUOTA_TRACK_HEIGHT
            + if self.status.job.text.is_empty() {
                0.0
            } else {
                4.0 + self.status.size().y
            }
    }
}
fn quota_row_layout(
    ui: &egui::Ui,
    meter: &token_ashtray::model::Meter,
    status: Status,
    width: f32,
) -> QuotaRowLayout {
    let countdown = ui.painter().layout_no_wrap(
        meter.resets_at.map(reset_label).unwrap_or_default(),
        egui::FontId::proportional(12.0),
        palette(ui.ctx()).secondary,
    );
    let has_countdown = !countdown.job.text.is_empty();
    let inline_countdown = !has_countdown || width >= countdown.size().x + 8.0 + 80.0;
    let label_width = if has_countdown && inline_countdown {
        width - countdown.size().x - 8.0
    } else {
        width
    };
    let label = ui.painter().layout(
        meter.label.clone(),
        egui::FontId::proportional(14.0),
        palette(ui.ctx()).text,
        label_width.max(1.0),
    );
    let countdown_height = if has_countdown {
        countdown.size().y
    } else {
        0.0
    };
    let header_height = if inline_countdown {
        label.size().y.max(countdown_height)
    } else {
        label.size().y + 4.0 + countdown_height
    };
    let status = ui.painter().layout(
        quota_status(meter, status).into(),
        egui::FontId::proportional(12.0),
        palette(ui.ctx()).secondary,
        width.max(1.0),
    );
    QuotaRowLayout {
        label,
        countdown,
        status,
        header_height,
        inline_countdown,
    }
}

fn draw_usage_breakdown(ui: &mut egui::Ui, card: &ProviderCard, status: Status) {
    ui.add_space(8.0);
    ui.separator();
    ui.label(
        egui::RichText::new("Shared quota used")
            .size(12.0)
            .color(palette(ui.ctx()).text),
    );
    ui.add(
        egui::Label::new(
            egui::RichText::new("Product attribution · one shared allowance")
                .size(12.0)
                .color(palette(ui.ctx()).secondary),
        )
        .wrap(),
    );
    for (index, product) in card.usage_breakdown.iter().enumerate() {
        let width = ui.available_width();
        let label = ui.painter().layout(
            product.label.clone(),
            egui::FontId::proportional(12.0),
            palette(ui.ctx()).text,
            (width - 64.0).max(1.0),
        );
        let height = label.size().y.max(28.0) + 10.0;
        let (row, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        ui.painter().galley(row.min, label, palette(ui.ctx()).text);
        let value = QuotaDisplayMode::Used.percent_label(product.used_percent);
        ui.painter().text(
            egui::pos2(row.right(), row.top()),
            egui::Align2::RIGHT_TOP,
            &value,
            egui::FontId::proportional(12.0),
            palette(ui.ctx()).text,
        );
        ui.painter().text(
            egui::pos2(row.right(), row.top() + 15.0),
            egui::Align2::RIGHT_TOP,
            "Used",
            egui::FontId::proportional(10.0),
            palette(ui.ctx()).secondary,
        );
        let track = egui::Rect::from_min_size(
            egui::pos2(row.left(), row.bottom() - 3.0),
            egui::vec2(width, 3.0),
        );
        ui.painter()
            .rect_filled(track, 1.5, palette(ui.ctx()).outline);
        let used = egui::Rect::from_min_size(
            track.min,
            egui::vec2(width * product.used_percent.clamp(0.0, 100.0) / 100.0, 3.0),
        );
        if used.width() > 0.0 {
            ui.painter().rect_filled(
                used,
                0.0,
                if status == Status::Live {
                    palette(ui.ctx()).blue
                } else {
                    palette(ui.ctx()).secondary.gamma_multiply(0.5)
                },
            );
        }
        let response = ui.interact(
            row,
            Id::new(("usage-breakdown", &card.id, index)),
            if card.link.is_empty() {
                egui::Sense::focusable_noninteractive()
            } else {
                egui::Sense::click()
            },
        );
        let description = format!(
            "{}: {value} of the shared quota used. This is usage attribution, not a separate allowance.",
            product.label
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                if card.link.is_empty() {
                    egui::WidgetType::Label
                } else {
                    egui::WidgetType::Link
                },
                true,
                &description,
            )
        });
        response.clone().on_hover_text(&description);
        if !card.link.is_empty() {
            if response.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(&card.link));
            }
            response
                .clone()
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                row,
                3.0,
                egui::Stroke::new(1.0_f32, palette(ui.ctx()).blue),
                egui::StrokeKind::Inside,
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn draw_quota_meter(
    ui: &mut egui::Ui,
    meter: &token_ashtray::model::Meter,
    status: Status,
    link: &str,
    id: Id,
    rect: egui::Rect,
    accent_index: usize,
    mode: QuotaDisplayMode,
) {
    let layout = quota_row_layout(ui, meter, status, rect.width());
    let header_height = layout.header_height;
    draw_quota_meter_layout(
        ui,
        meter,
        status,
        link,
        id,
        rect,
        accent_index,
        mode,
        layout,
        header_height,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_quota_meter_layout(
    ui: &mut egui::Ui,
    meter: &token_ashtray::model::Meter,
    status: Status,
    link: &str,
    id: Id,
    rect: egui::Rect,
    accent_index: usize,
    mode: QuotaDisplayMode,
    layout: QuotaRowLayout,
    header_height: f32,
) {
    let response = ui.interact(
        rect,
        id,
        if link.is_empty() {
            egui::Sense::focusable_noninteractive()
        } else {
            egui::Sense::click()
        },
    );
    let remaining = (100.0 - meter.used_percent).clamp(0.0, 100.0);
    let displayed = mode.percent(meter.used_percent);
    let value_label = mode.percent_label(meter.used_percent);
    let detail_text = quota_detail(meter, status);
    let label_height = layout.label.size().y;
    ui.painter()
        .galley(rect.min, layout.label, palette(ui.ctx()).text);
    if !layout.countdown.job.text.is_empty() {
        let y = if layout.inline_countdown {
            1.0
        } else {
            label_height + 4.0
        };
        let pos = egui::pos2(rect.right() - layout.countdown.size().x, rect.top() + y);
        ui.painter()
            .galley(pos, layout.countdown, palette(ui.ctx()).secondary);
    }
    let track = egui::Rect::from_min_size(
        rect.min + egui::vec2(0.0, header_height + 4.0),
        egui::vec2(rect.width(), QUOTA_TRACK_HEIGHT),
    );
    ui.painter()
        .rect_filled(track, 0.0, palette(ui.ctx()).outline);
    if displayed > 0.0 {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                track.min,
                egui::vec2(track.width() * displayed / 100.0, track.height()),
            ),
            0.0,
            meter_color(remaining, status, accent_index, ui.visuals().dark_mode),
        );
    }
    paint_meter_value(ui.painter(), track, &value_label);
    if !layout.status.job.text.is_empty() {
        ui.painter().galley(
            egui::pos2(rect.left(), track.bottom() + 4.0),
            layout.status,
            palette(ui.ctx()).secondary,
        );
    }

    if !link.is_empty() {
        if response.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(link));
        }
        response
            .clone()
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "{}: {} {} — {detail_text} — open provider usage page",
                meter.label,
                value_label,
                mode.label()
            ));
    } else {
        response.clone().on_hover_text(format!(
            "{}: {} {} — {detail_text} — provider did not report a usage page",
            meter.label,
            value_label,
            mode.label()
        ));
    }
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(
            if link.is_empty() {
                egui::WidgetType::ProgressIndicator
            } else {
                egui::WidgetType::Link
            },
            true,
            format!(
                "{}: {} {}; {detail_text}. {}",
                meter.label,
                value_label,
                mode.label(),
                if link.is_empty() {
                    "Provider did not report a usage page"
                } else {
                    "Activate to open provider usage page"
                }
            ),
        );
        info.value = Some(f64::from(displayed));
        info
    });
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(100.0);
    });
    if response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            egui::CornerRadius::same(6),
            egui::Stroke::new(1.0_f32, palette(ui.ctx()).blue),
            egui::StrokeKind::Outside,
        );
    }
}

fn paint_meter_value(painter: &egui::Painter, track: egui::Rect, value: &str) {
    let galley = painter.layout_no_wrap(
        value.into(),
        egui::FontId::proportional(12.0),
        egui::Color32::WHITE,
    );
    let pos = track.center() - galley.size() / 2.0;
    // A glyph outline, not an opaque badge, keeps the continuous fill visible.
    // White ink or its black edge has contrast on either side of any split.
    for offset in [
        egui::vec2(-1.0, 0.0),
        egui::vec2(1.0, 0.0),
        egui::vec2(0.0, -1.0),
        egui::vec2(0.0, 1.0),
        egui::vec2(-0.7, -0.7),
        egui::vec2(-0.7, 0.7),
        egui::vec2(0.7, -0.7),
        egui::vec2(0.7, 0.7),
    ] {
        painter.galley_with_override_text_color(pos + offset, galley.clone(), egui::Color32::BLACK);
    }
    painter.galley(pos, galley, egui::Color32::WHITE);
}

fn paint_centered_truncated(
    painter: &egui::Painter,
    center: Pos2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_width: f32,
) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
    let galley = painter.layout_job(job);
    painter.galley(
        Pos2::new(
            center.x - galley.size().x / 2.0,
            center.y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

fn header_metadata(card: &ProviderCard) -> String {
    match (&card.plan, &card.price) {
        (Some(plan), Some(price)) => format!("{} · {price}", title_case(plan)),
        (Some(plan), None) => title_case(plan),
        (None, Some(price)) => price.clone(),
        (None, None) => String::new(),
    }
}

fn draw_drag_handle_at(
    ui: &mut egui::Ui,
    id: Id,
    payload: String,
    provider_name: &str,
    rect: egui::Rect,
) {
    let mut handle_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let response = handle_ui
        .dnd_drag_source(id, payload, |ui| {
            let (rect, _) = ui.allocate_exact_size(rect.size(), egui::Sense::hover());
            if !ui.ctx().is_being_dragged(id) {
                paint_drag_dots(ui.painter(), rect);
            }
        })
        .response;
    let label = format!("Drag to reorder {provider_name}");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), &label)
    });
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(1.0),
            4.0,
            egui::Stroke::new(1.0_f32, palette(ui.ctx()).blue),
            egui::StrokeKind::Outside,
        );
    }
    response.on_hover_text(label);
}

fn paint_drag_dots(painter: &egui::Painter, rect: egui::Rect) {
    Icon::Drag.paint(painter, rect, palette(painter.ctx()).secondary);
}

fn primary_button(ui: &mut egui::Ui, rect: egui::Rect, selected: bool, provider_id: &str) -> bool {
    let response = ui.interact(
        rect,
        Id::new(("provider-primary", provider_id)),
        egui::Sense::click(),
    );
    let fill = if selected {
        palette(ui.ctx()).action
    } else {
        egui::Color32::TRANSPARENT
    };
    let stroke = if selected {
        palette(ui.ctx()).action
    } else {
        palette(ui.ctx()).outline.gamma_multiply(0.65)
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(6), fill);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(1.0_f32, stroke),
        egui::StrokeKind::Inside,
    );
    Icon::Primary.paint(
        ui.painter(),
        rect,
        if selected {
            egui::Color32::WHITE
        } else {
            palette(ui.ctx()).secondary
        },
    );
    response.clone().on_hover_text(if selected {
        "Primary provider"
    } else {
        "Make primary"
    });
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!(
                "{} {provider_id}",
                if selected {
                    "Clear primary provider"
                } else {
                    "Make primary provider"
                }
            ),
        )
    });
    if response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            egui::CornerRadius::same(6),
            egui::Stroke::new(1.0_f32, palette(ui.ctx()).blue),
            egui::StrokeKind::Outside,
        );
    }
    response.clicked()
}

fn pill_width(ui: &egui::Ui, label: &str, maximum: f32) -> f32 {
    (ui.painter()
        .layout_no_wrap(
            label.into(),
            egui::FontId::proportional(10.0),
            palette(ui.ctx()).text,
        )
        .size()
        .x
        + 10.0)
        .clamp(34.0, maximum)
}

fn paint_pill(painter: &egui::Painter, rect: egui::Rect, label: &str, color: egui::Color32) {
    let fill = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 32);
    painter.rect_filled(rect, egui::CornerRadius::same(4), fill);
    paint_centered_truncated(
        painter,
        rect.center(),
        label,
        egui::FontId::proportional(10.0),
        if color == palette(painter.ctx()).secondary {
            palette(painter.ctx()).secondary
        } else {
            palette(painter.ctx()).text
        },
        rect.width() - 8.0,
    );
}

fn paint_left_truncated(
    painter: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width().max(0.0));
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

fn meter_color(remaining: f32, status: Status, accent_index: usize, dark: bool) -> egui::Color32 {
    let (muted, critical, warning, accents) = if dark {
        (
            egui::Color32::from_rgb(103, 132, 161),
            egui::Color32::from_rgb(239, 98, 98),
            egui::Color32::from_rgb(245, 177, 70),
            [
                egui::Color32::from_rgb(45, 201, 166),
                egui::Color32::from_rgb(83, 157, 255),
                egui::Color32::from_rgb(160, 111, 240),
            ],
        )
    } else {
        (
            egui::Color32::from_rgb(88, 112, 138),
            egui::Color32::from_rgb(190, 44, 44),
            egui::Color32::from_rgb(156, 99, 0),
            [
                egui::Color32::from_rgb(0, 125, 102),
                egui::Color32::from_rgb(20, 104, 202),
                egui::Color32::from_rgb(118, 61, 190),
            ],
        )
    };
    // Theme affects contrast, not the documented consumption thresholds.
    if status != Status::Live {
        muted
    } else if remaining <= 10.0 {
        critical
    } else if remaining <= 25.0 {
        warning
    } else {
        accents[accent_index % accents.len()]
    }
}

fn status_label(status: Status) -> (&'static str, egui::Color32) {
    match status {
        Status::Live => ("Live", egui::Color32::from_rgb(51, 168, 104)),
        Status::Stale => ("Stale", egui::Color32::from_rgb(103, 132, 161)),
        Status::Reconnect => ("Reconnect", egui::Color32::from_rgb(214, 151, 48)),
        Status::Error => ("Error", egui::Color32::from_rgb(220, 82, 76)),
    }
}

fn grid_columns(width: f32) -> usize {
    if width >= 1440.0 {
        3
    } else if width >= 680.0 {
        2
    } else {
        1
    }
}

const GRID_GAP: f32 = 12.0;
/// Cards keep this minimum width under a manual column choice.
const MIN_CARD_WIDTH: f32 = 300.0;

fn resolve_columns(layout: ColumnLayout, width: f32) -> usize {
    let chosen = match layout {
        ColumnLayout::Auto => return grid_columns(width),
        ColumnLayout::One => 1,
        ColumnLayout::Two => 2,
        ColumnLayout::Three => 3,
    };
    // A manual choice still clamps to what fits at the minimum card width.
    let fits = ((width + GRID_GAP) / (MIN_CARD_WIDTH + GRID_GAP))
        .floor()
        .max(1.0) as usize;
    chosen.min(fits)
}

fn board_summary(
    cards: &[&ProviderCard],
    prefs: &crate::config::BoardPreferences,
) -> (usize, usize) {
    cards.iter().fold((0, 0), |(live, attention), card| {
        if card.effective_status(prefs.refresh_secs) == Status::Live {
            (live + 1, attention)
        } else {
            (live, attention + 1)
        }
    })
}
fn last_updated_label(status: Status, fetched_at: Option<OffsetDateTime>) -> Option<String> {
    (status != Status::Live)
        .then_some(fetched_at?)
        .map(|fetched_at| {
            format!(
                "Last updated {} ago",
                duration_label((quota_now() - fetched_at).whole_seconds())
            )
        })
}
fn friendly_label(label: &str) -> String {
    label.into()
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;
    #[test]
    fn empty_monitor_data_does_not_restore_unvalidated_offscreen_position() {
        let saved = WindowPreferences {
            x: Some(2610.0),
            y: Some(150.0),
            width: 520.0,
            height: 460.0,
            ..Default::default()
        };
        assert_eq!(restore_position(&saved, &[]), None);
        assert_eq!(
            fitted_board_viewport_builder(&saved, &[], true, true).position,
            None
        );
    }

    #[test]
    fn transient_origin_does_not_replace_saved_secondary_position() {
        let mut saved = WindowPreferences {
            x: Some(2610.0),
            y: Some(150.0),
            width: 520.0,
            height: 460.0,
            ..Default::default()
        };
        saved.pinned = true;
        let monitors = [
            platform::MonitorRect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            platform::MonitorRect {
                x: 1920.0,
                y: 0.0,
                width: 1280.0,
                height: 1080.0,
            },
        ];
        let start = Instant::now();
        let mut state = PositionRestore::default();
        assert_eq!(
            state.requested_position(&saved, &monitors, true, start,),
            (Some(Pos2::new(2610.0, 150.0)), true)
        );
        let outer = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(520.0, 460.0));
        let inner = outer;
        assert_eq!(
            state.capture(&mut saved, outer, inner, start + Duration::from_millis(50)),
            (false, None)
        );
        assert_eq!((saved.x, saved.y), (Some(2610.0), Some(150.0)));
        assert_eq!(
            state.requested_position(&saved, &monitors, true, start,).0,
            Some(Pos2::new(2610.0, 150.0)),
            "an origin callback must not redirect the board restore"
        );
        let native_client =
            egui::Rect::from_min_size(Pos2::new(2614.0, 181.0), egui::vec2(520.0, 460.0));
        assert_eq!(
            state.capture(
                &mut saved,
                native_client,
                native_client,
                start + Duration::from_millis(450)
            ),
            (false, None)
        );
        assert_eq!(
            state.capture(
                &mut saved,
                native_client,
                native_client,
                start + Duration::from_millis(600)
            ),
            (false, None)
        );
        assert_eq!(
            (saved.x, saved.y),
            (Some(2610.0), Some(150.0)),
            "client decoration offset must not drift the saved frame position"
        );
        let moved_client =
            egui::Rect::from_min_size(Pos2::new(2714.0, 231.0), egui::vec2(540.0, 480.0));
        assert_eq!(
            state.capture(
                &mut saved,
                moved_client,
                moved_client,
                start + Duration::from_millis(700)
            ),
            (true, None)
        );
        assert_eq!(
            state.capture(
                &mut saved,
                moved_client,
                moved_client,
                start + Duration::from_millis(950)
            ),
            (true, None)
        );
        assert_eq!(
            (saved.x, saved.y, saved.width, saved.height),
            (Some(2710.0), Some(200.0), 540.0, 480.0)
        );
        assert!(saved.pinned);
    }

    #[test]
    fn native_frame_becoming_available_after_startup_does_not_shift_later_moves() {
        let mut saved = WindowPreferences {
            x: Some(1960.0),
            y: Some(60.0),
            width: 600.0,
            height: 520.0,
            ..Default::default()
        };
        let monitors = [platform::MonitorRect {
            x: 1920.0,
            y: 0.0,
            width: 1280.0,
            height: 1080.0,
        }];
        let start = Instant::now();
        let mut state = PositionRestore::default();
        assert_eq!(
            state.requested_position(&saved, &monitors, true, start,),
            (Some(Pos2::new(1960.0, 60.0)), true)
        );
        let startup_client =
            egui::Rect::from_min_size(Pos2::new(1964.0, 91.0), egui::vec2(600.0, 520.0));
        state.capture(
            &mut saved,
            startup_client,
            startup_client,
            start + Duration::from_millis(200),
        );
        state.capture(
            &mut saved,
            startup_client,
            startup_client,
            start + Duration::from_millis(500),
        );
        let moved_frame =
            egui::Rect::from_min_size(Pos2::new(2200.0, 100.0), egui::vec2(628.0, 571.0));
        let moved_client =
            egui::Rect::from_min_size(Pos2::new(2204.0, 131.0), egui::vec2(620.0, 540.0));
        state.capture(
            &mut saved,
            moved_frame,
            moved_client,
            start + Duration::from_millis(600),
        );
        state.capture(
            &mut saved,
            moved_frame,
            moved_client,
            start + Duration::from_millis(850),
        );
        assert_eq!((saved.x, saved.y), (Some(2200.0), Some(100.0)));
        assert_eq!((saved.width, saved.height), (620.0, 540.0));
        assert_eq!(
            state.capture(
                &mut saved,
                moved_frame,
                moved_client,
                start + Duration::from_millis(900),
            ),
            (false, None),
            "stable native frame must not rearm position persistence"
        );
        let decoration_changed_client =
            egui::Rect::from_min_size(Pos2::new(2200.0, 100.0), egui::vec2(620.0, 540.0));
        state.capture(
            &mut saved,
            moved_frame,
            decoration_changed_client,
            start + Duration::from_millis(1000),
        );
        state.capture(
            &mut saved,
            moved_frame,
            decoration_changed_client,
            start + Duration::from_millis(1250),
        );
        assert_eq!((saved.x, saved.y), (Some(2200.0), Some(100.0)));
    }

    #[test]
    fn first_open_board_records_native_frame_after_first_position_becomes_available() {
        let mut saved = WindowPreferences {
            width: 520.0,
            height: 460.0,
            ..Default::default()
        };
        let monitors = [platform::MonitorRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }];
        let start = Instant::now();
        let mut state = PositionRestore::default();
        assert_eq!(
            state.requested_position(&saved, &monitors, true, start,),
            (None, false)
        );
        let frame = egui::Rect::from_min_size(Pos2::new(100.0, 100.0), egui::vec2(528.0, 491.0));
        let client = egui::Rect::from_min_size(Pos2::new(104.0, 131.0), egui::vec2(520.0, 460.0));
        state.capture(
            &mut saved,
            frame,
            client,
            start + Duration::from_millis(200),
        );
        state.capture(
            &mut saved,
            frame,
            client,
            start + Duration::from_millis(600),
        );
        let moved_frame =
            egui::Rect::from_min_size(Pos2::new(300.0, 220.0), egui::vec2(528.0, 491.0));
        let moved_client =
            egui::Rect::from_min_size(Pos2::new(304.0, 251.0), egui::vec2(520.0, 460.0));
        state.capture(
            &mut saved,
            moved_frame,
            moved_client,
            start + Duration::from_millis(700),
        );
        state.capture(
            &mut saved,
            moved_frame,
            moved_client,
            start + Duration::from_millis(950),
        );
        assert_eq!((saved.x, saved.y), (Some(300.0), Some(220.0)));
    }

    #[test]
    fn zoom_without_native_movement_keeps_durable_board_geometry() {
        let monitors = [platform::MonitorRect {
            x: 0.0,
            y: 0.0,
            width: 3200.0,
            height: 1080.0,
        }];
        let start = Instant::now();
        for (id, initial, moved) in [
            (0, Pos2::new(480.0, 190.0), Pos2::new(650.0, 260.0)),
            (1, Pos2::new(1880.0, 80.0), Pos2::new(2280.0, 240.0)),
        ] {
            let mut saved = WindowPreferences {
                x: Some(initial.x),
                y: Some(initial.y),
                width: 440.0,
                height: 520.0,
                compact: true,
                ..Default::default()
            };
            let mut state = PositionRestore::default();
            assert_eq!(
                state.requested_position(&saved, &monitors, true, start,).0,
                Some(initial)
            );
            let original = egui::Rect::from_min_size(initial, egui::vec2(440.0, 520.0));
            state.capture_with_zoom(
                &mut saved,
                original,
                original,
                Some(initial),
                1.0,
                start + Duration::from_millis(200),
            );
            state.capture_with_zoom(
                &mut saved,
                original,
                original,
                Some(initial),
                1.0,
                start + Duration::from_millis(600),
            );
            let zoom = 1.1;
            let zoomed_original =
                egui::Rect::from_min_max(original.min / zoom, original.max / zoom);
            for elapsed in [700, 950] {
                state.capture_with_zoom(
                    &mut saved,
                    zoomed_original,
                    zoomed_original,
                    Some(initial / zoom),
                    zoom,
                    start + Duration::from_millis(elapsed),
                );
            }
            assert_eq!(
                (saved.x, saved.y, saved.width, saved.height),
                (Some(initial.x), Some(initial.y), 440.0, 520.0),
                "UI zoom alone did not move or resize window {id}"
            );
            let moved_frame = egui::Rect::from_min_size(moved, egui::vec2(440.0, 520.0));
            let zoomed_moved =
                egui::Rect::from_min_max(moved_frame.min / zoom, moved_frame.max / zoom);
            for elapsed in [1100, 1350] {
                state.capture_with_zoom(
                    &mut saved,
                    zoomed_moved,
                    zoomed_moved,
                    Some(moved / zoom),
                    zoom,
                    start + Duration::from_millis(elapsed),
                );
            }
            assert_eq!(
                (saved.x, saved.y, saved.width, saved.height),
                (Some(moved.x), Some(moved.y), 440.0, 520.0),
                "a genuine zoomed move must save the physical frame in durable units"
            );
            assert_eq!(restore_position(&saved, &monitors), Some(moved));
        }
    }

    #[test]
    fn zoomed_board_builder_converts_durable_frame_and_client_size() {
        let saved = WindowPreferences {
            width: 440.0,
            height: 520.0,
            ..Default::default()
        };
        let frame = Pos2::new(2280.0, 240.0);
        let zoom = 1.1;
        let builder = board_viewport_builder(&saved, true, Some(frame), zoom);
        let position = builder.position.expect("saved frame is requested");
        let size = builder.inner_size.expect("saved client size is requested");
        assert!(same_position(position * zoom, frame));
        assert!((size.x * zoom - 440.0).abs() < 0.01);
        assert!((size.y * zoom - 520.0).abs() < 0.01);
        assert_eq!(valid_geometry_zoom(0.0), None);
        assert_eq!(valid_geometry_zoom(f32::NAN), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn egui_frame_then_equal_client_rect_waits_for_fresh_frame_evidence() {
        let mut saved = WindowPreferences {
            width: 520.0,
            height: 460.0,
            ..Default::default()
        };
        let monitors = [platform::MonitorRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }];
        let start = Instant::now();
        let mut state = PositionRestore::default();
        state.requested_position(&saved, &monitors, true, start);
        let initial_frame =
            egui::Rect::from_min_size(Pos2::new(100.0, 100.0), egui::vec2(528.0, 495.0));
        let initial_client =
            egui::Rect::from_min_size(Pos2::new(104.0, 131.0), egui::vec2(520.0, 460.0));
        state.capture(
            &mut saved,
            initial_frame,
            initial_client,
            start + Duration::from_millis(200),
        );
        state.capture(
            &mut saved,
            initial_frame,
            initial_client,
            start + Duration::from_millis(600),
        );
        let moved_frame =
            egui::Rect::from_min_size(Pos2::new(300.0, 220.0), egui::vec2(528.0, 495.0));
        let moved_client =
            egui::Rect::from_min_size(Pos2::new(304.0, 251.0), egui::vec2(520.0, 460.0));
        state.capture(
            &mut saved,
            moved_frame,
            moved_client,
            start + Duration::from_millis(700),
        );
        state.capture(
            &mut saved,
            moved_frame,
            moved_client,
            start + Duration::from_millis(950),
        );
        assert_eq!((saved.x, saved.y), (Some(300.0), Some(220.0)));

        let equal_client =
            egui::Rect::from_min_size(Pos2::new(404.0, 351.0), egui::vec2(520.0, 460.0));
        state.capture(
            &mut saved,
            equal_client,
            equal_client,
            start + Duration::from_millis(1100),
        );
        state.capture(
            &mut saved,
            equal_client,
            equal_client,
            start + Duration::from_millis(1350),
        );
        assert_eq!(
            (saved.x, saved.y),
            (Some(300.0), Some(220.0)),
            "a later equal rectangle cannot prove that the client origin is the frame"
        );
        state.capture_with_frame(
            &mut saved,
            equal_client,
            equal_client,
            Some(Pos2::new(400.0, 320.0)),
            start + Duration::from_millis(1500),
        );
        state.capture_with_frame(
            &mut saved,
            equal_client,
            equal_client,
            Some(Pos2::new(400.0, 320.0)),
            start + Duration::from_millis(1750),
        );
        assert_eq!((saved.x, saved.y), (Some(400.0), Some(320.0)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn first_session_move_without_frame_evidence_never_saves_client_as_frame() {
        let mut saved = WindowPreferences {
            width: 440.0,
            height: 520.0,
            ..Default::default()
        };
        let monitors = [platform::MonitorRect {
            x: 0.0,
            y: 0.0,
            width: 3200.0,
            height: 1080.0,
        }];
        let start = Instant::now();
        let mut state = PositionRestore::default();
        assert_eq!(
            state.requested_position(&saved, &monitors, true, start,),
            (None, false)
        );
        let initial_client =
            egui::Rect::from_min_size(Pos2::new(1380.0, 293.0), egui::vec2(440.0, 520.0));
        state.capture(
            &mut saved,
            initial_client,
            initial_client,
            start + Duration::from_millis(200),
        );
        state.capture(
            &mut saved,
            initial_client,
            initial_client,
            start + Duration::from_millis(600),
        );
        let moved_client =
            egui::Rect::from_min_size(Pos2::new(2230.0, 513.0), egui::vec2(440.0, 520.0));
        state.capture(
            &mut saved,
            moved_client,
            moved_client,
            start + Duration::from_millis(700),
        );
        state.capture(
            &mut saved,
            moved_client,
            moved_client,
            start + Duration::from_millis(950),
        );
        assert_eq!(
            (saved.x, saved.y),
            (None, None),
            "an unverified client origin cannot be persisted as a frame origin"
        );
        let moved_frame = Pos2::new(2226.0, 482.0);
        state.capture_with_frame(
            &mut saved,
            moved_client,
            moved_client,
            Some(moved_frame),
            start + Duration::from_millis(1100),
        );
        state.capture_with_frame(
            &mut saved,
            moved_client,
            moved_client,
            Some(moved_frame),
            start + Duration::from_millis(1350),
        );
        assert_eq!((saved.x, saved.y), (Some(2226.0), Some(482.0)));
        // A temporarily missing X11 read must keep the known client-to-frame
        // offset instead of committing a later equal-rect client origin.
        let later_client =
            egui::Rect::from_min_size(Pos2::new(2300.0, 600.0), egui::vec2(440.0, 520.0));
        state.capture(
            &mut saved,
            later_client,
            later_client,
            start + Duration::from_millis(1600),
        );
        state.capture(
            &mut saved,
            later_client,
            later_client,
            start + Duration::from_millis(1850),
        );
        assert_eq!((saved.x, saved.y), (Some(2296.0), Some(569.0)));
    }

    #[test]
    fn confirmed_undecorated_zero_extents_save_equal_client_and_frame_position() {
        let mut saved = WindowPreferences::default();
        let monitors = [platform::MonitorRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }];
        let start = Instant::now();
        let mut state = PositionRestore::default();
        state.requested_position(&saved, &monitors, true, start);
        let initial = egui::Rect::from_min_size(Pos2::new(100.0, 100.0), egui::vec2(440.0, 520.0));
        state.capture_with_frame(
            &mut saved,
            initial,
            initial,
            Some(initial.min),
            start + Duration::from_millis(200),
        );
        state.capture_with_frame(
            &mut saved,
            initial,
            initial,
            Some(initial.min),
            start + Duration::from_millis(600),
        );
        let moved = egui::Rect::from_min_size(Pos2::new(300.0, 220.0), egui::vec2(440.0, 520.0));
        state.capture_with_frame(
            &mut saved,
            moved,
            moved,
            Some(moved.min),
            start + Duration::from_millis(700),
        );
        state.capture_with_frame(
            &mut saved,
            moved,
            moved,
            Some(moved.min),
            start + Duration::from_millis(950),
        );
        assert_eq!((saved.x, saved.y), (Some(300.0), Some(220.0)));
    }

    #[test]
    fn root_retries_after_empty_monitor_data_and_unsupported_keeps_saved_coordinates() {
        let mut saved = WindowPreferences {
            x: Some(2610.0),
            y: Some(150.0),
            width: 600.0,
            height: 520.0,
            ..Default::default()
        };
        let primary = [platform::MonitorRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }];
        let start = Instant::now();
        let mut state = PositionRestore::default();
        assert_eq!(
            state.requested_position(&saved, &[], true, start,),
            (None, false)
        );
        assert!(state.needs_repaint());
        let origin = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 520.0));
        assert_eq!(
            state.capture(
                &mut saved,
                origin,
                origin,
                start + Duration::from_millis(20)
            ),
            (false, None)
        );
        assert_eq!((saved.x, saved.y), (Some(2610.0), Some(150.0)));
        assert_eq!(
            state.requested_position(&saved, &primary, true, start + Duration::from_millis(100),),
            (Some(Pos2::new(32.0, 32.0)), true)
        );
        assert_eq!(
            state.requested_position(&saved, &primary, true, start + Duration::from_millis(200),),
            (Some(Pos2::new(32.0, 32.0)), false)
        );

        let mut unsupported = PositionRestore::default();
        assert_eq!(
            unsupported.requested_position(&saved, &primary, false, start,),
            (None, false)
        );
        assert_eq!(
            unsupported.capture(
                &mut saved,
                origin,
                origin,
                start + Duration::from_millis(500)
            ),
            (false, None)
        );
        assert_eq!((saved.x, saved.y), (Some(2610.0), Some(150.0)));
        assert_eq!(
            fitted_board_viewport_builder(&saved, &primary, true, false).position,
            None
        );
    }

    #[test]
    fn secondary_intersection_stays_secondary_and_missing_monitor_uses_primary() {
        let mut saved = WindowPreferences {
            x: Some(2610.0),
            y: Some(150.0),
            width: 520.0,
            height: 460.0,
            ..Default::default()
        };
        let monitors = [
            platform::MonitorRect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            platform::MonitorRect {
                x: 1920.0,
                y: 0.0,
                width: 1280.0,
                height: 1080.0,
            },
        ];
        assert_eq!(
            restore_position(&saved, &monitors),
            Some(Pos2::new(2610.0, 150.0))
        );
        assert_eq!(
            restore_position(&saved, &monitors[..1]),
            Some(Pos2::new(32.0, 32.0))
        );
        saved.x = None;
        assert_eq!(restore_position(&saved, &monitors), None);
    }
    #[test]
    fn showing_saved_root_reapplies_pin_only_when_supported() {
        let commands = |supported| {
            let ctx = Context::default();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                show_root_viewport(ctx, true, supported);
            });
            output.viewport_output[&ViewportId::ROOT].commands.clone()
        };
        let supported = commands(true);
        let shown = supported
            .iter()
            .position(|command| *command == ViewportCommand::Visible(true))
            .expect("root must become visible");
        let pinned = supported
            .iter()
            .position(|command| *command == ViewportCommand::WindowLevel(WindowLevel::AlwaysOnTop))
            .expect("saved root pin must be restored");
        assert!(pinned > shown, "pin command must follow visibility");

        let unsupported = commands(false);
        assert!(unsupported.contains(&ViewportCommand::Visible(true)));
        assert!(
            !unsupported
                .iter()
                .any(|command| matches!(command, ViewportCommand::WindowLevel(_)))
        );
    }

    #[test]
    fn saved_pin_is_inactive_on_unsupported_session_and_returns_on_x11() {
        let mut saved = WindowPreferences::default();
        saved.pinned = true;
        let unsupported = fitted_board_viewport_builder(&saved, &[], false, false);
        assert_ne!(unsupported.window_level, Some(WindowLevel::AlwaysOnTop));
        assert!(!displayed_pin(saved.pinned, false));
        assert!(
            saved.pinned,
            "unsupported session must keep the saved X11 preference"
        );

        let x11 = fitted_board_viewport_builder(&saved, &[], true, true);
        assert_eq!(x11.window_level, Some(WindowLevel::AlwaysOnTop));
        assert!(displayed_pin(saved.pinned, true));
    }

    #[test]
    fn stale_cached_data_shows_its_elapsed_age() {
        let fetched_at = OffsetDateTime::now_utc() - time::Duration::minutes(7);
        assert_eq!(
            last_updated_label(Status::Stale, Some(fetched_at)).as_deref(),
            Some("Last updated 7m ago")
        );
        assert_eq!(last_updated_label(Status::Live, Some(fetched_at)), None);
        assert_eq!(last_updated_label(Status::Stale, None), None);
    }

    #[test]
    fn rendered_text_palette_meets_normal_text_contrast() {
        let contrast = |foreground: egui::Color32, background: egui::Color32| {
            let luminance = |color: egui::Color32| {
                let channel = |value: u8| {
                    let value = value as f32 / 255.0;
                    if value <= 0.04045 {
                        value / 12.92
                    } else {
                        ((value + 0.055) / 1.055).powf(2.4)
                    }
                };
                0.2126 * channel(color.r())
                    + 0.7152 * channel(color.g())
                    + 0.0722 * channel(color.b())
            };
            let (lighter, darker) = (luminance(foreground), luminance(background));
            (lighter.max(darker) + 0.05) / (lighter.min(darker) + 0.05)
        };
        for (foreground, background) in [
            (TEXT, SURFACE),
            (TEXT, SURFACE_RAISED),
            (SECONDARY, SURFACE),
            (egui::Color32::WHITE, ACTION_BLUE),
        ] {
            assert!(contrast(foreground, background) >= 4.5);
        }
    }

    #[test]
    fn dense_grid_breakpoints_keep_single_column_board_usable() {
        assert_eq!(grid_columns(460.0), 1);
        assert_eq!(grid_columns(679.0), 1);
        assert_eq!(grid_columns(680.0), 2);
        assert_eq!(grid_columns(979.0), 2);
        assert_eq!(grid_columns(1000.0), 2);
        assert_eq!(grid_columns(1439.0), 2);
        assert_eq!(grid_columns(1440.0), 3);
    }

    #[test]
    fn column_layout_resolution_clamps_manual_choice_to_min_card_width() {
        use crate::config::ColumnLayout::*;
        // Auto keeps the width-based default at every width.
        assert_eq!(resolve_columns(Auto, 460.0), 1);
        assert_eq!(resolve_columns(Auto, 1020.0), 2);
        assert_eq!(resolve_columns(Auto, 1440.0), 3);
        // A manual choice applies immediately when cards keep the minimum width.
        assert_eq!(resolve_columns(One, 2000.0), 1);
        assert_eq!(resolve_columns(Two, 1020.0), 2);
        assert_eq!(resolve_columns(Three, 1020.0), 3);
        // Too narrow for the chosen count: clamp to what fits.
        assert_eq!(resolve_columns(Two, 460.0), 1);
        assert_eq!(resolve_columns(Three, 700.0), 2);
        assert_eq!(resolve_columns(Three, 460.0), 1);
    }

    #[test]
    fn board_action_applies_column_choice_without_polling_update() {
        let mut prefs = BoardPreferences::default();
        let mut events = Vec::new();
        apply_board_preferences(
            &mut prefs,
            &BoardAction {
                columns: Some(ColumnLayout::Three),
                changed: true,
                ..Default::default()
            },
            |next| events.push(next.refresh_secs),
        );
        assert_eq!(prefs.columns, ColumnLayout::Three);
        assert!(
            events.is_empty(),
            "presentation edits must not notify the poller"
        );
    }

    #[test]
    fn stale_meters_are_muted_while_live_critical_quota_is_urgent() {
        assert_eq!(
            meter_color(8.0, Status::Live, 0, true),
            egui::Color32::from_rgb(239, 98, 98)
        );
        assert_eq!(
            meter_color(75.0, Status::Stale, 1, true),
            egui::Color32::from_rgb(103, 132, 161)
        );
    }

    #[test]
    fn positive_fractional_quota_never_looks_like_empty_quota() {
        assert_eq!(QuotaDisplayMode::Used.percent_label(0.4), "<1%");
        assert_eq!(QuotaDisplayMode::Used.percent_label(0.0), "0%");
        assert_eq!(QuotaDisplayMode::Used.percent_label(1.0), "1%");
    }

    #[test]
    fn header_metadata_preserves_plan_and_reported_price_in_one_bounded_region() {
        let card = ProviderCard {
            usage_breakdown: vec![],
            id: "provider".into(),
            short_name: "Provider with a deliberately long name".into(),
            plan: Some("pro".into()),
            price: Some("$20/month".into()),
            meters: Vec::new(),
            status: Status::Live,
            detail: String::new(),
            link: String::new(),
            fetched_at: None,
        };
        assert_eq!(header_metadata(&card), "Pro · $20/month");
    }
}

#[cfg(test)]
mod quota_display_tests {
    use super::*;
    include!("app/responsive_layout_test.rs");
    include!("app/meter_presentation_test.rs");
    use token_ashtray::model::{Meter, ProductUsage};

    fn card(used_percent: f32) -> ProviderCard {
        ProviderCard {
            id: "fixture".into(),
            short_name: "Fixture".into(),
            plan: None,
            price: None,
            meters: vec![Meter {
                label: "Weekly".into(),
                used_percent,
                resets_at: None,
            }],
            usage_breakdown: vec![],
            status: Status::Live,
            detail: String::new(),
            link: String::new(),
            fetched_at: Some(OffsetDateTime::now_utc()),
        }
    }

    #[test]
    fn red_settings_menu_exposes_about_token_ashtray() {
        use egui::accesskit::{Action, Role};
        {
            let ctx = Context::default();
            ctx.enable_accesskit();
            let render = |event: Option<egui::Event>| {
                let mut input = accessibility_input();
                input.events.extend(event);
                ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let _ = draw_board(
                            ui,
                            "Token Ashtray",
                            &[],
                            &BoardPreferences::default(),
                            false,
                            &mut false,
                            &mut false,
                        );
                    });
                })
            };
            let _ = render(None);
            let closed = render(None);
            let menu = accessibility_nodes(&closed)
                .into_iter()
                .find(|(_, node)| accessibility_text(node) == "Window settings")
                .expect("settings button exists")
                .0;
            let _ = render(Some(accessibility_click(menu)));
            let opened = render(None);
            let about = accessibility_nodes(&opened)
                .into_iter()
                .find(|(_, node)| accessibility_text(node) == "About Token Ashtray")
                .expect("Window settings menu must expose the existing About dialog");
            assert_eq!(about.1.role(), Role::Button);
            assert!(about.1.supports_action(Action::Click));
        }
    }

    fn accessibility_input() -> egui::RawInput {
        let mut input = egui::RawInput::default();

        input.screen_rect = Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        input
    }

    fn accessibility_nodes(
        output: &egui::FullOutput,
    ) -> Vec<(egui::accesskit::NodeId, &egui::accesskit::Node)> {
        let update = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("real AccessKit tree output");
        let root = update.tree.as_ref().expect("tree root").root;
        let mut pending = vec![root];
        let mut result = Vec::new();
        while let Some(id) = pending.pop() {
            assert!(
                !result.iter().any(|(seen, _)| *seen == id),
                "duplicate or cyclic child {id:?}"
            );
            let node = &update
                .nodes
                .iter()
                .find(|(candidate, _)| *candidate == id)
                .expect("reachable child exists")
                .1;
            pending.extend(node.children());
            result.push((id, node));
        }
        assert_eq!(result[0].1.role(), egui::accesskit::Role::Window);
        result
    }

    fn accessibility_text(node: &egui::accesskit::Node) -> &str {
        node.label().or_else(|| node.value()).unwrap_or("")
    }

    fn accessibility_click(id: egui::accesskit::NodeId) -> egui::Event {
        egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target: id,
            data: None,
        })
    }

    #[test]
    fn provider_drag_handles_name_the_actual_focus_target_in_board() {
        use egui::accesskit::{Action, Role};
        {
            for status in [Status::Live, Status::Reconnect] {
                let ctx = Context::default();
                ctx.enable_accesskit();
                let mut provider = card(25.0);
                provider.status = status;
                let id = Id::new((
                    if status == Status::Live {
                        "provider-card"
                    } else {
                        "provider-row"
                    },
                    &provider.id,
                ));
                let render = |focus| {
                    ctx.run(accessibility_input(), |ctx| {
                        if focus {
                            ctx.memory_mut(|memory| memory.request_focus(id));
                        }
                        egui::CentralPanel::default().show(ctx, |ui| {
                            draw_board(
                                ui,
                                "Token Ashtray",
                                &[provider.clone()],
                                &BoardPreferences::default(),
                                false,
                                &mut false,
                                &mut false,
                            );
                        });
                    })
                };
                let _ = render(false);
                let output = render(true);
                let nodes = accessibility_nodes(&output);
                let (node_id, node) = nodes
                    .iter()
                    .find(|(_, node)| accessibility_text(node) == "Drag to reorder Fixture")
                    .expect("provider drag handle has a meaningful accessible name");
                assert_ne!(node.role(), Role::Unknown);
                assert!(node.supports_action(Action::Focus));
                assert!(
                    !node.supports_action(Action::Click),
                    "drag grip must not promise an unimplemented click action"
                );
                assert_eq!(
                    output
                        .platform_output
                        .accesskit_update
                        .as_ref()
                        .unwrap()
                        .focus,
                    *node_id,
                    "name belongs to actual focused DnD target"
                );
            }
        }
    }

    #[test]
    fn unavailable_provider_links_are_named_and_keep_their_real_activation() {
        use egui::accesskit::{Action, Role};
        {
            for linked in [true, false] {
                let ctx = Context::default();
                ctx.enable_accesskit();
                let mut provider = card(25.0);
                provider.status = Status::Error;
                if linked {
                    provider.link = "https://example.invalid/usage/fixture".into();
                }
                let render = |event| {
                    let mut input = accessibility_input();
                    input.events.extend(event);
                    ctx.run(input, |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            draw_board(
                                ui,
                                "Token Ashtray",
                                &[provider.clone()],
                                &BoardPreferences::default(),
                                false,
                                &mut false,
                                &mut false,
                            );
                        });
                    })
                };
                let _ = render(None);
                let output = render(None);
                let nodes = accessibility_nodes(&output);
                let name = if linked {
                    "Fixture — open provider usage page"
                } else {
                    "Fixture"
                };
                let (id, node) = nodes
                    .iter()
                    .find(|(_, node)| accessibility_text(node) == name)
                    .expect("unavailable provider target has a meaningful accessible name");
                assert_eq!(node.role(), if linked { Role::Link } else { Role::Label });
                assert_eq!(node.supports_action(Action::Click), linked);
                if linked {
                    let activated = render(Some(accessibility_click(*id)));
                    assert!(activated.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::OpenUrl(open) if open.url == provider.link)));
                }
            }
        }
    }

    #[test]
    fn accessibility_settings_tree_exposes_selection_and_handles_actions_in_board() {
        use egui::accesskit::{Role, Toggled};
        {
            let ctx = Context::default();
            ctx.enable_accesskit();
            let mut prefs = BoardPreferences::default();
            let render = |prefs: &BoardPreferences, event: Option<egui::Event>| {
                let mut input = accessibility_input();
                let expected_viewport = input.viewport_id;
                input.events.extend(event);
                let mut action = BoardAction::default();
                let output = ctx.run(input, |ctx| {
                    assert_eq!(ctx.viewport_id(), expected_viewport);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = draw_board(
                            ui,
                            "Token Ashtray",
                            &[],
                            prefs,
                            false,
                            &mut false,
                            &mut false,
                        );
                    });
                });
                (output, action)
            };
            let _ = render(&prefs, None);
            let (closed, _) = render(&prefs, None);
            let menu = accessibility_nodes(&closed)
                .into_iter()
                .find(|(_, node)| accessibility_text(node) == "Window settings")
                .expect("named settings button")
                .0;
            let _ = render(&prefs, Some(accessibility_click(menu)));
            let (mut output, _) = render(&prefs, None);
            for expected in [
                QuotaDisplayMode::Remaining,
                QuotaDisplayMode::Used,
                QuotaDisplayMode::Remaining,
            ] {
                if prefs.quota_display_mode != expected {
                    let option = accessibility_nodes(&output)
                        .into_iter()
                        .find(|(_, node)| {
                            node.role() == Role::RadioButton
                                && accessibility_text(node) == expected.label()
                        })
                        .expect("choice exists")
                        .0;
                    let (clicked, action) = render(&prefs, Some(accessibility_click(option)));
                    let clicked_nodes = accessibility_nodes(&clicked);
                    let selected: Vec<_> = clicked_nodes
                        .iter()
                        .filter(|(_, node)| {
                            node.role() == Role::RadioButton
                                && matches!(accessibility_text(node), "Remaining" | "Used")
                                && node.toggled() == Some(Toggled::True)
                        })
                        .collect();
                    assert_eq!(
                        selected.len(),
                        1,
                        "the click frame already has one selection"
                    );
                    assert_eq!(accessibility_text(selected[0].1), expected.label());
                    assert_eq!(action.quota_display_mode, Some(expected));
                    assert!(action.changed);
                    assert!(
                        !action.refresh,
                        "accessibility action must not refresh providers"
                    );
                    prefs.quota_display_mode = action.quota_display_mode.unwrap();
                    output = render(&prefs, None).0;
                }
                let nodes = accessibility_nodes(&output);
                let group = nodes
                    .iter()
                    .find(|(_, node)| {
                        node.role() == Role::RadioGroup
                            && accessibility_text(node) == "Quota display"
                    })
                    .expect("named choice group")
                    .1;
                let radios: Vec<_> = nodes
                    .iter()
                    .filter(|(_, node)| {
                        node.role() == Role::RadioButton
                            && matches!(accessibility_text(node), "Remaining" | "Used")
                    })
                    .collect();
                assert_eq!(radios.len(), 2);
                assert_eq!(
                    radios
                        .iter()
                        .filter(|(_, node)| node.toggled() == Some(Toggled::True))
                        .count(),
                    1
                );
                for (id, node) in radios {
                    assert!(
                        group.children().contains(id),
                        "choice belongs to the named group"
                    );
                    assert_eq!(
                        node.toggled(),
                        Some(if accessibility_text(node) == expected.label() {
                            Toggled::True
                        } else {
                            Toggled::False
                        })
                    );
                    assert!(node.supports_action(egui::accesskit::Action::Click));
                }
            }
        }
    }

    #[test]
    fn accessibility_linked_quota_keeps_its_usage_page_action() {
        for mode in [QuotaDisplayMode::Used, QuotaDisplayMode::Remaining] {
            let ctx = Context::default();
            ctx.enable_accesskit();
            ctx.style_mut(|s| s.interaction.tooltip_delay = 0.0);
            let url = "https://example.invalid/usage";
            let mut provider = card(25.0);
            provider.link = url.into();
            let gauge_id = Id::new(("meter-gauge", &provider.id, 0usize));
            let gauge_rect = std::cell::Cell::new(None);
            let clock = std::cell::Cell::new(0.0);
            let render = |events: Vec<egui::Event>| {
                clock.set(clock.get() + 0.1);
                let mut input = accessibility_input();
                input.time = Some(clock.get());
                input.events = events;
                ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_board(
                            ui,
                            "Token Ashtray",
                            &[provider.clone()],
                            &BoardPreferences {
                                quota_display_mode: mode,
                                ..Default::default()
                            },
                            false,
                            &mut false,
                            &mut false,
                        );
                        gauge_rect.set(ctx.read_response(gauge_id).map(|r| r.rect));
                    });
                })
            };
            let _ = render(vec![]);
            let output = render(vec![]);
            let nodes = accessibility_nodes(&output);
            let (id, node) = nodes
                .iter()
                .find(|(_, node)| accessibility_text(node).starts_with("Weekly:"))
                .expect("linked board quota node");
            let meaning = format!("Weekly: {} {}", mode.percent_label(25.0), mode.label());
            assert_eq!(node.numeric_value(), Some(f64::from(mode.percent(25.0))));
            assert!(accessibility_text(node).contains(&meaning));
            assert!(node.supports_action(egui::accesskit::Action::Click));
            let opens = |output: &egui::FullOutput| {
                output.platform_output.commands.iter().filter(
                    |command| matches!(command, egui::OutputCommand::OpenUrl(open) if open.url == url)).count()
            };
            assert_eq!(opens(&render(vec![accessibility_click(*id)])), 1);
            ctx.memory_mut(|m| m.request_focus(gauge_id));
            let _ = render(vec![]);
            assert_eq!(
                opens(&render(vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }])),
                1
            );
            let _ = render(vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let rect = gauge_rect.get().expect("real gauge response");
            let pos = rect.center();
            let _ = render(vec![egui::Event::PointerMoved(pos)]);
            let _ = render(vec![egui::Event::PointerMoved(pos)]);
            let hover = render(vec![]);
            assert!(
                text(&hover).contains(&meaning),
                "explicit mode tooltip: {}",
                text(&hover)
            );
            assert!(
                text(&hover).contains("Reset unavailable"),
                "tooltip retains explicit reset wording"
            );
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let _ = render(vec![button(true)]);
            assert_eq!(opens(&render(vec![button(false)])), 1);
        }
    }

    fn shapes(output: &egui::FullOutput) -> Vec<&egui::Shape> {
        fn collect<'a>(shape: &'a egui::Shape, found: &mut Vec<&'a egui::Shape>) {
            if let egui::Shape::Vec(children) = shape {
                for child in children {
                    collect(child, found);
                }
            } else {
                found.push(shape);
            }
        }
        let mut found = Vec::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut found);
        }
        found
    }

    fn text(output: &egui::FullOutput) -> String {
        shapes(output)
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) if !is_value_outline(text) => {
                    Some(text.galley.job.text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn codex_windows_keep_their_reported_model_and_window_identity() {
        let labels = [
            "GPT-5.3-Codex-Spark 5 hours",
            "GPT-5.3-Codex-Spark Weekly",
            "Weekly",
            "Custom model rolling allowance",
        ];
        let ctx = Context::default();
        let mut provider = card(25.0);
        provider.id = "codex".into();
        provider.meters = labels
            .iter()
            .map(|label| Meter {
                label: (*label).into(),
                used_percent: 25.0,
                resets_at: None,
            })
            .collect();
        let output = ctx.run(accessibility_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_board(
                    ui,
                    "Token Ashtray",
                    &[provider.clone()],
                    &BoardPreferences::default(),
                    false,
                    &mut false,
                    &mut false,
                );
            });
        });
        let visible = text(&output);
        for label in labels {
            assert!(
                visible.lines().any(|line| line == label),
                "reported identity stays visible: {label}"
            );
        }
    }

    #[test]
    fn narrow_chart_paints_every_long_label_glyph_inside_its_clip() {
        // This is the supported 360-point native minimum, not a layout constant.

        let window = "ResearchModelIdentifierWithoutWordBreaksForNarrowWindowVerification custom rolling allowance";
        let product = "Background research agent tasks with a complete product name";
        for (width, scale, theme) in [360.0, 1040.0].into_iter().flat_map(move |width| {
            [1.0, 2.0].into_iter().flat_map(move |scale| {
                [ThemePreference::Light, ThemePreference::Dark]
                    .into_iter()
                    .map(move |theme| (width, scale, theme))
            })
        }) {
            let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 950.0));
            let ctx = Context::default();
            configure_visuals(&ctx);
            theme::Appearance::default().apply(&ctx, theme);
            let mut provider = card(92.0);
            provider.meters[0].resets_at =
                Some(OffsetDateTime::now_utc() + time::Duration::days(10000));
            provider.meters[0].label = window.into();
            provider.usage_breakdown.push(ProductUsage {
                label: product.into(),
                used_percent: 12.0,
            });
            let mut input = accessibility_input();
            input.screen_rect = Some(screen);
            input
                .viewports
                .get_mut(&input.viewport_id)
                .unwrap()
                .native_pixels_per_point = Some(scale);
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_board(
                        ui,
                        "Token Ashtray",
                        &[provider.clone()],
                        &BoardPreferences::default(),
                        false,
                        &mut false,
                        &mut false,
                    );
                });
            });
            let countdown = reset_label(provider.meters[0].resets_at.unwrap());
            let visible = text(&output);
            assert!(visible.contains("Nearly exhausted"));
            assert!(!visible.contains("Resets in"));
            for expected in [window, product, countdown.as_str(), "Nearly exhausted"] {
                let painted = output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) if text.galley.job.text == expected => {
                            Some((text, clipped.clip_rect))
                        }
                        _ => None,
                    })
                    .expect("full label has a painted text shape, not only an accessibility name");
                let actual: String = painted
                    .0
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter())
                    .map(|glyph| glyph.chr)
                    .filter(|ch| !ch.is_whitespace())
                    .collect();
                let expected_glyphs: String =
                    expected.chars().filter(|ch| !ch.is_whitespace()).collect();
                assert_eq!(
                    actual, expected_glyphs,
                    "painted glyphs must not replace the label with an ellipsis"
                );
                let clip = painted.1.intersect(screen);
                let mut vertices_checked = 0;
                for row in &painted.0.galley.rows {
                    for vertex in &row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()]
                    {
                        vertices_checked += 1;
                        let point = painted.0.pos + vertex.pos.to_vec2();
                        assert!(
                            clip.contains(point),
                            "actual glyph mesh vertex {point:?} lies outside visible clip {clip:?}"
                        );
                    }
                }
                assert!(
                    vertices_checked > 0,
                    "clip verification must inspect actual glyph vertices"
                );
            }
        }
    }

    #[test]
    fn meters_paint_selected_number_zero_based_fill_and_accessible_meaning_with_consumption_color()
    {
        for (used, remaining, consumed) in [
            (25.0, "75%", "25%"),
            (23.0, "77%", "23%"),
            (50.0, "50%", "50%"),
            (75.0, "25%", "75%"),
            (0.0, "100%", "0%"),
            (100.0, "0%", "100%"),
            (0.4, "100%", "<1%"),
            (99.6, "<1%", "100%"),
            (92.0, "8%", "92%"),
        ] {
            for status in [Status::Live, Status::Stale] {
                for (mode, expected) in [
                    (QuotaDisplayMode::Remaining, remaining),
                    (QuotaDisplayMode::Used, consumed),
                ] {
                    let ctx = Context::default();
                    ctx.enable_accesskit();
                    let id = Id::new("quota-test");
                    let output = ctx.run(egui::RawInput::default(), |ctx| {
                        ctx.memory_mut(|memory| memory.request_focus(id));
                        egui::CentralPanel::default().show(ctx, |ui| {
                            draw_quota_meter(
                                ui,
                                &card(used).meters[0],
                                status,
                                "",
                                id,
                                egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 200.0)),
                                0,
                                mode,
                            );
                        });
                    });
                    assert!(
                        text(&output).lines().any(|line| line == expected),
                        "{}",
                        text(&output)
                    );
                    let nodes = accessibility_nodes(&output);
                    let gauge = nodes
                        .iter()
                        .find(|(_, node)| accessibility_text(node).starts_with("Weekly:"))
                        .expect("reachable gauge node")
                        .1;
                    assert_eq!(gauge.role(), egui::accesskit::Role::ProgressIndicator);
                    assert_eq!(gauge.numeric_value(), Some(f64::from(mode.percent(used))));
                    assert_eq!(gauge.min_numeric_value(), Some(0.0));
                    assert_eq!(gauge.max_numeric_value(), Some(100.0));
                    assert!(!gauge.supports_action(egui::accesskit::Action::Click));
                    assert!(
                        accessibility_text(gauge)
                            .contains(&format!("Weekly: {expected} {}", mode.label()))
                    );
                    assert_eq!(
                        accessibility_text(gauge).contains("Stale cached quota"),
                        status == Status::Stale
                    );
                    let descriptions = output
                        .platform_output
                        .events
                        .iter()
                        .filter_map(|event| event.widget_info().label.clone())
                        .collect::<Vec<_>>()
                        .join("\n");
                    assert!(
                        descriptions.contains(&format!("Weekly: {expected} {}", mode.label())),
                        "{descriptions}"
                    );
                    assert_eq!(
                        descriptions.contains("Stale cached quota"),
                        status == Status::Stale
                    );
                    let rectangles: Vec<_> = shapes(&output)
                        .into_iter()
                        .filter_map(|shape| match shape {
                            egui::Shape::Rect(rect) => Some(rect),
                            _ => None,
                        })
                        .collect();
                    let tracks: Vec<_> = rectangles
                        .iter()
                        .filter(|rect| rect.fill == OUTLINE)
                        .collect();
                    assert_eq!(tracks.len(), 1, "one full-range quota track is painted");
                    let track = tracks[0].rect;
                    let painted = shapes(&output);
                    assert!(
                        !painted
                            .iter()
                            .any(|shape| matches!(shape, egui::Shape::LineSegment { .. })),
                        "one uninterrupted track has no endpoint or midpoint ticks"
                    );
                    assert!(
                        !text(&output)
                            .lines()
                            .any(|line| line == "50" || line == mode.label())
                    );
                    let value = painted
                        .iter()
                        .find_map(|shape| match shape {
                            egui::Shape::Text(text)
                                if text.galley.job.text == expected
                                    && !is_value_outline(text)
                                    && track.contains(text.pos + text.galley.size() / 2.0)
                                    && text.galley.job.sections[0].format.font_id.size > 10.0 =>
                            {
                                Some(text)
                            }
                            _ => None,
                        })
                        .expect("actual percentage is painted on the bar");
                    assert!(
                        (value.pos.x + value.galley.size().x / 2.0 - track.center().x).abs() < 1.0,
                        "actual percentage is centered on the scale"
                    );
                    assert!(
                        (value.pos.y + value.galley.size().y / 2.0 - track.center().y).abs() < 0.1,
                        "actual percentage is vertically centered on the track"
                    );
                    assert_eq!(
                        painted
                            .iter()
                            .filter(|shape| matches!(shape,
                        egui::Shape::Text(text) if text.galley.job.text == expected && !is_value_outline(text)))
                            .count(),
                        1,
                        "selected value appears once, with no fixed endpoint reference"
                    );
                    assert!(
                        !painted.iter().any(|shape| matches!(shape,
                        egui::Shape::LineSegment { points, .. } if points[0].x == track.center().x
                            && points[0].y == track.top() && points[1].y == track.bottom())),
                        "no midpoint line crosses the thin track"
                    );
                    let fills: Vec<_> = rectangles
                        .iter()
                        .filter(|rect| rect.fill == meter_color(100.0 - used, status, 0, true))
                        .collect();
                    let expected_percent = match mode {
                        QuotaDisplayMode::Used => used,
                        QuotaDisplayMode::Remaining => 100.0 - used,
                    };
                    let fraction = expected_percent / 100.0;
                    assert_eq!(fills.len(), usize::from(fraction > 0.0));
                    if fraction > 0.0 {
                        let fill = fills[0].rect;
                        assert_eq!(
                            fill.left(),
                            track.left(),
                            "fill begins at the zero baseline"
                        );
                        assert_eq!(fill.top(), track.top());
                        assert_eq!(fill.height(), track.height());
                        assert!(
                            (fill.width() / track.width() - fraction).abs() < 0.00001,
                            "painted extent represents the selected percentage without a minimum-fill distortion"
                        );
                    }
                    let cue = if status == Status::Stale {
                        "Stale cached quota"
                    } else if used == 100.0 {
                        "Allowance exhausted"
                    } else if used >= 90.0 {
                        "Nearly exhausted"
                    } else if used >= 75.0 {
                        "High usage"
                    } else {
                        ""
                    };
                    if !cue.is_empty() {
                        assert!(
                            text(&output).contains(cue),
                            "consumption or stale state is visible without color or hover"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn board_render_single_and_many_meters_and_keep_attribution_used() {
        for mode in [QuotaDisplayMode::Remaining, QuotaDisplayMode::Used] {
            for breakdown in [false, true] {
                let ctx = Context::default();
                ctx.enable_accesskit();
                let mut provider = card(25.0);
                if breakdown {
                    provider.usage_breakdown.push(ProductUsage {
                        label: "Product".into(),
                        used_percent: 12.0,
                    });
                } else {
                    provider.meters.push(Meter {
                        label: "Daily".into(),
                        used_percent: 40.0,
                        resets_at: None,
                    });
                }
                let output = ctx.run(accessibility_input(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_board(
                            ui,
                            "Token Ashtray",
                            &[provider.clone()],
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
                let nodes = accessibility_nodes(&output);
                assert!(
                    nodes
                        .iter()
                        .any(|(_, node)| accessibility_text(node) == "Fixture")
                );
                let gauge = nodes
                    .iter()
                    .find(|(_, node)| accessibility_text(node).starts_with("Weekly:"))
                    .expect("board quota node")
                    .1;
                assert!(
                    accessibility_text(gauge).contains(if mode == QuotaDisplayMode::Used {
                        "25% Used"
                    } else {
                        "75% Remaining"
                    })
                );
                if breakdown {
                    assert!(nodes.iter().any(|(_, node)| accessibility_text(node).contains("Product: 12% of the shared quota used. This is usage attribution, not a separate allowance.")));
                } else {
                    assert!(nodes.iter().any(|(_, node)| {
                        accessibility_text(node).starts_with(if mode == QuotaDisplayMode::Used {
                            "Daily: 40% Used"
                        } else {
                            "Daily: 60% Remaining"
                        })
                    }));
                }
                let visible = text(&output);
                assert!(
                    !visible.contains(&format!("Quota {}", mode.label())),
                    "{visible}"
                );
                assert!(
                    visible.lines().any(|line| line
                        == if mode == QuotaDisplayMode::Used {
                            "25%"
                        } else {
                            "75%"
                        }),
                    "{visible}"
                );
                if breakdown {
                    assert!(visible.contains("Shared quota used"));
                    assert!(visible.lines().any(|line| line == "12%"));
                    // Attribution fill is always 12% regardless of the allowance mode.
                    let bars: Vec<_> = shapes(&output)
                        .into_iter()
                        .filter_map(|shape| match shape {
                            egui::Shape::Rect(rect) if rect.rect.height() == 3.0 => Some(rect),
                            _ => None,
                        })
                        .collect();
                    assert_eq!(bars.len(), 2);
                    assert!((bars[1].rect.width() / bars[0].rect.width() - 0.12).abs() < 0.001);
                } else {
                    assert!(visible.lines().any(|line| line
                        == if mode == QuotaDisplayMode::Used {
                            "40%"
                        } else {
                            "60%"
                        }));
                }
            }
        }
    }

    #[test]
    fn board_statuses_preserve_stale_cue_and_never_invent_unavailable_numbers() {
        for mode in [QuotaDisplayMode::Remaining, QuotaDisplayMode::Used] {
            for status in [Status::Stale, Status::Error, Status::Reconnect] {
                let mut provider = card(25.0);
                provider.status = status;
                provider.fetched_at = Some(OffsetDateTime::now_utc() - time::Duration::minutes(7));
                let ctx = Context::default();
                ctx.enable_accesskit();
                let output = ctx.run(accessibility_input(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_board(
                            ui,
                            "Token Ashtray",
                            &[provider.clone()],
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
                let accessible = accessibility_nodes(&output)
                    .into_iter()
                    .map(|(_, node)| accessibility_text(node))
                    .collect::<Vec<_>>()
                    .join("\n");
                if status == Status::Stale {
                    assert!(accessible.contains("Stale cached quota"));
                    assert!(accessible.contains(if mode == QuotaDisplayMode::Used {
                        "Weekly: 25% Used"
                    } else {
                        "Weekly: 75% Remaining"
                    }));
                } else {
                    assert!(!accessible.contains('%'), "{accessible}");
                }
                let visible = text(&output);
                if status == Status::Stale {
                    assert!(visible.contains("Stale"));
                    assert!(visible.contains("Last updated 7m ago"));
                    assert!(visible.contains(if mode == QuotaDisplayMode::Used {
                        "25%"
                    } else {
                        "75%"
                    }));
                } else {
                    assert!(!visible.contains('%'), "{visible}");
                }
            }
        }
        let mut provider = card(25.0);
        provider.meters.clear();
        let ctx = Context::default();
        ctx.enable_accesskit();
        let output = ctx.run(accessibility_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_board(
                    ui,
                    "Token Ashtray",
                    &[provider.clone()],
                    &BoardPreferences::default(),
                    false,
                    &mut false,
                    &mut false,
                );
            });
        });
        assert!(!text(&output).contains('%'));
        assert!(
            accessibility_nodes(&output)
                .iter()
                .all(|(_, node)| !accessibility_text(node).contains('%'))
        );
    }

    #[test]
    fn root_apply_saves_standalone_and_emits_full_child_preferences() {
        for remote in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let (sender, receiver) = std::sync::mpsc::sync_channel(8);
            let config = ConfigStore::load_from(path.clone());
            let mut app = AshtrayApp {
                state: Arc::new(AppState::remote(Arc::new(RwLock::new(BTreeMap::new())))),
                poller: None,
                updater: (!remote).then(|| {
                    let inert = token_ashtray::updates::UpdaterConfig::inert();
                    let owner = crate::platform::instance::Primary::unguarded();
                    Arc::new(Updater::start(&owner, inert, false, Arc::new(|| {})))
                }),
                tray: None,
                activations: None,
                connection: None,
                preferences_sender: remote.then_some(sender),
                tray_snapshot: Arc::new(RwLock::new(TraySnapshot::default())),
                board_preferences: Arc::new(RwLock::new(config.preferences().board.clone())),
                appearance: theme::Appearance::default(),
                author_video: author_youtube::LatestVideo::default(),
                preferences_save_pending: false,
                config,

                root: WindowPreferences::default(),
                root_restore: PositionRestore::default(),
                main_visible: true,

                about: false,
                message: None,
                startup_visibility_applied: false,
                geometry_last_change: None,
                monitor_rects: vec![],
                remote,
                received_first_action: true,
                closed_message_sent: false,
                refresh_feedback: refresh_feedback::Feedback::default(),
            };
            app.apply(BoardAction {
                quota_display_mode: Some(QuotaDisplayMode::Used),
                changed: true,
                ..Default::default()
            });
            assert_eq!(
                app.board_preferences.read().unwrap().quota_display_mode,
                QuotaDisplayMode::Used
            );
            assert!(app.message.is_none());
            if remote {
                let ChildMessage::SavePreferences(saved) = receiver.try_recv().unwrap() else {
                    panic!("root must send full preferences");
                };
                assert_eq!(saved.board.quota_display_mode, QuotaDisplayMode::Used);
                assert_eq!(saved.window, app.root);
                assert!(
                    receiver.try_recv().is_err(),
                    "a mode edit must not also emit Refresh"
                );
            } else {
                assert_eq!(
                    ConfigStore::load_from(path.clone())
                        .preferences()
                        .board
                        .quota_display_mode,
                    QuotaDisplayMode::Used
                );
            }
            // The opt-in goes to the host from a UI child, or to the
            // standalone board's own updater.
            app.apply(BoardAction {
                updates_enabled: Some(true),
                changed: true,
                ..Default::default()
            });
            if remote {
                let ChildMessage::SavePreferences(saved) = receiver.try_recv().unwrap() else {
                    panic!("root must send full preferences");
                };
                assert!(saved.board.updates_enabled);
                assert_eq!(app.update_status(), UpdateStatus::Off);
            } else {
                let expected = UpdateStatus::Unavailable(Unavailable::NoTrustRoot);
                crate::test_wait::until("the standalone updater follows the opt-in", || {
                    app.update_status() == expected
                });
                assert!(
                    ConfigStore::load_from(path)
                        .preferences()
                        .board
                        .updates_enabled
                );
            }
        }
    }

    #[test]
    fn shared_actions_save_standalone_mode_without_polling_and_update_local_tray() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::load_from(path.clone());
        let shared = Arc::new(RwLock::new(BoardPreferences::default()));
        let window = shared.clone();
        let providers = [card(25.0)];
        let before = local_tray_snapshot(&providers, &shared.read().unwrap());
        let mut events = Vec::new();
        let action = BoardAction {
            quota_display_mode: Some(QuotaDisplayMode::Used),
            changed: true,
            ..Default::default()
        };
        assert!(!action.refresh);
        apply_board_preferences(&mut window.write().unwrap(), &action, |next| {
            events.push(next.refresh_secs)
        });
        store.preferences_mut().board = window.read().unwrap().clone();
        store.save().unwrap();
        assert!(events.is_empty());
        assert_eq!(
            shared.read().unwrap().quota_display_mode,
            QuotaDisplayMode::Used
        );
        assert_eq!(
            ConfigStore::load_from(path)
                .preferences()
                .board
                .quota_display_mode,
            QuotaDisplayMode::Used
        );
        let after = local_tray_snapshot(&providers, &shared.read().unwrap());
        assert_eq!(before.description(), "Fixture 75% Remaining");
        assert_eq!(after.description(), "Fixture 25% Used");
        assert_eq!(before.primary_remaining, after.primary_remaining);
        // A replay through the root also must not notify scheduling.
        apply_board_preferences(&mut shared.write().unwrap(), &action, |next| {
            events.push(next.refresh_secs)
        });
        assert!(events.is_empty());
        apply_board_preferences(
            &mut shared.write().unwrap(),
            &BoardAction {
                refresh_secs: Some(600),
                ..Default::default()
            },
            |next| events.push(next.refresh_secs),
        );
        assert_eq!(events, [600]);
        for status in [Status::Stale, Status::Error, Status::Reconnect] {
            let mut provider = providers[0].clone();
            provider.status = status;
            let snapshot = local_tray_snapshot(&[provider], &shared.read().unwrap());
            assert_eq!(snapshot.primary_remaining, None);
            assert!(!snapshot.description().contains('%'));
        }
    }
}

#[cfg(test)]
mod drag_preview_red_test;

#[cfg(test)]
mod settings_menu_test;

#[cfg(test)]
mod application_icon_tests {
    use super::*;

    #[test]
    fn static_window_icon_uses_approved_export_for_board_cache() {
        let expected = eframe::icon_data::from_png_bytes(include_bytes!(
            "../assets/application/token-ashtray-64.png"
        ))
        .unwrap();
        let cached = application_icon();
        assert_eq!(
            (cached.width, cached.height),
            (expected.width, expected.height)
        );
        assert_eq!(
            cached.rgba, expected.rgba,
            "window icon must contain the approved artwork"
        );
        let window =
            fitted_board_viewport_builder(&WindowPreferences::default(), &[], false, false);
        let board_icon = window.icon.expect("board icon");
        assert_eq!(board_icon.rgba, expected.rgba);
        assert!(
            Arc::ptr_eq(&cached, &board_icon),
            "all windows share the immutable static icon"
        );
    }
}

#[cfg(test)]
mod about_test;

#[cfg(test)]
mod geometry_lifecycle_test;

#[cfg(test)]
mod theme_preference_red_test {
    use super::*;

    #[test]
    fn system_light_input_uses_light_visuals() {
        let ctx = Context::default();
        let input = egui::RawInput {
            system_theme: Some(egui::Theme::Light),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            configure_visuals(ctx);
            assert_eq!(
                ctx.theme(),
                egui::Theme::Light,
                "egui received OS light mode"
            );
            assert!(
                !ctx.style().visuals.dark_mode,
                "System mode must paint a light board when the OS is light"
            );
        });
    }

    #[test]
    fn default_board_theme_preference_serializes_as_system() {
        let value = serde_json::to_value(BoardPreferences::default()).unwrap();
        assert_eq!(
            value["theme_preference"], "system",
            "global board theme should default to and persist System"
        );
    }
}

#[cfg(test)]
mod theme_test;

#[cfg(test)]
mod author_youtube_test;

#[cfg(test)]
mod work_area_tests {
    use super::*;
    #[test]
    fn first_open_and_saved_board_fit_small_work_area_and_minimum() {
        let areas = [platform::MonitorRect {
            x: -320.0,
            y: 24.0,
            width: 320.0,
            height: 276.0,
        }];
        for saved in [false, true] {
            let mut window = WindowPreferences::default();
            if saved {
                window.x = Some(-900.0);
                window.y = Some(900.0);
                window.width = 1400.0;
                window.height = 900.0;
            }
            let builder = fitted_board_viewport_builder(&window, &areas, false, true);
            let size = builder.inner_size.unwrap();
            assert!(
                size.x <= 320.0 && size.y <= 276.0,
                "client {size:?} exceeds usable area"
            );
            let min = builder.min_inner_size.unwrap();
            assert!(
                min.x <= size.x && min.y <= size.y,
                "minimum defeats fitting"
            );
            let pos = builder.position.expect("first open also needs fitting");
            assert!(
                pos.x >= -320.0
                    && pos.y >= 24.0
                    && pos.x + size.x <= 0.0
                    && pos.y + size.y <= 300.0
            );
        }
    }
}

#[cfg(test)]
mod scaled_work_area_tests {
    use super::*;
    #[test]
    fn physical_work_area_fits_at_native_scale_independent_of_ui_zoom() {
        for scale in [1.0, 1.5, 2.0] {
            for zoom in [1.0, 1.1] {
                let areas = [platform::MonitorRect {
                    x: -1920.0 / scale,
                    y: 0.0,
                    width: 1920.0 / scale,
                    height: 984.0 / scale,
                }];
                for saved in [false, true] {
                    let window = WindowPreferences {
                        x: saved.then_some(-1800.0),
                        y: saved.then_some(900.0),
                        width: if saved { 1400.0 } else { 440.0 },
                        height: if saved { 900.0 } else { 520.0 },
                        ..Default::default()
                    };
                    let overhead = egui::vec2(8.0, 30.0);
                    let (size, pos) = fit_geometry(&window, &areas, overhead).unwrap();
                    assert!((size.y + overhead.y) * scale <= 984.01);
                    assert!((size.x + overhead.x) * scale <= 1920.01);
                    assert!(pos.x >= areas[0].x && pos.y >= 0.0);
                    assert!(pos.x + size.x + overhead.x <= 0.01);
                    let fitted = fitted_preferences(&window, &areas, overhead);
                    let builder = board_viewport_builder(&fitted, false, Some(pos), zoom);
                    assert_eq!(builder.inner_size.unwrap() * zoom, size);
                    assert_eq!(builder.position.unwrap() * zoom, pos);
                }
            }
        }
    }
    #[test]
    fn root_commands_fit_first_open_and_keep_unsupported_saved_position() {
        let areas = [platform::MonitorRect {
            x: 0.0,
            y: 24.0,
            width: 320.0,
            height: 276.0,
        }];
        for supported in [true, false] {
            let mut window = WindowPreferences::default();
            let mut restore = PositionRestore::default();
            let ctx = Context::default();
            let out = ctx.run(egui::RawInput::default(), |ctx| {
                fit_viewport_in_area(
                    ctx,
                    &mut window,
                    &mut restore,
                    &areas,
                    egui::vec2(8.0, 30.0),
                    supported,
                );
            });
            let commands = &out.viewport_output[&ViewportId::ROOT].commands;
            assert!(commands.contains(&ViewportCommand::InnerSize(egui::vec2(312.0, 246.0))));
            assert!(commands.contains(&ViewportCommand::MinInnerSize(egui::vec2(312.0, 246.0))));
            assert_eq!(
                commands
                    .iter()
                    .any(|c| matches!(c, ViewportCommand::OuterPosition(_))),
                supported
            );
            if !supported {
                assert_eq!((window.x, window.y), (None, None));
            }
        }
        assert!(fit_geometry(&WindowPreferences::default(), &[], egui::Vec2::ZERO).is_none());
    }
}

#[cfg(test)]
mod toolbar_test;

#[cfg(all(test, target_os = "windows"))]
mod windows_work_area_test;

#[cfg(test)]
mod refresh_feedback_test;
