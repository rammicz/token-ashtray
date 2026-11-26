mod app_icon;
mod epi_integration;
mod event_loop_context;
#[cfg(all(target_os = "windows", feature = "glow"))]
mod first_show;
mod hidden_repaint;
pub mod run;

/// File storage which can be used by native backends.
#[cfg(feature = "persistence")]
pub mod file_storage;

pub(crate) mod winit_integration;

#[cfg(feature = "glow")]
mod glow_integration;

#[cfg(feature = "wgpu")]
mod wgpu_integration;
