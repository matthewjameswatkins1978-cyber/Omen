//! H doctor/repair/corruption/firewall tests.
//!
//! - Doctor is observational (double-run changes nothing).
//! - Repair plans safe fixes, applies them, refuses ambiguous ones.
//! - Corruption matrix: each kind classified repairable/refuse.
//! - Workspace config can never own lifecycle (hostile Omen.toml).
//! - Synthetic secrets never leak through doctor/diagnostics.
use omen_lifecycle::doctor::{self, DoctorInput};
use omen_lifecycle::install::{Channel, InstallRecord, Ownership};
use std::path::PathBuf;

const SYNTH: &str = "sk-h-test-SYNTHETIC-0000";
const SYNTH2: &str = "h-secret-plant-SYNTHETIC-1111";

fn exe_name() -> &'static str {
    if cfg!(windows) { "omen.exe" } else { "omen" }
}

fn base_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("state");
    std::fs::create_dir_all(base.join("evidence")).unwrap();
    std::fs::create_dir_all(base.join("workspaces")).unwrap();
    (tmp, base)
}

fn input(base: &std::path::Path, exe: &std::path::Path) -> DoctorInput {
    DoctorInput {
        base: base.to_path_buf(),
        version: "0.9.0-preview.16".to_string(),
        git_sha: "deadbeef".to_string(),
        exe_path: exe.to_path_buf(),
        adapters: vec![("codex".to_string(), true, true)],
    }
}

#[test]
fn doctor_observes_broken_pointer_and_repairs_when_safe() {
    let (_tmp, base) = base_fixture();
    // Broken active pointer (points at missing slot) + exactly one healthy slot.
    omen_lifecycle::update::write_active_pointer(&base, "ghost-slot").unwrap();
    let slot = base.join("versions").join("only-slot");
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::write(slot.join(exe_name()), b"bin").unwrap();

    let exe = base.join("versions").join("only-slot").join(exe_name());
    let report = doctor::run_doctor(&input(&base, &exe));
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.id == "slot.active" && f.status == doctor::Status::Fail)
    );

    let before = walk(&base);
    let report2 = doctor::run_doctor(&input(&base, &exe));
    assert_eq!(walk(&base), before, "second doctor run must change nothing");
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::to_value(&report2).unwrap(),
        "doctor is deterministic"
    );

    // Repair plans the re-point (exactly one slot: unambiguous).
    let (rplan, refused) = omen_lifecycle::repair::repair_plan(&base, &report);
    assert!(refused.is_empty());
    let item = rplan
        .items
        .iter()
        .find(|i| i.identity == "repoint-active:only-slot")
        .expect("repoint planned")
        .clone();
    // Apply ONLY the re-point under test: applying the whole plan would
    // execute path:register against the REAL registry User PATH (live
    // side effect observed on dev machines). Plan coverage stays whole;
    // apply coverage stays hermetic.
    let detail = omen_lifecycle::repair::apply_repair_item(&base, &item).unwrap();
    assert!(!detail.is_empty());
    // Re-run doctor: pointer healthy now.
    let after = doctor::run_doctor(&input(&base, &exe));
    assert!(
        after
            .findings
            .iter()
            .any(|f| f.id == "slot.active" && f.status == doctor::Status::Ok)
    );
}

#[test]
fn repair_refuses_ambiguous_pointer() {
    let (_tmp, base) = base_fixture();
    omen_lifecycle::update::write_active_pointer(&base, "ghost-slot").unwrap();
    // Two healthy slots + broken pointer: ambiguous, must refuse.
    for s in ["slot-a", "slot-b"] {
        let slot = base.join("versions").join(s);
        std::fs::create_dir_all(&slot).unwrap();
        std::fs::write(slot.join(exe_name()), b"bin").unwrap();
    }
    let exe = base.join("versions").join("slot-a").join(exe_name());
    let report = doctor::run_doctor(&input(&base, &exe));
    let (rplan, refused) = omen_lifecycle::repair::repair_plan(&base, &report);
    assert!(refused.iter().any(|r| r.id == "repoint-active"));
    assert!(
        !rplan
            .items
            .iter()
            .any(|i| i.identity.starts_with("repoint-active"))
    );
    // Pointer untouched.
    assert_eq!(
        omen_lifecycle::update::read_active_pointer(&base)
            .unwrap()
            .active_slot,
        "ghost-slot"
    );
}

