//! Canonical per-release manifest `omen-release.json` (Lucy repair B1).
//!
//! One small immutable machine-readable asset on every Omen GitHub
//! Release, produced by the conveyor from the exact candidate build —
//! never hand-maintained. Unknown JSON fields are IGNORED (not denied):
//! a manifest that smuggles a `download_url` cannot redirect an update
//! because download URLs never come from manifest content — only from
//! GitHub release-asset metadata bound by [`super::transport`].

use crate::error::LifecycleError;
use crate::install::Channel;
use serde::{Deserialize, Serialize};

/// Schema version of `omen-release.json`. Bump only with an explicit
/// migration story; readers reject anything else.
pub const RELEASE_MANIFEST_SCHEMA_VERSION: u32 = 1;
/// Hard size bound for a fetched manifest (64 KiB). Anything larger is
/// not a manifest.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Canonical manifest asset name on every release.
pub const RELEASE_MANIFEST_ASSET: &str = "omen-release.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub version: String,
    pub git_sha: String,
    pub channel: Channel,
    /// Exact package asset FILE NAME on the same release (basename only).
    pub package_asset: String,
    pub package_sha256: String,
    pub binary_sha256: String,
    pub min_state_schema: u32,
    pub machine_contract: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_binary_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_size: Option<u64>,
}

