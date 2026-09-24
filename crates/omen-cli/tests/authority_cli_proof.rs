//! H2 CLI proofs: `omen authority exec` over the REAL canonical Gate.
//!
//! Requires `OMEN_H2_GATE_EXE` + `OMEN_H2_ENGINE_EXE` + `OMEN_H2_TETHERS_SRC`
//! (set by the h2-gate-e2e CI job); skips loudly otherwise — the plain
//! matrix jobs keep proving the FakeGate paths.
//!
//! ALLOW / DENY / ASK-approve are proven by real observable markers plus
//! the CLI's closed-vocabulary outcome line. Seam rule (Lucy ruling):
//! on Windows hosts with the Tethers replay-owner seam, exec
//! deterministically refuses with exactly GATE_REPLAY_UNAVAILABLE and
//! zero spawn. On hosted CI (`GITHUB_ACTIONS`) that refusal is REQUIRED —
//! an unexpected success means the upstream seam changed without review
//! and must fail the run. Local runs accept either truthful outcome.

use omen_authority::protocol::{AUTHORITY_PROTOCOL, TETHERS_PRODUCT_VERSION};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const TETHERS_SHA: &str = "7e29110319c554a6586865ec6c47a45498696d16";

const CORE_TETHER: &str = r#"tether "J14 complete local scenario"

anchor
    coding.task_completed

when
    project.type is "software"
    and task.changed_files greater_than 0

do
    fixture.ping
        message: anchor.task
        path: anchor.path
"#;

const MARKER_RS: &str = r#"
fn main() {
    let marker = std::env::args().nth(1).expect("marker path");
    std::fs::write(&marker, std::process::id().to_string()).expect("marker");
    println!("marker-ok");
}
"#;

static N: AtomicU64 = AtomicU64::new(0);

struct LiveEnv {
    gate_exe: PathBuf,
    engine_exe: PathBuf,
    manifest_text: String,
}

fn live_env() -> Option<LiveEnv> {
    let gate_exe = std::env::var_os("OMEN_H2_GATE_EXE").map(PathBuf::from)?;
    let engine_exe = std::env::var_os("OMEN_H2_ENGINE_EXE").map(PathBuf::from)?;
    let src = std::env::var_os("OMEN_H2_TETHERS_SRC").map(PathBuf::from)?;
    if !gate_exe.is_file() || !engine_exe.is_file() {
        return None;
    }
    let sha = std::env::var("OMEN_H2_TETHERS_SHA").unwrap_or_default();
    assert_eq!(
        sha.to_lowercase(),
        omen_authority::TETHERS_SOURCE_SHA,
        "tethers checkout SHA must equal the pinned canonical SHA"
    );
    assert_eq!(omen_authority::TETHERS_SOURCE_SHA, TETHERS_SHA);
    let manifest_text = std::fs::read_to_string(
        src.join("tethers-0.1/protocol/capability-manifests/fixture-ping-standing-allow.json"),
    )
    .ok()?;
    Some(LiveEnv {
        gate_exe,
        engine_exe,
        manifest_text,
    })
}

fn skip(name: &str) {
    println!("SKIP-CLI ({name}): set OMEN_H2_GATE_EXE + OMEN_H2_ENGINE_EXE + OMEN_H2_TETHERS_SRC");
}

fn sha_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

fn marker_fixture() -> PathBuf {
    static COMPILED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    COMPILED
        .get_or_init(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let src = dir.path().join("cli_marker.rs");
            std::fs::write(&src, MARKER_RS).unwrap();
            let exe = dir.path().join("cli-marker-fixture.exe");
            let out = Command::new("rustc")
                .arg("--edition=2021")
                .arg(&src)
                .arg("-o")
                .arg(&exe)
                .output()
                .expect("rustc builds marker fixture");
            assert!(out.status.success(), "marker build failed");
            dir.keep().join("cli-marker-fixture.exe")
        })
        .clone()
}

struct CliWorld {
    _tmp: tempfile::TempDir,
    companion: PathBuf,
    runtime: PathBuf,
    marker: PathBuf,
    spec: PathBuf,
    journal: PathBuf,
}