#[test]
fn corruption_matrix_classification() {
    // Each corruption kind: repairable class or explicit refusal.
    // - truncated install record JSON -> doctor Fail finding, repair refuses
    //   (missing provenance: unknown ownership path).
    let (_tmp, base) = base_fixture();
    std::fs::create_dir_all(base.join("install")).unwrap();
    std::fs::write(
        base.join("install").join("record.json"),
        b"{\"schema_version\": 1, TRUNCATED",
    )
    .unwrap();
    let exe = base.join(exe_name());
    std::fs::write(&exe, b"bin").unwrap();
    let report = doctor::run_doctor(&input(&base, &exe));
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.id == "install.record" && f.status == doctor::Status::Fail)
    );
    // Record bytes preserved (no repair deletes them).
    assert!(base.join("install").join("record.json").is_file());

    // - corrupted disposable cache -> repairable (rebuild planned).
    std::fs::create_dir_all(base.join("cache")).unwrap();
    let mut bad_report = report.clone();
    bad_report.findings.push(doctor::Finding {
        id: "cache.corrupt".to_string(),
        status: doctor::Status::Fail,
        summary: "cache unreadable".to_string(),
        detail: None,
    });
    let (rplan, _) = omen_lifecycle::repair::repair_plan(&base, &bad_report);
    assert!(rplan.items.iter().any(|i| i.identity == "cache:rebuild"));

    // - history corruption finding -> refused, never "repaired" by deletion.
    bad_report.findings.push(doctor::Finding {
        id: "history.corrupt".to_string(),
        status: doctor::Status::Fail,
        summary: "history db corrupt".to_string(),
        detail: None,
    });
    let (_, refused) = omen_lifecycle::repair::repair_plan(&base, &bad_report);
    assert!(refused.iter().any(|r| r.id == "history.corrupt"));
}

#[test]
fn path_integration_gap_becomes_explicit_repair_intent() {
    let (_tmp, base) = base_fixture();
    let exe = base.join(exe_name());
    std::fs::write(&exe, b"bin").unwrap();
    let mut report = doctor::run_doctor(&input(&base, &exe));
    // Simulate the PATH finding without depending on machine PATH state.
    report.findings.push(doctor::Finding {
        id: "path.integrated".to_string(),
        status: doctor::Status::Warn,
        summary: "running executable not found on PATH".to_string(),
        detail: None,
    });
    let (rplan, _) = omen_lifecycle::repair::repair_plan(&base, &report);
    assert!(rplan.items.iter().any(|i| i.identity == "path:register"));
}

#[test]
fn workspace_config_cannot_own_lifecycle() {
    // Hostile workspace Omen.toml content is detected; user channel is
    // unaffected by anything a workspace says.
    let (_tmp, base) = base_fixture();
    omen_lifecycle::install::set_user_channel(&base, Channel::Preview).unwrap();
    let evil = "[workspace]\nname = \"evil\"\nchannel = \"stable\"\nupdate_source = \"https://evil.example/pwn\"\n[lifecycle]\nkeep_days = 1\n";
    let violations = omen_lifecycle::install::workspace_lifecycle_violations(evil);
    assert!(!violations.is_empty());
    // User channel unchanged by hostile content (nothing reads workspace).
    assert_eq!(
        omen_lifecycle::install::user_channel(&base),
        Channel::Preview
    );
}

#[test]
fn install_record_roundtrip_and_channel() {
    let (_tmp, base) = base_fixture();
    let mut rec = InstallRecord::new(
        Ownership::Omen,
        Channel::Preview,
        "0.9.0-preview.16",
        "cafe",
    );
    rec.owner_name = None;
    omen_lifecycle::install::save_install_record(&base, &rec).unwrap();
    let back = omen_lifecycle::install::load_install_record(&base)
        .unwrap()
        .unwrap();
    assert_eq!(back.version, "0.9.0-preview.16");
    assert_eq!(back.schema_version, 1);
    omen_lifecycle::install::set_user_channel(&base, Channel::Stable).unwrap();
    assert_eq!(
        omen_lifecycle::install::user_channel(&base),
        Channel::Stable
    );
}

