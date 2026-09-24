//! Companion fetch proofs: binding, digests, zip shape, install.
//!
//! A fake `ReleaseTransport` serves a crafted release; the companion
//! zip is built in-test with real hashes. Every refusal leaves no
//! installed companion behind.

use omen_authority::{fetch_companion, managed::write_provenance};
use omen_lifecycle::transport::{ApiAsset, ApiRelease, ReleaseTransport};
use sha2::{Digest, Sha256};
use std::io::Write;

struct FakeTransport {
    tag: String,
    manifest_json: String,
    zip_bytes: Vec<u8>,
    zip_digest_override: Option<String>,
    fail_download: bool,
}

impl FakeTransport {
    fn zip_digest(&self) -> String {
        if let Some(d) = &self.zip_digest_override {
            return d.clone();
        }
        format!("{:x}", Sha256::digest(&self.zip_bytes))
    }
}

impl ReleaseTransport for FakeTransport {
    fn list_releases(
        &self,
        _limit: usize,
    ) -> Result<Vec<ApiRelease>, omen_lifecycle::error::LifecycleError> {
        Ok(vec![ApiRelease {
            tag: self.tag.clone(),
            prerelease: true,
            assets: vec![
                ApiAsset {
                    name: "omen-release.json".to_string(),
                    browser_download_url:
                        "https://github.com/o/r/releases/download/t/omen-release.json".to_string(),
                    size: self.manifest_json.len() as u64,
                },
                ApiAsset {
                    name: "companion.zip".to_string(),
                    browser_download_url:
                        "https://github.com/o/r/releases/download/t/companion.zip".to_string(),
                    size: self.zip_bytes.len() as u64,
                },
            ],
        }])
    }

    fn fetch_manifest_bytes(
        &self,
        _url: &str,
    ) -> Result<Vec<u8>, omen_lifecycle::error::LifecycleError> {
        Ok(self.manifest_json.bytes().collect())
    }

    fn download_package(
        &self,
        _url: &str,
        part_path: &std::path::Path,
        _ceiling: u64,
    ) -> Result<String, omen_lifecycle::error::LifecycleError> {
        if self.fail_download {
            return Err(omen_lifecycle::error::LifecycleError::Download(
                "fake down".to_string(),
            ));
        }
        std::fs::write(part_path, &self.zip_bytes).unwrap();
        Ok(self.zip_digest())
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct Fixture {
    dir: tempfile::TempDir,
    gate_bytes: Vec<u8>,
    engine_bytes: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let gate_bytes = b"fake-gate-binary".to_vec();
        let engine_bytes = b"fake-engine-binary".to_vec();
        let comp = dir.path().join("comp");
        std::fs::create_dir_all(&comp).unwrap();
        std::fs::write(comp.join("tethers-gate.exe"), &gate_bytes).unwrap();
        std::fs::write(comp.join("tethers-engine.exe"), &engine_bytes).unwrap();
        write_provenance(
            &comp,
            &"7".repeat(40),
            "tethers-gate.exe",
            "tethers-engine.exe",
        )
        .unwrap();
        Self {
            dir,
            gate_bytes,
            engine_bytes,
        }
    }

    fn comp_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("comp")
    }

    fn zip_bytes(&self, extra: &[(&str, &[u8])], skip: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            let mut add = |name: &str, data: &[u8]| {
                zip.start_file(name, opts).unwrap();
                zip.write_all(data).unwrap();
            };
            if !skip.contains(&"tethers-gate.exe") {
                add("tethers-gate.exe", &self.gate_bytes);
            }
            if !skip.contains(&"tethers-engine.exe") {
                add("tethers-engine.exe", &self.engine_bytes);
            }
            if !skip.contains(&"provenance.json") {
                add(
                    "provenance.json",
                    &std::fs::read(self.comp_dir().join("provenance.json")).unwrap(),
                );
            }
            for (name, data) in extra {
                add(name, data);
            }
            zip.finish().unwrap();
        }
        buf
    }

    fn manifest(&self, zip_sha: &str) -> String {
        serde_json::json!({
            "schema_version": 1,
            "version": "0.9.0-preview.22",
            "git_sha": "a".repeat(40),
            "channel": "preview",
            "package_asset": "omen.zip",
            "package_sha256": "b".repeat(64),
            "binary_sha256": "c".repeat(64),
            "min_state_schema": 1,
            "machine_contract": "0.8",
            "gate_companion_asset": "companion.zip",
            "gate_companion_sha256": zip_sha,
            "gate_tethers_sha": "7".repeat(40),
            "gate_exe_sha256": sha(&self.gate_bytes),
            "gate_engine_sha256": sha(&self.engine_bytes),
        })
        .to_string()
    }
}

fn install_dir(fx: &Fixture) -> std::path::PathBuf {
    fx.dir.path().join("install")
}

fn work_dir(fx: &Fixture) -> std::path::PathBuf {
    fx.dir.path().join("work")
}

#[test]
fn fetch_installs_verified_companion() {
    let fx = Fixture::new();
    let zip = fx.zip_bytes(&[], &[]);
    let t = FakeTransport {
        tag: "v0.9.0-preview.22".to_string(),
        manifest_json: fx.manifest(&sha(&zip)),
        zip_bytes: zip,
        zip_digest_override: None,
        fail_download: false,
    };
    let fetched = fetch_companion(&t, "v0.9.0-preview.22", &install_dir(&fx), &work_dir(&fx))
        .expect("fetch works");
    assert!(fetched.dir.join("tethers-gate.exe").is_file());
    assert!(fetched.dir.join("tethers-engine.exe").is_file());
    assert!(fetched.dir.join("provenance.json").is_file());
    assert_eq!(fetched.provenance.tethers_source_sha, "7".repeat(40));
    // Resolvable by the managed path afterwards.
    let resolved = omen_authority::resolve_companion(&install_dir(&fx)).expect("resolves");
    assert_eq!(resolved.provenance.gate_exe_sha256, sha(&fx.gate_bytes));
}