fn build_world(env: &LiveEnv, name: &str, decision: &str, eval: &str) -> CliWorld {
    let id = N.fetch_add(1, Ordering::SeqCst);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(format!(
        "omen-h2-cli-{}-{}-{}",
        std::process::id(),
        id,
        name
    ));
    let companion = root.join("companion");
    let runtime = root.join("runtime");
    std::fs::create_dir_all(&companion).unwrap();
    std::fs::create_dir_all(runtime.join("tethers")).unwrap();
    std::fs::create_dir_all(runtime.join("manifests")).unwrap();
    std::fs::create_dir_all(runtime.join("host-data")).unwrap();

    // Companion: exact canonical binaries + provenance with real hashes.
    let gate_name = if cfg!(windows) {
        "tethers-gate.exe"
    } else {
        "tethers-gate"
    };
    let engine_name = if cfg!(windows) {
        "tethers-engine.exe"
    } else {
        "tethers-engine"
    };
    std::fs::copy(&env.gate_exe, companion.join(gate_name)).unwrap();
    std::fs::copy(&env.engine_exe, companion.join(engine_name)).unwrap();
    let prov = json!({
        "schema": "omen.gate-companion/1",
        "tethers_source_sha": TETHERS_SHA,
        "gate_exe_sha256": sha_file(&companion.join(gate_name)),
        "engine_exe_sha256": sha_file(&companion.join(engine_name)),
        "protocol": AUTHORITY_PROTOCOL,
        "product_version": TETHERS_PRODUCT_VERSION,
        "gate_exe_name": gate_name,
        "engine_exe_name": engine_name,
    });
    std::fs::write(
        companion.join("provenance.json"),
        serde_json::to_vec_pretty(&prov).unwrap(),
    )
    .unwrap();

    // Gate runtime (mirrors the e2e Workspace contract).
    std::fs::write(runtime.join("tethers/complete.tether"), CORE_TETHER).unwrap();
    std::fs::write(
        runtime.join("manifests/fixture-ping-standing-allow.json"),
        &env.manifest_text,
    )
    .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&env.manifest_text).unwrap();
    let digest = manifest["digest"].as_str().unwrap().to_string();
    let config = json!({
        "format_version": "0.1",
        "tether_set": {
            "id": "r2.authority-gate", "version": "1",
            "tethers": [{
                "id": "r2-complete", "version": "1",
                "source_path": "tethers/complete.tether",
                "core_environment": {
                    "program_id": "program.j14.complete", "core_version": "1",
                    "capabilities": [{
                        "source_name": "fixture.ping",
                        "capability_id": "cap.semantic.fixture-ping",
                        "contract_digest": "CORE-CONTRACT-J14",
                        "runtime_name": "fixture.ping"
                    }],
                    "input_facts": [
                        {"source_name": "project.type", "fact_id": "fact.project_type",
                         "host_snapshot_key": "project.type", "scalar_type": "string",
                         "schema_description": "project type"},
                        {"source_name": "task.changed_files", "fact_id": "fact.task_changed_files",
                         "host_snapshot_key": "task.changed_files", "scalar_type": "integer",
                         "schema_description": "number of changed files"}
                    ]
                }
            }],
            "capability_requirements": [
                {"name": "fixture.ping", "version": 1, "reason": "H2 CLI fixture"}
            ]
        },
        "providers": [{
            "id": "tethers-stdio-fixture", "display_name": "Tethers Stdio Fixture",
            "transport": {"kind": "stdio", "command": "pwsh.exe",
                          "args": ["-NoProfile", "-File", "providers/fixture.ps1"],
                          "protocol_version": "2025-11-25"},
            "capabilities": [{
                "name": "fixture.ping", "version": 1,
                "manifest_path": "manifests/fixture-ping-standing-allow.json",
                "pinned_digest": digest,
                "scope_binding": {"kind": "path_prefix", "argument_json_pointer": "/path"}
            }]
        }],
        "policy": {"default": "deny", "rules": [{"name": "fixture.ping", "version": 1, "decision": decision}]}
    });
    std::fs::write(
        runtime.join("runtime.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    #[cfg(windows)]
    {
        let script = format!(
            "$p='{}'; $i=[System.Security.Principal.WindowsIdentity]::GetCurrent().Name; $a=[System.Security.AccessControl.DirectorySecurity]::new(); $a.SetAccessRuleProtection($true,$false); $h=[System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit; foreach($t in @($i,'NT AUTHORITY\\SYSTEM','BUILTIN\\Administrators')) {{ $a.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($t,'FullControl',$h,'None','Allow')) }}; Set-Acl -LiteralPath $p -AclObject $a",
            runtime.join("host-data").display()
        );
        let st = Command::new("pwsh.exe")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .expect("acl harden");
        assert!(st.success());
    }

    let marker = root.join(format!("{name}.marker"));
    let fixture = marker_fixture();
    let spec = json!({
        "tether_id": "r2-complete", "tether_version": "1",
        "evaluation_id": eval, "action_id": "action_1",
        "event_id": "evt_h2_cli_001", "event_name": "coding.task_completed",
        "event_data": {"project": "lantern-keeper", "task": "LK-39", "path": "projects/r2-stdio"},
        "facts": {"project.type": "software", "task.changed_files": 3},
        "expected_arguments": {"message": "LK-39", "path": "projects/r2-stdio"},
        "expected_capability": "fixture.ping", "expected_capability_version": 1,
        "expected_manifest_digest": manifest["digest"].as_str().unwrap(),
        "expected_provider": "tethers-stdio-fixture",
        "argv": [fixture.to_string_lossy(), marker.to_string_lossy()],
        "cwd": tmp.path().to_string_lossy(),
        "timeout_ms": 30_000,
        "success_result": {"echo": "marker-ok"},
    });
    let spec_path = root.join("spec.json");
    std::fs::write(&spec_path, serde_json::to_vec_pretty(&spec).unwrap()).unwrap();
    let journal = root.join("journal.jsonl");
    CliWorld {
        _tmp: tmp,
        companion,
        runtime,
        marker,
        spec: spec_path,
        journal,
    }
}

struct ExecOut {
    status_ok: bool,
    combined: String,
}

/// Run `omen authority exec` synchronously; never invents authority.
fn cli_exec(world: &CliWorld, on_ask: &str) -> ExecOut {
    let out = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args([
            "authority",
            "exec",
            "--spec",
            &world.spec.to_string_lossy(),
            "--on-ask",
            on_ask,
            "--companion-dir",
            &world.companion.to_string_lossy(),
            "--gate-runtime",
            &world.runtime.to_string_lossy(),
            "--journal",
            &world.journal.to_string_lossy(),
        ])
        .output()
        .expect("omen authority exec spawns");
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    ExecOut {
        status_ok: out.status.success(),
        combined,
    }
}

/// True on Windows hosts where the Tethers owner seam is active
/// (replay-root owner != invoking user): the canonical Gate will refuse
/// with exactly GATE_REPLAY_UNAVAILABLE. Always false elsewhere.
fn owner_seam_active(runtime: &Path) -> bool {
    #[cfg(windows)]
    {
        let hd = runtime.join("host-data").to_string_lossy().into_owned();
        let owner = Command::new("pwsh.exe")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Acl -LiteralPath '{hd}').Owner"),
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let user = Command::new("whoami")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        !owner.is_empty() && !user.is_empty() && !owner.eq_ignore_ascii_case(&user)
    }
    #[cfg(not(windows))]
    {
        let _ = runtime;
        false
    }
}

