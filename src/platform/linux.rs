use std::{
    collections::VecDeque,
    env, fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::mpsc,
};

use super::{SharedTraySnapshot, TrayAction, ashtray_rgba};
use directories::BaseDirs;
use eframe::egui::{Context, Pos2, Rect};
use ksni::{
    Icon, MenuItem, ToolTip,
    blocking::{Handle, TrayMethods},
    menu::{CheckmarkItem, StandardItem},
};
use x11rb::{
    connect,
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        xproto::{AtomEnum, ConnectionExt as _, GetPropertyReply, Window},
    },
    rust_connection::RustConnection,
};

pub const PIN_UNSUPPORTED_EXPLANATION: &str =
    "Wayland compositors decide window stacking. Use an X11 session for this app's pin control.";

pub struct Tray {
    handle: Handle<StatusNotifierTray>,
    actions: mpsc::Receiver<TrayAction>,
}
impl Tray {
    pub fn new(context: Context, snapshot: SharedTraySnapshot) -> Result<Self, String> {
        let (sender, actions) = mpsc::channel();
        let handle = StatusNotifierTray {
            sender,
            context,
            snapshot,
        }
        .spawn()
        .map_err(|error| format!("No usable StatusNotifier tray host: {error}"))?;
        Ok(Self { handle, actions })
    }
    pub fn poll(&self) -> TrayAction {
        self.actions.try_recv().unwrap_or(TrayAction::None)
    }
    pub fn sync(&self) {
        let _ = self.handle.update(|_| ());
    }
}
struct StatusNotifierTray {
    sender: mpsc::Sender<TrayAction>,
    context: Context,
    snapshot: SharedTraySnapshot,
}

#[cfg(test)]
mod quota_display_tests {
    use super::*;
    use crate::{
        config::QuotaDisplayMode,
        platform::{TrayProvider, TraySnapshot},
    };
    use ksni::Tray as _;
    use std::sync::{Arc, RwLock};

    #[test]
    fn tray_primary_and_secondary_activation_open_normal_board() {
        let (sender, actions) = mpsc::channel();
        let mut tray = StatusNotifierTray {
            sender,
            context: Context::default(),
            snapshot: Arc::new(RwLock::new(TraySnapshot::default())),
        };
        tray.activate(1000, 1000);
        assert_eq!(actions.try_recv().unwrap(), TrayAction::OpenBoard);
        tray.secondary_activate(0, 0);
        assert_eq!(actions.try_recv().unwrap(), TrayAction::OpenBoard);
    }

    #[test]
    fn sni_renders_primary_quota_at_every_advertised_size() {
        let (sender, _) = mpsc::channel();
        let snapshot = Arc::new(RwLock::new(TraySnapshot {
            providers: vec![TrayProvider {
                name: "Other provider".into(),
                used: Some(17.0),
                reset: None,
            }],
            any_readable: true,
            quota_display_mode: QuotaDisplayMode::Used,
            ..Default::default()
        }));
        let tray = StatusNotifierTray {
            sender,
            context: Context::default(),
            snapshot: snapshot.clone(),
        };
        for remaining in [Some(100.0), Some(50.0), Some(10.0), Some(0.0), None] {
            snapshot.write().unwrap().primary_remaining = remaining;
            let icons = tray.icon_pixmap();
            assert_eq!(icons.len(), 4);
            for (icon, size) in icons.iter().zip([16, 24, 32, 48]) {
                assert_eq!((icon.width, icon.height), (size as i32, size as i32));
                assert_eq!(icon.data.len(), (size * size * 4) as usize);
                let rgba: Vec<_> = icon
                    .data
                    .chunks_exact(4)
                    .flat_map(|argb| [argb[1], argb[2], argb[3], argb[0]])
                    .collect();
                assert_eq!(
                    rgba,
                    ashtray_rgba(size, remaining),
                    "SNI must publish the primary remaining quota, even in Used mode"
                );
            }
        }
    }