#[test]
fn fetch_refuses_zip_digest_mismatch_without_install() {
    let fx = Fixture::new();
    let zip = fx.zip_bytes(&[], &[]);
    let t = FakeTransport {
        tag: "v0.9.0-preview.22".to_string(),
        manifest_json: fx.manifest(&"0".repeat(64)),
        zip_bytes: zip,
        zip_digest_override: None,
        fail_download: false,
    };
    let err = fetch_companion(&t, "v0.9.0-preview.22", &install_dir(&fx), &work_dir(&fx))
        .expect_err("digest mismatch must refuse");
    assert!(
        format!("{err:?}").contains("zip_digest_mismatch"),
        "got: {err:?}"
    );
    assert!(!install_dir(&fx).exists(), "refused fetch must not install");
}

#[test]
fn fetch_refuses_unexpected_zip_entry() {
    let fx = Fixture::new();
    let zip = fx.zip_bytes(&[("evil.exe", b"evil")], &[]);
    let t = FakeTransport {
        tag: "v0.9.0-preview.22".to_string(),
        manifest_json: fx.manifest(&sha(&zip)),
        zip_bytes: zip,
        zip_digest_override: None,
        fail_download: false,
    };
    let err = fetch_companion(&t, "v0.9.0-preview.22", &install_dir(&fx), &work_dir(&fx))
        .expect_err("unexpected entry must refuse");
    assert!(format!("{err:?}").contains("unexpected"), "got: {err:?}");
    assert!(!install_dir(&fx).exists());
}

#[test]
fn fetch_refuses_nested_executable() {
    let fx = Fixture::new();
    let zip = fx.zip_bytes(
        &[("sub/tethers-gate.exe", &fx.gate_bytes)],
        &["tethers-gate.exe"],
    );
    let t = FakeTransport {
        tag: "v0.9.0-preview.22".to_string(),
        manifest_json: fx.manifest(&sha(&zip)),
        zip_bytes: zip,
        zip_digest_override: None,
        fail_download: false,
    };
    let err = fetch_companion(&t, "v0.9.0-preview.22", &install_dir(&fx), &work_dir(&fx))
        .expect_err("nested exe must refuse");
    assert!(format!("{err:?}").contains("nested"), "got: {err:?}");
    assert!(!install_dir(&fx).exists());
}

#[test]
fn fetch_refuses_tampered_binary() {
    let fx = Fixture::new();
    // Rebuild with tampered gate bytes but the ORIGINAL manifest pins.
    let tampered = b"tampered-gate-binary".to_vec();
    let mut buf = Vec::new();
    {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("tethers-gate.exe", opts).unwrap();
        w.write_all(&tampered).unwrap();
        w.start_file("tethers-engine.exe", opts).unwrap();
        w.write_all(&fx.engine_bytes).unwrap();
        w.start_file("provenance.json", opts).unwrap();
        w.write_all(&std::fs::read(fx.comp_dir().join("provenance.json")).unwrap())
            .unwrap();
        w.finish().unwrap();
    }
    let zip = buf;
    let t = FakeTransport {
        tag: "v0.9.0-preview.22".to_string(),
        manifest_json: fx.manifest(&sha(&zip)),
        zip_bytes: zip,
        zip_digest_override: None,
        fail_download: false,
    };
    let err = fetch_companion(&t, "v0.9.0-preview.22", &install_dir(&fx), &work_dir(&fx))
        .expect_err("tampered binary must refuse");
    assert!(format!("{err:?}").contains("hash_mismatch"), "got: {err:?}");
    assert!(!install_dir(&fx).exists());
}

#[test]
fn fetch_refuses_unbound_release() {
    let fx = Fixture::new();
    let zip = fx.zip_bytes(&[], &[]);
    let plain = serde_json::json!({
        "schema_version": 1, "version": "0.9.0-preview.22",
        "git_sha": "a".repeat(40), "channel": "preview",
        "package_asset": "omen.zip", "package_sha256": "b".repeat(64),
        "binary_sha256": "c".repeat(64),
        "min_state_schema": 1, "machine_contract": "0.8",
    })
    .to_string();
    let t = FakeTransport {
        tag: "v0.9.0-preview.22".to_string(),
        manifest_json: plain,
        zip_bytes: zip,
        zip_digest_override: None,
        fail_download: false,
    };
    let err = fetch_companion(&t, "v0.9.0-preview.22", &install_dir(&fx), &work_dir(&fx))
        .expect_err("unbound release must refuse");
    assert!(format!("{err:?}").contains("unbound"), "got: {err:?}");
}

#[test]
fn fetch_refuses_wrong_tag() {
    let fx = Fixture::new();
    let zip = fx.zip_bytes(&[], &[]);
    let t = FakeTransport {
        tag: "v0.9.0-preview.22".to_string(),
        manifest_json: fx.manifest(&sha(&zip)),
        zip_bytes: zip,
        zip_digest_override: None,
        fail_download: false,
    };
    let err = fetch_companion(&t, "v0.9.0-preview.99", &install_dir(&fx), &work_dir(&fx))
        .expect_err("wrong tag must refuse");
    assert!(format!("{err:?}").contains("tag_missing"), "got: {err:?}");
}
