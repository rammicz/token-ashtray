//! The fixed public channel URLs and the allowed redirect graph.
//!
//! Every URL the updater requests comes from here or is an observed CDN hop
//! validated here. There is no caller- or manifest-supplied URL.
use super::{Reject, Target, Version, require};

const RELEASES: &str = "https://github.com/rammicz/token-ashtray/releases/";
const MANIFEST_NAME: &str = "token-ashtray-updates.json";
/// The single discovery URL of the contract.
pub const DISCOVERY_URL: &str = concat!(
    "https://github.com/rammicz/token-ashtray/releases/",
    "latest/download/token-ashtray-updates.json"
);
/// Asset hops may go to this host only, over HTTPS on the default port.
const CDN_PREFIX: &str = "https://release-assets.githubusercontent.com/";
const MAX_URL: usize = 4096;

pub fn manifest_url(version: Version) -> String {
    format!("{RELEASES}download/v{version}/{MANIFEST_NAME}")
}

pub fn artifact_url(target: Target, version: Version) -> String {
    format!(
        "{RELEASES}download/v{version}/{}",
        target.artifact_name(version)
    )
}

/// Validates the first discovery hop: the exact stable-tag manifest URL.
pub fn tagged_manifest(location: &str) -> Result<Version, Reject> {
    const CODE: &str = "manifest-tag";
    let version = location
        .strip_prefix(RELEASES)
        .and_then(|rest| rest.strip_prefix("download/v"))
        .and_then(|rest| rest.strip_suffix(MANIFEST_NAME))
        .and_then(|rest| rest.strip_suffix('/'))
        .ok_or(Reject(CODE))
        .and_then(|text| Version::parse(text).map_err(|_| Reject(CODE)))?;
    require(location == manifest_url(version), CODE)?;
    Ok(version)
}

/// Validates an asset's single CDN hop: HTTPS, exact host, default port, no
/// credentials or fragment. The opaque path and query are used for that one
/// request only.
pub fn cdn_hop(location: &str) -> Result<(), Reject> {
    require(
        location.len() <= MAX_URL && location.bytes().all(|byte| (0x21..0x7f).contains(&byte)),
        "url-bound",
    )?;
    require(
        location.starts_with(CDN_PREFIX) && !location.contains('#'),
        "asset-cdn",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_match_the_contract_grammar() {
        let v = |text| Version::parse(text).unwrap();
        let base = "https://github.com/rammicz/token-ashtray/releases/";
        assert_eq!(
            DISCOVERY_URL,
            format!("{base}latest/download/token-ashtray-updates.json")
        );
        let tagged = format!("{base}download/v0.2.2/token-ashtray-updates.json");
        assert_eq!(manifest_url(v("0.2.2")), tagged);
        assert_eq!(
            artifact_url(Target::LinuxX86_64, v("0.2.2")),
            format!("{base}download/v0.2.2/token-ashtray-setup-0.2.2-x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            artifact_url(Target::WindowsX86_64, v("0.2.2")),
            format!("{base}download/v0.2.2/token-ashtray-setup-0.2.2-x86_64-pc-windows-msvc.exe")
        );
        assert_eq!(tagged_manifest(&tagged), Ok(v("0.2.2")));
        for wrong in [
            format!("{tagged}?x=y"),
            format!("{tagged}#f"),
            tagged.replace("/v0.2.2/", "/latest/"),
            tagged.replace("/v0.2.2/", "/0.2.2/"),
            tagged.replace("v0.2.2", "v0.02.2"),
            tagged.replace("v0.2.2", "v0.2.2-rc1"),
            tagged.replace("/rammicz/", "/foreign/"),
            tagged.replace("https:", "http:"),
            tagged.replace("github.com", "github.com:443"),
            tagged.replace("updates.json", "updates.json/"),
            DISCOVERY_URL.to_owned(),
        ] {
            assert_eq!(
                tagged_manifest(&wrong),
                Err(Reject("manifest-tag")),
                "{wrong}"
            );
        }
        let cdn = "https://release-assets.githubusercontent.com/opaque/1?sig=TRANSPORT_ONLY";
        assert_eq!(cdn_hop(cdn), Ok(()));
        for (wrong, expected) in [
            (cdn.replace("https:", "http:"), "asset-cdn"),
            (cdn.replace(".com/", ".com:443/"), "asset-cdn"),
            (cdn.replace(".com/", ".com.evil.test/"), "asset-cdn"),
            (cdn.replace("https://", "https://user@"), "asset-cdn"),
            (format!("{cdn}#fragment"), "asset-cdn"),
            ("https://evil.test/a".to_owned(), "asset-cdn"),
            (format!("{cdn}\n"), "url-bound"),
            (format!("{cdn} "), "url-bound"),
            (format!("{cdn}{}", "a".repeat(4096)), "url-bound"),
        ] {
            assert_eq!(cdn_hop(&wrong), Err(Reject(expected)), "{wrong}");
        }
    }
}
