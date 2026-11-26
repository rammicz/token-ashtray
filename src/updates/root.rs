//! Root policy validation, signature thresholds and sequential root rotation.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::json::{self, Value};
use super::{
    MAX_METADATA, MAX_TRANSITIONS, ROOT_SCHEMA, Reject, exact, header, hex, integer, lower_hex,
    require, sha256, string, to_hex,
};

/// A validated root policy and its canonical bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct RootPolicy {
    canonical: Vec<u8>,
    version: u64,
    keys: BTreeMap<String, [u8; 32]>,
    root: RoleRule,
    release: RoleRule,
}

#[derive(Clone, PartialEq, Eq)]
struct RoleRule {
    keyids: BTreeSet<String>,
    threshold: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Root,
    Release,
}

impl RootPolicy {
    /// Parses and validates a root policy, such as an embedded bootstrap blob.
    pub fn from_json(raw: &[u8]) -> Result<Self, Reject> {
        require(raw.len() <= MAX_METADATA, "metadata-bound")?;
        Self::from_value(&json::parse(raw)?)
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// The canonical serialization, suitable for durable storage and hashing.
    pub fn canonical_json(&self) -> &[u8] {
        &self.canonical
    }

    fn from_value(value: &Value) -> Result<Self, Reject> {
        let fields = exact(
            value,
            &[
                "schema",
                "product",
                "repository",
                "channel",
                "version",
                "keys",
                "roles",
            ],
        )?;
        header(fields, ROOT_SCHEMA)?;
        let version = integer(&fields["version"]).ok_or(Reject("root-version"))?;
        let Value::Object(entries) = &fields["keys"] else {
            return Err(Reject("root-key-count"));
        };
        require((2..=6).contains(&entries.len()), "root-key-count")?;
        let mut keys = BTreeMap::new();
        for (keyid, key) in entries {
            let public =
                public_key(key).filter(|_| *keyid == to_hex(&sha256(&json::canonical(key))));
            keys.insert(keyid.clone(), public.ok_or(Reject("root-key"))?);
        }
        let roles = exact(&fields["roles"], &["root", "release"])?;
        let root = role_rule(&roles["root"], &keys, &BTreeSet::new())?;
        require(root.threshold >= 2, "root-threshold")?;
        let release = role_rule(&roles["release"], &keys, &root.keyids)?;
        require(
            root.keyids.len() + release.keyids.len() == keys.len(),
            "unused-policy-key",
        )?;
        Ok(Self {
            canonical: json::canonical(value),
            version,
            keys,
            root,
            release,
        })
    }

    /// Requires `role`'s threshold of distinct valid signatures over the signed
    /// body. A release envelope may carry release-key signatures only.
    pub(super) fn signed_by(&self, envelope: &Envelope<'_>, role: Role) -> Result<(), Reject> {
        let (rule, code) = match role {
            Role::Root => (&self.root, "signature-root"),
            Role::Release => {
                require(
                    envelope
                        .signatures
                        .iter()
                        .all(|(keyid, _)| self.release.keyids.contains(keyid)),
                    "release-key-authority",
                )?;
                (&self.release, "signature-release")
            }
        };
        let valid = envelope
            .signatures
            .iter()
            .filter(|(keyid, signature)| {
                rule.keyids.contains(keyid)
                    && ring::signature::UnparsedPublicKey::new(
                        &ring::signature::ED25519,
                        &self.keys[keyid],
                    )
                    .verify(&envelope.message, signature)
                    .is_ok()
            })
            .count() as u64;
        require(valid >= rule.threshold, code)
    }
}

impl fmt::Debug for RootPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RootPolicy")
            .field("version", &self.version)
            .field("canonical", &String::from_utf8_lossy(&self.canonical))
            .finish()
    }
}

