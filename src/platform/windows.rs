use std::{env, io::ErrorKind, sync::mpsc};

use super::{SharedTraySnapshot, TrayAction, ashtray_rgba, removed_or_absent};
use directories::BaseDirs;
use eframe::egui::Context;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconId,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};
use windows_sys::Win32::{
    Foundation::{LPARAM, RECT},
    Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
};
use windows_sys::core::BOOL;
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

pub const PIN_UNSUPPORTED_EXPLANATION: &str = "Always-on-top is available on this Windows session.";
pub struct Tray {
    icon: TrayIcon,
    _ids: Vec<MenuId>,
    actions: mpsc::Receiver<TrayAction>,
    snapshot: SharedTraySnapshot,
    autostart: CheckMenuItem,
}
impl Tray {
    pub fn new(context: Context, snapshot: SharedTraySnapshot) -> Result<Self, String> {
        let menu = Menu::new();
        let refresh = MenuItem::with_id("refresh", "Refresh now", true, None);
        let open = MenuItem::with_id("open", "Open board", true, None);
        let autostart = CheckMenuItem::with_id(
            "autostart",
            "Start at login",
            true,
            autostart_enabled(),
            None,
        );
        let about = MenuItem::with_id("about", "About Token Ashtray", true, None);
        let quit = MenuItem::with_id("quit", "Quit", true, None);
        menu.append_items(&[
            &refresh,
            &open,
            &autostart,
            &PredefinedMenuItem::separator(),
            &about,
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Token Ashtray — loading provider data")
            .with_menu(Box::new(menu))
            .with_icon(Icon::from_rgba(ashtray_rgba(32, None), 32, 32).map_err(|e| e.to_string())?)
            .build()
            .map_err(|e| e.to_string())?;
        let (sender, actions) = mpsc::channel();
        let tray_id = icon.id().clone();
        let click_sender = sender.clone();
        let click_context = context.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let Some(action) = tray_click_action(event, &tray_id) {
                let _ = click_sender.send(action);
                click_context.request_repaint();
            }
        }));
        let ids = vec![
            refresh.id().clone(),
            open.id().clone(),
            autostart.id().clone(),
            about.id().clone(),
            quit.id().clone(),
        ];
        let event_ids = ids.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == event_ids[0] {
                TrayAction::Refresh
            } else if event.id == event_ids[1] {
                TrayAction::OpenBoard
            } else if event.id == event_ids[2] {
                TrayAction::ToggleAutostart
            } else if event.id == event_ids[3] {
                TrayAction::About
            } else if event.id == event_ids[4] {
                TrayAction::Quit
            } else {
                TrayAction::None
            };
            if action != TrayAction::None {
                let _ = sender.send(action);
                context.request_repaint();
            }
        }));
        Ok(Self {
            icon,
            _ids: ids,
            actions,
            snapshot,
            autostart,
        })
    }
    pub fn poll(&self) -> TrayAction {
        self.actions.try_recv().unwrap_or(TrayAction::None)
    }
    pub fn sync(&self) {
        self.autostart.set_checked(autostart_enabled());
        if let Ok(snapshot) = self.snapshot.read() {
            let _ = self.icon.set_icon(Some(
                Icon::from_rgba(ashtray_rgba(32, snapshot.primary_remaining), 32, 32)
                    .expect("icon pixels are valid"),
            ));
            let tip = format!("Token Ashtray\n{}", snapshot.description());
            let _ = self.icon.set_tooltip(Some(&tip));
        }
    }
}

