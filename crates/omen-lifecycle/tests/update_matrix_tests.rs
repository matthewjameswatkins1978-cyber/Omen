//! H update hostile matrix + rollback proof + ownership matrix.
//!
//! Fixture release source: local directory with `releases.json` + zip
//! packages carrying REAL digests. Deterministic stub health/launcher so no
//! real process launch is needed at this layer (real bytes are proven in
//! the installed-product gates).
use omen_lifecycle::install::{Channel, InstallRecord, Ownership};
use omen_lifecycle::update::{
    self, CheckOutcome, FailureHooks, HealthChecker, HealthEvidence, Launcher, ReleaseMeta,
    ReleaseSource,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

struct StubHealth {
    ok: bool,
    version: String,
    sha: String,
}

impl HealthChecker for StubHealth {
    fn check(
        &self,
        _staged_binary: &Path,
        _expect: &ReleaseMeta,
    ) -> Result<HealthEvidence, omen_lifecycle::LifecycleError> {
        if !self.ok {
            return Err(omen_lifecycle::LifecycleError::Health(
                "stub health failure".to_string(),
            ));
        }
        Ok(HealthEvidence {
            launched: true,
            version: Some(self.version.clone()),
            git_sha: Some(self.sha.clone()),
            contract: Some("0.8".to_string()),
            state_opens: true,
        })
    }
}

struct StubLauncher(bool);
impl Launcher for StubLauncher {
    fn launches(&self, _binary: &Path) -> bool {
        self.0
    }
}

struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    source: PathBuf,
}

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "omen.exe" } else { "omen" }
}

