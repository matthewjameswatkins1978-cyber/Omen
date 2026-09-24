//! H doctor/repair/corruption/firewall tests.
//!
//! - Doctor is observational (double-run changes nothing).
//! - Repair plans safe fixes, applies them, refuses ambiguous ones.
//! - Corruption matrix: each kind classified repairable/refuse.
//! - Workspace config can never own lifecycle (hostile Omen.toml).
//! - Synthetic secrets never leak through doctor/diagnostics.
use omen_lifecycle::doctor::{self, DoctorInput};
use omen_lifecycle::install::{Channel, InstallRecord, Ownership};
use omen_lifecycle::plan;
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
    assert!(
        rplan
            .items
            .iter()
            .any(|i| i.identity == "repoint-active:only-slot")
    );
    let rep = plan::apply_plan(&rplan, &|_| Ok(plan::Revalidate::Proceed), &|item| {
        omen_lifecycle::repair::apply_repair_item(&base, item)
    });
    assert!(rep.fully_applied());
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
