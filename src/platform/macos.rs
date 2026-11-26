pub const PIN_UNSUPPORTED_EXPLANATION: &str = "macOS is explicitly unsupported in v1.";

pub struct Tray;

impl Tray {
    pub fn new(
        _context: eframe::egui::Context,
        _snapshot: super::SharedTraySnapshot,
    ) -> Result<Self, String> {
        Err("macOS is explicitly unsupported in v1; it has not been run on a Mac".into())
    }

    pub fn poll(&self) -> super::TrayAction {
        super::TrayAction::None
    }
    pub fn sync(&self) {}
}

pub fn pin_supported() -> bool {
    false
}

pub fn placement_supported() -> bool {
    false
}
pub fn monitor_rects() -> Vec<super::MonitorRect> {
    Vec::new()
}
pub fn autostart_enabled() -> bool {
    false
}
pub fn set_autostart(_enabled: bool) -> Result<(), String> {
    Err("macOS is unsupported in v1".into())
}

pub fn credential_base_paths() -> super::CredentialBasePaths {
    todo!("macOS credential discovery is unsupported in v1")
}

/// No main-thread native tray message pump is needed on this platform.
pub fn pump_events() {}
