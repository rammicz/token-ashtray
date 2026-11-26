//! Windows: the installer that applies a verified staged setup after Quit.
//!
//! Only an installation made by the graphical setup is updated: the running
//! app sits beside the exact installation marker and the setup resources
//! marker. A loose copy, including a ZIP extracted to any folder and a
//! helper-only installation, never replaces itself. The staged artifact is
//! the next release's per-user Setup.exe. It runs unattended, waits until
//! every app process has exited (`/WAITAPP`), repeats its ownership and
//! running checks, and installs inside its rollback-capable file transaction.
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::windows::{fs::OpenOptionsExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::ArtifactRecord;
use super::apply::Installer;

pub const APP: &str = "token-ashtray.exe";
pub const MARKER: &str = ".token-ashtray-install";
pub const MARKER_TEXT: &str = "Token Ashtray per-user installation\n";
/// The folder the graphical setup keeps its resources and uninstaller in.
pub const SETUP_RESOURCES: &str = ".setup";
pub const SETUP_MARKER: &str = ".token-ashtray-setup";
pub const SETUP_MARKER_TEXT: &str = "Token Ashtray graphical setup resources\n";
/// No window, message box, restart, Cancel button or startup prompt.
pub const UNATTENDED: [&str; 5] = [
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/NOCANCEL",
    "/SP-",
];
/// Makes the setup wait until every app process has exited (setup.iss).
pub const WAIT_APP: &str = "/WAITAPP";
/// While the staged setup is held open, others may read it but not change,
/// rename or delete it.
const FILE_SHARE_READ: u32 = 0x1;

/// The per-user installation the running app belongs to.
pub struct WindowsInstaller;

/// Whether `path` is an ordinary file, not a link, with exactly `text`.
fn marked(path: &Path, text: &str) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
        && fs::read(path).is_ok_and(|bytes| bytes == text.as_bytes())
}

/// `switch` with a quoted path, as the setup documents it: `/DIR="C:\a b"`.
/// A Windows path cannot contain a quote.
fn quoted(switch: &str, path: &Path) -> OsString {
    let mut argument = OsString::from(format!("{switch}\""));
    argument.push(path);
    argument.push("\"");
    argument
}

impl Installer for WindowsInstaller {
    fn locate(&self, running: &Path) -> Option<PathBuf> {
        let install = running.parent()?;
        let resources = install.join(SETUP_RESOURCES);
        let owned = running.file_name()?.to_str()?.eq_ignore_ascii_case(APP)
            && marked(&install.join(MARKER), MARKER_TEXT)
            && fs::symlink_metadata(&resources).is_ok_and(|metadata| metadata.is_dir())
            && marked(&resources.join(SETUP_MARKER), SETUP_MARKER_TEXT);
        owned.then(|| install.to_owned())
    }

    fn installed(&self, install: &Path, record: &ArtifactRecord) -> bool {
        record.app_installed(&install.join(APP))
    }

    /// The setup's own `/DIR` keeps a custom installation folder, and its
    /// report goes to `log` in the update state folder.
    fn arguments(&self, install: &Path, log: &Path) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = UNATTENDED.iter().map(OsString::from).collect();
        arguments.push(quoted("/DIR=", install));
        arguments.push(quoted("/LOG=", log));
        arguments.push(WAIT_APP.into());
        arguments
    }

    /// Opens the staged setup so that it cannot change, hashes it through
    /// that handle and starts it while the handle is still held. The
    /// arguments are passed exactly as given. The setup outlives this
    /// process, and the instance guard is not inheritable, so it does not
    /// keep the guard alive while it waits for the app to exit.
    fn start(
        &self,
        program: &Path,
        record: &ArtifactRecord,
        arguments: &[OsString],
        _log: &Path,
    ) -> io::Result<()> {
        let staged = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(program)?;
        if !record.matches(&staged) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the staged setup no longer matches the signed release",
            ));
        }
        let mut command = Command::new(program);
        for argument in arguments {
            command.raw_arg(argument);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        drop(staged);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
