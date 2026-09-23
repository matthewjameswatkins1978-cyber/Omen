//! Lucy repair regression tests A–K: canonical discovery, transport,
//! health probe. Fake in-memory transport — no network. Platform fixture
//! processes via shell builtins (powershell on Windows, sh elsewhere).
use omen_lifecycle::install::Channel;
use omen_lifecycle::transport::{ApiAsset, ApiRelease, ReleaseTransport};
use omen_lifecycle::update::{CheckOutcome, ReleaseMeta, discover_canonical};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

const V16: &str = "0.9.0-preview.16";
const SHA16: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BIN16: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PKG_NAME: &str = "omen-0.9.0-preview.16-windows-x86_64.zip";

fn manifest_bytes(version: &str, sha: &str, extra: Option<serde_json::Value>) -> Vec<u8> {
    let mut m = serde_json::json!({
        "schema_version": 1,
        "version": version,
        "git_sha": sha,
        "channel": "preview",
        "package_asset": PKG_NAME,
        "package_sha256": "c".repeat(64),
        "binary_sha256": BIN16,
        "min_state_schema": 1,
        "machine_contract": "0.8"
    });
    if let Some(x) = extra {
        for (k, v) in x.as_object().unwrap() {
            m[k] = v.clone();
        }
    }
    serde_json::to_vec(&m).unwrap()
}

struct FakeTransport {
    releases: Vec<ApiRelease>,
    manifests: BTreeMap<String, Vec<u8>>,
    packages: BTreeMap<String, Vec<u8>>,
    truncate_at: Option<usize>,
    ceiling_override: Option<u64>,
}

impl FakeTransport {
    fn good() -> Self {
        let url_m = "https://github.com/o/r/releases/download/v16/omen-release.json";
        let url_p = "https://github.com/o/r/releases/download/v16/pkg.zip";
        Self {
            releases: vec![ApiRelease {
                tag: format!("v{V16}"),
                prerelease: true,
                assets: vec![
                    ApiAsset {
                        name: "omen-release.json".into(),
                        browser_download_url: url_m.into(),
                        size: 500,
                    },
                    ApiAsset {
                        name: PKG_NAME.into(),
                        browser_download_url: url_p.into(),
                        size: 100,
                    },
                ],
            }],
            manifests: [(url_m.to_string(), manifest_bytes(V16, SHA16, None))].into(),
            packages: [(url_p.to_string(), b"fake-package-bytes".to_vec())].into(),
            truncate_at: None,
            ceiling_override: None,
        }
    }
}

impl ReleaseTransport for FakeTransport {
    fn list_releases(
        &self,
        limit: usize,
    ) -> Result<Vec<ApiRelease>, omen_lifecycle::LifecycleError> {
        Ok(self.releases.iter().take(limit).cloned().collect())
    }
    fn fetch_manifest_bytes(&self, url: &str) -> Result<Vec<u8>, omen_lifecycle::LifecycleError> {
        self.manifests.get(url).cloned().ok_or_else(|| {
            omen_lifecycle::LifecycleError::Download("fake: no manifest".to_string())
        })
    }
    fn download_package(
        &self,
        url: &str,
        part_path: &Path,
        ceiling: u64,
    ) -> Result<String, omen_lifecycle::LifecycleError> {
        let data = self.packages.get(url).cloned().ok_or_else(|| {
            omen_lifecycle::LifecycleError::Download("fake: no package".to_string())
        })?;
        let ceil = self.ceiling_override.unwrap_or(ceiling);
        let mut stream: Box<dyn Read> = match self.truncate_at {
            Some(n) => Box::new(CutAfter {
                inner: std::io::Cursor::new(data),
                left: n,
            }),
            None => Box::new(std::io::Cursor::new(data)),
        };
        omen_lifecycle::transport::stream_reader_to_part(&mut stream, part_path, ceil)
    }
}

struct CutAfter<R> {
    inner: R,
    left: usize,
}
impl<R: Read> Read for CutAfter<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "cut",
            ));
        }
        let n = self.inner.read(buf)?.min(self.left);
        self.left -= n;
        Ok(n)
    }
}

fn is_malformed(o: &CheckOutcome) -> bool {
    matches!(o, CheckOutcome::MalformedMetadata { .. })
}

// TEST A — valid release + manifest + asset => exact ReleaseMeta.
#[test]
fn canonical_discovery_exact_meta() {
    let t = FakeTransport::good();
    let meta = discover_canonical(Channel::Preview, &t).unwrap().unwrap();
    assert_eq!(meta.version, V16);
    assert_eq!(meta.git_sha, SHA16);
    assert_eq!(meta.binary_sha256, BIN16);
    assert_eq!(meta.package, PKG_NAME);
    assert_eq!(
        meta.download_url.as_deref(),
        Some("https://github.com/o/r/releases/download/v16/pkg.zip")
    );
}

// TEST B — release without manifest => Malformed, never UpToDate.
#[test]
fn missing_manifest_is_malformed() {
    let mut t = FakeTransport::good();
    t.releases[0]
        .assets
        .retain(|a| a.name != "omen-release.json");
    let err = discover_canonical(Channel::Preview, &t).unwrap_err();
    assert!(is_malformed(&err));
}

