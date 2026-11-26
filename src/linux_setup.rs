use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

// The app's updater shares these, so it recognizes the installation it may update.
use token_ashtray::updates::linux::{APP, APPLY_ARGUMENT, INSTALL_DIR, MARKER, MARKER_TEXT};
const SETUP: &str = "token-ashtray-setup";
const ICON: &str = "token-ashtray-256.png";
const MAGIC: &[u8] = b"TOKEN-ASHTRAY-SETUP-v1\n";
const NAMES: &[&str] = &[
    APP,
    ICON,
    "LICENSE",
    "THIRD-PARTY.md",
    "LICENSE-REVIEW.md",
    "INSTALL-LINUX.md",
    "RELEASE-METRICS.json",
];
const IDENTITY: &str = "SETUP-MANIFEST.json";
type Result<T> = std::result::Result<T, String>;

// SHA-256 over exact shipped bytes, independent of host commands or libraries.
fn sha256(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((bytes.len() as u64) * 8).to_be_bytes());
    for block in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, bytes) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*bytes);
        }
        for i in 16..64 {
            let x = w[i - 15];
            let y = w[i - 2];
            let s0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let s1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let t1 = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
    state.iter().map(|n| format!("{n:08x}")).collect()
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
struct Member {
    name: String,
    size: usize,
    sha256: String,
}
#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    source_sha: String,
    stub_size: usize,
    stub_sha256: String,
    members: Vec<Member>,
}
struct Payload {
    manifest: Manifest,
    files: BTreeMap<String, Vec<u8>>,
    executable: Vec<u8>,
}

impl Payload {
    /// The installed identity file: setup writes it on install and compares
    /// it on update.
    fn identity(&self) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(&self.manifest).map_err(|e| e.to_string())
    }
    fn parse(executable: Vec<u8>) -> Result<Self> {
        let end = executable
            .len()
            .checked_sub(MAGIC.len() + 8)
            .ok_or("Setup payload is missing")?;
        if !executable.ends_with(MAGIC) {
            return Err("Setup payload is missing; download the complete installer".into());
        }
        let len = u64::from_le_bytes(executable[end..end + 8].try_into().unwrap());
        let start = end
            .checked_sub(usize::try_from(len).map_err(|_| "Invalid manifest length")?)
            .ok_or("Invalid manifest length")?;
        let manifest: Manifest = serde_json::from_slice(&executable[start..end])
            .map_err(|e| format!("Invalid setup manifest: {e}"))?;
        if manifest.format != 1
            || manifest.source_sha.len() != 40
            || !manifest
                .source_sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || manifest.members.len() != NAMES.len()
        {
            return Err("Unsupported setup manifest".into());
        }
        let mut offset = manifest.stub_size;
        if offset > start || sha256(&executable[..offset]) != manifest.stub_sha256 {
            return Err("Setup executable integrity check failed".into());
        }
        let mut files = BTreeMap::new();
        for item in &manifest.members {
            if !NAMES.contains(&item.name.as_str())
                || files.contains_key(&item.name)
                || item.size == 0
            {
                return Err("Unexpected or empty setup resource".into());
            }
            let next = offset
                .checked_add(item.size)
                .filter(|n| *n <= start)
                .ok_or("Truncated setup resource")?;
            let bytes = &executable[offset..next];
            if sha256(bytes) != item.sha256 {
                return Err(format!("Integrity check failed: {}", item.name));
            }
            files.insert(item.name.clone(), bytes.to_vec());
            offset = next;
        }
        if offset != start {
            return Err("Unexpected setup payload bytes".into());
        }
        let metrics: serde_json::Value =
            serde_json::from_slice(&files["RELEASE-METRICS.json"]).map_err(|e| e.to_string())?;
        if metrics["binary_sha256"].as_str() != Some(sha256(&files[APP]).as_str())
            || !files[APP].starts_with(b"\x7fELF")
        {
            return Err("App identity does not match release measurements".into());
        }
        Ok(Self {
            manifest,
            files,
            executable,
        })
    }
}

fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|e| e.to_string())
}
fn regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.is_symlink() => {
            Err(format!("Refusing non-regular file: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
fn safe_directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .to_str()
            .is_none_or(|s| s.chars().any(char::is_control))
    {
        return Err("Setup requires absolute UTF-8 paths without control characters".into());
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(m) if !m.is_dir() || m.is_symlink() => {
                return Err(format!(
                    "Refusing non-directory or symbolic link: {}",
                    ancestor.display()
                ));
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => (),
        }
    }
    Ok(())
}
fn command(path: &Path, argument: &str) -> String {
    let escaped = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    format!("\"{escaped}\"{argument}").replace('\\', "\\\\")
}
fn fields(body: &str) -> BTreeMap<&str, &str> {
    let mut active = false;
    let mut values = BTreeMap::new();
    for line in body.lines().map(str::trim) {
        if line.starts_with('[') {
            active = line == "[Desktop Entry]";
        } else if active
            && !line.starts_with('#')
            && let Some((k, v)) = line.split_once('=')
        {
            values.insert(k.trim(), v.trim());
        }
    }
    values
}
fn owned_entry(path: &Path, marker: &str, exec: &str) -> Result<Option<String>> {
    regular(path)?;
    if !path.exists() {
        return Ok(None);
    }
    let body = io(fs::read_to_string(path))?;
    let values = fields(&body);
    if values.get("Type") != Some(&"Application")
        || values.get(marker) != Some(&"true")
        || values.get("Exec") != Some(&exec)
        || body
            .lines()
            .filter(|l| l.trim().starts_with("Exec="))
            .count()
            != 1
    {
        return Err(format!(
            "Unrelated or stale desktop entry: {}",
            path.display()
        ));
    }
    Ok(Some(body))
}
#[derive(Debug, PartialEq, Eq)]
enum Updated {
    Installed,
    AlreadyCurrent,
}
struct Layout {
    destination: PathBuf,
    launcher: PathBuf,
    uninstaller: PathBuf,
    autostart: PathBuf,
}
impl Layout {
    fn from_environment() -> Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
        let xdg = |name, fallback| {
            std::env::var_os(name)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(fallback))
        };
        let data = xdg("XDG_DATA_HOME", ".local/share");
        let config = xdg("XDG_CONFIG_HOME", ".config");
        Ok(Self {
            destination: home.join(INSTALL_DIR),
            launcher: data.join("applications/token-ashtray.desktop"),
            uninstaller: data.join("applications/token-ashtray-setup.desktop"),
            autostart: config.join("autostart/token-ashtray.desktop"),
        })
    }
    fn preflight(
        &self,
        removing: bool,
    ) -> Result<(Option<String>, Option<String>, Option<String>)> {
        for dir in [
            &self.destination,
            self.launcher.parent().unwrap(),
            self.uninstaller.parent().unwrap(),
            self.autostart.parent().unwrap(),
        ] {
            safe_directory(dir)?;
        }
        for name in NAMES.iter().copied().chain([SETUP, IDENTITY, MARKER]) {
            regular(&self.destination.join(name))?;
        }
        let marker = self.destination.join(MARKER);
        if marker.exists() {
            if io(fs::read_to_string(&marker))? != MARKER_TEXT {
                return Err("Installation marker belongs to something else".into());
            }
        } else if self.destination.exists()
            && io(fs::read_dir(&self.destination))?.next().is_some()
            && (!removing
                || NAMES
                    .iter()
                    .copied()
                    .chain([SETUP, IDENTITY])
                    .any(|name| self.destination.join(name).exists()))
        {
            return Err("Installation directory contains unowned files".into());
        }
        let launcher = owned_entry(
            &self.launcher,
            "X-Token-Ashtray-Installed",
            &command(&self.destination.join(APP), ""),
        )?;
        let setup = owned_entry(
            &self.uninstaller,
            "X-Token-Ashtray-Setup",
            &command(&self.destination.join(SETUP), " --uninstall"),
        )?;
        let startup = owned_entry(
            &self.autostart,
            "X-Token-Ashtray",
            &command(&self.destination.join(APP), " --autostart"),
        )?;
        Ok((launcher, setup, startup))
    }
    fn operation_lock(&self) -> Result<fs::File> {
        // This stable parent is never removed by setup, so independent windows
        // lock the same inode even while the installation itself is removed.
        let parent = self
            .destination
            .parent()
            .ok_or("Missing installation parent")?;
        safe_directory(parent)?;
        io(fs::create_dir_all(parent))?;
        let directory = io(fs::File::open(parent))?;
        directory.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => {
                "Another setup operation is running. Wait for it to finish, then try again."
                    .to_owned()
            }
            fs::TryLockError::Error(error) => {
                format!("Cannot lock the installation parent: {error}")
            }
        })?;
        Ok(directory)
    }
    fn install(&self, payload: &Payload) -> Result<()> {
        self.write(payload, false).map(drop)
    }
    /// The headless upgrade the app starts after it quits. It updates only an
    /// existing marked installation and does nothing when this build is
    /// already installed.
    fn update(&self, payload: &Payload) -> Result<Updated> {
        self.write(payload, true)
    }
    /// Installs or upgrades under the operation lock. With `update_only`,
    /// an unmarked destination is refused and the same build is left alone,
    /// both decided under the lock.
    fn write(&self, payload: &Payload, update_only: bool) -> Result<Updated> {
        let _lock = self.operation_lock()?;
        self.preflight(false)?;
        let identity = payload.identity()?;
        if update_only {
            if !self.destination.join(MARKER).exists() {
                return Err(
                    "Token Ashtray is not installed for this user; nothing was updated".into(),
                );
            }
            if fs::read(self.destination.join(IDENTITY)).ok().as_ref() == Some(&identity) {
                return Ok(Updated::AlreadyCurrent);
            }
        }
        let mut changes = Vec::new();
        for (name, bytes) in &payload.files {
            changes.push(Change::write(
                &self.destination.join(name),
                bytes,
                if name == APP { 0o755 } else { 0o644 },
            )?);
        }
        for (name, bytes, mode) in [
            (SETUP, payload.executable.as_slice(), 0o755),
            (IDENTITY, identity.as_slice(), 0o644),
            (MARKER, MARKER_TEXT.as_bytes(), 0o644),
        ] {
            changes.push(Change::write(&self.destination.join(name), bytes, mode)?);
        }
        let launcher = format!(
            "[Desktop Entry]\nType=Application\nName=Token Ashtray\nExec={}\nIcon={}\nTerminal=false\nCategories=Utility;\nX-Token-Ashtray-Installed=true\n",
            command(&self.destination.join(APP), ""),
            self.destination.join(ICON).display()
        );
        let uninstall = format!(
            "[Desktop Entry]\nType=Application\nName=Token Ashtray Setup\nComment=Uninstall Token Ashtray (keeps preferences)\nExec={}\nIcon={}\nTerminal=false\nCategories=Utility;\nX-Token-Ashtray-Setup=true\n",
            command(&self.destination.join(SETUP), " --uninstall"),
            self.destination.join(ICON).display()
        );
        changes.push(Change::write(&self.launcher, launcher.as_bytes(), 0o644)?);
        changes.push(Change::write(
            &self.uninstaller,
            uninstall.as_bytes(),
            0o644,
        )?);
        // All replacement bytes and prior versions are staged before commit.
        // Autostart and all config/provider files are never rewritten.
        commit_changes(changes)?;
        Ok(Updated::Installed)
    }
    fn uninstall(&self) -> Result<()> {
        let _lock = self.operation_lock()?;
        let (launcher, setup, startup) = self.preflight(true)?;
        let mut changes = Vec::new();
        let mut remove_startup_dir = false;
        if let Some(body) = startup {
            let meta = io(fs::metadata(self.autostart.parent().unwrap()))?;
            let identity = format!("{}:{}", meta.dev(), meta.ino());
            remove_startup_dir = fields(&body).get("X-Token-Ashtray-Created-Autostart-Dir")
                == Some(&identity.as_str());
            changes.push(Change::remove(&self.autostart)?);
        }
        if launcher.is_some() {
            changes.push(Change::remove(&self.launcher)?);
        }
        if setup.is_some() {
            changes.push(Change::remove(&self.uninstaller)?);
        }
        let owned_directory = self.destination.join(MARKER).exists();
        if owned_directory {
            for name in NAMES.iter().copied().chain([SETUP, IDENTITY, MARKER]) {
                let path = self.destination.join(name);
                if path.exists() {
                    changes.push(Change::remove(&path)?);
                }
            }
        }
        commit_changes(changes)?;
        if remove_startup_dir {
            let _ = fs::remove_dir(self.autostart.parent().unwrap());
        }
        if owned_directory {
            let _ = fs::remove_dir(&self.destination);
        }
        Ok(())
    }
}
#[cfg(test)]
thread_local! {
    static FAIL_COMMIT_AT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
fn commit_step() -> Result<()> {
    #[cfg(test)]
    return FAIL_COMMIT_AT.with(|counter| match counter.get() {
        Some(0) => {
            counter.set(None);
            Err(std::io::Error::from_raw_os_error(5).to_string())
        }
        Some(left) => {
            counter.set(Some(left - 1));
            Ok(())
        }
        None => Ok(()),
    });
    #[cfg(not(test))]
    Ok(())
}

struct Change {
    path: PathBuf,
    staged: Option<tempfile::NamedTempFile>,
    backup: Option<tempfile::NamedTempFile>,
}
impl Change {
    fn write(path: &Path, bytes: &[u8], mode: u32) -> Result<Self> {
        let staged = stage_atomic(path, bytes, mode)?;
        Self::prepare(path, Some(staged))
    }
    fn remove(path: &Path) -> Result<Self> {
        Self::prepare(path, None)
    }
    fn prepare(path: &Path, staged: Option<tempfile::NamedTempFile>) -> Result<Self> {
        regular(path)?;
        let backup = if path.exists() {
            let file = io(tempfile::NamedTempFile::new_in(path.parent().unwrap()))?;
            io(fs::copy(path, file.path()))?;
            io(file.as_file().sync_all())?;
            Some(file)
        } else {
            None
        };
        Ok(Self {
            path: path.to_owned(),
            staged,
            backup,
        })
    }
    fn apply(&self) -> Result<()> {
        commit_step()?;
        match &self.staged {
            Some(file) => io(fs::rename(file.path(), &self.path)),
            None => io(fs::remove_file(&self.path)),
        }
    }
    fn restore(&self) -> std::io::Result<()> {
        match &self.backup {
            Some(file) => fs::rename(file.path(), &self.path),
            None => fs::remove_file(&self.path),
        }
    }
}
fn commit_changes(mut changes: Vec<Change>) -> Result<()> {
    for index in 0..changes.len() {
        if let Err(error) = changes[index].apply() {
            let mut recovery_errors = Vec::new();
            for change in changes[..index].iter_mut().rev() {
                if let Err(restore_error) = change.restore() {
                    let mut detail = format!("{}: {restore_error}", change.path.display());
                    // Never delete the only saved prior bytes when the filesystem
                    // also refuses rollback. Expose their real recovery location.
                    if let Some(mut backup) = change.backup.take() {
                        detail.push_str(&format!(
                            "; prior file retained at {}",
                            backup.path().display()
                        ));
                        backup.disable_cleanup(true);
                    }
                    recovery_errors.push(detail);
                }
            }
            return Err(if recovery_errors.is_empty() {
                format!(
                    "{}: {error}. Previous files restored; you can retry setup.",
                    changes[index].path.display()
                )
            } else {
                format!(
                    "{}: {error}. Recovery needs attention: {}",
                    changes[index].path.display(),
                    recovery_errors.join("; ")
                )
            });
        }
    }
    Ok(())
}
fn stage_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<tempfile::NamedTempFile> {
    io(fs::create_dir_all(path.parent().unwrap()))?;
    let mut file = io(tempfile::NamedTempFile::new_in(path.parent().unwrap()))?;
    io(file.write_all(bytes))?;
    io(file.as_file().sync_all())?;
    io(file
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode)))?;
    Ok(file)
}