impl ReleaseManifest {
    /// Strict parse + shape validation. Returns the manifest or a refusal
    /// naming the exact defect. Cross-checks against release/tag truth
    /// (tag_version, prerelease flag) happen in discovery, not here.
    pub fn parse_strict(bytes: &[u8]) -> Result<Self, LifecycleError> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(LifecycleError::Manifest(format!(
                "release manifest {} bytes exceeds {MAX_MANIFEST_BYTES} bound",
                bytes.len()
            )));
        }
        let m: ReleaseManifest = serde_json::from_slice(bytes)
            .map_err(|e| LifecycleError::Manifest(format!("release manifest invalid: {e}")))?;
        m.validate_shape()?;
        Ok(m)
    }

    fn validate_shape(&self) -> Result<(), LifecycleError> {
        let bad = |f: &str| LifecycleError::Manifest(format!("release manifest bad {f}"));
        if self.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION {
            return Err(bad(&format!(
                "schema_version {} (expected {RELEASE_MANIFEST_SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        if self.version.is_empty() || self.version.len() > 64 {
            return Err(bad("version"));
        }
        if !is_full_sha(&self.git_sha) {
            return Err(bad("git_sha (want 40 lowercase hex)"));
        }
        if !is_sha256(&self.package_sha256) {
            return Err(bad("package_sha256 (want 64 hex)"));
        }
        if !is_sha256(&self.binary_sha256) {
            return Err(bad("binary_sha256 (want 64 hex)"));
        }
        if let Some(d) = self.daemon_binary_sha256.as_ref()
            && !is_sha256(d)
        {
            return Err(bad("daemon_binary_sha256 (want 64 hex)"));
        }
        // Basename only: no directories, no traversal, no URLs.
        if self.package_asset.is_empty()
            || self.package_asset.len() > 128
            || self.package_asset.contains('/')
            || self.package_asset.contains('\\')
            || self.package_asset.contains("..")
            || self.package_asset.contains(':')
        {
            return Err(bad("package_asset (want plain file name)"));
        }
        if self.machine_contract.is_empty() || self.machine_contract.len() > 16 {
            return Err(bad("machine_contract"));
        }
        if let Some(size) = self.package_size
            && (size == 0 || size > super::transport::MAX_PACKAGE_BYTES)
        {
            return Err(bad("package_size"));
        }
        Ok(())
    }
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Hosts an update may talk to. Manifest content can NEVER add hosts:
/// download URLs come only from GitHub release-asset metadata, and every
/// hop of the transport must satisfy this predicate BEFORE contact.
///
/// Narrow by construction: the exact registries Omen uses
/// (`github.com`, `api.github.com`), the `*.githubusercontent.com`
/// family observed serving release assets (`objects`, `raw`), and the
/// authenticated-asset host `release-assets.githubusercontent.com`.
/// There is deliberately NO S3 form: no S3 hop has ever been observed in
/// Omen's release traffic, and breadth "just in case" is forbidden. If
/// real GitHub traffic ever needs another host, the transport refuses
/// loudly naming the hop — evidence first, allowlist second.
pub fn is_update_host(host: &str) -> bool {
    let h = host.to_lowercase();
    h == "github.com"
        || h == "api.github.com"
        || h == "release-assets.githubusercontent.com"
        || h == "objects.githubusercontent.com"
        || h == "raw.githubusercontent.com"
        || h.ends_with(".githubusercontent.com")
}

/// Parse + validate an update URL with the `url` crate (WHATWG parse —
/// no hand-split authority logic). Returns the parsed URL or a refusal
/// naming the defect. EVERY destination is validated BEFORE contact.
///
/// Refused: unparseable input, relative URLs, scheme-relative bypass
/// (`//evil/...` fails absolute parse), non-https schemes, missing or
/// disallowed hosts, any userinfo (`user@github.com` — the old
/// `split('@')` logic accepted this; the parser exposes `username()` so
/// it is now refused by construction), any explicit port, and
/// encoded-host confusion (the parser rejects `%` in special-scheme
/// hosts; IDNA is normalized before the allowlist check).
pub fn validate_update_url(raw: &str) -> Result<url::Url, LifecycleError> {
    let url = raw.trim();
    let bad = |why: &str| {
        LifecycleError::Manifest(format!("update URL refused ({why}): {}", shorten(url)))
    };
    let u = url::Url::parse(url).map_err(|_| bad("unparseable"))?;
    if u.scheme() != "https" {
        return Err(bad("scheme must be https"));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(bad("userinfo forbidden"));
    }
    if u.port().is_some() {
        return Err(bad("explicit port forbidden"));
    }
    match u.host_str() {
        Some(host) if is_update_host(host) => Ok(u),
        _ => Err(bad("host not allowed")),
    }
}

fn shorten(url: &str) -> String {
    if url.len() > 80 {
        format!("{}…", &url[..80])
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "version": "0.9.0-preview.16",
            "git_sha": "a".repeat(40),
            "channel": "preview",
            "package_asset": "omen-0.9.0-preview.16-windows-x86_64.zip",
            "package_sha256": "b".repeat(64),
            "binary_sha256": "c".repeat(64),
            "min_state_schema": 1,
            "machine_contract": "0.8"
        })
    }

    #[test]
    fn strict_accepts_good_manifest() {
        let m =
            ReleaseManifest::parse_strict(serde_json::to_vec(&good()).unwrap().as_slice()).unwrap();
        assert_eq!(m.version, "0.9.0-preview.16");
    }

    #[test]
    fn strict_rejects_shapes() {
        for (k, v) in [
            ("git_sha", serde_json::json!("short")),
            ("package_sha256", serde_json::json!("zz")),
            ("package_asset", serde_json::json!("../evil.zip")),
            (
                "package_asset",
                serde_json::json!("https://evil.example/x.zip"),
            ),
            ("schema_version", serde_json::json!(99)),
            ("machine_contract", serde_json::json!("")),
        ] {
            let mut g = good();
            g[k] = v;
            assert!(
                ReleaseManifest::parse_strict(serde_json::to_vec(&g).unwrap().as_slice()).is_err(),
                "{k}"
            );
        }
    }

    #[test]
    fn url_allowlist() {
        assert!(validate_update_url("https://github.com/a/b/releases/download/v1/x.zip").is_ok());
        assert!(validate_update_url("https://objects.githubusercontent.com/a/b?x=1").is_ok());
        assert!(
            validate_update_url("https://api.github.com/repos/o/r/releases?per_page=20").is_ok()
        );
        assert!(
            validate_update_url("https://release-assets.githubusercontent.com/1/x?token=y").is_ok()
        );
        assert!(validate_update_url("https://evil.example/x.zip").is_err());
        assert!(validate_update_url("http://github.com/x").is_err());
        // The old split('@') logic accepted this; the parser must not.
        assert!(validate_update_url("https://user@github.com/a/b.zip").is_err());
        assert!(validate_update_url("https://user:pass@github.com/a/b.zip").is_err());
        // WHATWG normalizes the DEFAULT port away (port() == None), so an
        // explicit :443 is canonically identical to no port — harmless, no
        // smuggling. A non-default explicit port is refused.
        assert!(validate_update_url("https://github.com:443/a/b.zip").is_ok());
        assert!(validate_update_url("https://github.com:8443/a/b.zip").is_err());
        assert!(validate_update_url("//github.com/a/b.zip").is_err());
        assert!(validate_update_url("/relative/path.zip").is_err());
        assert!(validate_update_url("https://%65vil.example/x.zip").is_err());
        assert!(validate_update_url("https://github-cloud.s3.amazonaws.com/x.zip").is_err());
        assert!(validate_update_url("https://notgithub.com/x.zip").is_err());
        assert!(validate_update_url("https://github.com.evil.example/x.zip").is_err());
        assert!(validate_update_url("not a url at all").is_err());
        assert!(validate_update_url("").is_err());
    }
}