#[test]
fn synthetic_secrets_never_leak() {
    // Plant synthetics in env + files; doctor machine output + diagnostic
    // bundle must contain labels at most, never values.
    unsafe {
        std::env::set_var("OMEN_H_TEST_API_KEY", SYNTH);
        std::env::set_var("MYAPP_TOKEN", SYNTH2);
    }
    let (_tmp, base) = base_fixture();
    std::fs::write(
        base.join("evidence").join("note.txt"),
        format!("saw {SYNTH} today"),
    )
    .unwrap();
    let exe = base.join(exe_name());
    std::fs::write(&exe, b"bin").unwrap();
    let report = doctor::run_doctor(&input(&base, &exe));
    let machine = doctor::machine_json(&report);
    assert!(!machine.contains(SYNTH));
    assert!(!machine.contains(SYNTH2));

    let bundle = omen_lifecycle::diagnostics::DiagnosticBundle {
        schema_version: 1,
        created_at: "t".to_string(),
        version: "v".to_string(),
        git_sha: "g".to_string(),
        contract_version: "0.8".to_string(),
        ownership: "omen".to_string(),
        channel: "preview".to_string(),
        doctor: report,
        env_shape: omen_lifecycle::diagnostics::collect_env_shape(),
        config_shape: serde_json::json!({"token": SYNTH2}),
        logs_tail: vec![format!("used key {SYNTH}")],
        storage_summary: serde_json::json!({}),
    };
    // Redaction applied at write; verify the redactor directly on shape.
    let red = omen_lifecycle::diagnostics::redact_json(&serde_json::to_value(&bundle).unwrap());
    let text = serde_json::to_string(&red).unwrap();
    assert!(!text.contains(SYNTH));
    assert!(!text.contains(SYNTH2));
    unsafe {
        std::env::remove_var("OMEN_H_TEST_API_KEY");
        std::env::remove_var("MYAPP_TOKEN");
    }
}

