//! Canonical release transport (Lucy repair B1).
//!
//! The ONLY production update source is GitHub Releases. A
//! [`ReleaseTransport`] abstracts the wire so tests run against a fake
//! without network, while production uses [`GithubTransport`]:
//! - API + manifest fetches: bounded timeouts, manifest byte ceiling.
//! - Redirects: followed (max 5), then EVERY hop is audited against the
//!   [`crate::release::is_update_host`] allowlist. One bad hop refuses.
//! - Package download: streamed to `.part` with a byte ceiling and a
//!   running SHA-256; the part is renamed only after complete transport
//!   AND digest match; any failure removes the part.
//!
//! No automatic retry: transport failure stays transport failure.

use crate::error::LifecycleError;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

/// Hard ceiling for a downloaded package (256 MiB). Refused before OOM.
pub const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
/// Max redirect hops followed, then audited.
pub const MAX_REDIRECTS: u32 = 5;
/// Metadata (API list, manifest) global timeout.
pub const METADATA_TIMEOUT_SECS: u64 = 20;
/// Package download global timeout (connect + streaming).
pub const DOWNLOAD_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct ApiAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct ApiRelease {
    pub tag: String,
    pub prerelease: bool,
    pub assets: Vec<ApiAsset>,
}

pub trait ReleaseTransport {
    /// Newest-first bounded release list (limit enforced by impl).
    fn list_releases(&self, limit: usize) -> Result<Vec<ApiRelease>, LifecycleError>;
    /// Fetch a small metadata URL (manifest). Impl enforces the byte bound.
    fn fetch_manifest_bytes(&self, url: &str) -> Result<Vec<u8>, LifecycleError>;
    /// Stream a package URL into `part_path` with ceiling + running hash.
    /// Returns the hex digest of the COMPLETE stream. Removes the part on
    /// any failure.
    fn download_package(
        &self,
        url: &str,
        part_path: &Path,
        ceiling: u64,
    ) -> Result<String, LifecycleError>;
}

/// Production transport over HTTPS GitHub endpoints.
pub struct GithubTransport;

impl GithubTransport {
    fn agent(&self, timeout_secs: u64) -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
            .max_redirects(MAX_REDIRECTS)
            .max_redirects_will_error(true)
            .save_redirect_history(true)
            .build();
        ureq::Agent::new_with_config(config)
    }

    /// GET with redirect-chain audit. Returns (final_url, status, body).
    /// Every hop including the final URL must satisfy the host allowlist.
    fn get_audited(
        &self,
        agent: &ureq::Agent,
        url: &str,
    ) -> Result<(String, u16, ureq::Body), LifecycleError> {
        use ureq::ResponseExt;
        let start = crate::release::validate_update_url(url)
            .map_err(|e| LifecycleError::Download(e.to_string()))?;
        let mut resp = agent
            .get(&start)
            .header("User-Agent", "omen-update")
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| map_ureq("api", e))?;
        let status = resp.status().as_u16();
        // Audit the full redirect chain (request + hops + final).
        if let Some(history) = resp.get_redirect_history() {
            for uri in history {
                let hop = uri.to_string();
                if crate::release::validate_update_url(&hop).is_err() {
                    return Err(LifecycleError::Download(format!(
                        "redirect leaves update hosts: {}",
                        shorten(&hop)
                    )));
                }
            }
            let final_url = resp.get_uri().to_string();
            let body = std::mem::replace(resp.body_mut(), empty_body());
            Ok((final_url, status, body))
        } else {
            let body = std::mem::replace(resp.body_mut(), empty_body());
            Ok((start, status, body))
        }
    }
}

fn empty_body() -> ureq::Body {
    ureq::Body::builder().data(Vec::new())
}

fn shorten(url: &str) -> String {
    if url.len() > 80 {
        format!("{}…", &url[..80])
    } else {
        url.to_string()
    }
}

fn map_ureq(phase: &str, e: ureq::Error) -> LifecycleError {
    match e {
        ureq::Error::StatusCode(401)
        | ureq::Error::StatusCode(403)
        | ureq::Error::StatusCode(429) => LifecycleError::Download(format!(
            "{phase}: registry rate-limited/forbidden (no retry in H)"
        )),
        ureq::Error::Timeout(_) => {
            LifecycleError::Download(format!("{phase}: transport timed out"))
        }
        other => LifecycleError::Download(format!("{phase}: transport failed: {other}")),
    }
}