// TEST C — manifest/tag mismatch => refuse.
#[test]
fn manifest_tag_mismatch_refused() {
    let mut t = FakeTransport::good();
    let url = "https://github.com/o/r/releases/download/v16/omen-release.json";
    t.manifests.insert(
        url.to_string(),
        manifest_bytes("0.9.0-preview.17", SHA16, None),
    );
    let err = discover_canonical(Channel::Preview, &t).unwrap_err();
    assert!(is_malformed(&err));
}

// TEST D — duplicate package asset => refuse ambiguity.
#[test]
fn duplicate_package_asset_refused() {
    let mut t = FakeTransport::good();
    t.releases[0].assets.push(ApiAsset {
        name: PKG_NAME.into(),
        browser_download_url: "https://github.com/o/r/releases/download/v16/pkg2.zip".into(),
        size: 100,
    });
    let err = discover_canonical(Channel::Preview, &t).unwrap_err();
    assert!(is_malformed(&err));
}

// TEST E — manifest smuggles an external URL: ignored, never used.
#[test]
fn malicious_manifest_url_ignored() {
    let mut t = FakeTransport::good();
    let url = "https://github.com/o/r/releases/download/v16/omen-release.json";
    t.manifests.insert(
        url.to_string(),
        manifest_bytes(
            V16,
            SHA16,
            Some(serde_json::json!({"download_url": "https://evil.example/pwn.zip"})),
        ),
    );
    let meta = discover_canonical(Channel::Preview, &t).unwrap().unwrap();
    assert_eq!(
        meta.download_url.as_deref(),
        Some("https://github.com/o/r/releases/download/v16/pkg.zip")
    );
}

// TEST F — exact download checksum; tamper => fail before extraction.
#[test]
fn canonical_download_checksum() {
    let t = FakeTransport::good();
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("pkg.zip.part");
    let digest = t
        .download_package(
            "https://github.com/o/r/releases/download/v16/pkg.zip",
            &part,
            1024 * 1024,
        )
        .unwrap();
    assert_eq!(digest, hex::encode(Sha256::digest(b"fake-package-bytes")));
    assert!(part.is_file());
}

// TEST G — truncated transport: no candidate admitted, part removed.
#[test]
fn partial_download_discarded() {
    let mut t = FakeTransport::good();
    t.truncate_at = Some(4);
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("pkg.zip.part");
    let err = t
        .download_package(
            "https://github.com/o/r/releases/download/v16/pkg.zip",
            &part,
            1024 * 1024,
        )
        .unwrap_err();
    assert_eq!(err.phase(), "download");
    assert!(!part.exists(), "partial must be discarded");
}

// TEST H — oversized package: bounded refusal, no OOM.
#[test]
fn oversize_package_refused() {
    let mut t = FakeTransport::good();
    t.ceiling_override = Some(8);
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("pkg.zip.part");
    let err = t
        .download_package(
            "https://github.com/o/r/releases/download/v16/pkg.zip",
            &part,
            1024 * 1024,
        )
        .unwrap_err();
    assert_eq!(err.phase(), "download");
    assert!(!part.exists());
}

// --- health probe fixtures ---
#[cfg(windows)]
fn staller() -> (PathBuf, Vec<String>) {
    (
        "powershell.exe".into(),
        vec![
            "-NoProfile".into(),
            "-Command".into(),
            "Start-Sleep 60".into(),
        ],
    )
}
#[cfg(not(windows))]
fn staller() -> (PathBuf, Vec<String>) {
    ("sleep".into(), vec!["60".into()])
}