/// Antigravity dogfood B: when the executing binary's embedded identity
/// disagrees with the install record, doctor must say so loudly — not
/// report both side by side with no verdict.
#[test]
fn doctor_flags_binary_record_identity_mismatch() {
    let (_tmp, base) = base_fixture();
    let record = InstallRecord::new(
        Ownership::Omen,
        Channel::Preview,
        "0.9.0-preview.15",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    omen_lifecycle::install::save_install_record(&base, &record).unwrap();
    let exe = base.join(exe_name());
    std::fs::write(&exe, b"bin").unwrap();
    // Embedded identity (0.9.0-preview.16 / deadbeef) != record (15 / aaaa).
    let report = doctor::run_doctor(&input(&base, &exe));
    let finding = report
        .findings
        .iter()
        .find(|f| f.id == "install.identity")
        .expect("install.identity finding present");
    assert_eq!(finding.status, omen_lifecycle::doctor::Status::Warn);
}

/// Antigravity dogfood B (conveyor side): a conveyor installed.json whose
/// active identity disagrees with the running binary must be flagged, so a
/// dev/manual binary run against a conveyor install is explicit.
#[test]
fn doctor_flags_conveyor_identity_mismatch() {
    let (_tmp, base) = base_fixture();
    let exe = base.join(exe_name());
    std::fs::write(&exe, b"bin").unwrap();
    std::fs::create_dir_all(base.join("state")).unwrap();
    std::fs::write(
        base.join("state").join("installed.json"),
        serde_json::to_vec(&serde_json::json!({
            "active_slot": "0.9.0-preview.21-ccccccc",
            "active": {
                "provenance": "omen-preview-conveyor",
                "preview_version": "0.9.0-preview.21",
                "git_sha": "cccccccccccccccccccccccccccccccccccccccc",
            },
        }))
        .unwrap(),
    )
    .unwrap();
    let report = doctor::run_doctor(&input(&base, &exe));
    let finding = report
        .findings
        .iter()
        .find(|f| f.id == "install.conveyor_identity")
        .expect("install.conveyor_identity finding present");
    assert_eq!(finding.status, omen_lifecycle::doctor::Status::Warn);
}

fn walk(dir: &std::path::Path) -> Vec<String> {
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

// ---------------------------------------------------------------------------
// Install-truth incident regressions (OMEN-INSTALL-TRUTH-01).
// ---------------------------------------------------------------------------

fn install_record_with_slot(base: &std::path::Path, slot: &str) {
    let mut record = InstallRecord::new(
        Ownership::Omen,
        Channel::Preview,
        "0.9.0-preview.25",
        "dddddddddddddddddddddddddddddddddddddddd",
    );
    record.active_slot = Some(slot.to_string());
    omen_lifecycle::install::save_install_record(base, &record).unwrap();
}

/// R1: a debug build earlier on PATH is detected, planned for removal,
/// and the apply refuses anything that is not verified-stale. No test
/// mutates the process PATH: detection runs over explicit entries, and
/// planning reads the finding detail (machine list), so parallel tests
/// cannot observe each other.
#[test]
fn path_shadowing_detected_planned_and_guarded() {
    let (_tmp, base) = base_fixture();
    let shadow = _tmp.path().join("shadow").join("target").join("debug");
    std::fs::create_dir_all(&shadow).unwrap();
    std::fs::write(shadow.join(exe_name()), b"fake-debug-omen").unwrap();
    let entries = vec![
        "C:/Windows/system32".to_string(),
        shadow.to_string_lossy().to_string(),
        base.join("bin").to_string_lossy().to_string(),
    ];
    // Pure detection over explicit entries.
    let found = omen_lifecycle::path_audit::shadowing_omen_exe_dirs_in(&base, &entries);
    assert_eq!(found, vec![shadow.to_string_lossy().to_string()]);
    // Planning reads the doctor finding detail, never the live env.
    let report = shadowed_report(&found);
    let (rplan, _) = omen_lifecycle::repair::repair_plan(&base, &report);
    let identity = format!("path:clean:{}", shadow.to_string_lossy());
    assert!(
        rplan.items.iter().any(|i| i.identity == identity),
        "shadowing entry must be planned for removal"
    );
    // Apply proves ownership: the fake binary cannot answer --version.
    let item = rplan.items.iter().find(|i| i.identity == identity).unwrap();
    let err = omen_lifecycle::repair::apply_repair_item(&base, item).unwrap_err();
    assert!(
        format!("{err:?}").contains("Omen-owned"),
        "foreign binary must be refused, got: {err:?}"
    );
    // Anything not verified-stale is refused, even if hand-planned.
    let hostile = omen_lifecycle::plan::PlannedItem {
        identity: "path:clean:C:/Program Files/nodejs".to_string(),
        role: "runtime".to_string(),
        reason: "hostile".to_string(),
        size_bytes: None,
        consequence: "none".to_string(),
        protected_at_plan: false,
        fingerprint: None,
    };
    assert!(
        omen_lifecycle::repair::apply_repair_item(&base, &hostile).is_err(),
        "non-stale entries are never removed"
    );
}

/// R1 positive ownership path (Unix): a script binary answering the Omen
/// version banner passes the ownership proof and cleans without touching
/// real registry state (Linux apply returns guidance; the proof is the
/// Ok here plus the exact refusal behavior above on foreign binaries).
#[test]
#[cfg(unix)]
fn owned_script_binary_passes_ownership_proof() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("t").join("target").join("debug");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("omen");
    std::fs::write(
        &script,
        "#!/bin/sh\necho 'omen 0.9.0-preview.25 contract:0.8 commit:abc'\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(omen_lifecycle::path_audit::omen_owned_exe_dir(&dir));
    // And the full plan→apply chain accepts it (guidance Ok on Linux).
    let (_tmp, base) = base_fixture();
    let report = shadowed_report(&[dir.to_string_lossy().to_string()]);
    let (rplan, _) = omen_lifecycle::repair::repair_plan(&base, &report);
    let item = rplan
        .items
        .iter()
        .find(|i| i.identity.starts_with("path:clean:"))
        .expect("owned entry planned")
        .clone();
    let detail = omen_lifecycle::repair::apply_repair_item(&base, &item).unwrap();
    assert!(!detail.is_empty());
}

fn shadowed_report(found: &[String]) -> omen_lifecycle::doctor::DoctorReport {
    omen_lifecycle::doctor::DoctorReport {
        schema_version: 1,
        version: "0.9.0-preview.16".to_string(),
        git_sha: "deadbeef".to_string(),
        contract_version: "0.8".to_string(),
        ownership: Ownership::Unknown,
        channel: Channel::Preview,
        findings: vec![omen_lifecycle::doctor::Finding {
            id: "path.shadowed".to_string(),
            status: omen_lifecycle::doctor::Status::Warn,
            summary: "1 stale build dir(s) shadow omen on PATH (repair: path:clean)".to_string(),
            detail: Some(found.join(";")),
        }],
    }
}

/// R2: record + pointer agree but the conveyor projection lags: repair
/// re-projects from the authoritative slot manifest, preserving history.
#[test]
fn record_pointer_agreement_reprojects_lagging_projection() {
    let (_tmp, base) = base_fixture();
    let slot = "slot-new";
    install_record_with_slot(&base, slot);
    omen_lifecycle::update::write_active_pointer(&base, slot).unwrap();
    let slot_dir = base.join("versions").join(slot);
    std::fs::create_dir_all(&slot_dir).unwrap();
    std::fs::write(slot_dir.join(exe_name()), b"bin").unwrap();
    let manifest = serde_json::json!({
        "schema_version": 2,
        "provenance": "ci",
        "preview_version": "0.9.0-preview.25",
        "git_sha": "dddddddddddddddddddddddddddddddddddddddd",
    });
    std::fs::write(
        slot_dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    // Lagging projection (the Preview 24/25 incident shape).
    std::fs::create_dir_all(base.join("state")).unwrap();
    std::fs::write(
        base.join("state").join("installed.json"),
        serde_json::to_vec(&serde_json::json!({
            "active_slot": "slot-old",
            "previous_slot": "slot-older",
            "active": {"preview_version": "0.9.0-preview.24"},
        }))
        .unwrap(),
    )
    .unwrap();

    let exe = slot_dir.join(exe_name());
    let report = doctor::run_doctor(&input(&base, &exe));
    let (rplan, refused) = omen_lifecycle::repair::repair_plan(&base, &report);
    assert!(refused.iter().all(|r| r.id != "state:reproject"));
    let item = rplan
        .items
        .iter()
        .find(|i| i.identity == format!("state:reproject:{slot}"))
        .expect("reproject planned")
        .clone();
    omen_lifecycle::repair::apply_repair_item(&base, &item).unwrap();
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(base.join("state").join("installed.json")).unwrap())
            .unwrap();
    assert_eq!(after["active_slot"], slot);
    assert_eq!(
        after["active"]["git_sha"],
        "dddddddddddddddddddddddddddddddddddddddd"
    );
    assert_eq!(after["previous_slot"], "slot-older", "history preserved");
    // R8: second plan no longer proposes the reproject (idempotent).
    let report2 = doctor::run_doctor(&input(&base, &exe));
    let (rplan2, _) = omen_lifecycle::repair::repair_plan(&base, &report2);
    assert!(
        !rplan2
            .items
            .iter()
            .any(|i| i.identity.starts_with("state:reproject")),
        "repaired projection must not re-plan"
    );
}

/// R2 refusal: record and pointer disagree — the mechanism must not guess.
#[test]
fn reproject_refuses_record_pointer_disagreement() {
    let (_tmp, base) = base_fixture();
    install_record_with_slot(&base, "slot-a");
    omen_lifecycle::update::write_active_pointer(&base, "slot-b").unwrap();
    let exe = base.join(exe_name());
    std::fs::write(&exe, b"bin").unwrap();
    let report = doctor::run_doctor(&input(&base, &exe));
    let (_, refused) = omen_lifecycle::repair::repair_plan(&base, &report);
    assert!(
        refused.iter().any(|r| r.id == "state:reproject"),
        "ambiguous record/pointer must refuse, got: {refused:?}"
    );
}

/// R6: the registry User PATH is readable (Windows) and parse-covered;
/// elsewhere the reader is honestly absent.
#[test]
fn registry_user_path_readable_where_it_exists() {
    #[cfg(windows)]
    {
        let path = omen_lifecycle::path_audit::read_registry_user_path()
            .expect("registry User PATH must be readable on Windows");
        assert!(!path.is_empty());
    }
    #[cfg(not(windows))]
    assert_eq!(omen_lifecycle::path_audit::read_registry_user_path(), None);
}

fn finding(
    id: &str,
    status: omen_lifecycle::doctor::Status,
    detail: Option<&str>,
) -> omen_lifecycle::doctor::Finding {
    omen_lifecycle::doctor::Finding {
        id: id.to_string(),
        status,
        summary: id.to_string(),
        detail: detail.map(str::to_string),
    }
}

fn bare_report(
    findings: Vec<omen_lifecycle::doctor::Finding>,
) -> omen_lifecycle::doctor::DoctorReport {
    omen_lifecycle::doctor::DoctorReport {
        schema_version: 1,
        version: "0.9.0-preview.16".to_string(),
        git_sha: "deadbeef".to_string(),
        contract_version: "0.8".to_string(),
        ownership: Ownership::Unknown,
        channel: Channel::Preview,
        findings,
    }
}

/// Review §2: full identity — a slot manifest that disagrees with the
/// record on version/SHA refuses even when the slot string matches.
#[test]
fn reproject_refuses_authoritative_identity_mismatch() {
    let (_tmp, base) = base_fixture();
    let slot = "slot-new";
    install_record_with_slot(&base, slot);
    omen_lifecycle::update::write_active_pointer(&base, slot).unwrap();
    let slot_dir = base.join("versions").join(slot);
    std::fs::create_dir_all(&slot_dir).unwrap();
    // Same slot string, DIFFERENT authoritative identity.
    std::fs::write(
        slot_dir.join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 2,
            "preview_version": "0.9.0-preview.99",
            "git_sha": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        }))
        .unwrap(),
    )
    .unwrap();
    let exe = base.join(exe_name());
    std::fs::write(&exe, b"bin").unwrap();
    let report = doctor::run_doctor(&input(&base, &exe));
    let (rplan, refused) = omen_lifecycle::repair::repair_plan(&base, &report);
    assert!(
        refused.iter().any(|r| r.id == "state:reproject"),
        "record/manifest identity mismatch must refuse"
    );
    assert!(
        !rplan
            .items
            .iter()
            .any(|i| i.identity.starts_with("state:reproject")),
        "no reproject may be planned on authoritative disagreement"
    );
}