fn tray_click_action(event: TrayIconEvent, tray_id: &TrayIconId) -> Option<TrayAction> {
    if let TrayIconEvent::Click {
        id,
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
        && id == tray_id
    {
        Some(TrayAction::OpenBoard)
    } else {
        None
    }
}

#[cfg(test)]
mod tray_click_tests {
    use super::*;

    fn click(id: &str, button: MouseButton, button_state: MouseButtonState) -> TrayIconEvent {
        TrayIconEvent::Click {
            id: TrayIconId::new(id),
            // Actual cursor coordinates in the 150% native RED trace.
            position: (953.0, 634.0).into(),
            // Actual Shell_NotifyIconGetRect bounds in the 150% native RED trace.
            rect: tray_icon::Rect {
                position: (1402.0, 923.0).into(),
                size: (60, 60).into(),
            },
            button,
            button_state,
        }
    }

    #[test]
    fn tray_click_opens_board_without_using_icon_coordinates() {
        assert_eq!(
            tray_click_action(
                click("ashtray", MouseButton::Left, MouseButtonState::Up),
                &TrayIconId::new("ashtray"),
            ),
            Some(TrayAction::OpenBoard),
        );
    }

    #[test]
    fn tray_click_ignores_other_icons_buttons_and_button_down() {
        let tray_id = TrayIconId::new("ashtray");
        for event in [
            click("other", MouseButton::Left, MouseButtonState::Up),
            click("ashtray", MouseButton::Right, MouseButtonState::Up),
            click("ashtray", MouseButton::Left, MouseButtonState::Down),
        ] {
            assert_eq!(tray_click_action(event, &tray_id), None);
        }
    }

    #[test]
    fn tray_click_opens_board_on_negative_monitor_coordinates() {
        let mut event = click("ashtray", MouseButton::Left, MouseButtonState::Up);
        let TrayIconEvent::Click { rect, .. } = &mut event else {
            unreachable!();
        };
        rect.position = (-1462.0, -981.0).into();
        assert_eq!(
            tray_click_action(event, &TrayIconId::new("ashtray")),
            Some(TrayAction::OpenBoard),
        );
    }

    #[test]
    fn tray_click_opens_even_when_icon_bounds_are_invalid() {
        for invalid in [
            tray_icon::Rect::default(),
            tray_icon::Rect {
                position: (f64::NAN, 923.0).into(),
                size: (60, 60).into(),
            },
        ] {
            let mut event = click("ashtray", MouseButton::Left, MouseButtonState::Up);
            let TrayIconEvent::Click { rect, .. } = &mut event else {
                unreachable!();
            };
            *rect = invalid;
            assert_eq!(
                tray_click_action(event, &TrayIconId::new("ashtray")),
                Some(TrayAction::OpenBoard),
            );
        }
    }
}
pub fn pin_supported() -> bool {
    true
}
pub fn placement_supported() -> bool {
    true
}
pub fn monitor_rects() -> Vec<super::MonitorRect> {
    monitor_rectangles(false)
}

pub fn work_areas() -> Vec<super::MonitorRect> {
    monitor_rectangles(true)
}

fn monitor_rectangle(info: &MONITORINFO, work_area: bool) -> super::MonitorRect {
    let rect = if work_area {
        info.rcWork
    } else {
        info.rcMonitor
    };
    super::MonitorRect {
        x: rect.left as f32,
        y: rect.top as f32,
        width: (rect.right - rect.left) as f32,
        height: (rect.bottom - rect.top) as f32,
    }
}

fn monitor_rectangles(work_area: bool) -> Vec<super::MonitorRect> {
    struct Query {
        rectangles: Vec<super::MonitorRect>,
        work_area: bool,
    }
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let query = unsafe { &mut *(data as *mut Query) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let rectangle = monitor_rectangle(&info, query.work_area);
            // MONITORINFOF_PRIMARY = 1; recovery falls back to rectangle zero.
            if info.dwFlags & 1 != 0 {
                query.rectangles.insert(0, rectangle);
            } else {
                query.rectangles.push(rectangle);
            }
        }
        1
    }
    let mut query = Query {
        rectangles: Vec::new(),
        work_area,
    };
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut query as *mut _ as LPARAM,
        );
    }
    query.rectangles
}

#[cfg(test)]
mod work_area_tests {
    use super::*;

