//! Updater core for the signed public release channel.
//!
//! It implements the candidate profile in `docs/updates/public-channel-contract.md`.
//! [`verify`] and the URL grammar are pure: no network, filesystem, clock or
//! installer access. [`Transport`] performs the bounded HTTPS fetches. Callers
//! persist the returned state; this module only proposes it. No production root
//! is embedded yet, so nothing here can authorize a real update.
pub mod apply;
mod channel;
mod json;
#[cfg(target_os = "linux")]
pub mod linux;
/// QA builds only (#52); release builds never enable the feature.
#[cfg(feature = "qa-offline-update")]
pub mod qa;
mod root;
mod service;
mod store;
#[cfg(test)]
mod tests;
mod transport;
mod updater;
#[cfg(windows)]
mod windows;

use std::collections::BTreeMap;
use std::fmt;
use std::io;

use json::Value;
pub use root::RootPolicy;
pub use service::{Failure, Unavailable, UpdateStatus, Updater, UpdaterConfig, embedded_root};
pub use store::{Store, StoreError};
pub use transport::{Discovered, FetchError, Transport};
pub use updater::{CheckError, Outcome, check_once};

pub const MANIFEST_SCHEMA: &str = "token-ashtray-release-manifest-v1";
pub const ROOT_SCHEMA: &str = "token-ashtray-release-root-v1";
/// Complete manifest plus rotation bundle bound from the contract.
pub const MAX_METADATA: usize = 64 * 1024;
pub const MAX_ARTIFACT: u64 = 128 * 1024 * 1024;
pub const MAX_TRANSITIONS: usize = 8;
const MAX_INTEGER: u64 = (1 << 53) - 1;

/// A rejected input, named by the same codes as the Python release fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reject(&'static str);

impl Reject {
    pub fn code(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Reject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "update metadata rejected: {}", self.0)
    }
}

impl std::error::Error for Reject {}

fn require(condition: bool, code: &'static str) -> Result<(), Reject> {
    if condition { Ok(()) } else { Err(Reject(code)) }
}

/// The two platform rows every signed manifest must carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    LinuxX86_64,
    WindowsX86_64,
}

impl Target {
    pub const ALL: [Self; 2] = [Self::LinuxX86_64, Self::WindowsX86_64];

    /// The row for the platform this build runs on, if the channel has one.
    pub fn current() -> Option<Self> {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Self::LinuxX86_64)
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(Self::WindowsX86_64)
        } else {
            None
        }
    }

    pub fn triple(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "x86_64-unknown-linux-gnu",
            Self::WindowsX86_64 => "x86_64-pc-windows-msvc",
        }
    }

    pub fn artifact_kind(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "linux-setup-elf-v1",
            Self::WindowsX86_64 => "windows-setup-exe-v1",
        }
    }

    pub fn artifact_name(self, version: Version) -> String {
        match self {
            Self::LinuxX86_64 => format!("token-ashtray-setup-{version}-{}", self.triple()),
            Self::WindowsX86_64 => format!("token-ashtray-setup-{version}-{}.exe", self.triple()),
        }
    }
}

/// A strict stable `X.Y.Z` version without prerelease, build metadata or leading zeros.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    pub fn parse(text: &str) -> Result<Self, Reject> {
        const CODE: &str = "stable-version";
        let part = |part: &str| {
            require(
                !part.is_empty()
                    && part.bytes().all(|byte| byte.is_ascii_digit())
                    && (part == "0" || !part.starts_with('0')),
                CODE,
            )?;
            part.parse().map_err(|_| Reject(CODE))
        };
        require(text.len() <= 64, CODE)?;
        match text.split('.').collect::<Vec<_>>()[..] {
            [major, minor, patch] => Ok(Self {
                major: part(major)?,
                minor: part(minor)?,
                patch: part(patch)?,
            }),
            _ => Err(Reject(CODE)),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// One signed platform row, checked against the constructed filename and kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub target: Target,
    /// The verified manifest version the row belongs to.
    pub version: Version,
    pub filename: String,
    pub length: u64,
    pub sha256: [u8; 32],
    pub app_sha256: [u8; 32],
    pub descriptor_sha256: [u8; 32],
}

impl ArtifactRecord {
    #[cfg(test)]
    fn verify_bytes(&self, data: &[u8]) -> Result<(), Reject> {
        self.check_digest(data.len() as u64, sha256(data))
    }