fn public_key(key: &Value) -> Option<[u8; 32]> {
    let fields = exact(key, &["keytype", "scheme", "keyval"]).ok()?;
    let keyval = exact(&fields["keyval"], &["public"]).ok()?;
    let ed25519 = Some("ed25519");
    if string(&fields["keytype"]) != ed25519 || string(&fields["scheme"]) != ed25519 {
        return None;
    }
    hex::<32>(string(&keyval["public"])?)
}

/// Validates one role, which must be disjoint from `taken`.
fn role_rule(
    value: &Value,
    keys: &BTreeMap<String, [u8; 32]>,
    taken: &BTreeSet<String>,
) -> Result<RoleRule, Reject> {
    const CODE: &str = "role-policy";
    let fields = exact(value, &["keyids", "threshold"])?;
    let Value::Array(items) = &fields["keyids"] else {
        return Err(Reject(CODE));
    };
    let mut keyids = BTreeSet::new();
    for item in items {
        let keyid = string(item).ok_or(Reject(CODE))?;
        require(
            keys.contains_key(keyid) && !taken.contains(keyid) && keyids.insert(keyid.to_owned()),
            CODE,
        )?;
    }
    let threshold = integer(&fields["threshold"]).ok_or(Reject(CODE))?;
    require(
        (1..=3).contains(&keyids.len()) && threshold <= keyids.len() as u64,
        CODE,
    )?;
    Ok(RoleRule { keyids, threshold })
}

/// A `{signed, signatures}` envelope with distinct, well-formed signatures.
pub(super) struct Envelope<'a> {
    pub(super) signed: &'a Value,
    /// Canonical bytes of `signed`, the message every signature covers.
    message: Vec<u8>,
    signatures: Vec<(String, [u8; 64])>,
}

impl<'a> Envelope<'a> {
    pub(super) fn parse(value: &'a Value) -> Result<Self, Reject> {
        let fields = exact(value, &["signed", "signatures"])?;
        let items = match &fields["signatures"] {
            Value::Array(items) if (1..=6).contains(&items.len()) => items,
            _ => return Err(Reject("signature-count")),
        };
        let mut signatures: Vec<(String, [u8; 64])> = Vec::with_capacity(items.len());
        for item in items {
            let signature = exact(item, &["keyid", "sig"])?;
            let keyid = string(&signature["keyid"]).filter(|keyid| lower_hex(keyid, 64));
            let sig = string(&signature["sig"]).and_then(hex::<64>);
            match (keyid, sig) {
                (Some(keyid), Some(sig)) if signatures.iter().all(|(seen, _)| seen != keyid) => {
                    signatures.push((keyid.to_owned(), sig));
                }
                _ => return Err(Reject("signature-shape")),
            }
        }
        let signed = &fields["signed"];
        Ok(Self {
            signed,
            message: json::canonical(signed),
            signatures,
        })
    }
}

/// Applies the bundle's root transitions to `trusted`, strictly N to N+1.
///
/// Each new root needs the old root's threshold and its own. Retained
/// records at or below the trusted version confer no trust, but an equal
/// version must be byte-identical.
pub(super) fn advance(trusted: &RootPolicy, transitions: &Value) -> Result<RootPolicy, Reject> {
    let transitions = match transitions {
        Value::Array(items) if items.len() <= MAX_TRANSITIONS => items,
        _ => return Err(Reject("rotation-bound")),
    };
    let mut current = trusted.clone();
    let mut previous = 0;
    for transition in transitions {
        let envelope = Envelope::parse(transition)?;
        let candidate = RootPolicy::from_value(envelope.signed)?;
        require(candidate.version > previous, "root-order")?;
        previous = candidate.version;
        if candidate.version <= current.version {
            require(
                candidate.version < current.version || candidate.canonical == current.canonical,
                "root-equivocation",
            )?;
            continue;
        }
        require(candidate.version == current.version + 1, "root-sequence")?;
        current.signed_by(&envelope, Role::Root)?;
        candidate.signed_by(&envelope, Role::Root)?;
        current = candidate;
    }
    Ok(current)
}