impl ReleaseTransport for GithubTransport {
    fn list_releases(&self, limit: usize) -> Result<Vec<ApiRelease>, LifecycleError> {
        let agent = self.agent(METADATA_TIMEOUT_SECS);
        let url =
            "https://api.github.com/repos/matthewjameswatkins1978-cyber/Omen/releases?per_page=20";
        let (_, status, mut body) = self.get_audited(&agent, url)?;
        if status == 403 || status == 429 {
            return Err(LifecycleError::Download(
                "api: registry rate-limited/forbidden (no retry in H)".to_string(),
            ));
        }
        if !(200..300).contains(&status) {
            return Err(LifecycleError::Download(format!("api: status {status}")));
        }
        let v: serde_json::Value = body
            .read_json()
            .map_err(|e| LifecycleError::Download(format!("api: invalid release metadata: {e}")))?;
        let arr = v.as_array().ok_or_else(|| {
            LifecycleError::Download("api: release list was not a list".to_string())
        })?;
        let mut out = Vec::new();
        for rel in arr.iter().take(limit.max(1)) {
            let tag = rel
                .get("tag_name")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if tag.is_empty() {
                continue;
            }
            let prerelease = rel
                .get("prerelease")
                .and_then(|p| p.as_bool())
                .unwrap_or(false);
            let mut assets = Vec::new();
            if let Some(arr) = rel.get("assets").and_then(|a| a.as_array()) {
                for a in arr {
                    assets.push(ApiAsset {
                        name: a
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string(),
                        browser_download_url: a
                            .get("browser_download_url")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string(),
                        size: a.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                    });
                }
            }
            out.push(ApiRelease {
                tag,
                prerelease,
                assets,
            });
        }
        Ok(out)
    }

    fn fetch_manifest_bytes(&self, url: &str) -> Result<Vec<u8>, LifecycleError> {
        let agent = self.agent(METADATA_TIMEOUT_SECS);
        let (_, status, mut body) = self.get_audited(&agent, url)?;
        if !(200..300).contains(&status) {
            return Err(LifecycleError::Download(format!(
                "manifest: status {status}"
            )));
        }
        let bytes = body
            .with_config()
            .limit(crate::release::MAX_MANIFEST_BYTES + 1)
            .read_to_vec()
            .map_err(|e| LifecycleError::Download(format!("manifest: transport failed: {e}")))?;
        if bytes.len() as u64 > crate::release::MAX_MANIFEST_BYTES {
            return Err(LifecycleError::Download(
                "manifest: exceeds size bound".to_string(),
            ));
        }
        Ok(bytes)
    }

    fn download_package(
        &self,
        url: &str,
        part_path: &Path,
        ceiling: u64,
    ) -> Result<String, LifecycleError> {
        let agent = self.agent(DOWNLOAD_TIMEOUT_SECS);
        let (_, status, body) = self.get_audited(&agent, url)?;
        if !(200..300).contains(&status) {
            return Err(LifecycleError::Download(format!(
                "package: status {status}"
            )));
        }
        stream_to_part(body, part_path, ceiling)
    }
}

/// Stream a response body into `.part` with ceiling + running hash.
/// Shared by the real transport and (via adapters) the fake transport:
/// transport bounds live HERE, not in callers.
pub fn stream_to_part(
    body: ureq::Body,
    part_path: &Path,
    ceiling: u64,
) -> Result<String, LifecycleError> {
    if let Some(parent) = part_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let mut reader = body.into_reader();
    stream_reader_to_part(&mut reader, part_path, ceiling)
}

/// Transport-agnostic core: any `Read` (socket, fake, truncated fixture)
/// streams under the same bounds. TEST seam for partial/oversize/flood.
pub fn stream_reader_to_part(
    reader: &mut dyn Read,
    part_path: &Path,
    ceiling: u64,
) -> Result<String, LifecycleError> {
    if let Some(parent) = part_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let file = std::fs::File::create(part_path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let mut out = std::io::BufWriter::new(file);
    let mut hash = Sha256::new();
    let mut total: u64 = 0;
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            std::fs::remove_file(part_path).ok();
            LifecycleError::Download(format!("package: transport broke mid-stream: {e}"))
        })?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > ceiling {
            std::fs::remove_file(part_path).ok();
            drop(out);
            return Err(LifecycleError::Download(format!(
                "package: exceeds {ceiling} byte ceiling"
            )));
        }
        hash.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| {
            std::fs::remove_file(part_path).ok();
            LifecycleError::Io(e.to_string())
        })?;
    }
    out.flush().map_err(|e| LifecycleError::Io(e.to_string()))?;
    drop(out);
    Ok(hex::encode(hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn stream_hashes_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("x.pkg.part");
        let mut data = Cursor::new(b"hello-package".to_vec());
        let digest = stream_reader_to_part(&mut data, &part, 1024).unwrap();
        assert!(part.is_file());
        assert_eq!(digest.len(), 64);

        struct FailAfter<R> {
            inner: R,
            left: usize,
        }
        impl<R: Read> Read for FailAfter<R> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.left == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "cut",
                    ));
                }
                let n = self.inner.read(buf)?.min(self.left);
                self.left -= n;
                Ok(n)
            }
        }
        let part2 = dir.path().join("y.pkg.part");
        let mut cut = FailAfter {
            inner: Cursor::new(vec![0u8; 100]),
            left: 10,
        };
        let err = stream_reader_to_part(&mut cut, &part2, 1024).unwrap_err();
        assert_eq!(err.phase(), "download");
        assert!(!part2.exists(), "part must be discarded");

        let part3 = dir.path().join("z.pkg.part");
        let mut big = Cursor::new(vec![0u8; 100]);
        let err = stream_reader_to_part(&mut big, &part3, 16).unwrap_err();
        assert_eq!(err.phase(), "download");
        assert!(!part3.exists());
    }
}
