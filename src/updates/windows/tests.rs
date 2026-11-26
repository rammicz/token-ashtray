//! The Windows installer against temporary folders and a stand-in setup.
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};

use super::*;
use crate::platform::instance::{Instance, Scope, acquire};
use crate::updates::{Target, Version, sha256};

/// The stand-in setup reads the guard's scope from this file beside itself.
const SCOPE_EXTENSION: &str = "scope";
/// It marks that it found the guard and waits for it to disappear.
const WAITING_EXTENSION: &str = "waiting";
/// It reports whether the guard was released while it waited.
const REPORT_EXTENSION: &str = "report";
const STAND_IN: [&str; 3] = [
    "updates::windows::tests::stand_in_setup",
    "--exact",
    "--nocapture",
];

fn record(artifact: &[u8], app: &[u8]) -> ArtifactRecord {
    ArtifactRecord {
        target: Target::WindowsX86_64,
        version: Version::parse("0.2.2").unwrap(),
        filename: "token-ashtray-setup-0.2.2-x86_64-pc-windows-msvc.exe".into(),
        length: artifact.len() as u64,
        sha256: sha256(artifact),
        app_sha256: sha256(app),
        descriptor_sha256: [0; 32],
    }
}

/// Whether a guard object with this name still exists. The handle opened to
/// check is closed at once.
fn guard_exists(name: &str) -> bool {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    // The handle was just returned to this process and is closed once.
    drop(unsafe { OwnedHandle::from_raw_handle(handle) });
    true
}

/// Plays the staged setup, only when a copy of this test binary was started
/// with a scope file beside it: waits, as `/WAITAPP` does, until the app's
/// guard object is gone, and reports the result.
#[test]
fn stand_in_setup() {
    let program = std::env::current_exe().unwrap();
    let Ok(id) = fs::read_to_string(program.with_extension(SCOPE_EXTENSION)) else {
        return;
    };
    let name = format!(r"Local\TokenAshtray-isolated-{id}");
    let report = if guard_exists(&name) {
        fs::write(program.with_extension(WAITING_EXTENSION), "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while guard_exists(&name) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if guard_exists(&name) {
            "held"
        } else {
            "released"
        }
    } else {
        "never seen"
    };
    fs::write(program.with_extension(REPORT_EXTENSION), report).unwrap();
}

#[test]
fn the_setup_installation_is_located_only_by_its_own_markers() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join("Token Ashtray");
    fs::create_dir_all(&install).unwrap();
    fs::write(install.join(APP), b"MZ").unwrap();
    fs::write(install.join(MARKER), MARKER_TEXT).unwrap();
    let app = install.join(APP);
    // The setup resources must be a folder holding the setup marker.
    fs::write(install.join(SETUP_RESOURCES), SETUP_MARKER_TEXT).unwrap();
    assert_eq!(WindowsInstaller.locate(&app), None);
    fs::remove_file(install.join(SETUP_RESOURCES)).unwrap();
    fs::create_dir(install.join(SETUP_RESOURCES)).unwrap();
    let setup_marker = install.join(SETUP_RESOURCES).join(SETUP_MARKER);
    fs::create_dir(&setup_marker).unwrap();
    assert_eq!(WindowsInstaller.locate(&app), None);
    fs::remove_dir(&setup_marker).unwrap();
    fs::write(&setup_marker, SETUP_MARKER_TEXT).unwrap();
    assert_eq!(WindowsInstaller.locate(&app), Some(install.clone()));
    // The file name is compared as Windows does, without case.
    assert_eq!(
        WindowsInstaller.locate(&install.join("Token-Ashtray.EXE")),
        Some(install)
    );
}

#[test]
fn the_signed_app_hash_tells_whether_the_release_is_installed() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(APP), b"MZ installed 0.2.1").unwrap();
    let installer = WindowsInstaller;
    assert!(installer.installed(root.path(), &record(b"setup", b"MZ installed 0.2.1")));
    assert!(!installer.installed(root.path(), &record(b"setup", b"MZ next 0.2.2")));
}

#[test]
fn changed_staged_bytes_are_refused_before_anything_starts() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("token-ashtray-setup-0.2.2.exe");
    fs::write(&program, b"MZ setup 0.2.2").unwrap();
    let signed = record(b"MZ setup 0.2.3", b"");
    let error = WindowsInstaller
        .start(&program, &signed, &[], &root.path().join("apply.log"))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn the_started_setup_does_not_keep_the_app_guard_alive() {
    let id = format!(
        "update-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let Ok(Instance::Primary(primary)) = acquire(&Scope::isolated(&id)) else {
        panic!("a fresh isolated scope has no running instance");
    };
    let root = tempfile::tempdir().unwrap();
    let program = root
        .path()
        .join("token-ashtray-setup-0.2.2-x86_64-pc-windows-msvc.exe");
    fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
    fs::write(program.with_extension(SCOPE_EXTENSION), &id).unwrap();
    let signed = record(&fs::read(&program).unwrap(), b"");
    let arguments: Vec<OsString> = STAND_IN.iter().map(OsString::from).collect();
    WindowsInstaller
        .start(
            &program,
            &signed,
            &arguments,
            &root.path().join("apply.log"),
        )
        .unwrap();
    let (waiting, report) = (
        program.with_extension(WAITING_EXTENSION),
        program.with_extension(REPORT_EXTENSION),
    );
    // A new copy of a program can start slowly while it is scanned.
    let until = |path: &Path| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !path.exists() {
            assert!(Instant::now() < deadline, "the stand-in setup is silent");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    until(&waiting);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!report.exists(), "the setup must wait while the app runs");
    // Quit: the guard object disappears unless the setup inherited it.
    drop(primary);
    until(&report);
    assert_eq!(fs::read_to_string(&report).unwrap(), "released");
}
