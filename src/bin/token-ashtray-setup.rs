//! Linux per-user graphical setup. The release packager appends the app payload.
#[cfg(target_os = "linux")]
#[path = "../linux_setup.rs"]
mod linux_setup;

#[cfg(target_os = "linux")]
fn main() -> eframe::Result<()> {
    linux_setup::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This setup is supported on Linux x86_64 only.");
}