    #[test]
    fn sni_tooltip_changes_meaning_without_changing_icon_pixels() {
        let (sender, _) = mpsc::channel();
        let snapshot = Arc::new(RwLock::new(TraySnapshot {
            providers: vec![TrayProvider {
                name: "Fixture".into(),
                used: Some(25.0),
                reset: None,
            }],
            primary_remaining: Some(75.0),
            any_readable: true,
            ..Default::default()
        }));
        // Invoke the actual SNI trait methods without registering on any bus.
        let tray = StatusNotifierTray {
            sender,
            context: Context::default(),
            snapshot: snapshot.clone(),
        };
        let before = tray.icon_pixmap();
        assert_eq!(tray.tool_tip().description, "Fixture 75% Remaining");
        snapshot.write().unwrap().quota_display_mode = QuotaDisplayMode::Used;
        assert_eq!(tray.tool_tip().description, "Fixture 25% Used");
        assert_eq!(
            before.iter().map(|icon| &icon.data).collect::<Vec<_>>(),
            tray.icon_pixmap()
                .iter()
                .map(|icon| &icon.data)
                .collect::<Vec<_>>()
        );
    }
}
impl StatusNotifierTray {
    fn dispatch(&self, action: TrayAction) {
        let _ = self.sender.send(action);
        self.context.request_repaint();
    }
    fn snapshot(&self) -> super::TraySnapshot {
        self.snapshot.read().map(|s| s.clone()).unwrap_or_default()
    }
}
impl ksni::Tray for StatusNotifierTray {
    fn id(&self) -> String {
        "token-ashtray".into()
    }
    fn title(&self) -> String {
        "Token Ashtray".into()
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        [16, 24, 32, 48]
            .into_iter()
            .map(|size| {
                let rgba = ashtray_rgba(size, self.snapshot().primary_remaining);
                let (pixels, remainder) = rgba.as_chunks::<4>();
                debug_assert!(remainder.is_empty());
                let data = pixels
                    .iter()
                    .flat_map(|pixel| [pixel[3], pixel[0], pixel[1], pixel[2]])
                    .collect();
                Icon {
                    width: size as i32,
                    height: size as i32,
                    data,
                }
            })
            .collect()
    }
    fn tool_tip(&self) -> ToolTip {
        let snapshot = self.snapshot();
        let description = snapshot.description();
        ToolTip {
            title: "Token Ashtray".into(),
            description,
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.dispatch(TrayAction::OpenBoard);
    }
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.dispatch(TrayAction::OpenBoard);
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let action = |label: &str, wanted: TrayAction| {
            let sender = self.sender.clone();
            let context = self.context.clone();
            StandardItem {
                label: label.into(),
                activate: Box::new(move |_| {
                    let _ = sender.send(wanted);
                    context.request_repaint();
                }),
                ..Default::default()
            }
            .into()
        };
        let sender = self.sender.clone();
        let context = self.context.clone();
        vec![
            action("Refresh now", TrayAction::Refresh),
            action("Open board", TrayAction::OpenBoard),
            CheckmarkItem {
                label: "Start at login".into(),
                checked: autostart_enabled(),
                activate: Box::new(move |_| {
                    let _ = sender.send(TrayAction::ToggleAutostart);
                    context.request_repaint();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            action("About Token Ashtray", TrayAction::About),
            action("Quit", TrayAction::Quit),
        ]
    }
}
pub fn pin_supported() -> bool {
    env::var("XDG_SESSION_TYPE").is_ok_and(|session| session.eq_ignore_ascii_case("x11"))
}
pub fn placement_supported() -> bool {
    pin_supported()
}
pub fn monitor_rects() -> Vec<super::MonitorRect> {
    let Ok((connection, screen)) = connect(None) else {
        return Vec::new();
    };
    let Some(root) = connection
        .setup()
        .roots
        .get(screen)
        .map(|screen| screen.root)
    else {
        return Vec::new();
    };
    connection
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|mut reply| {
            // Off-screen recovery uses the first rectangle as its fallback.
            reply.monitors.sort_by_key(|monitor| !monitor.primary);
            reply
                .monitors
                .into_iter()
                .map(|monitor| super::MonitorRect {
                    x: monitor.x as f32,
                    y: monitor.y as f32,
                    width: monitor.width as f32,
                    height: monitor.height as f32,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn x_atom(connection: &RustConnection, name: &str) -> Option<u32> {
    let atom = connection
        .intern_atom(true, name.as_bytes())
        .ok()?
        .reply()
        .ok()?
        .atom;
    (atom != 0).then_some(atom)
}

fn x_property(
    connection: &RustConnection,
    window: Window,
    property: u32,
    kind: u32,
    max_words: u32,
) -> Option<GetPropertyReply> {
    let reply = connection
        .get_property(false, window, property, kind, 0, max_words)
        .ok()?
        .reply()
        .ok()?;
    (reply.bytes_after == 0 && reply.type_ == kind).then_some(reply)
}

fn x_cardinals(reply: &GetPropertyReply, count: usize) -> Option<Vec<u32>> {
    let values = reply.value32()?.collect::<Vec<_>>();
    (reply.format == 32 && values.len() == count).then_some(values)
}

fn class_matches(value: &[u8], class: &str) -> bool {
    let parts = value.split(|byte| *byte == 0).collect::<Vec<_>>();
    parts.len() == 3 && !parts[0].is_empty() && parts[1] == class.as_bytes() && parts[2].is_empty()
}

fn client_origin(connection: &RustConnection, window: Window, root: Window) -> Option<(f32, f32)> {
    let translated = connection
        .translate_coordinates(window, root, 0, 0)
        .ok()?
        .reply()
        .ok()?;
    Some((translated.dst_x as f32, translated.dst_y as f32))
}

fn frame_origin_px(
    connection: &RustConnection,
    window: Window,
    root: Window,
    client: (f32, f32),
    extents_atom: Option<u32>,
) -> Option<(f32, f32)> {
    if let Some(reply) = extents_atom
        .and_then(|atom| x_property(connection, window, atom, AtomEnum::CARDINAL.into(), 4))
    {
        let extents = x_cardinals(&reply, 4)?;
        let left = i32::try_from(extents[0]).ok()?;
        let top = i32::try_from(extents[2]).ok()?;
        return Some((client.0 - left as f32, client.1 - top as f32));
    }

    // Some X11 window managers do not publish EWMH extents. The topmost
    // client ancestor below the root is their native frame; an un-reparented
    // client (including an undecorated window) is its own frame.
    let mut ancestor = window;
    for _ in 0..16 {
        let tree = connection.query_tree(ancestor).ok()?.reply().ok()?;
        if tree.parent == root {
            let position = client_origin(connection, ancestor, root)?;
            let border = connection
                .get_geometry(ancestor)
                .ok()?
                .reply()
                .ok()?
                .border_width as f32;
            return Some((position.0 - border, position.1 - border));
        }
        if tree.parent == 0 || tree.parent == ancestor {
            return None;
        }
        ancestor = tree.parent;
    }
    None
}

fn frame_points(frame_px: (f32, f32), pixels_per_point: f32) -> Option<Pos2> {
    if !pixels_per_point.is_finite() || pixels_per_point <= 0.0 {
        return None;
    }
    Some(Pos2::new(
        frame_px.0 / pixels_per_point,
        frame_px.1 / pixels_per_point,
    ))
}

fn client_windows(connection: &RustConnection, root: Window) -> Option<Vec<Window>> {
    if let Some(atom) = x_atom(connection, "_NET_CLIENT_LIST")
        && let Some(reply) = x_property(connection, root, atom, AtomEnum::WINDOW.into(), 4096)
        && reply.format == 32
    {
        return Some(reply.value32()?.collect());
    }

    // A non-EWMH window manager can still reparent clients. Search its live
    // hierarchy with a finite bound rather than leaving new positions unsaved.
    let mut queue = VecDeque::from([(root, 0_u8)]);
    let mut windows = Vec::new();
    while let Some((parent, depth)) = queue.pop_front() {
        let children = connection.query_tree(parent).ok()?.reply().ok()?.children;
        if windows.len() + children.len() > 4096 {
            return None;
        }
        for child in children {
            windows.push(child);
            if depth < 16 {
                queue.push_back((child, depth + 1));
            }
        }
    }
    Some(windows)
}

/// Fresh frame coordinates for exactly one owned, live X11 viewport.
/// A failed or ambiguous identity check never supplies a guessed position.
pub fn verified_frame_origin(title: &str, inner: Rect, pixels_per_point: f32) -> Option<Pos2> {
    if !pixels_per_point.is_finite() || pixels_per_point <= 0.0 {
        return None;
    }
    let (connection, screen) = connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let title_atom = x_atom(&connection, "_NET_WM_NAME")?;
    let utf8_atom = x_atom(&connection, "UTF8_STRING")?;
    let pid_atom = x_atom(&connection, "_NET_WM_PID")?;
    let extents_atom = x_atom(&connection, "_NET_FRAME_EXTENTS");
    let windows = client_windows(&connection, root)?;
    let class = std::env::args_os()
        .next()
        .and_then(|arg| PathBuf::from(arg).file_name().map(|name| name.to_owned()))?
        .into_string()
        .ok()?;
    let expected_client = inner.min * pixels_per_point;
    let expected_size = inner.size() * pixels_per_point;
    let mut match_frame = None;
    for window in windows {
        let Some(name) = x_property(&connection, window, title_atom, utf8_atom, 128) else {
            continue;
        };
        if name.format != 8 || name.value != title.as_bytes() {
            continue;
        }
        let Some(pid) = x_property(&connection, window, pid_atom, AtomEnum::CARDINAL.into(), 1)
        else {
            continue;
        };
        if x_cardinals(&pid, 1).and_then(|values| values.first().copied())
            != Some(std::process::id())
        {
            continue;
        }
        let Some(window_class) = x_property(
            &connection,
            window,
            AtomEnum::WM_CLASS.into(),
            AtomEnum::STRING.into(),
            128,
        ) else {
            continue;
        };
        if window_class.format != 8 || !class_matches(&window_class.value, &class) {
            continue;
        }
        let native_client = client_origin(&connection, window, root)?;
        let geometry = connection.get_geometry(window).ok()?.reply().ok()?;
        if (native_client.0 - expected_client.x).abs() > 2.0
            || (native_client.1 - expected_client.y).abs() > 2.0
            || (geometry.width as f32 - expected_size.x).abs() > 2.0
            || (geometry.height as f32 - expected_size.y).abs() > 2.0
        {
            continue;
        }
        let frame_px = frame_origin_px(&connection, window, root, native_client, extents_atom)?;
        let frame = frame_points(frame_px, pixels_per_point)?;
        if match_frame.replace(frame).is_some() {
            return None;
        }
    }
    match_frame
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn frame_points_use_current_zoom_inclusive_scale() {
        assert_eq!(
            frame_points((2226.0, 482.0), 1.5),
            Some(Pos2::new(1484.0, 482.0 / 1.5))
        );
        assert_eq!(
            frame_points((300.0, 220.0), 1.0),
            Some(Pos2::new(300.0, 220.0))
        );
        assert_eq!(frame_points((300.0, 220.0), 0.0), None);
    }

    #[test]
    fn class_identity_requires_complete_instance_and_exact_class() {
        assert!(class_matches(
            b"token-ashtray\0token-ashtray\0",
            "token-ashtray"
        ));
        assert!(!class_matches(
            b"token-ashtray\0token-ashtray-old\0",
            "token-ashtray"
        ));
        assert!(!class_matches(
            b"token-ashtray\0token-ashtray",
            "token-ashtray"
        ));
    }
}
pub fn credential_base_paths() -> super::CredentialBasePaths {
    super::CredentialBasePaths::from_home(
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_owned())
            .unwrap_or_default(),
    )
}
fn autostart_path() -> Option<PathBuf> {
    BaseDirs::new().map(|dirs| dirs.config_dir().join("autostart/token-ashtray.desktop"))
}
pub fn autostart_enabled() -> bool {
    autostart_path().is_some_and(|path| {
        fs::read_to_string(path).is_ok_and(|file| {
            env::current_exe().is_ok_and(|binary| desktop_entry_enabled(&file, &binary))
        })
    })
}
fn desktop_command(binary: &std::path::Path) -> String {
    let escaped = binary
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    // Desktop-entry string unescaping happens before Exec argument unquoting.
    format!("\"{escaped}\" --autostart").replace('\\', "\\\\")
}
fn desktop_entry_enabled(file: &str, binary: &std::path::Path) -> bool {
    let mut in_entry = false;
    let mut fields = std::collections::HashMap::new();
    for line in file.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry
            && !line.starts_with('#')
            && let Some((key, value)) = line.split_once('=')
        {
            fields.insert(key.trim(), value.trim());
        }
    }
    fields.get("Type") == Some(&"Application")
        && fields.get("X-Token-Ashtray") == Some(&"true")
        && fields.get("Exec").copied() == Some(desktop_command(binary).as_str())
        && fields.get("Hidden") != Some(&"true")
        && fields.get("X-GNOME-Autostart-enabled") != Some(&"false")
}
fn directory_identity(parent: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(parent)
        .ok()
        .filter(|metadata| metadata.is_dir())
        .map(|metadata| (metadata.dev(), metadata.ino()))
}
// The desktop entry survives process restarts; its device/inode marker lets us
// remove only the same directory this app created, and only while it is empty.
fn owned_directory_identity(file: &str) -> Option<(u64, u64)> {
    let mut in_entry = false;
    for line in file.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry
            && let Some(value) = line.strip_prefix("X-Token-Ashtray-Created-Autostart-Dir=")
        {
            let (device, inode) = value.split_once(':')?;
            return Some((device.parse().ok()?, inode.parse().ok()?));
        }
    }
    None
}
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let path = autostart_path().ok_or("could not determine XDG config directory")?;
    if enabled {
        let binary = env::current_exe().map_err(|e| e.to_string())?;
        enable_autostart_at(&path, &binary)
    } else {
        disable_autostart_at(&path)
    }
}
fn disable_autostart_at(path: &std::path::Path) -> Result<(), String> {
    let parent = path.parent().ok_or("autostart path has no parent")?;
    let ownership = fs::read_to_string(path)
        .ok()
        .and_then(|file| owned_directory_identity(&file));
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    }
    if ownership.is_some_and(|identity| directory_identity(parent) == Some(identity)) {
        match fs::remove_dir(parent) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => {
                return Err(format!(
                    "desktop entry removed, but could not remove its empty autostart directory: {error}"
                ));
            }
        }
    }
    Ok(())
}
fn enable_autostart_at(path: &std::path::Path, binary: &std::path::Path) -> Result<(), String> {
    let parent = path.parent().ok_or("autostart path has no parent")?;
    let old_ownership = fs::read_to_string(path)
        .ok()
        .and_then(|file| owned_directory_identity(&file))
        .filter(|identity| directory_identity(parent) == Some(*identity));
    fs::create_dir_all(
        parent
            .parent()
            .ok_or("autostart path has no config parent")?,
    )
    .map_err(|error| error.to_string())?;
    let created_ownership = match fs::create_dir(parent) {
        Ok(()) => Some(
            directory_identity(parent)
                .ok_or("could not inspect newly created autostart directory")?,
        ),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => None,
        Err(error) => return Err(error.to_string()),
    };
    let ownership = created_ownership.or(old_ownership);
    let command = desktop_command(binary);
    let marker = ownership.map_or_else(String::new, |(device, inode)| {
        format!("X-Token-Ashtray-Created-Autostart-Dir={device}:{inode}\n")
    });
    let body = format!(
        "[Desktop Entry]\nType=Application\nName=Token Ashtray\nExec={command}\nTerminal=false\nX-GNOME-Autostart-enabled=true\nX-Token-Ashtray=true\n{marker}"
    );
    if let Err(error) = fs::write(path, body) {
        if created_ownership.is_some() {
            let _ = fs::remove_dir(parent);
        }
        return Err(error.to_string());
    }
    Ok(())
}

/// No main-thread native tray message pump is needed on this platform.
pub fn pump_events() {}

#[cfg(test)]
mod tests {
    use super::{
        desktop_command, desktop_entry_enabled, disable_autostart_at, enable_autostart_at,
    };
    use std::{fs, path::Path};