struct SetupApp {
    layout: Result<Layout>,
    payload: Result<Payload>,
    removing: bool,
    complete: bool,
    error: Option<String>,
}
impl eframe::App for SetupApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(12.0); ui.heading(if self.removing {"Uninstall Token Ashtray"} else {"Token Ashtray Setup"}); ui.add_space(12.0);
            if self.complete {
                ui.label(if self.removing {"Token Ashtray was uninstalled. Preferences and provider logins were kept."} else {"Installed. Open Token Ashtray from your applications menu."});
            } else {
                ui.label("Quit Token Ashtray before continuing. Restart it after an upgrade.");
                ui.add_space(8.0);
                ui.label(if self.removing {"Removes the application, menu entries and autostart. Your preferences and provider logins stay on this computer."} else {"Install or upgrade for your user. Preferences, autostart choice and provider logins are preserved. No administrator access is needed."});
                if let Ok(layout) = &self.layout { ui.add_space(8.0); ui.label(format!("Location: {}",layout.destination.display())); }
                if !self.removing && let Ok(payload) = &self.payload { ui.small(format!("Build: {}",payload.manifest.source_sha)); }
                let failure = self.layout.as_ref().err().or_else(|| if self.removing {None} else {self.payload.as_ref().err()});
                if let Some(error) = failure { ui.colored_label(egui::Color32::LIGHT_RED,error); }
                ui.add_space(12.0);
                if ui.add_enabled(failure.is_none(),egui::Button::new(if self.removing {"Uninstall"} else {"Install / Upgrade"})).clicked() {
                    let result = self.layout.as_ref().map_err(Clone::clone).and_then(|layout| if self.removing {layout.uninstall()} else {self.payload.as_ref().map_err(Clone::clone).and_then(|payload|layout.install(payload))});
                    match result {Ok(())=>self.complete=true,Err(error)=>self.error=Some(error)}
                }
            }
            if let Some(error) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED,format!("Setup could not finish: {error}")); }
            ui.add_space(12.0);
            if ui.button("Close").clicked() { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        });
    }
}
pub fn run() -> eframe::Result<()> {
    let executable = std::env::current_exe()
        .and_then(fs::read)
        .map_err(|e| e.to_string())
        .and_then(Payload::parse);
    if std::env::args().any(|a| a == APPLY_ARGUMENT) {
        let result = executable
            .and_then(|payload| Layout::from_environment()?.update(&payload))
            .map_err(|error| format!("Token Ashtray update was not applied: {error}"));
        return match result {
            Ok(updated) => {
                eprintln!("Token Ashtray update: {updated:?}");
                Ok(())
            }
            Err(error) => {
                eprintln!("{error}");
                Err(eframe::Error::AppCreation(Box::new(std::io::Error::other(
                    error,
                ))))
            }
        };
    }
    if std::env::args().any(|a| a == "--inspect") {
        match executable {
            Ok(payload) => println!(
                "{}",
                serde_json::to_string_pretty(&payload.manifest).unwrap()
            ),
            Err(error) => {
                return Err(eframe::Error::AppCreation(Box::new(std::io::Error::other(
                    error,
                ))));
            }
        }
        return Ok(());
    }
    let app = SetupApp {
        layout: Layout::from_environment(),
        payload: executable,
        removing: std::env::args().any(|a| a == "--uninstall"),
        complete: false,
        error: None,
    };
    eframe::run_native(
        "Token Ashtray Setup",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([600.0, 330.0])
                .with_resizable(false),
            ..Default::default()
        },
        Box::new(|_| Ok(Box::new(app))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn payload() -> Payload {
        payload_with_app(b"\x7fELFapproved executable")
    }
    fn payload_with_app(app: &[u8]) -> Payload {
        let mut files: BTreeMap<String, Vec<u8>> = NAMES
            .iter()
            .map(|s| (s.to_string(), b"notice".to_vec()))
            .collect();
        files.insert(APP.into(), app.to_vec());
        files.insert(
            "RELEASE-METRICS.json".into(),
            format!("{{\"binary_sha256\":\"{}\"}}", sha256(app)).into_bytes(),
        );
        let stub = b"\x7fELFsetup";
        let manifest = Manifest {
            format: 1,
            source_sha: "a".repeat(40),
            stub_size: stub.len(),
            stub_sha256: sha256(stub),
            members: files
                .iter()
                .map(|(name, bytes)| Member {
                    name: name.clone(),
                    size: bytes.len(),
                    sha256: sha256(bytes),
                })
                .collect(),
        };
        let mut bytes = stub.to_vec();
        for body in files.values() {
            bytes.extend(body);
        }
        let json = serde_json::to_vec(&manifest).unwrap();
        bytes.extend(&json);
        bytes.extend((json.len() as u64).to_le_bytes());
        bytes.extend(MAGIC);
        Payload::parse(bytes).unwrap()
    }
    fn layout(root: &Path) -> Layout {
        Layout {
            destination: root.join("home path/.local/opt/token-ashtray"),
            launcher: root.join("data path/applications/token-ashtray.desktop"),
            uninstaller: root.join("data path/applications/token-ashtray-setup.desktop"),
            autostart: root.join("config path/autostart/token-ashtray.desktop"),
        }
    }
    fn file_snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u32)> {
        fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, u32)>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(root, &path, out);
                } else {
                    out.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        (
                            fs::read(&path).unwrap(),
                            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                        ),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        visit(root, root, &mut out);
        out
    }
    #[test]
    fn late_fresh_install_failure_rolls_back_and_same_payload_retries() {
        let tmp = tempfile::tempdir().unwrap();
        let l = layout(tmp.path());
        let canary = tmp.path().join("provider-credentials");
        fs::write(&canary, b"keep credentials").unwrap();
        let before = file_snapshot(tmp.path());
        FAIL_COMMIT_AT.with(|counter| counter.set(Some(2)));
        assert!(
            l.install(&payload())
                .unwrap_err()
                .contains("Input/output error")
        );
        assert_eq!(
            file_snapshot(tmp.path()),
            before,
            "failed fresh setup must restore original files"
        );
        l.install(&payload()).unwrap();
        assert_eq!(
            fs::read(l.destination.join(APP)).unwrap(),
            payload().files[APP]
        );
        l.uninstall().unwrap();
        assert_eq!(file_snapshot(tmp.path()), before);
    }
    #[test]
    fn late_upgrade_failure_rolls_back_old_files_and_same_payload_retries() {
        for fail_after in [2, 10, 11] {
            let tmp = tempfile::tempdir().unwrap();
            let l = layout(tmp.path());
            let old = payload();
            let new = payload_with_app(b"\x7fELFnew approved executable");
            l.install(&old).unwrap();
            fs::write(l.destination.join("user-canary"), b"keep user file").unwrap();
            fs::create_dir_all(l.autostart.parent().unwrap()).unwrap();
            fs::write(&l.autostart,format!("[Desktop Entry]\nType=Application\nExec={}\nX-Token-Ashtray=true\nHidden=true\n",command(&l.destination.join(APP)," --autostart"))).unwrap();
            let before = file_snapshot(tmp.path());
            FAIL_COMMIT_AT.with(|counter| counter.set(Some(fail_after)));
            assert!(l.install(&new).is_err());
            assert_eq!(
                file_snapshot(tmp.path()),
                before,
                "upgrade fault at {fail_after} must restore app/setup/manifest/menu/startup and user bytes"
            );
            l.install(&new).unwrap();
            assert_eq!(fs::read(l.destination.join(APP)).unwrap(), new.files[APP]);
            assert_eq!(
                fs::read(l.destination.join("user-canary")).unwrap(),
                b"keep user file"
            );
        }
    }
    #[test]
    fn headless_update_upgrades_only_a_marked_install_and_skips_the_same_build() {
        let tmp = tempfile::tempdir().unwrap();
        let l = layout(tmp.path());
        let old = payload();
        let new = payload_with_app(b"\x7fELFnew approved executable");
        let before = file_snapshot(tmp.path());
        assert!(l.update(&new).unwrap_err().contains("not installed"));
        assert_eq!(file_snapshot(tmp.path()), before, "no fresh install");

        l.install(&old).unwrap();
        fs::write(l.destination.join("user-canary"), b"keep user file").unwrap();
        fs::create_dir_all(l.autostart.parent().unwrap()).unwrap();
        let autostart = format!(
            "[Desktop Entry]\nType=Application\nExec={}\nX-Token-Ashtray=true\n",
            command(&l.destination.join(APP), " --autostart")
        );
        fs::write(&l.autostart, &autostart).unwrap();
        assert_eq!(l.update(&old).unwrap(), Updated::AlreadyCurrent);
        assert_eq!(l.update(&new).unwrap(), Updated::Installed);
        assert_eq!(fs::read(l.destination.join(APP)).unwrap(), new.files[APP]);
        assert_eq!(fs::read_to_string(&l.autostart).unwrap(), autostart);
        assert_eq!(
            fs::read(l.destination.join("user-canary")).unwrap(),
            b"keep user file"
        );
        assert_eq!(l.update(&new).unwrap(), Updated::AlreadyCurrent);
    }
    #[test]
    fn failed_headless_update_keeps_the_previous_version_and_retries() {
        let tmp = tempfile::tempdir().unwrap();
        let l = layout(tmp.path());
        let new = payload_with_app(b"\x7fELFnew approved executable");
        l.install(&payload()).unwrap();
        let before = file_snapshot(tmp.path());
        FAIL_COMMIT_AT.with(|counter| counter.set(Some(3)));
        assert!(l.update(&new).is_err());
        assert_eq!(
            file_snapshot(tmp.path()),
            before,
            "the old version stays runnable"
        );
        assert_eq!(l.update(&new).unwrap(), Updated::Installed);
    }
    #[test]
    fn late_uninstall_failure_restores_all_files_then_retries() {
        let tmp = tempfile::tempdir().unwrap();
        let l = layout(tmp.path());
        l.install(&payload()).unwrap();
        fs::write(l.destination.join("user-canary"), b"keep user file").unwrap();
        fs::create_dir_all(l.autostart.parent().unwrap()).unwrap();
        fs::write(
            &l.autostart,
            format!(
                "[Desktop Entry]\nType=Application\nExec={}\nX-Token-Ashtray=true\n",
                command(&l.destination.join(APP), " --autostart")
            ),
        )
        .unwrap();
        let before = file_snapshot(tmp.path());
        FAIL_COMMIT_AT.with(|counter| counter.set(Some(2)));
        assert!(l.uninstall().is_err());
        assert_eq!(
            file_snapshot(tmp.path()),
            before,
            "failed removal must restore the existing installation and startup"
        );
        l.uninstall().unwrap();
        assert!(!l.launcher.exists() && !l.uninstaller.exists() && !l.autostart.exists());
        assert_eq!(
            fs::read(l.destination.join("user-canary")).unwrap(),
            b"keep user file"
        );
    }
    #[test]
    fn concurrent_setup_actions_cannot_mutate_while_operation_lock_is_held() {
        let tmp = tempfile::tempdir().unwrap();
        let l = layout(tmp.path());
        let p = payload();
        l.install(&p).unwrap();
        let before = file_snapshot(tmp.path());
        let lock = l.operation_lock().unwrap();
        std::thread::scope(|scope| {
            let install = scope.spawn(|| l.install(&p));
            let uninstall = scope.spawn(|| l.uninstall());
            assert!(
                install
                    .join()
                    .unwrap()
                    .unwrap_err()
                    .contains("Another setup operation")
            );
            assert!(
                uninstall
                    .join()
                    .unwrap()
                    .unwrap_err()
                    .contains("Another setup operation")
            );
        });
        assert_eq!(file_snapshot(tmp.path()), before);
        drop(lock);
        l.install(&p).unwrap();
        l.uninstall().unwrap();
        assert!(!l.destination.exists());
    }
    #[test]
    fn sha256_standard_vectors() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
    #[test]
    fn refuses_corrupt_payloads() {
        let p = payload();
        let mut corrupt = p.executable.clone();
        corrupt[p.manifest.stub_size] ^= 1;
        assert!(Payload::parse(corrupt).err().unwrap().contains("Integrity"));
        let mut corrupt = p.executable.clone();
        corrupt[4] ^= 1;
        assert!(Payload::parse(corrupt).err().unwrap().contains("integrity"));
        assert!(Payload::parse(p.executable[..100].to_vec()).is_err());
    }
    #[test]
    fn legacy_and_gui_upgrade_keep_config_startup_providers_then_remove_only_owned_files() {
        for disabled in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let l = layout(tmp.path());
            let p = payload();
            fs::create_dir_all(&l.destination).unwrap();
            fs::write(l.destination.join(MARKER), MARKER_TEXT).unwrap();
            fs::write(l.destination.join(APP), "old ZIP app").unwrap();
            fs::write(l.destination.join("unrelated"), "keep").unwrap();
            fs::create_dir_all(l.autostart.parent().unwrap()).unwrap();
            let startup = format!(
                "[Desktop Entry]\nType=Application\nExec={}\nX-Token-Ashtray=true\nHidden={}\nX-GNOME-Autostart-enabled={}\nX-Custom=preserve me\n",
                command(&l.destination.join(APP), " --autostart"),
                disabled,
                !disabled
            );
            fs::write(&l.autostart, &startup).unwrap();
            let config = tmp.path().join("config path/token-ashtray/config.toml");
            fs::create_dir_all(config.parent().unwrap()).unwrap();
            fs::write(&config, "unknown = 'retained'\n").unwrap();
            let providers = [
                ".claude/.credentials.json",
                ".codex/auth.json",
                ".cursor/auth.json",
                ".config/gh/hosts.yml",
                ".grok/auth.json",
                ".kimi/credentials.json",
                ".gemini/oauth_creds.json",
            ];
            for name in providers {
                let file = tmp.path().join(name);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, name).unwrap();
            }
            for _ in 0..3 {
                l.install(&p).unwrap();
                assert_eq!(fs::read(l.destination.join(APP)).unwrap(), p.files[APP]);
                assert_eq!(fs::read_to_string(&l.autostart).unwrap(), startup);
            }
            l.uninstall().unwrap();
            l.uninstall().unwrap();
            assert!(!l.launcher.exists() && !l.uninstaller.exists() && !l.autostart.exists());
            assert_eq!(
                fs::read_to_string(config).unwrap(),
                "unknown = 'retained'\n"
            );
            assert_eq!(
                fs::read_to_string(l.destination.join("unrelated")).unwrap(),
                "keep"
            );
            for name in providers {
                assert_eq!(fs::read_to_string(tmp.path().join(name)).unwrap(), name);
            }
        }
    }
    #[test]
    fn fresh_install_has_no_startup_and_removes_owned_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let l = layout(tmp.path());
        l.install(&payload()).unwrap();
        assert!(!l.autostart.exists());
        l.uninstall().unwrap();
        assert!(!l.destination.exists());
    }
    #[test]
    fn refuses_unrelated_marker_entry_and_symlink_before_replacing_app() {
        for kind in ["marker", "launcher", "setup", "startup", "symlink"] {
            let tmp = tempfile::tempdir().unwrap();
            let l = layout(tmp.path());
            let p = payload();
            l.install(&p).unwrap();
            match kind {
                "marker" => fs::write(l.destination.join(MARKER), "other").unwrap(),
                "launcher" => fs::write(
                    &l.launcher,
                    "[Desktop Entry]\nType=Application\nExec=other\n",
                )
                .unwrap(),
                "setup" => fs::write(&l.uninstaller, "other").unwrap(),
                "startup" => {
                    fs::create_dir_all(l.autostart.parent().unwrap()).unwrap();
                    fs::write(&l.autostart, "other").unwrap();
                }
                _ => {
                    fs::remove_file(l.destination.join(ICON)).unwrap();
                    std::os::unix::fs::symlink(
                        tmp.path().join("elsewhere"),
                        l.destination.join(ICON),
                    )
                    .unwrap();
                }
            }
            assert!(l.install(&p).is_err(), "{kind}");
            assert!(l.uninstall().is_err(), "{kind}");
            assert_eq!(fs::read(l.destination.join(APP)).unwrap(), p.files[APP]);
        }
    }
    #[test]
    fn exact_legacy_desktop_escaping_and_bad_paths() {
        assert_eq!(
            command(Path::new("/home/a b/app"), " --autostart"),
            "\"/home/a b/app\" --autostart"
        );
        assert_eq!(
            command(Path::new("/a/$`\"%\\"), ""),
            "\"/a/\\\\$\\\\`\\\\\"%%\\\\\\\\\""
        );
        assert!(safe_directory(Path::new("relative")).is_err());
        assert!(safe_directory(Path::new("/home/new\nline")).is_err());
    }
    #[test]
    fn uninstall_preserves_unowned_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let l = layout(tmp.path());
        fs::create_dir_all(&l.destination).unwrap();
        l.uninstall().unwrap();
        assert!(l.destination.is_dir());
        assert!(fs::read_dir(&l.destination).unwrap().next().is_none());
    }
    #[test]
    fn permission_failure_is_reported() {
        // Linux procfs is unwritable even when tests run with elevated identity.
        let l = Layout {
            destination: PathBuf::from("/proc/token-ashtray-setup-test"),
            ..layout(Path::new("/proc"))
        };
        assert!(l.install(&payload()).is_err());
    }
}