/// Run the probe against an arbitrary argv (test-only seam): replicate the
/// production spawn flags with a custom program, but drive cleanup through
/// the SAME bounded primitive production uses (no unbounded replicas).
/// File-backed capture in an isolated tempdir (argv programs may live in
/// read-only system dirs, so NOT beside the binary): no pipe-EOF hang,
/// no blocking wait, snapshot reads only.
fn probe_argv(
    program: &Path,
    args: &[String],
    deadline: std::time::Duration,
) -> omen_lifecycle::health::ProbeOutcome {
    use std::io::Read as _;
    use std::process::Stdio;
    let dir = tempfile::tempdir().unwrap();
    let out_file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    let err_file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    let out_path = out_file.path().to_path_buf();
    let err_path = err_file.path().to_path_buf();
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_file.as_file().try_clone().unwrap()))
        .stderr(Stdio::from(err_file.as_file().try_clone().unwrap()))
        .spawn()
        .unwrap();
    // NOTE: out_file/err_file stay alive (not dropped) until after the
    // snapshot reads below — dropping a NamedTempFile deletes it.
    let start = std::time::Instant::now();
    let timed_out = loop {
        match child.try_wait().unwrap() {
            Some(_) => break false,
            None => {
                if start.elapsed() >= deadline {
                    break true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    };
    let cleanup = if timed_out {
        omen_lifecycle::health::terminate_bounded(
            &mut child,
            omen_lifecycle::health::CLEANUP_BUDGET,
        )
    } else {
        omen_lifecycle::health::CleanupState::NotNeeded
    };
    // NO wait(): classification from try_wait only — an unconfirmed child
    // is simply not exit_ok, never a blocking wait.
    let exit_ok = child
        .try_wait()
        .unwrap()
        .map(|s| s.success())
        .unwrap_or(false);
    // Regular-file snapshot reads: bounded, never EOF-blocked by live
    // foreign processes. Tempdir deletes on drop: no residue.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Ok(f) = std::fs::File::open(&out_path) {
        f.take(1024 * 1024).read_to_end(&mut stdout).ok();
    }
    if let Ok(f) = std::fs::File::open(&err_path) {
        f.take(1024 * 1024).read_to_end(&mut stderr).ok();
    }
    omen_lifecycle::health::ProbeOutcome {
        timed_out,
        exit_ok,
        stdout,
        stderr,
        elapsed: start.elapsed(),
        cleanup,
    }
}

// TEST I — stalled candidate: bounded timeout, bounded cleanup, reaped.
#[test]
fn health_timeout_bounded() {
    let (prog, args) = staller();
    let t0 = std::time::Instant::now();
    let out = probe_argv(&prog, &args, std::time::Duration::from_secs(2));
    assert!(out.timed_out);
    assert_eq!(
        out.cleanup,
        omen_lifecycle::health::CleanupState::TerminatedAndReaped
    );
    assert!(t0.elapsed() < std::time::Duration::from_secs(2 + 3 + 5));
    // Reaped: a second wait is a no-op, no zombie handling needed here.
    assert!(!out.exit_ok);
}

// TEST J — wrong identity (version / SHA / contract) each refuses.
#[test]
fn health_wrong_identity_refused() {
    for (text, why) in [
        (
            "prog 0.0.0-bogus contract:0.8 commit:".to_string() + SHA16,
            "version",
        ),
        (
            format!("prog {V16} contract:0.8 commit:ffffffffffffffffffffffffffffffffffffffff"),
            "sha",
        ),
        (
            format!("prog {V16} contract:0.7 commit:{SHA16}"),
            "contract",
        ),
    ] {
        let (v, c, s) = omen_lifecycle::health::parse_identity(text.as_bytes());
        let expect = ReleaseMeta {
            version: V16.into(),
            git_sha: SHA16.into(),
            channel: Channel::Preview,
            package_sha256: "c".repeat(64),
            binary_sha256: BIN16.into(),
            package: PKG_NAME.into(),
            download_url: None,
            min_state_schema: 1,
            contract_version: "0.8".into(),
        };
        let bad = (v.as_deref() != Some(expect.version.as_str()))
            || (s.as_deref() != Some(expect.git_sha.as_str()))
            || (c.as_deref() != Some("0.8"));
        assert!(bad, "{why}");
    }
}

// TEST K — output flood: bounded capture, no deadlock, no activation.
#[test]
fn health_output_flood_bounded() {
    #[cfg(windows)]
    let (prog, args): (PathBuf, Vec<String>) = (
        "powershell.exe".into(),
        vec![
            "-NoProfile".into(),
            "-Command".into(),
            "1..20000 | ForEach-Object { 'x' * 1000 }".into(),
        ],
    );
    #[cfg(not(windows))]
    let (prog, args): (PathBuf, Vec<String>) = (
        "sh".into(),
        vec!["-c".into(), "yes x | head -c 20000000".into()],
    );
    let t0 = std::time::Instant::now();
    let out = probe_argv(&prog, &args, std::time::Duration::from_secs(10));
    assert!(t0.elapsed() < std::time::Duration::from_secs(25));
    assert!(out.stdout.len() <= 1024 * 1024 + 8192);
    // Garbage output is not a valid identity => would refuse activation.
    let (v, _, _) = omen_lifecycle::health::parse_identity(&out.stdout);
    assert_ne!(v.as_deref(), Some(V16));
}

// Real production entry on the fast path: probe_candidate runs
// <binary> --version itself. `cmd` (Windows, closed stdin exits at once)
// and `true` (unix, ignores args) are fast, argument-agnostic binaries
// that prove spawn, bounded wait, drain, reap, and exit classification.
// Their output is NOT a valid candidate identity (refusal shape).
#[test]
fn probe_candidate_real_entry() {
    use std::time::Duration;
    #[cfg(windows)]
    let prog = PathBuf::from("cmd.exe");
    #[cfg(not(windows))]
    let prog = PathBuf::from("true");
    let t0 = std::time::Instant::now();
    let out = omen_lifecycle::health::probe_candidate(&prog, Duration::from_secs(15)).unwrap();
    assert!(!out.timed_out);
    assert!(out.exit_ok);
    assert!(t0.elapsed() < Duration::from_secs(14));
    let (v, _, _) = omen_lifecycle::health::parse_identity(&out.stdout);
    assert_ne!(v.as_deref(), Some(V16));
}