/// Build a fixture source dir with one release. Returns (meta, source dir).
/// `tamper` corrupts the package bytes AFTER the manifest digests are
/// computed (checksum-mismatch case).
fn make_release(dir: &Path, version: &str, git_sha: &str, tamper: bool) -> ReleaseMeta {
    let binary_bytes = format!("fake-omen-binary-{version}-{git_sha}").into_bytes();
    let binary_sha = sha_hex(&binary_bytes);
    let pkg_path = dir.join(format!("omen-{version}.zip"));
    {
        let f = std::fs::File::create(&pkg_path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file(exe_name(), opts).unwrap();
        use std::io::Write;
        w.write_all(&binary_bytes).unwrap();
        w.start_file("manifest.json", opts).unwrap();
        let manifest = serde_json::json!({
            "preview_version": version,
            "git_sha": git_sha,
            "binary_sha256": binary_sha,
        });
        w.write_all(serde_json::to_vec(&manifest).unwrap().as_slice())
            .unwrap();
        w.finish().unwrap();
    }
    let mut pkg_bytes = std::fs::read(&pkg_path).unwrap();
    let package_sha = sha_hex(&pkg_bytes);
    if tamper {
        pkg_bytes.push(0xFF);
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();
    }
    let meta = ReleaseMeta {
        version: version.to_string(),
        git_sha: git_sha.to_string(),
        channel: Channel::Preview,
        package_sha256: package_sha,
        binary_sha256: binary_sha,
        package: format!("omen-{version}.zip"),
        download_url: None,
        min_state_schema: 1,
        contract_version: "0.8".to_string(),
    };
    let index = serde_json::json!({"releases": [meta]});
    std::fs::write(
        dir.join("releases.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    meta
}

fn setup() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("state");
    let source = tmp.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    // Existing healthy install: version A with slot binary present.
    let mut record = InstallRecord::new(
        Ownership::Omen,
        Channel::Preview,
        "0.9.0-preview.15",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    record.active_slot = Some("0.9.0-preview.15-aaaaaaa".to_string());
    let slot = base.join("versions").join("0.9.0-preview.15-aaaaaaa");
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::write(slot.join(exe_name()), b"fake-binary-A").unwrap();
    omen_lifecycle::install::save_install_record(&base, &record).unwrap();
    update::write_active_pointer(&base, "0.9.0-preview.15-aaaaaaa").unwrap();
    Fx {
        _tmp: tmp,
        base,
        source,
    }
}

fn load_record(fx: &Fx) -> InstallRecord {
    omen_lifecycle::install::load_install_record(&fx.base)
        .unwrap()
        .unwrap()
}

fn good_hooks() -> FailureHooks {
    FailureHooks::default()
}

#[test]
fn good_update_end_to_end() {
    let fx = setup();
    let meta = make_release(
        &fx.source,
        "0.9.0-preview.16",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        false,
    );
    let mut record = load_record(&fx);
    let health = StubHealth {
        ok: true,
        version: meta.version.clone(),
        sha: meta.git_sha.clone(),
    };
    let tx = update::run_update(
        &fx.base,
        &mut record,
        meta.clone(),
        &ReleaseSource::Directory(fx.source.clone()),
        &good_hooks(),
        &health,
    )
    .unwrap();
    assert_eq!(tx.stage, update::TxStage::Activated);
    assert_eq!(record.version, "0.9.0-preview.16");
    assert_eq!(
        record.previous_slot.as_deref(),
        Some("0.9.0-preview.15-aaaaaaa")
    );
    let active = record.active_slot.clone().unwrap();
    assert!(active.starts_with("0.9.0-preview.16-"));
    // Previous slot binary preserved.
    assert!(
        fx.base
            .join("versions")
            .join("0.9.0-preview.15-aaaaaaa")
            .join(exe_name())
            .is_file()
    );
    // Active pointer truthful.
    let ptr = update::read_active_pointer(&fx.base).unwrap();
    assert_eq!(ptr.active_slot, active);
}

#[test]
fn checksum_mismatch_fails_closed() {
    let fx = setup();
    let meta = make_release(
        &fx.source,
        "0.9.0-preview.16",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        true,
    );
    let mut record = load_record(&fx);
    let health = StubHealth {
        ok: true,
        version: meta.version.clone(),
        sha: meta.git_sha.clone(),
    };
    let err = update::run_update(
        &fx.base,
        &mut record,
        meta,
        &ReleaseSource::Directory(fx.source.clone()),
        &good_hooks(),
        &health,
    )
    .unwrap_err();
    assert_eq!(err.phase(), "verify");
    // Previous healthy install preserved + truthful.
    let record2 = load_record(&fx);
    assert_eq!(record2.version, "0.9.0-preview.15");
    assert_eq!(
        record2.active_slot.as_deref(),
        Some("0.9.0-preview.15-aaaaaaa")
    );
    assert_eq!(
        update::read_active_pointer(&fx.base).unwrap().active_slot,
        "0.9.0-preview.15-aaaaaaa"
    );
    // Quarantined, no half-active candidate.
    assert!(fx.base.join("update").join("quarantine").is_dir());
}

#[test]
fn swapped_manifest_fails_closed() {
    // Package bytes intact but the inner manifest names a different build:
    // staging must refuse the mislabeled package.
    let fx = setup();
    let meta = make_release(
        &fx.source,
        "0.9.0-preview.16",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        false,
    );
    let pkg = fx.source.join(&meta.package);
    {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&pkg).unwrap()).unwrap();
        let mut names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        names.retain(|n| n != "manifest.json");
        let mut out = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
            for n in &names {
                let mut f = zip.by_name(n).unwrap();
                let mut data = Vec::new();
                use std::io::Read;
                f.read_to_end(&mut data).unwrap();
                w.start_file(n, zip::write::SimpleFileOptions::default())
                    .unwrap();
                use std::io::Write;
                w.write_all(&data).unwrap();
            }
            w.start_file("manifest.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            let evil = serde_json::json!({"preview_version": "0.9.0-preview.16", "git_sha": "cccccccccccccccccccccccccccccccccccccccc"});
            use std::io::Write;
            w.write_all(serde_json::to_vec(&evil).unwrap().as_slice())
                .unwrap();
            w.finish().unwrap();
        }
        std::fs::write(&pkg, &out).unwrap();
    }
    // Recompute the package digest so the checksum passes and the binding is tested.
    let mut meta2 = meta.clone();
    meta2.package_sha256 = sha_hex(&std::fs::read(&pkg).unwrap());
    let index = serde_json::json!({"releases": [meta2.clone()]});
    std::fs::write(
        fx.source.join("releases.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    let mut record = load_record(&fx);
    let health = StubHealth {
        ok: true,
        version: meta2.version.clone(),
        sha: meta2.git_sha.clone(),
    };
    let err = update::run_update(
        &fx.base,
        &mut record,
        meta2,
        &ReleaseSource::Directory(fx.source.clone()),
        &good_hooks(),
        &health,
    )
    .unwrap_err();
    assert_eq!(err.phase(), "verify");
    assert_eq!(load_record(&fx).version, "0.9.0-preview.15");
}

#[test]
fn corrupt_archive_records_failure_and_quarantines() {
    // Truncated zip: magic intact, central directory broken. The failure
    // must be RECORDED (tx Failed) with the partial candidate quarantined —
    // never a silent transaction death with stale CandidateStaged state.
    let fx = setup();
    let meta = make_release(
        &fx.source,
        "0.9.0-preview.16",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        false,
    );
    let pkg = fx.source.join(&meta.package);
    let mut bytes = std::fs::read(&pkg).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&pkg, &bytes).unwrap();
    let mut meta2 = meta.clone();
    meta2.package_sha256 = sha_hex(&std::fs::read(&pkg).unwrap());
    let mut record = load_record(&fx);
    let health = StubHealth {
        ok: true,
        version: meta2.version.clone(),
        sha: meta2.git_sha.clone(),
    };
    let err = update::run_update(
        &fx.base,
        &mut record,
        meta2,
        &ReleaseSource::Directory(fx.source.clone()),
        &good_hooks(),
        &health,
    )
    .unwrap_err();
    // Truncated mid-extraction: structural verification fails (phase
    // verify: the persisted stage was Downloaded).
    assert_eq!(err.phase(), "verify");
    // Failure recorded: no open transaction masquerades as staged.
    let states = update::classify_update_restart(&fx.base);
    assert!(
        states
            .iter()
            .all(|s| matches!(s, update::UpdateRestart::PreviousStillActive { .. }))
    );
    // Partial candidate quarantined.
    assert!(fx.base.join("update").join("quarantine").is_dir());
    assert_eq!(load_record(&fx).version, "0.9.0-preview.15");
}

#[test]
fn source_sha_mismatch_means_malformed() {
    // A release whose manifest lacks digest provenance is malformed, never
    // "up to date".
    let fx = setup();
    let meta = ReleaseMeta {
        version: "0.9.0-preview.16".to_string(),
        git_sha: String::new(),
        channel: Channel::Preview,
        package_sha256: String::new(),
        binary_sha256: String::new(),
        package: "x".to_string(),
        download_url: None,
        min_state_schema: 1,
        contract_version: "0.8".to_string(),
    };
    let index = serde_json::json!({"releases": [meta]});
    std::fs::write(
        fx.source.join("releases.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    let report = update::check_for_update(
        "0.9.0-preview.15",
        Channel::Preview,
        Ownership::Omen,
        &ReleaseSource::Directory(fx.source.clone()),
    );
    assert!(matches!(
        report.outcome,
        CheckOutcome::MalformedMetadata { .. }
    ));
}

#[test]
fn incompatible_candidate_is_explicit() {
    let fx = setup();
    let mut meta = make_release(
        &fx.source,
        "0.9.0-preview.16",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        false,
    );
    meta.contract_version = "0.7".to_string();
    let index = serde_json::json!({"releases": [meta]});
    std::fs::write(
        fx.source.join("releases.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    let report = update::check_for_update(
        "0.9.0-preview.15",
        Channel::Preview,
        Ownership::Omen,
        &ReleaseSource::Directory(fx.source.clone()),
    );
    assert!(matches!(
        report.outcome,
        CheckOutcome::IncompatibleCandidate { .. }
    ));
}

#[test]
fn interrupted_download_discarded() {
    let fx = setup();
    let mut meta = make_release(
        &fx.source,
        "0.9.0-preview.16",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        false,
    );
    // Empty package on the source = interrupted download.
    std::fs::write(fx.source.join(&meta.package), b"").unwrap();
    meta.package_sha256 = sha_hex(b"");
    let index = serde_json::json!({"releases": [meta.clone()]});
    std::fs::write(
        fx.source.join("releases.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    let mut record = load_record(&fx);
    let health = StubHealth {
        ok: true,
        version: meta.version.clone(),
        sha: meta.git_sha.clone(),
    };
    let err = update::run_update(
        &fx.base,
        &mut record,
        meta,
        &ReleaseSource::Directory(fx.source.clone()),
        &good_hooks(),
        &health,
    )
    .unwrap_err();
    assert_eq!(err.phase(), "download");
    assert_eq!(load_record(&fx).version, "0.9.0-preview.15");
}

#[test]
fn failure_matrix_each_stage() {
    // health failure, activation failure, migration failure: previous
    // install preserved, explicit stage error, no ambiguous state.
    for (hook, phase) in [
        (
            FailureHooks {
                fail_health: true,
                ..Default::default()
            },
            "health",
        ),
        (
            FailureHooks {
                fail_activate: true,
                ..Default::default()
            },
            "activate",
        ),
        (
            FailureHooks {
                fail_migrate: true,
                ..Default::default()
            },
            "migrate",
        ),
        (
            FailureHooks {
                fail_stage: true,
                ..Default::default()
            },
            "stage",
        ),
    ] {
        let fx = setup();
        let meta = make_release(
            &fx.source,
            "0.9.0-preview.16",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            false,
        );
        let mut record = load_record(&fx);
        let health = StubHealth {
            ok: true,
            version: meta.version.clone(),
            sha: meta.git_sha.clone(),
        };
        let err = update::run_update(
            &fx.base,
            &mut record,
            meta,
            &ReleaseSource::Directory(fx.source.clone()),
            &hook,
            &health,
        )
        .unwrap_err();
        assert_eq!(err.phase(), phase);
        assert_eq!(load_record(&fx).version, "0.9.0-preview.15");
        assert_eq!(
            update::read_active_pointer(&fx.base).unwrap().active_slot,
            "0.9.0-preview.15-aaaaaaa"
        );
    }
}

#[test]
fn restart_after_interruption_is_classified() {
    let fx = setup();
    // Craft transaction records at mid-flight stages.
    for stage in [
        update::TxStage::Staged,
        update::TxStage::Migrated,
        update::TxStage::HealthChecked,
        update::TxStage::Failed,
    ] {
        let meta = make_release(
            &fx.source,
            "0.9.0-preview.16",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            false,
        );
        let mut tx = update::UpdateTransaction::new(
            meta,
            "0.9.0-preview.15",
            Some("0.9.0-preview.15-aaaaaaa".to_string()),
        );
        tx.stage = stage;
        update::save_tx(&fx.base, &tx).unwrap();
    }
    let states = update::classify_update_restart(&fx.base);
    assert_eq!(states.len(), 4);
    assert!(
        states
            .iter()
            .any(|s| matches!(s, update::UpdateRestart::CandidateStaged { .. }))
    );
    assert!(
        states
            .iter()
            .any(|s| matches!(s, update::UpdateRestart::MigrationIncomplete { .. }))
    );
    assert!(
        states
            .iter()
            .any(|s| matches!(s, update::UpdateRestart::ActivationIncomplete { .. }))
    );
    assert!(
        states
            .iter()
            .any(|s| matches!(s, update::UpdateRestart::PreviousStillActive { .. }))
    );
    assert!(
        !states
            .iter()
            .any(|s| matches!(s, update::UpdateRestart::Clean))
    );
}

#[test]
fn rollback_proof_binary_then_state() {
    let fx = setup();
    let meta = make_release(
        &fx.source,
        "0.9.0-preview.16",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        false,
    );
    let mut record = load_record(&fx);
    let health = StubHealth {
        ok: true,
        version: meta.version.clone(),
        sha: meta.git_sha.clone(),
    };
    update::run_update(
        &fx.base,
        &mut record,
        meta,
        &ReleaseSource::Directory(fx.source.clone()),
        &good_hooks(),
        &health,
    )
    .unwrap();
    assert_eq!(record.version, "0.9.0-preview.16");

    // Binary rollback: A returns active; resolved slot verified.
    let back = update::rollback_binary(&fx.base, &mut record, &StubLauncher(true)).unwrap();
    assert_eq!(back, "0.9.0-preview.15-aaaaaaa");
    assert_eq!(
        update::read_active_pointer(&fx.base).unwrap().active_slot,
        back
    );
    assert_eq!(record.active_slot.as_deref(), Some(back.as_str()));
    // Stable copies refreshed from the re-activated slot: the resolved
    // product and the pointer never disagree.
    assert_eq!(
        std::fs::read(fx.base.join("bin").join(exe_name())).unwrap(),
        b"fake-binary-A"
    );

    // State rollback is separate: snapshot then restore.
    let snap = "snap_test1";
    update::take_snapshot(&fx.base, snap).unwrap();
    record.version = "MUTATED".to_string();
    omen_lifecycle::install::save_install_record(&fx.base, &record).unwrap();
    let restored = update::rollback_state(&fx.base, snap).unwrap();
    assert!(restored.iter().any(|r| r == "install/record.json"));
    let record2 = load_record(&fx);
    assert_eq!(record2.version, "0.9.0-preview.16");

    // Snapshot without manifest: refuse, never guess.
    assert!(update::rollback_state(&fx.base, "nope-missing").is_err());
}

#[test]
fn ownership_matrix_self_update_differs() {
    for (owner, allowed) in [
        (Ownership::Omen, true),
        (Ownership::PackageManager, false),
        (Ownership::Development, false),
        (Ownership::Unknown, false),
    ] {
        let fx = setup();
        let meta = make_release(
            &fx.source,
            "0.9.0-preview.16",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            false,
        );
        let mut record = load_record(&fx);
        record.owner = owner;
        omen_lifecycle::install::save_install_record(&fx.base, &record).unwrap();
        let health = StubHealth {
            ok: true,
            version: meta.version.clone(),
            sha: meta.git_sha.clone(),
        };
        let res = update::run_update(
            &fx.base,
            &mut record,
            meta,
            &ReleaseSource::Directory(fx.source.clone()),
            &good_hooks(),
            &health,
        );
        assert_eq!(res.is_ok(), allowed, "{owner:?}");
    }
}

#[test]
fn check_is_readonly() {
    let fx = setup();
    let meta = make_release(
        &fx.source,
        "0.9.0-preview.16",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        false,
    );
    let before = walk(&fx.base);
    let report = update::check_for_update(
        "0.9.0-preview.15",
        Channel::Preview,
        Ownership::Omen,
        &ReleaseSource::Directory(fx.source.clone()),
    );
    match report.outcome {
        CheckOutcome::Candidate {
            candidate,
            compatible,
            ..
        } => {
            assert_eq!(candidate.version, meta.version);
            assert!(compatible);
        }
        other => panic!("unexpected {other:?}"),
    }
    // Check downloaded nothing, migrated nothing, changed nothing.
    assert_eq!(walk(&fx.base), before);
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                out.push(e.path().to_string_lossy().to_string());
                if e.path().is_dir() {
                    stack.push(e.path());
                }
            }
        }
    }
    out.sort();
    out
}