fn hosted() -> bool {
    std::env::var_os("GITHUB_ACTIONS").is_some()
}

/// Shared verdict: live success (markers prove the path) or, on Windows
/// seam hosts, the exact characterised refusal with zero spawn. Any other
/// outcome — including an unexpected hosted success — fails loudly.
fn verdict(name: &str, world: &CliWorld, out: &ExecOut, want_marker: bool, want_line: &str) {
    if out.status_ok {
        // On hosted Windows with the seam active, live success is
        // impossible without an upstream Tethers change — fail for review.
        if hosted() && owner_seam_active(&world.runtime) {
            panic!(
                "CLI UPSTREAM-SEAM-REVIEW-REQUIRED ({name}): live success with the owner seam active; review before accepting"
            );
        }
        assert!(
            out.combined.contains(want_line),
            "CLI {name}: want line {want_line:?} in output:\n{}",
            out.combined
        );
        assert_eq!(
            world.marker.is_file(),
            want_marker,
            "CLI {name}: marker truth violated"
        );
        println!("CLI-LIVE-PROOF ({name}): {want_line} + marker={want_marker}");
        return;
    }
    // Nonzero exit: only the exact, proven-active Windows owner seam is
    // acceptable — with zero spawn and no fabricated outcome.
    assert!(
        out.combined.contains("GATE_REPLAY_UNAVAILABLE"),
        "CLI {name}: unexpected failure (not the known seam):\n{}",
        out.combined
    );
    assert!(
        !world.marker.exists(),
        "CLI {name}: seam refusal executed (marker exists)"
    );
    assert!(
        owner_seam_active(&world.runtime),
        "CLI {name}: seam refusal without the owner seam active"
    );
    println!(
        "CLI-SEAM-CHARACTERISED ({name}): exact GATE_REPLAY_UNAVAILABLE, zero spawn — fail-closed, NOT live proof"
    );
}

#[test]
fn cli_allow_executes_with_marker() {
    let Some(env) = live_env() else {
        skip("cli-allow");
        return;
    };
    let world = build_world(&env, "cli-allow", "allow", "eval_cli_allow");
    let out = cli_exec(&world, "defer");
    verdict("cli-allow", &world, &out, true, "admitted:");
}

#[test]
fn cli_deny_zero_spawn() {
    let Some(env) = live_env() else {
        skip("cli-deny");
        return;
    };
    let world = build_world(&env, "cli-deny", "deny", "eval_cli_deny");
    let out = cli_exec(&world, "defer");
    verdict("cli-deny", &world, &out, false, "denied:");
}

#[test]
fn cli_ask_approve_executes_with_marker() {
    let Some(env) = live_env() else {
        skip("cli-ask");
        return;
    };
    let world = build_world(&env, "cli-ask", "ask", "eval_cli_ask");
    let out = cli_exec(&world, "approve");
    verdict("cli-ask", &world, &out, true, "admitted:");
}
