//! OS seams only. Product code asks these questions; it never branches on an OS.

#[path = "../icon_pixels.rs"]
mod icon_pixels;
pub mod instance;
pub use icon_pixels::ashtray_rgba;
use std::sync::{Arc, RwLock};
use std::{env, path::PathBuf};

/// OS directory inputs for provider credential discovery. Provider adapters
/// retain their own filename and precedence rules.
pub struct CredentialBasePaths {
    pub home: PathBuf,
    pub xdg_config_home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub roaming_app_data: Option<PathBuf>,
}

impl CredentialBasePaths {
    fn from_home(home: PathBuf) -> Self {
        Self {
            home,
            xdg_config_home: env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            local_app_data: env::var_os("LOCALAPPDATA").map(PathBuf::from),
            roaming_app_data: env::var_os("APPDATA").map(PathBuf::from),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TrayProvider {
    pub name: String,
    /// The used share exactly as the tightest meter shows it, so the tooltip
    /// rounds the same value as that meter.
    pub used: Option<f32>,
    pub reset: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TraySnapshot {
    pub providers: Vec<TrayProvider>,
    pub primary_remaining: Option<f32>,
    pub any_readable: bool,
    pub quota_display_mode: crate::config::QuotaDisplayMode,
}

impl TraySnapshot {
    /// Native tooltips share percentage formatting and meaning; icon inputs stay remaining.
    pub fn description(&self) -> String {
        if !self.any_readable {
            return "No provider can be read yet".into();
        }
        if self.providers.is_empty() {
            return "No provider data yet".into();
        }
        self.providers
            .iter()
            .map(|provider| match provider.used {
                Some(used) => {
                    format!(
                        "{} {} {}{}",
                        provider.name,
                        self.quota_display_mode.percent_label(used),
                        self.quota_display_mode.label(),
                        provider
                            .reset
                            .as_ref()
                            .map(|reset| {
                                if reset.eq_ignore_ascii_case(crate::model::RESETS_SOON) {
                                    " · resets soon".into()
                                } else {
                                    format!(" · resets in {reset}")
                                }
                            })
                            .unwrap_or_default()
                    )
                }
                None => format!("{} unavailable", provider.name),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub type SharedTraySnapshot = Arc<RwLock<TraySnapshot>>;

#[cfg(test)]
mod quota_display_tests {
    use super::*;
    use crate::config::QuotaDisplayMode;

    #[test]
    fn shared_native_tooltip_formats_boundaries_fractions_and_unavailable_providers() {
        for (used, expected_remaining, expected_used) in [
            (25.0, "75% Remaining", "25% Used"),
            (0.0, "100% Remaining", "0% Used"),
            (100.0, "0% Remaining", "100% Used"),
            (99.6, "<1% Remaining", "100% Used"),
            (0.4, "100% Remaining", "<1% Used"),
            // Ties round the used share half up; Remaining is its complement.
            (50.5, "49% Remaining", "51% Used"),
            (99.5, "<1% Remaining", "100% Used"),
        ] {
            let remaining = 100.0 - used;
            let mut snapshot = TraySnapshot {
                providers: vec![
                    TrayProvider {
                        name: "Fixture".into(),
                        used: Some(used),
                        reset: Some("2h".into()),
                    },
                    TrayProvider {
                        name: "Unknown".into(),
                        ..Default::default()
                    },
                ],
                primary_remaining: Some(remaining),
                any_readable: true,
                ..Default::default()
            };
            assert_eq!(
                snapshot.description(),
                format!("Fixture {expected_remaining} · resets in 2h\nUnknown unavailable")
            );
            snapshot.quota_display_mode = QuotaDisplayMode::Used;
            assert_eq!(
                snapshot.description(),
                format!("Fixture {expected_used} · resets in 2h\nUnknown unavailable")
            );
            assert_eq!(snapshot.primary_remaining, Some(remaining));
        }
    }

    #[test]
    fn native_tooltip_formats_passed_reset_as_resets_soon_without_duplication() {
        let snapshot = TraySnapshot {
            providers: vec![TrayProvider {
                name: "Fixture".into(),
                used: Some(25.0),
                reset: Some("Resets soon".into()),
            }],
            primary_remaining: Some(75.0),
            any_readable: true,
            ..Default::default()
        };
        let description = snapshot.description();
        assert_eq!(description, "Fixture 75% Remaining · resets soon");
        assert!(!description.contains("resets in Resets soon"));
        assert!(!description.contains("0s"));
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MonitorRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TrayAction {
    None,
    Refresh,
    OpenBoard,
    ToggleAutostart,
    About,
    Quit,
}

#[cfg(test)]
mod tests {
    use super::ashtray_rgba;
    use std::io::Write;

    #[test]
    #[ignore = "writes a local visual inspection artifact"]
    fn write_icon_contact_sheet() {
        let sizes = [16, 24, 32, 48];
        let levels = [None, Some(100.0), Some(50.0), Some(10.0), Some(0.0)];
        let cell = 56usize;
        let width = cell * levels.len() * 2;
        let height = cell * sizes.len();
        let mut rgb = vec![0u8; width * height * 3];
        for (row, size) in sizes.into_iter().enumerate() {
            for (col, level) in levels.into_iter().enumerate() {
                for (side, background) in [[28u8; 3], [238u8; 3]].into_iter().enumerate() {
                    let offset_x = (col * 2 + side) * cell;
                    let offset_y = row * cell;
                    let pixels = ashtray_rgba(size, level);
                    for y in 0..cell {
                        for x in 0..cell {
                            let destination = ((offset_y + y) * width + offset_x + x) * 3;
                            rgb[destination..destination + 3].copy_from_slice(&background);
                        }
                    }
                    for y in 0..size as usize {
                        for x in 0..size as usize {
                            let source = (y * size as usize + x) * 4;
                            let destination = ((offset_y + 4 + y) * width + offset_x + 4 + x) * 3;
                            let alpha = pixels[source + 3] as u16;
                            for channel in 0..3 {
                                rgb[destination + channel] =
                                    ((pixels[source + channel] as u16 * alpha
                                        + background[channel] as u16 * (255 - alpha))
                                        / 255) as u8;
                            }
                        }
                    }
                }
            }
        }
        let mut file = std::fs::File::create("/tmp/token-ashtray-icons.ppm").unwrap();
        writeln!(file, "P6\n{width} {height}\n255").unwrap();
        file.write_all(&rgb).unwrap();
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "windows", test))]
fn removed_or_absent(result: std::io::Result<()>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod autostart_result_tests {
    use super::removed_or_absent;
    use std::io::{Error, ErrorKind};

    #[test]
    fn absent_value_is_idempotent_but_failed_removal_is_reported() {
        assert!(removed_or_absent(Ok(())).is_ok());
        assert!(removed_or_absent(Err(Error::from(ErrorKind::NotFound))).is_ok());
        let error = removed_or_absent(Err(Error::from(ErrorKind::PermissionDenied)));
        assert!(
            error.is_err(),
            "a value left in the Run key cannot report success"
        );
    }
}
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
pub use windows::*;

/// Usable desktop rectangles in physical pixels. No display-bounds fallback:
/// adapters without a verified work-area source leave fitting unavailable.
pub fn work_areas() -> Vec<MonitorRect> {
    #[cfg(target_os = "linux")]
    {
        linux::work_areas()
    }
    #[cfg(target_os = "windows")]
    {
        windows::work_areas()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// Measured outer-frame overhead in physical pixels for an owned viewport.
pub fn frame_size(title: &str) -> Option<eframe::egui::Vec2> {
    #[cfg(target_os = "linux")]
    {
        linux::frame_size(title)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = title;
        None
    }
}

/// Read the desktop's reduced-animation preference without changing it.
pub fn reduced_motion_requested() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
        };
        let mut enabled = 1i32;
        // SPI_GET only reads the current user's preference.
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            );
        }
        enabled == 0
    }
    #[cfg(target_os = "linux")]
    {
        static REDUCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *REDUCED.get_or_init(|| {
            std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.interface", "enable-animations"])
                .output()
                .ok()
                .is_some_and(|output| {
                    output.status.success() && output.stdout.starts_with(b"false")
                })
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        false
    }
}