    #[test]
    fn native_work_area_keeps_taskbar_insets_and_signed_physical_origins() {
        let info = MONITORINFO {
            rcMonitor: RECT {
                left: -1920,
                top: -1080,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: -1880,
                top: -1050,
                right: -20,
                bottom: -96,
            },
            ..Default::default()
        };
        let usable = monitor_rectangle(&info, true);
        assert_eq!(
            (usable.x, usable.y, usable.width, usable.height),
            (-1880.0, -1050.0, 1860.0, 954.0)
        );
        let display = monitor_rectangle(&info, false);
        assert_eq!(
            (display.x, display.y, display.width, display.height),
            (-1920.0, -1080.0, 1920.0, 1080.0)
        );
    }
}
pub fn credential_base_paths() -> super::CredentialBasePaths {
    super::CredentialBasePaths::from_home(
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_owned())
            .unwrap_or_default(),
    )
}
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const AUTOSTART_NAME: &str = "Token Ashtray";
/// Settings > Apps > Startup and Task Manager record their per-entry switch here,
/// under the same value name as the Run entry.
const STARTUP_APPROVED_RUN_KEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
pub fn autostart_enabled() -> bool {
    env::current_exe().is_ok_and(|executable| {
        autostart_enabled_in(
            &RegKey::predef(HKEY_CURRENT_USER),
            &autostart_command(&executable),
        )
    })
}
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let command = if enabled {
        Some(autostart_command(
            &env::current_exe().map_err(|e| e.to_string())?,
        ))
    } else {
        None
    };
    set_autostart_in(&RegKey::predef(HKEY_CURRENT_USER), command.as_deref())
}
fn autostart_command(executable: &std::path::Path) -> String {
    format!("\"{}\" --autostart", executable.display())
}
fn autostart_enabled_in(root: &RegKey, command: &str) -> bool {
    let registered = root
        .open_subkey(RUN_KEY)
        .ok()
        .and_then(|key| key.get_value::<String, _>(AUTOSTART_NAME).ok())
        .is_some_and(|value| value.trim() == command);
    registered
        && !root
            .open_subkey(STARTUP_APPROVED_RUN_KEY)
            .ok()
            .and_then(|key| key.get_raw_value(AUTOSTART_NAME).ok())
            .is_some_and(|value| startup_switch_disabled(&value))
}
/// A 12-byte binary value whose first byte has its low bit set marks the entry
/// disabled. Windows starts the entry when the value is absent, so anything that
/// is not in that format counts as enabled.
fn startup_switch_disabled(value: &winreg::RegValue) -> bool {
    value.vtype == winreg::enums::REG_BINARY && value.bytes.len() == 12 && value.bytes[0] & 1 == 1
}
fn set_autostart_in(root: &RegKey, command: Option<&str>) -> Result<(), String> {
    match root.open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_READ | winreg::enums::KEY_WRITE) {
        Ok(key) => match command {
            Some(command) => key
                .set_value(AUTOSTART_NAME, &command)
                .map_err(|e| e.to_string())?,
            None => removed_or_absent(key.delete_value(AUTOSTART_NAME))?,
        },
        Err(error) if command.is_none() && error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    // Enabling from the menu overrides a disable made in Windows Settings, and
    // disabling leaves no switch behind. An absent switch means enabled.
    match root.open_subkey_with_flags(STARTUP_APPROVED_RUN_KEY, winreg::enums::KEY_SET_VALUE) {
        Ok(key) => removed_or_absent(key.delete_value(AUTOSTART_NAME)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Dispatch native tray/menu HWND messages on their creating main thread.
pub fn pump_events() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(test)]
mod autostart_registry_tests {
    use super::*;
    use winreg::{
        RegValue,
        enums::{REG_BINARY, REG_SZ},
        types::ToRegValue,
    };

    const COMMAND: &str = r#""C:\Program Files\Token Ashtray\token-ashtray.exe" --autostart"#;
    // Pinned independently of the implementation: Settings > Apps > Startup and
    // Task Manager record their per-entry switch under this HKCU path.
    const APPROVED: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    /// A disposable HKCU subtree standing in for HKCU; removed on drop.
    struct Hive {
        path: String,
        root: RegKey,
    }

    impl Hive {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = format!(
                r"Software\token-ashtray-test-{}-{label}-{nanos}",
                std::process::id()
            );
            let (root, _) = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(&path)
                .unwrap();
            Self { path, root }
        }

        fn set(&self, key: &str, name: &str, value: RegValue) {
            let (key, _) = self.root.create_subkey(key).unwrap();
            key.set_raw_value(name, &value).unwrap();
        }

        fn remove(&self, key: &str, name: &str) {
            if let Ok(key) = self
                .root
                .open_subkey_with_flags(key, winreg::enums::KEY_SET_VALUE)
            {
                let _ = key.delete_value(name);
            }
        }

        fn get(&self, key: &str, name: &str) -> Option<RegValue> {
            self.root.open_subkey(key).ok()?.get_raw_value(name).ok()
        }
    }

    impl Drop for Hive {
        fn drop(&mut self) {
            let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.path);
        }
    }

    fn run_value(command: &str) -> RegValue {
        command.to_reg_value()
    }

    /// The 12-byte StartupApproved layout: a flag byte, three zero bytes and a FILETIME.
    fn approval(flag: u8) -> RegValue {
        binary(&[
            flag, 0, 0, 0, 0x58, 0x2F, 0xA4, 0xC7, 0x8F, 0x76, 0xDD, 0x01,
        ])
    }

    fn binary(bytes: &[u8]) -> RegValue {
        RegValue {
            bytes: bytes.to_vec(),
            vtype: REG_BINARY,
        }
    }

    #[test]
    fn startup_apps_switch_decides_whether_an_exact_run_entry_is_checked() {
        let hive = Hive::new("decode");
        hive.set(RUN_KEY, AUTOSTART_NAME, run_value(COMMAND));
        let cases: [(&str, Option<RegValue>, bool); 9] = [
            ("absent", None, true),
            ("02 enabled", Some(approval(0x02)), true),
            ("03 disabled", Some(approval(0x03)), false),
            ("06 enabled", Some(approval(0x06)), true),
            ("07 disabled", Some(approval(0x07)), false),
            ("one-byte 03", Some(binary(&[0x03])), true),
            ("eleven-byte 03", Some(binary(&[0x03; 11])), true),
            ("empty", Some(binary(&[])), true),
            (
                "string type",
                Some(RegValue {
                    bytes: approval(0x03).bytes,
                    vtype: REG_SZ,
                }),
                true,
            ),
        ];
        for (label, value, expected) in cases {
            match value {
                Some(value) => hive.set(APPROVED, AUTOSTART_NAME, value),
                None => hive.remove(APPROVED, AUTOSTART_NAME),
            }
            assert_eq!(
                autostart_enabled_in(&hive.root, COMMAND),
                expected,
                "StartupApproved case: {label}"
            );
        }
    }

    #[test]
    fn startup_apps_enabled_does_not_check_a_missing_or_drifted_run_entry() {
        let hive = Hive::new("runonly");
        hive.set(APPROVED, AUTOSTART_NAME, approval(0x02));
        assert!(!autostart_enabled_in(&hive.root, COMMAND));
        hive.set(
            RUN_KEY,
            AUTOSTART_NAME,
            run_value(r#""C:\old\token-ashtray.exe" --autostart"#),
        );
        assert!(!autostart_enabled_in(&hive.root, COMMAND));
        hive.set(RUN_KEY, AUTOSTART_NAME, run_value(COMMAND));
        assert!(autostart_enabled_in(&hive.root, COMMAND));
    }

    #[test]
    fn enabling_overrides_a_startup_apps_disable_and_disabling_leaves_no_residue() {
        let hive = Hive::new("toggle");
        let other_run = || run_value(r#""C:\Other\other.exe""#);
        let other_approval = || approval(0x03);
        hive.set(RUN_KEY, "Other", other_run());
        hive.set(APPROVED, "Other", other_approval());
        hive.set(APPROVED, AUTOSTART_NAME, approval(0x03));

        set_autostart_in(&hive.root, Some(COMMAND)).unwrap();
        assert_eq!(hive.get(RUN_KEY, AUTOSTART_NAME), Some(run_value(COMMAND)));
        assert_eq!(hive.get(APPROVED, AUTOSTART_NAME), None);
        assert!(autostart_enabled_in(&hive.root, COMMAND));

        // Windows Settings disables it again; the app's toggle then turns it off.
        hive.set(APPROVED, AUTOSTART_NAME, approval(0x03));
        set_autostart_in(&hive.root, None).unwrap();
        assert_eq!(hive.get(RUN_KEY, AUTOSTART_NAME), None);
        assert_eq!(hive.get(APPROVED, AUTOSTART_NAME), None);
        assert!(!autostart_enabled_in(&hive.root, COMMAND));

        // A stale disable marker without a Run value is still ours to clear.
        hive.set(APPROVED, AUTOSTART_NAME, approval(0x03));
        set_autostart_in(&hive.root, None).unwrap();
        assert_eq!(hive.get(APPROVED, AUTOSTART_NAME), None);
        set_autostart_in(&hive.root, None).unwrap();

        assert_eq!(hive.get(RUN_KEY, "Other"), Some(other_run()));
        assert_eq!(hive.get(APPROVED, "Other"), Some(other_approval()));
    }

    #[test]
    fn disabling_without_startup_keys_succeeds_and_creates_nothing() {
        let hive = Hive::new("empty");
        set_autostart_in(&hive.root, None).unwrap();
        assert!(hive.root.open_subkey(RUN_KEY).is_err());
        assert!(hive.root.open_subkey(APPROVED).is_err());
    }
}