    fn check_digest(&self, length: u64, sha256: [u8; 32]) -> Result<(), Reject> {
        require(
            length > 0 && length == self.length && sha256 == self.sha256,
            "artifact-bytes",
        )
    }

    /// Whether `bytes` read to its end match the signed size and SHA-256.
    /// Reading one byte past the signed length is enough to reject it.
    pub(crate) fn matches(&self, bytes: impl io::Read) -> bool {
        let mut digest = Sha256::new();
        io::copy(&mut bytes.take(self.length + 1), &mut digest)
            .is_ok_and(|length| self.check_digest(length, digest.finish()).is_ok())
    }

    /// Whether the installed app at `app` already has the signed app hash.
    pub(crate) fn app_installed(&self, app: &std::path::Path) -> bool {
        let mut digest = Sha256::new();
        std::fs::File::open(app)
            .and_then(|mut app| io::copy(&mut app, &mut digest))
            .is_ok()
            && digest.finish() == self.app_sha256
    }
}

/// The last accepted release, kept as a floor beside the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseFloor {
    pub sequence: u64,
    pub version: Version,
    /// SHA-256 of the exact accepted bundle bytes.
    pub digest: [u8; 32],
    pub pending: bool,
}

/// Durable floors the caller stores outside staging and cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub root: RootPolicy,
    /// `None` until a release has been accepted.
    pub release: Option<ReleaseFloor>,
}

impl State {
    pub fn initial(root: RootPolicy) -> Self {
        Self {
            root,
            release: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The signed release is already installed.
    NoUpdate,
    /// A newer release was accepted for the first time.
    Pending,
    /// The exact known pending bundle was observed again.
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub status: Status,
    pub artifact: ArtifactRecord,
}

/// The result of checking one bundle.
///
/// The caller persists `state` whatever the result. After a rejection it
/// keeps the previous release floor but still carries any root advance the
/// transition chain authenticated, which the contract requires to be stored
/// before any release is used.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    pub state: State,
    pub result: Result<Accepted, Reject>,
}

const MANIFEST_FIELDS: [&str; 10] = [
    "schema",
    "product",
    "repository",
    "channel",
    "version",
    "tag",
    "sequence",
    "root_version",
    "source_sha",
    "artifacts",
];
const ARTIFACT_FIELDS: [&str; 7] = [
    "filename",
    "purpose",
    "artifact_kind",
    "length",
    "sha256",
    "app_sha256",
    "descriptor_sha256",
];

/// Verifies a raw manifest bundle for `target` against stored floors.
///
/// `observed` is the version in the tag of the validated discovery redirect.
/// The source SHA is checked for shape only: a client cannot establish build
/// provenance, which is left to reviewed build receipts.
fn verify(
    raw: &[u8],
    state: &State,
    target: Target,
    installed: Version,
    observed: Version,
) -> Verification {
    let mut state = state.clone();
    let result = check(raw, &mut state, target, installed, observed);
    Verification { state, result }
}

/// Updates `state.root` as soon as the rotation chain authenticates, and
/// `state.release` only once the release is accepted.
fn check(
    raw: &[u8],
    state: &mut State,
    target: Target,
    installed: Version,
    observed: Version,
) -> Result<Accepted, Reject> {
    require(
        !raw.is_empty() && raw.len() <= MAX_METADATA,
        "metadata-bound",
    )?;
    let bundle = json::parse(raw)?;
    let bundle = exact(&bundle, &["manifest", "transitions"])?;
    state.root = root::advance(&state.root, &bundle["transitions"])?;
    let root = &state.root;

    let envelope = root::Envelope::parse(&bundle["manifest"])?;
    let body = exact(envelope.signed, &MANIFEST_FIELDS)?;
    header(body, MANIFEST_SCHEMA)?;
    root.signed_by(&envelope, root::Role::Release)?;
    let version_text = string(&body["version"]).ok_or(Reject("stable-version"))?;
    let version = Version::parse(version_text)?;
    require(
        string(&body["tag"]) == Some(format!("v{version_text}").as_str()) && version == observed,
        "version-tag",
    )?;
    require(
        integer(&body["root_version"]) == Some(root.version()),
        "root-binding",
    )?;
    require(
        string(&body["source_sha"]).is_some_and(|sha| lower_hex(sha, 40)),
        "source-binding",
    )?;
    let sequence = integer(&body["sequence"]).ok_or(Reject("release-sequence"))?;
    let triples = Target::ALL.map(Target::triple);
    let rows = exact(&body["artifacts"], &triples)?;
    let mut selected = None;
    for platform in Target::ALL {
        let record = artifact(platform, version, &rows[platform.triple()])?;
        if platform == target {
            selected = Some(record);
        }
    }
    let artifact = selected.expect("every target has a checked row");
    require(version >= installed, "installed-downgrade")?;

    let digest = sha256(raw);
    let retry = match state.release {
        Some(known) if known.sequence == sequence => {
            require(
                known.digest == digest && known.version == version,
                "same-sequence-change",
            )?;
            require(known.pending || version == installed, "not-pending")?;
            true
        }
        Some(known) => {
            require(
                sequence > known.sequence && version >= known.version,
                "release-rollback",
            )?;
            require(version > known.version, "same-version-change")?;
            false
        }
        None => false,
    };
    let status = if version == installed {
        Status::NoUpdate
    } else if retry {
        Status::Retry
    } else {
        Status::Pending
    };
    state.release = Some(ReleaseFloor {
        sequence,
        version,
        digest,
        pending: status != Status::NoUpdate,
    });
    Ok(Accepted { status, artifact })
}

fn artifact(target: Target, version: Version, value: &Value) -> Result<ArtifactRecord, Reject> {
    const CODE: &str = "artifact-binding";
    let row = exact(value, &ARTIFACT_FIELDS)?;
    let filename = target.artifact_name(version);
    require(
        string(&row["filename"]) == Some(filename.as_str())
            && string(&row["purpose"]) == Some("installer")
            && string(&row["artifact_kind"]) == Some(target.artifact_kind()),
        CODE,
    )?;
    let hash = |field: &str| string(&row[field]).and_then(hex::<32>).ok_or(Reject(CODE));
    Ok(ArtifactRecord {
        target,
        version,
        filename,
        length: integer(&row["length"])
            .filter(|length| *length <= MAX_ARTIFACT)
            .ok_or(Reject(CODE))?,
        sha256: hash("sha256")?,
        app_sha256: hash("app_sha256")?,
        descriptor_sha256: hash("descriptor_sha256")?,
    })
}

/// Returns the object's fields when its key set is exactly `fields`.
fn exact<'a>(value: &'a Value, fields: &[&str]) -> Result<&'a BTreeMap<String, Value>, Reject> {
    match value {
        Value::Object(map)
            if map.len() == fields.len() && fields.iter().all(|field| map.contains_key(*field)) =>
        {
            Ok(map)
        }
        _ => Err(Reject("schema-fields")),
    }
}

