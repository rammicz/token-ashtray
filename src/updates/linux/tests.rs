//! The Linux installer against a temporary home and a stand-in setup.
use std::time::{Duration, Instant};

use super::*;
use crate::updates::{Target, Version};

const INSTALLED: &[u8] = b"\x7fELF installed 0.2.1";

/// A home with the installation, marked with `marker` if given.
fn home(marker: Option<&str>) -> (tempfile::TempDir, LinuxInstaller, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join(INSTALL_DIR);
    fs::create_dir_all(&install).unwrap();
    fs::write(install.join(APP), INSTALLED).unwrap();
    if let Some(marker) = marker {
        fs::write(install.join(MARKER), marker).unwrap();
    }
    let installer = LinuxInstaller {
        home: root.path().to_owned(),
    };
    (root, installer, install.join(APP))
}

fn record(app: &[u8]) -> ArtifactRecord {
    ArtifactRecord {
        target: Target::LinuxX86_64,
        version: Version::parse("0.2.2").unwrap(),
        filename: "token-ashtray-setup-0.2.2-x86_64-unknown-linux-gnu".into(),
        length: 1,
        sha256: [0; 32],
        app_sha256: crate::updates::sha256(app),
        descriptor_sha256: [0; 32],
    }
}

#[test]
fn only_the_marked_installation_is_located() {
    let (root, installer, app) = home(Some(MARKER_TEXT));
    assert_eq!(installer.locate(&app), Some(root.path().join(INSTALL_DIR)));
    let loose = root.path().join("Downloads/token-ashtray");
    fs::create_dir_all(loose.parent().unwrap()).unwrap();
    fs::write(&loose, INSTALLED).unwrap();
    assert_eq!(installer.locate(&loose), None);
    for marker in [None, Some("something else\n")] {
        let (_root, installer, app) = home(marker);
        assert_eq!(installer.locate(&app), None);
    }
    // A marker that is a symbolic link does not count.
    let (root, installer, app) = home(None);
    let elsewhere = root.path().join("marker");
    fs::write(&elsewhere, MARKER_TEXT).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.path().join(INSTALL_DIR).join(MARKER)).unwrap();
    assert_eq!(installer.locate(&app), None);
}

#[test]
fn the_signed_app_hash_tells_whether_the_release_is_installed() {
    let (root, installer, _) = home(Some(MARKER_TEXT));
    let install = root.path().join(INSTALL_DIR);
    assert!(installer.installed(&install, &record(INSTALLED)));
    assert!(!installer.installed(&install, &record(b"\x7fELF next 0.2.2")));
}

#[test]
fn the_staged_setup_starts_in_apply_mode_and_reports_to_the_log() {
    let (root, installer, _) = home(Some(MARKER_TEXT));
    let setup = root.path().join("token-ashtray-setup-0.2.2");
    fs::write(&setup, "#!/bin/sh\necho \"$@\" >&2\n").unwrap();
    let log = root.path().join("apply-0.2.2-1.log");
    let arguments = installer.arguments(&root.path().join(INSTALL_DIR), &log);
    assert_eq!(arguments, [OsString::from(APPLY_ARGUMENT)]);
    installer
        .start(&setup, &record(INSTALLED), &arguments, &log)
        .unwrap();
    let mode = fs::metadata(&setup).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    let deadline = Instant::now() + Duration::from_secs(10);
    while fs::read_to_string(&log).map_or(true, |text| text.is_empty()) {
        assert!(Instant::now() < deadline, "the staged setup did not run");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(&log).unwrap(), "--apply-update\n");
}
