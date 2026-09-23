//! Canonical release transport (Lucy repair B1, hardened Preview 18).
//!
//! The ONLY production update source is GitHub Releases. A
//! [`ReleaseTransport`] abstracts the wire so tests run against a fake
//! without network, while production uses [`GithubTransport`]:
//! - API + manifest fetches: bounded timeouts, manifest byte ceiling.
//! - Redirects: NEVER followed automatically (`max_redirects(0)`).
//!   [`follow_manual`] resolves each `Location` with the `url` crate and
//!   [`crate::release::validate_update_url`]s the destination BEFORE any
//!   contact. A forbidden hop is never requested — not "requested then
//!   rejected". Max 5 follows, then refusal; loops refused via a visited
//!   set.
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
        // max_redirects(0): ureq never follows on its own; the 3xx
        // response is always returned so OUR loop validates each hop
        // before contact. There is no post-hoc audit because there is
        // nothing to audit after the fact — forbidden hosts are never
        // requested.
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
            .max_redirects(0)
            .build();
        ureq::Agent::new_with_config(config)
    }

    /// Single GET with NO redirect following. One request, one response.
    fn request_one(
        &self,
        agent: &ureq::Agent,
        url: &url::Url,
    ) -> Result<HopOutcome, LifecycleError> {
        let mut resp = agent
            .get(url.as_str())
            .header("User-Agent", "omen-update")
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| map_ureq("api", e))?;
        let status = resp.status().as_u16();
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = std::mem::replace(resp.body_mut(), empty_body());
        Ok(HopOutcome {
            status,
            location,
            body,
        })
    }

    /// GET with bounded MANUAL redirect handling. Returns the final
    /// validated URL, its status, and its body.
    fn get_manual(
        &self,
        agent: &ureq::Agent,
        url: &str,
    ) -> Result<(url::Url, u16, ureq::Body), LifecycleError> {
        let start = crate::release::validate_update_url(url)
            .map_err(|e| LifecycleError::Download(e.to_string()))?;
        let agent_ref = agent;
        let (final_url, hop) = follow_manual(&start, |next| self.request_one(agent_ref, next))?;
        Ok((final_url, hop.status, hop.body))
    }
}

/// One unfollowed HTTP response: status, optional redirect target, body.
pub struct HopOutcome {
    pub status: u16,
    pub location: Option<String>,
    pub body: ureq::Body,
}

impl std::fmt::Debug for HopOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HopOutcome")
            .field("status", &self.status)
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}