/// Review §3: one installed build — superseded slots retire, active +
/// previous (rollback) and unmanaged dirs are preserved.
#[test]
fn retire_obsolete_slots_keeps_rollback_and_ignores_unmanaged() {
    let (_tmp, base) = base_fixture();
    let mut record = InstallRecord::new(
        Ownership::Omen,
        Channel::Preview,
        "0.9.0-preview.25",
        "dddddddddddddddddddddddddddddddddddddddd",
    );
    record.active_slot = Some("slot-active".to_string());
    record.previous_slot = Some("slot-prev".to_string());
    omen_lifecycle::install::save_install_record(&base, &record).unwrap();
    for s in ["slot-active", "slot-prev", "slot-old"] {
        let dir = base.join("versions").join(s);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(exe_name()), b"bin").unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "preview_version": "0.9.0-preview.25",
                "git_sha": "dddddddddddddddddddddddddddddddddddddddd",
            }))
            .unwrap(),
        )
        .unwrap();
    }
    // Unmanaged dir (no manifest): never planned, never touched.
    let junk = base.join("versions").join("junk-dir");
    std::fs::create_dir_all(&junk).unwrap();
    std::fs::write(junk.join(exe_name()), b"bin").unwrap();

    let exe = base.join(exe_name());
    std::fs::write(&exe, b"bin").unwrap();
    let report = doctor::run_doctor(&input(&base, &exe));
    let (rplan, _) = omen_lifecycle::repair::repair_plan(&base, &report);
    let retire: Vec<_> = rplan
        .items
        .iter()
        .filter(|i| i.identity.starts_with("versions:retire:"))
        .collect();
    assert_eq!(retire.len(), 1);
    assert_eq!(retire[0].identity, "versions:retire:slot-old");
    omen_lifecycle::repair::apply_repair_item(&base, retire[0]).unwrap();
    assert!(!base.join("versions").join("slot-old").exists());
    assert!(
        base.join("versions")
            .join("slot-active")
            .join(exe_name())
            .is_file()
    );
    assert!(
        base.join("versions")
            .join("slot-prev")
            .join(exe_name())
            .is_file()
    );
    assert!(junk.join(exe_name()).is_file(), "unmanaged dir preserved");
    // No obsolete Omen binary remains under versions/.
    let report2 = doctor::run_doctor(&input(&base, &exe));
    let (rplan2, _) = omen_lifecycle::repair::repair_plan(&base, &report2);
    assert!(
        !rplan2
            .items
            .iter()
            .any(|i| i.identity.starts_with("versions:retire:")),
        "finalized promotion leaves nothing retireable"
    );
}