fn header(fields: &BTreeMap<String, Value>, schema: &str) -> Result<(), Reject> {
    require(
        string(&fields["schema"]) == Some(schema)
            && string(&fields["product"]) == Some("token-ashtray")
            && string(&fields["repository"]) == Some("rammicz/token-ashtray")
            && string(&fields["channel"]) == Some("stable"),
        "schema-product-channel",
    )
}

fn string(value: &Value) -> Option<&str> {
    match value {
        Value::Str(text) => Some(text),
        _ => None,
    }
}

/// A positive integer within the JSON-safe range.
fn integer(value: &Value) -> Option<u64> {
    match value {
        Value::Int(number) if (1..=MAX_INTEGER).contains(number) => Some(*number),
        _ => None,
    }
}

fn lower_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Decodes exactly `2 * N` lowercase hex digits.
fn hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if !lower_hex(text, 2 * N) {
        return None;
    }
    let mut bytes = [0; N];
    let (pairs, _) = text.as_bytes().as_chunks::<2>();
    for (byte, pair) in bytes.iter_mut().zip(pairs) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(data);
    digest.finish()
}

/// Incremental SHA-256 for streamed artifacts.
struct Sha256(ring::digest::Context);

impl Sha256 {
    fn new() -> Self {
        Self(ring::digest::Context::new(&ring::digest::SHA256))
    }

    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    fn finish(self) -> [u8; 32] {
        self.0
            .finish()
            .as_ref()
            .try_into()
            .expect("SHA-256 output is 32 bytes")
    }
}

impl io::Write for Sha256 {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.update(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