/// Statuses our loop treats as redirects (the agent-followed set; other
/// 3xx like 300/304/305 are returned as-is and refused downstream by the
/// callers' 2xx checks).
fn is_followable_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Bounded manual redirect loop. `fetch` performs EXACTLY ONE request and
/// returns its outcome; this loop validates every destination BEFORE
/// calling `fetch` for it:
///
/// validate(start) -> fetch -> [3xx? resolve Location against the
/// current URL, validate destination, fetch] -> final.
///
/// Guarantees: at most [`MAX_REDIRECTS`] follows (then refusal);
/// redirect loops refused via a visited set; a missing `Location` on a
/// redirect refused; the forbidden-host fetch is never invoked (the
/// hostile test asserts request count == 0, not contacted-then-rejected).
pub fn follow_manual<F>(
    start: &url::Url,
    mut fetch: F,
) -> Result<(url::Url, HopOutcome), LifecycleError>
where
    F: FnMut(&url::Url) -> Result<HopOutcome, LifecycleError>,
{
    let mut current = start.clone();
    let mut visited = vec![start.to_string()];
    for _ in 0..=MAX_REDIRECTS {
        let hop = fetch(&current)?;
        if !is_followable_redirect(hop.status) {
            return Ok((current, hop));
        }
        let raw_loc = hop.location.as_deref().ok_or_else(|| {
            LifecycleError::Download(format!(
                "redirect without Location from {}",
                shorten(current.as_str())
            ))
        })?;
        // Resolve relative Locations against the current URL (WHATWG
        // join — no string concatenation). Scheme-relative
        // `//evil/...` resolves, then FAILS validation below.
        let next = current.join(raw_loc).map_err(|_| {
            LifecycleError::Download(format!(
                "unresolvable redirect target from {}",
                shorten(current.as_str())
            ))
        })?;
        // Validate BEFORE contact. This is the security boundary.
        let next = crate::release::validate_update_url(next.as_str()).map_err(|_| {
            LifecycleError::Download(format!(
                "redirect leaves update hosts: {}",
                shorten(next.as_str())
            ))
        })?;
        if visited.contains(&next.to_string()) {
            return Err(LifecycleError::Download(
                "redirect loop detected".to_string(),
            ));
        }
        visited.push(next.to_string());
        current = next;
    }
    Err(LifecycleError::Download(format!(
        "too many redirects (>{MAX_REDIRECTS})"
    )))
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
        let (_, status, mut body) = self.get_manual(&agent, url)?;
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
        let (_, status, mut body) = self.get_manual(&agent, url)?;
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
        let (_, status, body) = self.get_manual(&agent, url)?;
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
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::rc::Rc;

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

    /// Deterministic redirect fixture: a scripted wire. Each URL maps to
    /// (status, location); EVERY invocation is recorded in the contact
    /// log. There is no network — the security property under test is
    /// which URLs the loop ASKS to contact.
    struct ScriptedWire {
        routes: HashMap<String, (u16, Option<String>)>,
        contacted: Rc<RefCell<Vec<String>>>,
    }

    impl ScriptedWire {
        fn fetch(&self, url: &url::Url) -> Result<HopOutcome, LifecycleError> {
            self.contacted.borrow_mut().push(url.to_string());
            match self.routes.get(url.as_str()) {
                Some((status, loc)) => Ok(HopOutcome {
                    status: *status,
                    location: loc.clone(),
                    body: empty_body(),
                }),
                None => Err(LifecycleError::Download("wire: no route".to_string())),
            }
        }

        fn contacts_to(&self, host: &str) -> usize {
            self.contacted
                .borrow()
                .iter()
                .filter(|u| {
                    url::Url::parse(u)
                        .ok()
                        .and_then(|p| p.host_str().map(str::to_string))
                        .as_deref()
                        == Some(host)
                })
                .count()
        }
    }

    fn wire(routes: &[(&str, u16, Option<&str>)]) -> ScriptedWire {
        ScriptedWire {
            routes: routes
                .iter()
                .map(|(u, s, l)| (u.to_string(), (*s, l.map(str::to_string))))
                .collect(),
            contacted: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn start(url: &str) -> url::Url {
        crate::release::validate_update_url(url).expect("fixture start must validate")
    }

    #[test]
    fn forbidden_redirect_is_never_contacted() {
        // approved -> redirect -> forbidden. The assertion that matters:
        // the forbidden host sees ZERO requests (refused BEFORE contact,
        // not contacted-then-rejected).
        let w = wire(&[
            (
                "https://github.com/o/r/releases/download/v18/pkg.zip",
                302,
                Some("https://evil.example/pwned.zip"),
            ),
            ("https://evil.example/pwned.zip", 200, None),
        ]);
        let err = follow_manual(
            &start("https://github.com/o/r/releases/download/v18/pkg.zip"),
            |u| w.fetch(u),
        )
        .unwrap_err();
        assert_eq!(err.phase(), "download");
        assert_eq!(
            w.contacts_to("evil.example"),
            0,
            "forbidden host contacted!"
        );
        assert_eq!(
            w.contacted.borrow().len(),
            1,
            "only the start URL is fetched"
        );
    }

    #[test]
    fn approved_chain_succeeds_relative_resolves() {
        let w = wire(&[
            (
                "https://github.com/o/r/releases/download/v18/pkg.zip",
                302,
                Some("https://objects.githubusercontent.com/a/b?x=1"),
            ),
            (
                "https://objects.githubusercontent.com/a/b?x=1",
                301,
                Some("/a/c?x=1"),
            ),
            ("https://objects.githubusercontent.com/a/c?x=1", 200, None),
        ]);
        let (final_url, hop) = follow_manual(
            &start("https://github.com/o/r/releases/download/v18/pkg.zip"),
            |u| w.fetch(u),
        )
        .unwrap();
        assert_eq!(hop.status, 200);
        assert_eq!(
            final_url.as_str(),
            "https://objects.githubusercontent.com/a/c?x=1"
        );
        assert_eq!(w.contacted.borrow().len(), 3);
    }

    #[test]
    fn too_many_redirects_refuses() {
        // 6 chained redirects exceeds MAX_REDIRECTS (5).
        let mut routes = vec![];
        for i in 0..7 {
            routes.push((
                format!("https://github.com/hop{i}"),
                302,
                Some(format!("https://github.com/hop{}", i + 1)),
            ));
        }
        let owned: Vec<(String, u16, Option<String>)> = routes.into_iter().collect();
        let refs: Vec<(&str, u16, Option<&str>)> = owned
            .iter()
            .map(|(a, b, c)| (a.as_str(), *b, c.as_deref()))
            .collect();
        let w = wire(&refs);
        let err = follow_manual(&start("https://github.com/hop0"), |u| w.fetch(u)).unwrap_err();
        assert!(err.to_string().contains("too many redirects"));
        assert_eq!(w.contacted.borrow().len(), (MAX_REDIRECTS + 1) as usize);
    }

    #[test]
    fn redirect_loop_is_bounded() {
        let w = wire(&[
            (
                "https://github.com/loop-a",
                302,
                Some("https://github.com/loop-b"),
            ),
            (
                "https://github.com/loop-b",
                302,
                Some("https://github.com/loop-a"),
            ),
        ]);
        let err = follow_manual(&start("https://github.com/loop-a"), |u| w.fetch(u)).unwrap_err();
        assert!(err.to_string().contains("loop"));
        assert!(w.contacted.borrow().len() <= (MAX_REDIRECTS + 1) as usize);
    }

    #[test]
    fn hostile_location_forms_refused_before_contact() {
        for bad_loc in [
            "http://github.com/downgrade.zip",
            "https://user@github.com/x.zip",
            "https://github.com:8443/x.zip",
            "//evil.example/x.zip",
            "https://evil.example/x.zip",
        ] {
            let w = wire(&[("https://github.com/o/r/start.zip", 302, Some(bad_loc))]);
            let err = follow_manual(&start("https://github.com/o/r/start.zip"), |u| w.fetch(u))
                .unwrap_err();
            assert_eq!(err.phase(), "download", "{bad_loc}");
            assert_eq!(w.contacted.borrow().len(), 1, "{bad_loc}");
        }
        // Redirect without Location is a refusal, not a hang.
        let w = wire(&[("https://github.com/o/r/noloc.zip", 302, None)]);
        assert!(follow_manual(&start("https://github.com/o/r/noloc.zip"), |u| w.fetch(u)).is_err());
    }
}