/// Review §4: duplicate canonical entries collapse; unknown-ownership
/// Omen executables refuse automatic removal.
#[test]
fn dedupe_planned_and_unknown_ownership_refused() {
    let (_tmp, base) = base_fixture();
    let report = bare_report(vec![
        finding(
            "path.canonical_single",
            omen_lifecycle::doctor::Status::Warn,
            Some("C:/Omen/bin"),
        ),
        finding(
            "path.unknown_omen",
            omen_lifecycle::doctor::Status::Warn,
            Some("D:/tools/omen-standalone"),
        ),
    ]);
    let (rplan, refused) = omen_lifecycle::repair::repair_plan(&base, &report);
    assert!(
        rplan.items.iter().any(|i| i.identity == "path:dedupe"),
        "duplicates must be planned for collapse"
    );
    assert!(
        refused.iter().any(|r| r.id == "path:unknown-omen"),
        "unknown ownership must refuse, got: {refused:?}"
    );
    assert!(
        !rplan.items.iter().any(|i| i.identity.contains("unknown")),
        "unknown entries are never planned"
    );
    // Dedupe apply is safe everywhere (no-op or guidance when clean).
    let item = rplan
        .items
        .iter()
        .find(|i| i.identity == "path:dedupe")
        .unwrap();
    let detail = omen_lifecycle::repair::apply_repair_item(&base, item).unwrap();
    assert!(!detail.is_empty());
}

/// Review §1: plan detail with non-stale and exe-less entries plans
/// only the qualifying dir.
#[test]
fn plan_detail_filters_nonqualifying_entries() {
    let (_tmp, base) = base_fixture();
    let good = _tmp.path().join("g").join("target").join("debug");
    std::fs::create_dir_all(&good).unwrap();
    std::fs::write(good.join(exe_name()), b"x").unwrap();
    let ghost = _tmp.path().join("h").join("target").join("debug");
    std::fs::create_dir_all(&ghost).unwrap(); // no exe inside
    let detail = format!(
        "C:/Program Files/nodejs;{};{}",
        ghost.to_string_lossy(),
        good.to_string_lossy()
    );
    let report = bare_report(vec![finding(
        "path.shadowed",
        omen_lifecycle::doctor::Status::Warn,
        Some(&detail),
    )]);
    let (rplan, _) = omen_lifecycle::repair::repair_plan(&base, &report);
    let cleans: Vec<_> = rplan
        .items
        .iter()
        .filter(|i| i.identity.starts_with("path:clean:"))
        .collect();
    assert_eq!(cleans.len(), 1);
    assert_eq!(
        cleans[0].identity,
        format!("path:clean:{}", good.to_string_lossy())
    );
}
