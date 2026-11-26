//! Linux: the installer that applies a verified staged setup after Quit.
//!
//! Only the per-user installation that setup created and marked is updated.
//! A copy run from anywhere else, including a ZIP extracted to a custom
//! folder, never replaces itself. The staged artifact is the next release's
//! setup, which applies its own payload with the setup operation lock and
//! staged rollback (`token-ashtray-setup --apply-update`).
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::ArtifactRecord;
use super::apply::Installer;

/// The installation directory under `$HOME`, shared with the setup program.
pub const INSTALL_DIR: &str = ".local/opt/token-ashtray";
pub const APP: &str = "token-ashtray";
pub const MARKER: &str = ".token-ashtray-install";
pub const MARKER_TEXT: &str = "Token Ashtray per-user installation\n";
/// The setup mode that upgrades an existing installation without a window.
pub const APPLY_ARGUMENT: &str = "--apply-update";

/// The per-user installation under a home directory.
pub struct LinuxInstaller {
    home: PathBuf,
}

impl LinuxInstaller {
    pub fn from_environment() -> Option<Self> {
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
        Some(Self { home: home.into() })
    }

    /// The installation setup created: its marker is a regular file with the
    /// exact setup text.
    fn owned_install(&self) -> Option<PathBuf> {
        let directory = self.home.join(INSTALL_DIR);
        let marker = directory.join(MARKER);
        let regular = fs::symlink_metadata(&marker).ok()?.is_file();
        (regular && fs::read_to_string(&marker).ok()? == MARKER_TEXT).then_some(directory)
    }
}

impl Installer for LinuxInstaller {
    fn locate(&self, running: &Path) -> Option<PathBuf> {
        let install = self.owned_install()?;
        let app = install.join(APP).canonicalize().ok()?;
        (running.canonicalize().ok()? == app).then_some(install)
    }

    fn installed(&self, install: &Path, record: &ArtifactRecord) -> bool {
        record.app_installed(&install.join(APP))
    }

    fn arguments(&self, _install: &Path, _log: &Path) -> Vec<OsString> {
        // The setup's layout fixes the installation directory.
        vec![APPLY_ARGUMENT.into()]
    }

    fn start(
        &self,
        program: &Path,
        _record: &ArtifactRecord,
        arguments: &[OsString],
        log: &Path,
    ) -> io::Result<()> {
        fs::set_permissions(program, fs::Permissions::from_mode(0o700))?;
        Command::new(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(log)?)
            // Its own process group, so it outlives this process and terminal.
            .process_group(0)
            .spawn()
            .map(drop)
    }
}

#[cfg(test)]
mod tests;