    #[test]
    fn app_created_autostart_directory_is_removed_after_restart_and_repeat_enable() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        fs::create_dir(&config).unwrap();
        let parent = config.join("autostart");
        let entry = parent.join("token-ashtray.desktop");
        let binary = Path::new("/opt/Token Ashtray/token-ashtray");

        enable_autostart_at(&entry, binary).unwrap();
        assert!(parent.is_dir());
        assert!(entry.is_file());
        let installed = fs::read_to_string(&entry).unwrap();
        // A later process has only the on-disk entry, not in-memory ownership.
        fs::write(&entry, installed.replace("/opt/", "/stale/")).unwrap();
        assert!(!desktop_entry_enabled(
            &fs::read_to_string(&entry).unwrap(),
            binary
        ));
        enable_autostart_at(&entry, binary).unwrap();
        assert!(desktop_entry_enabled(
            &fs::read_to_string(&entry).unwrap(),
            binary
        ));
        disable_autostart_at(&entry).unwrap();
        assert!(!entry.exists());
        assert!(!parent.exists());
        assert!(installed.contains("X-Token-Ashtray-Created-Autostart-Dir="));
        disable_autostart_at(&entry).unwrap();
    }

    #[test]
    fn preexisting_empty_directory_survives_enable_and_disable() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("autostart");
        fs::create_dir(&parent).unwrap();
        let entry = parent.join("token-ashtray.desktop");
        enable_autostart_at(&entry, Path::new("/opt/token-ashtray")).unwrap();
        disable_autostart_at(&entry).unwrap();
        assert!(parent.is_dir());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
    }

    #[test]
    fn unrelated_entry_keeps_created_directory_and_its_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("autostart");
        let entry = parent.join("token-ashtray.desktop");
        enable_autostart_at(&entry, Path::new("/opt/token-ashtray")).unwrap();
        let other = parent.join("another.desktop");
        let original = b"[Desktop Entry]\nName=Another app\n";
        fs::write(&other, original).unwrap();
        disable_autostart_at(&entry).unwrap();
        assert!(!entry.exists());
        assert!(parent.is_dir());
        assert_eq!(fs::read(&other).unwrap(), original);
    }

    #[test]
    fn preexisting_nonempty_directory_preserves_unrelated_entry() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("autostart");
        fs::create_dir(&parent).unwrap();
        let other = parent.join("another.desktop");
        let original = b"[Desktop Entry]\nName=Another app\n";
        fs::write(&other, original).unwrap();
        let entry = parent.join("token-ashtray.desktop");
        enable_autostart_at(&entry, Path::new("/opt/token-ashtray")).unwrap();
        disable_autostart_at(&entry).unwrap();
        assert!(parent.is_dir());
        assert_eq!(fs::read(&other).unwrap(), original);
    }

    #[test]
    fn replacement_directory_is_not_removed_by_stale_ownership_marker() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("autostart");
        let old = temp.path().join("old-autostart");
        let entry = parent.join("token-ashtray.desktop");
        enable_autostart_at(&entry, Path::new("/opt/token-ashtray")).unwrap();
        let installed = fs::read(&entry).unwrap();
        fs::rename(&parent, &old).unwrap();
        fs::create_dir(&parent).unwrap();
        fs::write(&entry, installed).unwrap();
        disable_autostart_at(&entry).unwrap();
        assert!(parent.is_dir());
        assert!(!entry.exists());
    }

    #[test]
    fn failed_entry_deletion_returns_error_and_preserves_directory() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("autostart");
        let entry = parent.join("token-ashtray.desktop");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&entry).unwrap();
        assert!(disable_autostart_at(&entry).is_err());
        assert!(entry.is_dir());
        assert!(parent.is_dir());
    }

    #[test]
    fn externally_corrupted_entry_can_still_be_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("autostart");
        let entry = parent.join("token-ashtray.desktop");
        fs::create_dir(&parent).unwrap();
        fs::write(&entry, [0xff]).unwrap();
        disable_autostart_at(&entry).unwrap();
        assert!(!entry.exists());
        assert!(parent.is_dir());
    }

    #[test]
    fn autostart_requires_a_launchable_current_entry() {
        let binary = Path::new("/opt/Token Ashtray/token-ashtray");
        let valid = "[Desktop Entry]\nType=Application\nX-Token-Ashtray=true\nExec=\"/opt/Token Ashtray/token-ashtray\" --autostart\n";
        assert!(desktop_entry_enabled(valid, binary));
        assert!(!desktop_entry_enabled("X-Token-Ashtray=true", binary));
        assert!(!desktop_entry_enabled(
            &valid.replace("/opt/", "/old/"),
            binary
        ));
        assert!(!desktop_entry_enabled(
            &valid.replace(" --autostart", ""),
            binary
        ));
        assert!(!desktop_entry_enabled(
            &format!("{valid}Hidden=true\n"),
            binary
        ));
        assert!(!desktop_entry_enabled(
            &valid.replace("[Desktop Entry]", "[Unrelated]"),
            binary
        ));
    }

    #[test]
    fn desktop_exec_escapes_both_parsing_layers() {
        assert_eq!(
            desktop_command(Path::new(r#"/opt/a$b`c"d\e%f/token-ashtray"#)),
            r#""/opt/a\\$b\\`c\\"d\\\\e%%f/token-ashtray" --autostart"#,
        );
    }
}

/// EWMH work area for the current desktop, intersected with RandR monitors.
/// Native Wayland deliberately does not use Xwayland desktop properties.
pub fn work_areas() -> Vec<super::MonitorRect> {
    if !placement_supported() {
        return Vec::new();
    }
    let Some(area) = (|| {
        let (connection, screen) = connect(None).ok()?;
        let root = connection.setup().roots.get(screen)?.root;
        let desktop = x_cardinals(
            &x_property(
                &connection,
                root,
                x_atom(&connection, "_NET_CURRENT_DESKTOP")?,
                AtomEnum::CARDINAL.into(),
                1,
            )?,
            1,
        )?[0] as usize;
        let reply = x_property(
            &connection,
            root,
            x_atom(&connection, "_NET_WORKAREA")?,
            AtomEnum::CARDINAL.into(),
            4096,
        )?;
        let words = reply.value32()?.collect::<Vec<_>>();
        let values = words.get(desktop.checked_mul(4)?..desktop.checked_mul(4)?.checked_add(4)?)?;
        Some(super::MonitorRect {
            x: values[0] as i32 as f32,
            y: values[1] as i32 as f32,
            width: values[2] as f32,
            height: values[3] as f32,
        })
    })() else {
        return Vec::new();
    };
    monitor_rects()
        .into_iter()
        .filter_map(|m| {
            let x = m.x.max(area.x);
            let y = m.y.max(area.y);
            let width = (m.x + m.width).min(area.x + area.width) - x;
            let height = (m.y + m.height).min(area.y + area.height) - y;
            (width > 0.0 && height > 0.0).then_some(super::MonitorRect {
                x,
                y,
                width,
                height,
            })
        })
        .collect()
}

pub fn frame_size(title: &str) -> Option<eframe::egui::Vec2> {
    if !placement_supported() {
        return None;
    }
    let (connection, screen) = connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let name = x_atom(&connection, "_NET_WM_NAME")?;
    let utf8 = x_atom(&connection, "UTF8_STRING")?;
    let pid = x_atom(&connection, "_NET_WM_PID")?;
    let extents = x_atom(&connection, "_NET_FRAME_EXTENTS")?;
    let mut found = None;
    for window in client_windows(&connection, root)? {
        if x_property(&connection, window, name, utf8, 128)
            .is_none_or(|p| p.value != title.as_bytes())
        {
            continue;
        }
        if x_property(&connection, window, pid, AtomEnum::CARDINAL.into(), 1)
            .and_then(|p| x_cardinals(&p, 1))
            .is_none_or(|p| p[0] != std::process::id())
        {
            continue;
        }
        let p = x_cardinals(
            &x_property(&connection, window, extents, AtomEnum::CARDINAL.into(), 4)?,
            4,
        )?;
        let size = eframe::egui::vec2(p[0] as f32 + p[1] as f32, p[2] as f32 + p[3] as f32);
        if found.is_some() {
            return None;
        }
        found = Some(size);
    }
    found
}
