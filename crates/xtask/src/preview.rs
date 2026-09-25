//! Deterministic Preview Conveyor.  This module deliberately owns packaging
//! and installation identity, but never claims that a local build is external
//! trial ready.
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const CONTRACT_VERSION: &str = "0.8";
const FIXTURE: &[&str] = &["Cargo.toml", "Cargo.lock", "src/lib.rs"];

#[derive(Args, Debug)]
pub struct PreviewArgs {
    #[command(subcommand)]
    pub command: PreviewCommand,
}

#[derive(Subcommand, Debug)]
pub enum PreviewCommand {
    Status {
        #[arg(long)]
        json: bool,
    },
    Bump {
        #[arg(long)]
        to: Option<String>,
    },
    Preflight {
        #[arg(long)]
        package: Option<String>,
    },
    Package {
        #[arg(long)]
        ci_run_id: Option<u64>,
        #[arg(long)]
        artifact_id: Option<u64>,
        /// Provisioned gate-companion dir (gate exe + engine exe +
        /// provenance.json, built from the canonical Tethers SHA via
        /// `provision-gate`). Adds the companion zip + release binding.
        #[arg(long)]
        gate_companion: Option<PathBuf>,
    },
    Install {
        #[arg(long)]
        artifact: Option<PathBuf>,
    },
    Prove {
        #[arg(long, default_value = "2024-11-05")]
        mcp_protocol: String,
    },
    Cycle {
        #[arg(long, default_value = "2024-11-05")]
        mcp_protocol: String,
    },
    Rollback,
    Promote {
        #[arg(long)]
        expect_sha: String,
    },
    /// Build the managed Gate companion from an exact Tethers checkout:
    /// verifies the source SHA, builds the release Gate binary, requires
    /// the canonical engine binary, and writes provenance.json. Never
    /// modifies Tethers semantics; packaging only.
    ProvisionGate {
        /// Tethers checkout at the exact canonical SHA.
        #[arg(long)]
        tethers_dir: PathBuf,
        /// Expected canonical source SHA (fail closed on mismatch).
        #[arg(long)]
        expect_sha: String,
        /// Output dir for tethers-gate + tethers-engine + provenance.json.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    Local,
    Ci,
    Release,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema_version: u32,
    pub provenance: Provenance,
    pub preview_version: String,
    pub git_sha: String,
    pub contract_version: String,
    pub target: String,
    pub profile: String,
    pub binary_sha256: String,
    /// Schema v1 only: the historical payload digest under its legacy
    /// (misnamed) field. This was NEVER the enclosing ZIP's digest, even
    /// though the name suggests it. Absent in v2 emits. Do not reinterpret
    /// a v1 value as archive identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_sha256: Option<String>,
    /// Schema v2: explicit payload identity (see `payload_digest_v2`).
    /// Absent in v1 manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_sha256: Option<String>,
    pub ci_run_id: Option<u64>,
    pub artifact_id: Option<u64>,
    pub fixture_files: std::collections::BTreeMap<String, String>,
    /// SHA-256 of the sibling daemon executable (`omend`) shipped in the
    /// same package. `None` only for packages produced before Preview 8.
    #[serde(default)]
    pub daemon_binary_sha256: Option<String>,
}

/// Embedded package-manifest schema versions. Canonical law:
/// PAYLOAD HASH DESCRIBES CONTENT IDENTITY. PACKAGE HASH DESCRIBES FINAL
/// ARCHIVE BYTES. NEVER CALL ONE THE OTHER. The enclosing archive's SHA
/// therefore lives OUTSIDE the archive (release manifest, CI provenance,
/// install record) and never inside the embedded manifest.
pub const EMBEDDED_MANIFEST_SCHEMA_V1: u32 = 1;
pub const EMBEDDED_MANIFEST_SCHEMA_V2: u32 = 2;

impl Manifest {
    /// Explicit version dispatch for payload identity. No serde ambiguity:
    /// v1 reads the legacy field AS legacy payload identity; v2 reads the
    /// explicit field and REFUSES a manifest that also carries the legacy
    /// name (an archive SHA can never truthfully sit inside its archive).
    /// Unknown schema versions fail closed.
    pub fn payload_identity(&self) -> Result<&str, String> {
        match self.schema_version {
            EMBEDDED_MANIFEST_SCHEMA_V1 => self.package_sha256.as_deref().ok_or_else(|| {
                fail(
                    "OMEN_PREVIEW_MANIFEST_IDENTITY",
                    "v1 manifest lacks legacy package_sha256 (legacy payload digest)",
                )
            }),
            EMBEDDED_MANIFEST_SCHEMA_V2 => {
                if self.package_sha256.is_some() {
                    return Err(fail(
                        "OMEN_PREVIEW_MANIFEST_IDENTITY",
                        "v2 manifest must not carry package_sha256; enclosing-archive SHA lives outside the archive",
                    ));
                }
                self.payload_sha256.as_deref().ok_or_else(|| {
                    fail(
                        "OMEN_PREVIEW_MANIFEST_IDENTITY",
                        "v2 manifest lacks payload_sha256",
                    )
                })
            }
            v => Err(fail(
                "OMEN_PREVIEW_MANIFEST_IDENTITY",
                format!("unsupported embedded manifest schema_version {v}"),
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct InstallState {
    active_slot: Option<String>,
    previous_slot: Option<String>,
    active: Option<Manifest>,
    last_proof: Option<Proof>,
    ci_green: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct Proof {
    result: String,
    provenance: Option<Provenance>,
    git_sha: Option<String>,
    timestamp: String,
    external_trial_ready: bool,
    changed_files: u32,
}

struct McpProbe {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Result<serde_json::Value, String>>,
}

pub fn run(args: PreviewArgs, root: &Path) {
    let result = match args.command {
        PreviewCommand::Status { json } => status(root, json),
        PreviewCommand::Bump { to } => bump(root, to),
        PreviewCommand::Preflight { package } => preflight(root, package),
        PreviewCommand::Package {
            ci_run_id,
            artifact_id,
            gate_companion,
        } => package(root, ci_run_id, artifact_id, gate_companion),
        PreviewCommand::Install { artifact } => install(root, artifact),
        PreviewCommand::Prove { mcp_protocol } => prove(root, &mcp_protocol),
        PreviewCommand::Cycle { mcp_protocol } => cycle(root, &mcp_protocol),
        PreviewCommand::Rollback => rollback(root),
        PreviewCommand::Promote { expect_sha } => promote(root, &expect_sha),
        PreviewCommand::ProvisionGate {
            tethers_dir,
            expect_sha,
            out,
        } => provision_gate(root, &tethers_dir, &expect_sha, &out),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn fail(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}
fn run_cmd(root: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| fail("OMEN_PREVIEW_COMMAND_FAILED", e))?;
    if status.success() {
        Ok(())
    } else {
        Err(fail(
            "OMEN_PREVIEW_COMMAND_FAILED",
            format!("{program} {}", args.join(" ")),
        ))
    }
}
fn command_output(root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| fail("OMEN_PREVIEW_GITHUB_UNAVAILABLE", e))?;
    if !out.status.success() {
        return Err(fail(
            "OMEN_PREVIEW_GITHUB_UNAVAILABLE",
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| fail("OMEN_PREVIEW_GIT_FAILED", e))?;
    if !out.status.success() {
        return Err(fail(
            "OMEN_PREVIEW_GIT_FAILED",
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
fn digest_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| fail("OMEN_PREVIEW_HASH_MISMATCH", e))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
/// Legacy v1 payload algorithm. BYTE-EXACT preservation: v1 packages
/// (Preview 22/23 and older) verified against this digest, and rollback/
/// install of those packages must reproduce it bit-for-bit. Covers the omen
/// binary digest plus fixture names + digests in BTreeMap (deterministic)
/// order. Deliberately does NOT cover the daemon digest (v1 checked the
/// daemon binary directly instead). Never alter; v2 uses `payload_digest_v2`.
fn payload_digest(binary: &str, fixtures: &std::collections::BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    h.update(binary.as_bytes());
    for (name, digest) in fixtures {
        h.update(name.as_bytes());
        h.update(digest.as_bytes());
    }
    hex::encode(h.finalize())
}

/// v2 payload algorithm: explicit canonical encoding with a domain
/// separator, covering the omen digest, the omend digest (or the literal
/// `absent` marker for daemon-less packages), and fixture names + digests
/// in deterministic (BTreeMap) order. Never covers the final ZIP digest:
/// payload identity and archive identity are different ontologies.
fn payload_digest_v2(
    binary: &str,
    daemon: Option<&str>,
    fixtures: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut h = Sha256::new();
    h.update(b"omen-payload/2\x00");
    h.update(b"omen\x00");
    h.update(binary.as_bytes());
    h.update(b"\x00");
    h.update(b"omend\x00");
    h.update(daemon.unwrap_or("absent").as_bytes());
    h.update(b"\x00");
    for (name, digest) in fixtures {
        h.update(b"fixture\x00");
        h.update(name.as_bytes());
        h.update(b"\x00");
        h.update(digest.as_bytes());
        h.update(b"\x00");
    }
    hex::encode(h.finalize())
}

/// Constructor for new (schema v2) embedded manifests. The ONLY manifest
/// shape `package` emits. Carries `payload_sha256` and NEVER `package_sha256`:
/// the enclosing archive's SHA cannot truthfully sit inside the archive.
// Allow: nine parameters map 1:1 onto manifest fields; a params struct
// would obscure that mapping without adding safety.
#[allow(clippy::too_many_arguments)]
fn new_v2_manifest(
    provenance: Provenance,
    preview_version: String,
    git_sha: String,
    target: String,
    binary_sha256: String,
    daemon_binary_sha256: Option<String>,
    fixture_files: std::collections::BTreeMap<String, String>,
    ci_run_id: Option<u64>,
    artifact_id: Option<u64>,
) -> Manifest {
    let payload_sha256 = payload_digest_v2(
        &binary_sha256,
        daemon_binary_sha256.as_deref(),
        &fixture_files,
    );
    Manifest {
        schema_version: EMBEDDED_MANIFEST_SCHEMA_V2,
        provenance,
        preview_version,
        git_sha,
        contract_version: CONTRACT_VERSION.into(),
        target,
        profile: "release".into(),
        binary_sha256,
        package_sha256: None,
        payload_sha256: Some(payload_sha256),
        ci_run_id,
        artifact_id,
        fixture_files,
        daemon_binary_sha256,
    }
}

/// Pure package-content verification over archive BYTES (no filesystem
/// globals), shared by `install` and unit tests. `daemon_bytes` is `None`
/// only for pre-Preview-8 packages that ship no daemon executable.
/// v1 preserves the exact legacy checks; v2 additionally verifies every
/// fixture's bytes against the manifest map (v1 never did).
fn verify_package_contents(
    manifest: &Manifest,
    binary_bytes: &[u8],
    daemon_bytes: Option<&[u8]>,
    fixture_bytes: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let actual_binary = hex::encode(Sha256::digest(binary_bytes));
    if actual_binary != manifest.binary_sha256 {
        return Err(fail(
            "OMEN_PREVIEW_HASH_MISMATCH",
            "binary digest differs from manifest",
        ));
    }
    let actual_daemon = daemon_bytes.map(|b| hex::encode(Sha256::digest(b)));
    // v1 preserves the exact legacy daemon check (pre-Preview-8 packages
    // ship no daemon; a recorded digest mismatching shipped bytes fails).
    // v2 is stricter: a shipped daemon the manifest does not record also
    // fails — unrecorded executables must never ride inside a v2 package.
    let daemon_strict = manifest.schema_version == EMBEDDED_MANIFEST_SCHEMA_V2;
    match (&manifest.daemon_binary_sha256, &actual_daemon) {
        (Some(expected), Some(actual)) if expected != actual => {
            return Err(fail(
                "OMEN_PREVIEW_HASH_MISMATCH",
                "daemon digest differs from manifest",
            ));
        }
        (None, Some(_)) if daemon_strict => {
            return Err(fail(
                "OMEN_PREVIEW_HASH_MISMATCH",
                "package ships a daemon executable the manifest does not record",
            ));
        }
        _ => {}
    }
    match manifest.schema_version {
        EMBEDDED_MANIFEST_SCHEMA_V1 => {
            // Byte-exact legacy semantics: internal consistency of the
            // manifest's recorded binary digest + fixture map against the
            // legacy field. Fixture BYTES were never checked in v1.
            if payload_digest(&manifest.binary_sha256, &manifest.fixture_files)
                != manifest.payload_identity()?
            {
                return Err(fail(
                    "OMEN_PREVIEW_HASH_MISMATCH",
                    "package digest differs from manifest",
                ));
            }
        }
        EMBEDDED_MANIFEST_SCHEMA_V2 => {
            for (name, expected) in &manifest.fixture_files {
                let actual = fixture_bytes.get(name).ok_or_else(|| {
                    fail(
                        "OMEN_PREVIEW_HASH_MISMATCH",
                        format!("fixture {name} missing from package"),
                    )
                })?;
                if hex::encode(Sha256::digest(actual)) != *expected {
                    return Err(fail(
                        "OMEN_PREVIEW_HASH_MISMATCH",
                        format!("fixture {name} digest differs from manifest"),
                    ));
                }
            }
            if payload_digest_v2(
                &manifest.binary_sha256,
                manifest.daemon_binary_sha256.as_deref(),
                &manifest.fixture_files,
            ) != manifest.payload_identity()?
            {
                return Err(fail(
                    "OMEN_PREVIEW_HASH_MISMATCH",
                    "payload digest differs from manifest",
                ));
            }
        }
        v => {
            return Err(fail(
                "OMEN_PREVIEW_MANIFEST_IDENTITY",
                format!("unsupported embedded manifest schema_version {v}"),
            ));
        }
    }
    Ok(())
}

/// Canonical artifact-identity assignment for the install record.
/// `actual_package_sha256` is ALWAYS the SHA-256 of the final artifact
/// bytes (computed before extraction); the payload identity travels under
/// its own name. Pure for testability; called by `install`.
fn assign_artifact_identity(
    record: &mut omen_lifecycle::install::InstallRecord,
    actual_package_sha256: &str,
    payload_identity: &str,
) {
    record.package_sha256 = Some(actual_package_sha256.to_string());
    record.payload_sha256 = Some(payload_identity.to_string());
}
fn preview_version(root: &Path) -> Result<String, String> {
    let text = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    text.lines()
        .find(|l| {
            l.starts_with("version = \"0.8.0-preview.")
                || l.starts_with("version = \"0.9.0-preview.")
        })
        .and_then(|l| l.split('"').nth(1))
        .map(str::to_string)
        .ok_or_else(|| fail("OMEN_PREVIEW_VERSION_INVALID", "workspace version missing"))
}
fn parse_preview(v: &str) -> Result<u32, String> {
    let n = v
        .strip_prefix("0.8.0-preview.")
        .or_else(|| v.strip_prefix("0.9.0-preview."))
        .ok_or_else(|| fail("OMEN_PREVIEW_VERSION_INVALID", v))?;
    n.parse()
        .map_err(|_| fail("OMEN_PREVIEW_VERSION_INVALID", v))
}
fn fixture(root: &Path) -> PathBuf {
    root.join("tests").join("fixtures").join("auth_project")
}
fn fixture_hashes(root: &Path) -> Result<std::collections::BTreeMap<String, String>, String> {
    fixture_hashes_at(&fixture(root))
}
fn fixture_hashes_at(base: &Path) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut m = std::collections::BTreeMap::new();
    for rel in FIXTURE {
        m.insert((*rel).to_string(), digest_file(&base.join(rel))?);
    }
    Ok(m)
}
fn install_root() -> PathBuf {
    dirs_local().join("Omen")
}
fn dirs_local() -> PathBuf {
    std::env::var_os(if cfg!(windows) {
        "LOCALAPPDATA"
    } else {
        "HOME"
    })
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("."))
}
fn state_path() -> PathBuf {
    install_root().join("state").join("installed.json")
}
fn read_state() -> InstallState {
    fs::read(state_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn write_state(state: &InstallState) -> Result<(), String> {
    fs::create_dir_all(install_root().join("state")).map_err(|e| e.to_string())?;
    fs::write(state_path(), serde_json::to_vec_pretty(state).unwrap()).map_err(|e| e.to_string())
}
fn active_binary() -> PathBuf {
    install_root()
        .join("bin")
        .join(if cfg!(windows) { "omen.exe" } else { "omen" })
}
fn proof_path() -> PathBuf {
    install_root().join("evidence").join("last-proof.json")
}
fn installed_fixture() -> PathBuf {
    install_root().join("demo").join("acceptance-fixture-clean")
}
fn run_capture(
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
) -> Result<std::process::Output, String> {
    let mut c = Command::new(program);
    c.args(args);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    c.output().map_err(|e| fail("OMEN_MCP_PROOF_FAILED", e))
}
fn rpc_frame(value: &serde_json::Value) -> Vec<u8> {
    let mut out = serde_json::to_vec(value).unwrap();
    out.push(b'\n');
    out
}
fn spawn_reader(
    stdout: impl Read + Send + 'static,
) -> mpsc::Receiver<Result<serde_json::Value, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(line.trim()) {
                Ok(v) => {
                    if tx.send(Ok(v)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    break;
                }
            }
        }
    });
    rx
}
fn mcp_send(
    probe: &mut McpProbe,
    request: serde_json::Value,
    id: u64,
) -> Result<serde_json::Value, String> {
    probe
        .stdin
        .write_all(&rpc_frame(&request))
        .map_err(|e| fail("OMEN_MCP_PROOF_FAILED", e))?;
    probe
        .stdin
        .flush()
        .map_err(|e| fail("OMEN_MCP_PROOF_FAILED", e))?;
    loop {
        match probe
            .rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| {
                fail(
                    "OMEN_MCP_PROOF_FAILED",
                    format!("deadline waiting for response {id}"),
                )
            })?? {
            v if v.get("id").and_then(|x| x.as_u64()) == Some(id) => return Ok(v),
            _ => {}
        }
    }
}
fn mcp_probe(binary: &Path, fixture: &Path, mcp_protocol: &str) -> Result<McpProbeResult, String> {
    let mut child = Command::new(binary)
        .args([
            "mcp",
            "--workspace",
            fixture
                .to_str()
                .ok_or_else(|| fail("OMEN_MCP_PROOF_FAILED", "fixture path is not UTF-8"))?,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| fail("OMEN_MCP_PROOF_FAILED", e))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| fail("OMEN_MCP_PROOF_FAILED", "stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("OMEN_MCP_PROOF_FAILED", "stdout unavailable"))?;
    let rx = spawn_reader(stdout);
    let mut p = McpProbe { child, stdin, rx };
    let init = mcp_send(
        &mut p,
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":mcp_protocol,"capabilities":{},"clientInfo":{"name":"omen-preview-conveyor","version":"0.1"}}}),
        1,
    )?;
    p.stdin
        .write_all(&rpc_frame(
            &serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        ))
        .map_err(|e| e.to_string())?;
    p.stdin.flush().map_err(|e| e.to_string())?;
    let list = mcp_send(
        &mut p,
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        2,
    )?;
    let started = Instant::now();
    let search = mcp_send(
        &mut p,
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"omen_symbol_search","arguments":{"query":"refresh_token","limit":50}}}),
        3,
    )?;
    let search_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let definition = mcp_send(
        &mut p,
        serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"omen_symbol_definition","arguments":{"symbol":"refresh_token"}}}),
        4,
    )?;
    let definition_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let references = mcp_send(
        &mut p,
        serde_json::json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"omen_symbol_references","arguments":{"symbol":"refresh_token","limit":50}}}),
        5,
    )?;
    let references_ms = started.elapsed().as_millis();
    let _ = p.child.kill();
    Ok(McpProbeResult {
        initialize: init,
        tools: list,
        search,
        definition,
        references,
        search_ms,
        definition_ms,
        references_ms,
    })
}
fn mcp_text(response: &serde_json::Value) -> String {
    response
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn status(root: &Path, json: bool) -> Result<(), String> {
    let state = read_state();
    let source = serde_json::json!({"version":preview_version(root)?,"SHA":git(root,&["rev-parse","HEAD"])? ,"branch":git(root,&["branch","--show-current"])? ,"worktree_clean":git(root,&["status","--porcelain"] )?.is_empty()});
    let installed=state.active.as_ref().map(|m| serde_json::json!({"version":m.preview_version,"SHA":m.git_sha,"provenance":m.provenance,"binary_hash":m.binary_sha256,"payload_hash":m.payload_identity().unwrap_or("unknown"),"slot":state.active_slot,"active_path":install_root().join("bin").join(if cfg!(windows){"omen.exe"}else{"omen"})})).unwrap_or(serde_json::Value::Null);
    let v =
        serde_json::json!({"source":source,"installed":installed,"last_proof":state.last_proof});
    if json {
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        println!(
            "SOURCE\nversion: {}\nSHA: {}\nbranch: {}\nworktree clean: {}\nINSTALLED\n{}",
            source["version"],
            source["SHA"],
            source["branch"],
            source["worktree_clean"],
            serde_json::to_string_pretty(&installed).unwrap()
        );
    }
    Ok(())
}

fn bump(root: &Path, to: Option<String>) -> Result<(), String> {
    if !git(root, &["status", "--porcelain"])?.is_empty() {
        return Err(fail(
            "OMEN_PREVIEW_WORKTREE_DIRTY",
            "bump requires a clean worktree",
        ));
    }
    let old = preview_version(root)?;
    let next = to.unwrap_or_else(|| {
        let base = old
            .split_once("-preview.")
            .map(|(base, _)| base)
            .unwrap_or("0.9.0");
        format!("{base}-preview.{}", parse_preview(&old).unwrap() + 1)
    });
    parse_preview(&next)?;
    let text = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let replaced = text.replace(&old, &next);
    if replaced == text {
        return Err(fail("OMEN_PREVIEW_VERSION_INVALID", "version not found"));
    }
    fs::write(root.join("Cargo.toml"), replaced).map_err(|e| e.to_string())?;
    run_cmd(root, "cargo", &["update", "--workspace"])?;
    println!("{old} -> {next}\nchanged: Cargo.toml, Cargo.lock");
    Ok(())
}
fn preflight(root: &Path, package: Option<String>) -> Result<(), String> {
    run_cmd(root, "cargo", &["fmt", "--check"])?;
    run_cmd(root, "cargo", &["check", "--workspace"])?;
    if let Some(p) = package {
        run_cmd(root, "cargo", &["test", "-p", &p])?;
    }
    println!("READY_FOR_CI: YES");
    Ok(())
}

fn package(
    root: &Path,
    ci_run_id: Option<u64>,
    artifact_id: Option<u64>,
    gate_companion: Option<PathBuf>,
) -> Result<(), String> {
    run_cmd(root, "cargo", &["build", "--release", "-p", "omen-cli"])?;
    run_cmd(root, "cargo", &["build", "--release", "-p", "omen-daemon"])?;
    let exe =
        root.join("target")
            .join("release")
            .join(if cfg!(windows) { "omen.exe" } else { "omen" });
    let daemon_exe =
        root.join("target")
            .join("release")
            .join(if cfg!(windows) { "omend.exe" } else { "omend" });
    let bin_hash = digest_file(&exe)?;
    let daemon_bin_hash = digest_file(&daemon_exe)?;
    let version = preview_version(root)?;
    let sha = git(root, &["rev-parse", "HEAD"])?;
    let fixture_files = fixture_hashes(root)?;
    let manifest = new_v2_manifest(
        if ci_run_id.is_some() {
            Provenance::Ci
        } else {
            Provenance::Local
        },
        version.clone(),
        sha,
        target_triple()?,
        bin_hash.clone(),
        Some(daemon_bin_hash.clone()),
        fixture_files,
        ci_run_id,
        artifact_id,
    );
    let suffix = if matches!(manifest.provenance, Provenance::Ci) {
        "windows-x86_64"
    } else {
        "local"
    };
    let out = root
        .join("work")
        .join(format!("omen-{version}-{suffix}.zip"));
    fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    let file = fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut bytes = Vec::new();
    fs::File::open(&exe)
        .map_err(|e| e.to_string())?
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    zip.start_file(if cfg!(windows) { "omen.exe" } else { "omen" }, opts)
        .unwrap();
    zip.write_all(&bytes).unwrap();
    // Sibling daemon executable: `omen daemon start` locates omend next to
    // the running omen binary (Preview 8+ packages; older packages lack it).
    let mut daemon_bytes = Vec::new();
    fs::File::open(&daemon_exe)
        .map_err(|e| e.to_string())?
        .read_to_end(&mut daemon_bytes)
        .map_err(|e| e.to_string())?;
    zip.start_file(if cfg!(windows) { "omend.exe" } else { "omend" }, opts)
        .unwrap();
    zip.write_all(&daemon_bytes).unwrap();
    zip.start_file("manifest.json", opts).unwrap();
    zip.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes())
        .unwrap();
    for rel in FIXTURE {
        let mut fixture_bytes = Vec::new();
        fs::File::open(fixture(root).join(rel))
            .map_err(|e| e.to_string())?
            .read_to_end(&mut fixture_bytes)
            .map_err(|e| e.to_string())?;
        zip.start_file(format!("fixture/{rel}"), opts)
            .map_err(|e| e.to_string())?;
        zip.write_all(&fixture_bytes).map_err(|e| e.to_string())?;
    }
    zip.finish().unwrap();
    // Archive identity is computed DIRECTLY from the finalised ZIP bytes and
    // lives OUTSIDE the archive: the embedded manifest truthfully cannot
    // contain its own enclosing SHA (self-reference). The release manifest
    // below is the authority for `package_sha256`.
    let archive_sha256 = digest_file(&out)?;
    // Canonical per-release manifest (Lucy repair B1): derived from the
    // exact same candidate build — never hand-maintained. Published beside
    // the package on the GitHub Release; the update path binds
    // manifest.package_asset -> exact release asset -> asset URL.
    let payload_sha256 = manifest
        .payload_identity()
        .map(str::to_string)
        .map_err(|e| e.to_string())?;
    let mut release_manifest = serde_json::json!({
        "schema_version": 1,
        "version": version,
        "git_sha": manifest.git_sha,
        "channel": if version.contains("preview") { "preview" } else { "stable" },
        "package_asset": out.file_name().and_then(|n| n.to_str()).unwrap_or("package.zip"),
        "package_sha256": archive_sha256,
        "payload_sha256": payload_sha256,
        "package_size": fs::metadata(&out).map(|m| m.len()).ok(),
        "binary_sha256": bin_hash,
        "daemon_binary_sha256": daemon_bin_hash,
        "min_state_schema": 1,
        "machine_contract": CONTRACT_VERSION,
    });
    // H2 managed Gate companion: separate release asset (never inside
    // the main zip — the archive allowlist forbids nested executables,
    // and the previous updater must keep accepting the main package).
    // Binding fields are optional and ignored by old readers.
    if let Some(comp_dir) = gate_companion {
        let comp_zip = package_gate_companion(root, &version, suffix, &comp_dir)?;
        let comp_manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(comp_dir.join("provenance.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let obj = release_manifest.as_object_mut().ok_or_else(|| {
            fail(
                "OMEN_PREVIEW_GATE_COMPANION",
                "release manifest is not an object",
            )
        })?;
        obj.insert(
            "gate_companion_asset".to_string(),
            serde_json::Value::String(
                comp_zip
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("gate-companion.zip")
                    .to_string(),
            ),
        );
        obj.insert(
            "gate_companion_sha256".to_string(),
            serde_json::Value::String(digest_file(&comp_zip)?),
        );
        for (prov_key, rel_key) in [
            ("tethers_source_sha", "gate_tethers_sha"),
            ("gate_exe_sha256", "gate_exe_sha256"),
            ("engine_exe_sha256", "gate_engine_sha256"),
        ] {
            let value = comp_manifest.get(prov_key).cloned().ok_or_else(|| {
                fail(
                    "OMEN_PREVIEW_GATE_COMPANION",
                    format!("provenance.json lacks {prov_key}"),
                )
            })?;
            obj.insert(rel_key.to_string(), value);
        }
        println!("GATE_COMPANION: {}", comp_zip.display());
    }
    let release_path = root.join("work").join("omen-release.json");
    fs::write(
        &release_path,
        serde_json::to_vec_pretty(&release_manifest).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("RELEASE_MANIFEST: {}", release_path.display());
    println!(
        "PACKAGE: {}\nPROVENANCE: {:?}\nLOCAL_PROOF_PASS: NOT_RUN\nREADY_FOR_CI: YES\nREADY_FOR_EXTERNAL_TRIAL: {}",
        out.display(),
        manifest.provenance,
        if matches!(manifest.provenance, Provenance::Local) {
            "NO"
        } else {
            "YES"
        }
    );
    Ok(())
}

fn package_gate_companion(
    root: &Path,
    version: &str,
    suffix: &str,
    comp_dir: &Path,
) -> Result<PathBuf, String> {
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
    for name in [gate_name, engine_name, "provenance.json"] {
        if !comp_dir.join(name).is_file() {
            return Err(fail(
                "OMEN_PREVIEW_GATE_COMPANION",
                format!("companion dir lacks {name}: {}", comp_dir.display()),
            ));
        }
    }
    // Provenance pins must match the shipped bytes (fail closed here so
    // a stale/mismatched companion can never be published).
    let prov_text =
        fs::read_to_string(comp_dir.join("provenance.json")).map_err(|e| e.to_string())?;
    let prov: serde_json::Value = serde_json::from_str(&prov_text).map_err(|e| e.to_string())?;
    for (file, key) in [
        (gate_name, "gate_exe_sha256"),
        (engine_name, "engine_exe_sha256"),
    ] {
        let actual = digest_file(&comp_dir.join(file))?;
        let pinned = prov.get(key).and_then(|v| v.as_str()).ok_or_else(|| {
            fail(
                "OMEN_PREVIEW_GATE_COMPANION",
                format!("provenance lacks {key}"),
            )
        })?;
        if actual.to_lowercase() != pinned.to_lowercase() {
            return Err(fail(
                "OMEN_PREVIEW_GATE_COMPANION",
                format!("companion {file} hash {actual} != pinned {pinned}"),
            ));
        }
    }
    let out = root
        .join("work")
        .join(format!("omen-{version}-gate-companion-{suffix}.zip"));
    let file = fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for name in [gate_name, engine_name, "provenance.json"] {
        let mut bytes = Vec::new();
        fs::File::open(comp_dir.join(name))
            .map_err(|e| e.to_string())?
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        zip.start_file(name, opts).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap();
    Ok(out)
}

/// Build the managed Gate companion from an exact Tethers checkout.
/// Verifies the source SHA, builds the release Gate, requires the
/// canonical engine binary, writes provenance. Packaging only — never
/// touches Tethers semantics.
fn provision_gate(
    root: &Path,
    tethers_dir: &Path,
    expect_sha: &str,
    out: &Path,
) -> Result<(), String> {
    let actual = git(tethers_dir, &["rev-parse", "HEAD"])?;
    if actual.to_lowercase() != expect_sha.to_lowercase() {
        return Err(fail(
            "OMEN_PREVIEW_GATE_SHA_MISMATCH",
            format!("tethers checkout {actual} != expected {expect_sha}"),
        ));
    }
    if !git(
        tethers_dir,
        &["status", "--porcelain", "--untracked-files=no"],
    )?
    .is_empty()
    {
        // A dirty Tethers worktree would poison provenance: refuse.
        // (Untracked build outputs like target/ and _build/ do not count.)
        return Err(fail(
            "OMEN_PREVIEW_GATE_DIRTY",
            "tethers checkout has uncommitted changes; refusing to provision",
        ));
    }
    let host_rust = tethers_dir.join("tethers-0.1").join("host-rust");
    run_cmd(
        &host_rust,
        "cargo",
        &["build", "--release", "-p", "tethers-reference-host"],
    )?;
    let gate_built = host_rust
        .join("target")
        .join("release")
        .join(if cfg!(windows) {
            "tethers.exe"
        } else {
            "tethers"
        });
    let engine_built = tethers_dir
        .join("tethers-0.1")
        .join("engine-ocaml")
        .join("_build")
        .join("default")
        .join("bin")
        .join(if cfg!(windows) {
            "tethers_mcp_main.exe"
        } else {
            "tethers_mcp_main"
        });
    if !engine_built.is_file() {
        return Err(fail(
            "OMEN_PREVIEW_GATE_ENGINE_MISSING",
            format!(
                "canonical engine not built: {}; build with dune (opam) in tethers-0.1/engine-ocaml first",
                engine_built.display()
            ),
        ));
    }
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
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
    fs::copy(&gate_built, out.join(gate_name)).map_err(|e| e.to_string())?;
    fs::copy(&engine_built, out.join(engine_name)).map_err(|e| e.to_string())?;
    let provenance = serde_json::json!({
        "schema": "omen.gate-companion/1",
        "tethers_source_sha": actual.to_lowercase(),
        "gate_exe_sha256": digest_file(&out.join(gate_name))?,
        "engine_exe_sha256": digest_file(&out.join(engine_name))?,
        "protocol": "tethers.authority/1",
        "product_version": "0.8.0",
        "gate_exe_name": gate_name,
        "engine_exe_name": engine_name,
    });
    fs::write(
        out.join("provenance.json"),
        serde_json::to_vec_pretty(&provenance).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let _ = root;
    println!(
        "GATE_COMPANION_PROVISIONED: {}\nTETHERS_SHA: {actual}",
        out.display()
    );
    Ok(())
}

#[derive(Debug, Serialize)]
struct McpProbeResult {
    initialize: serde_json::Value,
    tools: serde_json::Value,
    search: serde_json::Value,
    definition: serde_json::Value,
    references: serde_json::Value,
    search_ms: u128,
    definition_ms: u128,
    references_ms: u128,
}

fn target_triple() -> Result<String, String> {
    let output = Command::new("rustc")
        .args(["-vV"])
        .output()
        .map_err(|e| fail("OMEN_PREVIEW_TARGET_UNKNOWN", e))?;
    if !output.status.success() {
        return Err(fail(
            "OMEN_PREVIEW_TARGET_UNKNOWN",
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| fail("OMEN_PREVIEW_TARGET_UNKNOWN", "rustc host triple missing"))
}

fn install(_root: &Path, artifact: Option<PathBuf>) -> Result<(), String> {
    let path = artifact.ok_or_else(|| {
        fail(
            "OMEN_PREVIEW_GITHUB_UNAVAILABLE",
            "default install requires an exact CI artifact; use --artifact for local debugging",
        )
    })?;
    // Archive identity FIRST, independently of extraction: the InstallRecord
    // represents the installed ARTIFACT, so its package_sha256 is the
    // SHA-256 of these final bytes — never anything from inside the archive.
    let actual_package_sha256 = digest_file(&path)?;
    let file = fs::File::open(&path).map_err(|e| fail("OMEN_PREVIEW_ARTIFACT_NOT_FOUND", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| fail("OMEN_PREVIEW_HASH_MISMATCH", e))?;
    let mut m = String::new();
    archive
        .by_name("manifest.json")
        .map_err(|e| e.to_string())?
        .read_to_string(&mut m)
        .map_err(|e| e.to_string())?;
    let manifest: Manifest =
        serde_json::from_str(&m).map_err(|e| fail("OMEN_PREVIEW_HASH_MISMATCH", e))?;
    let payload_id = manifest.payload_identity()?.to_string();
    let mut archive_binary = Vec::new();
    archive
        .by_name(if cfg!(windows) { "omen.exe" } else { "omen" })
        .map_err(|e| fail("OMEN_PREVIEW_HASH_MISMATCH", e))?
        .read_to_end(&mut archive_binary)
        .map_err(|e| e.to_string())?;
    let daemon_name = if cfg!(windows) { "omend.exe" } else { "omend" };
    let mut archive_daemon = Vec::new();
    let daemon_bytes = match archive.by_name(daemon_name) {
        Ok(mut entry) => {
            entry
                .read_to_end(&mut archive_daemon)
                .map_err(|e| e.to_string())?;
            Some(archive_daemon.as_slice())
        }
        Err(_) => {
            // Packages before Preview 8 ship no daemon executable; install
            // stays compatible and `omen daemon start` keeps its PATH fallback.
            // (v2 packages always ship one; the verifier refuses unrecorded
            // daemons, so a v2 package without a recorded daemon but WITH
            // daemon bytes still fails below.)
            archive_daemon.clear();
            // Re-check: entry missing means no bytes; but a v2 manifest with
            // daemon_binary_sha256 recorded and no shipped daemon must fail.
            // verify_package_contents compares recorded-vs-actual only when
            // bytes exist, so enforce presence here for v2.
            if manifest.schema_version == EMBEDDED_MANIFEST_SCHEMA_V2
                && manifest.daemon_binary_sha256.is_some()
            {
                return Err(fail(
                    "OMEN_PREVIEW_HASH_MISMATCH",
                    "manifest records a daemon the package does not ship",
                ));
            }
            None
        }
    };
    let mut archive_fixtures = std::collections::BTreeMap::new();
    for rel in FIXTURE {
        let mut fixture_bytes = Vec::new();
        archive
            .by_name(&format!("fixture/{rel}"))
            .map_err(|e| fail("OMEN_PREVIEW_HASH_MISMATCH", e))?
            .read_to_end(&mut fixture_bytes)
            .map_err(|e| e.to_string())?;
        archive_fixtures.insert((*rel).to_string(), fixture_bytes);
    }
    verify_package_contents(&manifest, &archive_binary, daemon_bytes, &archive_fixtures)?;
    // Slot identity: opaque after creation; never renamed. For new installs
    // the trailing component is the PAYLOAD digest prefix (v1: the legacy
    // field value, so v1 slot names are unchanged; v2: payload_sha256).
    // This names content identity, not archive identity, and is explicit.
    let slot = format!(
        "{}-{}-{}-{}-{}",
        manifest.preview_version,
        &manifest.git_sha[..7.min(manifest.git_sha.len())],
        match manifest.provenance {
            Provenance::Local => "local",
            Provenance::Ci => "ci",
            Provenance::Release => "release",
        },
        &manifest.binary_sha256[..8],
        &payload_id[..8.min(payload_id.len())]
    );
    let dir = install_root().join("versions").join(&slot);
    if dir.exists() {
        let existing = fs::read_to_string(dir.join("manifest.json")).map_err(|e| e.to_string())?;
        if existing != m {
            return Err(fail("OMEN_INSTALL_SLOT_IDENTITY_CONFLICT", slot));
        }
    } else {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut z = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
        let mut b = Vec::new();
        z.by_name(if cfg!(windows) { "omen.exe" } else { "omen" })
            .unwrap()
            .read_to_end(&mut b)
            .unwrap();
        fs::write(dir.join(if cfg!(windows) { "omen.exe" } else { "omen" }), b)
            .map_err(|e| e.to_string())?;
        fs::write(dir.join("manifest.json"), m).map_err(|e| e.to_string())?;
        if !archive_daemon.is_empty() {
            fs::write(dir.join(daemon_name), &archive_daemon).map_err(|e| e.to_string())?;
        }
        for rel in FIXTURE {
            let mut fixture_bytes = Vec::new();
            z.by_name(&format!("fixture/{rel}"))
                .map_err(|e| fail("OMEN_PREVIEW_HASH_MISMATCH", e))?
                .read_to_end(&mut fixture_bytes)
                .map_err(|e| e.to_string())?;
            let dest = installed_fixture().join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(dest, fixture_bytes).map_err(|e| e.to_string())?;
        }
    }
    fs::create_dir_all(install_root().join("bin")).map_err(|e| e.to_string())?;
    let stable = install_root()
        .join("bin")
        .join(if cfg!(windows) { "omen.exe" } else { "omen" });
    let candidate = dir.join(if cfg!(windows) { "omen.exe" } else { "omen" });
    fs::copy(candidate, &stable).map_err(|e| fail("OMEN_INSTALL_IDENTITY_MISMATCH", e))?;
    // Sibling daemon follows the active slot so `omen daemon start` finds
    // the exact matching omend next to the running binary.
    let stable_daemon = install_root().join("bin").join(daemon_name);
    let candidate_daemon = dir.join(daemon_name);
    if candidate_daemon.exists() {
        fs::copy(candidate_daemon, &stable_daemon)
            .map_err(|e| fail("OMEN_INSTALL_IDENTITY_MISMATCH", e))?;
    }
    let mut s = read_state();
    s.previous_slot = s.active_slot.take();
    s.active_slot = Some(slot.clone());
    s.active = Some(manifest.clone());
    s.ci_green = false;
    write_state(&s)?;
    // H runtime provenance: the conveyor placement IS an Omen-owned managed
    // install, so record canonical ownership for `omen doctor/update`.
    // Channel is preserved when the user already chose one; default Preview.
    // Slot ids are opaque: runtime update/rollback flows resolve
    // `versions/<slot>/omen(.exe)` and never re-derive the name.
    {
        let base = omen_lifecycle::state::state_base_dir();
        let prev_record = omen_lifecycle::install::load_install_record(&base)
            .ok()
            .flatten();
        let channel = prev_record
            .as_ref()
            .map(|r| r.channel)
            .unwrap_or(omen_lifecycle::install::Channel::Preview);
        let mut record = omen_lifecycle::install::InstallRecord::new(
            omen_lifecycle::install::Ownership::Omen,
            channel,
            &manifest.preview_version,
            &manifest.git_sha,
        );
        record.active_slot = Some(slot.clone());
        record.previous_slot = prev_record
            .as_ref()
            .and_then(|r| r.active_slot.clone())
            .or(s.previous_slot.clone());
        // InstallRecord.package_sha256 is the ARTIFACT identity, filled by
        // assign_artifact_identity below from the independently hashed ZIP
        // bytes — never from embedded manifest fields.
        assign_artifact_identity(&mut record, &actual_package_sha256, &payload_id);
        record.binary_sha256 = Some(manifest.binary_sha256.clone());
        if let Err(e) = omen_lifecycle::install::save_install_record(&base, &record) {
            return Err(fail("OMEN_INSTALL_RECORD_FAILED", e));
        }
        if let Err(e) = omen_lifecycle::update::write_active_pointer(&base, &slot) {
            return Err(fail("OMEN_INSTALL_RECORD_FAILED", e));
        }
    }
    println!("INSTALLED: {}", stable.display());
    Ok(())
}

fn prove(_root: &Path, mcp_protocol: &str) -> Result<(), String> {
    let mut s = read_state();
    let m = s
        .active
        .as_ref()
        .ok_or_else(|| fail("OMEN_INSTALL_IDENTITY_MISMATCH", "no installed candidate"))?;
    let binary = active_binary();
    if !binary.exists() {
        return Err(fail("OMEN_INSTALL_IDENTITY_MISMATCH", binary.display()));
    }
    if digest_file(&binary)? != m.binary_sha256 {
        return Err(fail(
            "OMEN_INSTALL_IDENTITY_MISMATCH",
            "active executable digest differs from manifest",
        ));
    }
    let fixture = installed_fixture();
    let before = fixture_hashes_at(&fixture)?;
    let started = Instant::now();
    let orient = run_capture(
        &binary,
        &[
            "orient",
            "--machine",
            "--workspace",
            fixture
                .to_str()
                .ok_or_else(|| fail("OMEN_MCP_PROOF_FAILED", "fixture path is not UTF-8"))?,
        ],
        None,
    )?;
    let orient_cold_ms = started.elapsed().as_millis();
    if !orient.status.success() {
        return Err(fail("OMEN_MCP_PROOF_FAILED", "installed orient failed"));
    }
    let orient_json: serde_json::Value =
        serde_json::from_slice(&orient.stdout).map_err(|e| fail("OMEN_MCP_PROOF_FAILED", e))?;
    if orient_json.get("contract_version").and_then(|v| v.as_str()) != Some(CONTRACT_VERSION) {
        return Err(fail(
            "OMEN_MCP_PROOF_FAILED",
            "installed contract version mismatch",
        ));
    }
    let started = Instant::now();
    let orient_warm = run_capture(
        &binary,
        &[
            "orient",
            "--machine",
            "--workspace",
            fixture
                .to_str()
                .ok_or_else(|| fail("OMEN_MCP_PROOF_FAILED", "fixture path is not UTF-8"))?,
        ],
        None,
    )?;
    let orient_warm_ms = started.elapsed().as_millis();
    if !orient_warm.status.success() {
        return Err(fail("OMEN_MCP_PROOF_FAILED", "warm orient failed"));
    }
    let started = Instant::now();
    let context = run_capture(
        &binary,
        &[
            "context",
            "--machine",
            "--workspace",
            fixture
                .to_str()
                .ok_or_else(|| fail("OMEN_MCP_PROOF_FAILED", "fixture path is not UTF-8"))?,
        ],
        None,
    )?;
    let context_cold_ms = started.elapsed().as_millis();
    if !context.status.success() {
        return Err(fail("OMEN_MCP_PROOF_FAILED", "cold context failed"));
    }
    let started = Instant::now();
    let context_warm = run_capture(
        &binary,
        &[
            "context",
            "--machine",
            "--workspace",
            fixture
                .to_str()
                .ok_or_else(|| fail("OMEN_MCP_PROOF_FAILED", "fixture path is not UTF-8"))?,
        ],
        None,
    )?;
    let context_warm_ms = started.elapsed().as_millis();
    if !context_warm.status.success() {
        return Err(fail("OMEN_MCP_PROOF_FAILED", "warm context failed"));
    }
    let verification_target = install_root().join("evidence").join("proof-target");
    fs::create_dir_all(&verification_target).map_err(|e| e.to_string())?;
    let cargo = Command::new("cargo")
        .args([
            "check",
            "--manifest-path",
            fixture
                .join("Cargo.toml")
                .to_str()
                .ok_or_else(|| fail("OMEN_SEMANTIC_PROOF_FAILED", "fixture path is not UTF-8"))?,
        ])
        .env("CARGO_TARGET_DIR", &verification_target)
        .output()
        .map_err(|e| fail("OMEN_SEMANTIC_PROOF_FAILED", e))?;
    if !cargo.status.success() {
        return Err(fail(
            "OMEN_SEMANTIC_PROOF_FAILED",
            String::from_utf8_lossy(&cargo.stderr),
        ));
    }
    let mcp = mcp_probe(&binary, &fixture, mcp_protocol)?;
    let required = [
        "omen_orient",
        "omen_symbol_search",
        "omen_symbol_definition",
        "omen_symbol_references",
    ];
    let names = mcp
        .tools
        .pointer("/result/tools")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if required.iter().any(|name| !names.contains(name)) {
        return Err(fail(
            "OMEN_MCP_PROOF_FAILED",
            "required tool missing from tools/list",
        ));
    }
    let search_text = serde_json::to_string(&mcp.search).unwrap();
    let definition_text = mcp_text(&mcp.definition);
    let references_text = mcp_text(&mcp.references);
    if !search_text.contains("refresh_token") {
        return Err(fail(
            "OMEN_SEMANTIC_PROOF_FAILED",
            "refresh_token search not found",
        ));
    }
    let definition_json: serde_json::Value = serde_json::from_str(&definition_text)
        .map_err(|e| fail("OMEN_SEMANTIC_PROOF_FAILED", e))?;
    if definition_json
        .pointer("/data/resolved/file")
        .and_then(|v| v.as_str())
        != Some("src/lib.rs")
    {
        return Err(fail(
            "OMEN_SEMANTIC_PROOF_FAILED",
            "refresh_token definition not found",
        ));
    }
    let references_json: serde_json::Value = serde_json::from_str(&references_text)
        .map_err(|e| fail("OMEN_SEMANTIC_PROOF_FAILED", e))?;
    let definition_line = definition_json
        .pointer("/data/resolved/range/start_line")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if !references_json
        .pointer("/data/references")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter().any(|r| {
                r.pointer("/location/file").and_then(|v| v.as_str()) == Some("src/lib.rs")
                    && r.pointer("/location/range/start_line")
                        .and_then(|v| v.as_u64())
                        .map(|line| line > definition_line)
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
    {
        return Err(fail(
            "OMEN_SEMANTIC_PROOF_FAILED",
            "refresh_token references not found",
        ));
    }
    let after = fixture_hashes_at(&fixture)?;
    let efficiency = serde_json::json!({
        "orient_cold_ms": orient_cold_ms,
        "orient_warm_ms": orient_warm_ms,
        "context_cold_ms": context_cold_ms,
        "context_warm_ms": context_warm_ms,
        "symbol_search_ms": mcp.search_ms,
        "definition_ms": mcp.definition_ms,
        "references_ms": mcp.references_ms,
        "provider_starts": 1,
        "discovery_calls_before_semantic": 1,
        "representative_response_bytes": serde_json::to_vec(&mcp.search).unwrap().len()
            + serde_json::to_vec(&mcp.definition).unwrap().len()
            + serde_json::to_vec(&mcp.references).unwrap().len(),
    });
    let changed = before
        .iter()
        .filter(|(k, v)| after.get(*k) != Some(v))
        .count() as u32
        + after.keys().filter(|k| !before.contains_key(*k)).count() as u32;
    if changed != 0 {
        return Err(fail("OMEN_MUTATION_DETECTED", changed));
    }
    let ready = matches!(m.provenance, Provenance::Ci | Provenance::Release) && s.ci_green;
    let p = Proof {
        result: "PASS".into(),
        provenance: Some(m.provenance.clone()),
        git_sha: Some(m.git_sha.clone()),
        timestamp: chrono::Utc::now().to_rfc3339(),
        external_trial_ready: ready,
        changed_files: changed,
    };
    s.last_proof = Some(p.clone());
    write_state(&s)?;
    fs::create_dir_all(install_root().join("evidence")).map_err(|e| e.to_string())?;
    fs::write(proof_path(),serde_json::to_vec_pretty(&serde_json::json!({"schema_version":1,"manifest":m,"identity":"PASS","orient":orient_json,"initialize":mcp.initialize,"tools":mcp.tools,"semantic_search":mcp.search,"semantic_definition":mcp.definition,"semantic_references":mcp.references,"efficiency":efficiency,"fixture":{"changed_files":changed},"readiness":{"local_proof_pass":true,"ready_for_ci":true,"ready_for_external_trial":ready},"proof":p})).unwrap()).map_err(|e|e.to_string())?;
    println!(
        "LOCAL_PROOF_PASS: YES\nREADY_FOR_CI: YES\nREADY_FOR_EXTERNAL_TRIAL: {}\nchanged_files: 0",
        if ready { "YES" } else { "NO" }
    );
    Ok(())
}
fn cycle(root: &Path, mcp_protocol: &str) -> Result<(), String> {
    command_output(root, "gh", &["--version"])?;
    command_output(root, "gh", &["auth", "status"])?;
    let sha = git(root, &["rev-parse", "HEAD"])?;
    let runs_text = command_output(
        root,
        "gh",
        &[
            "run",
            "list",
            "--workflow",
            "ci.yml",
            "--commit",
            &sha,
            "--limit",
            "20",
            "--json",
            "databaseId,status,conclusion,headSha",
        ],
    )?;
    let runs: Vec<serde_json::Value> =
        serde_json::from_str(&runs_text).map_err(|e| fail("OMEN_PREVIEW_CI_NOT_GREEN", e))?;
    let run = runs
        .into_iter()
        .find(|v| {
            v.get("headSha").and_then(|x| x.as_str()) == Some(&sha)
                && v.get("status").and_then(|x| x.as_str()) == Some("completed")
                && v.get("conclusion").and_then(|x| x.as_str()) == Some("success")
        })
        .ok_or_else(|| {
            fail(
                "OMEN_PREVIEW_CI_NOT_GREEN",
                format!("no successful CI run for {sha}"),
            )
        })?;
    let id = run
        .get("databaseId")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| fail("OMEN_PREVIEW_CI_NOT_GREEN", "run has no database id"))?;
    let id_text = id.to_string();
    let jobs_text = command_output(root, "gh", &["run", "view", &id_text, "--json", "jobs"])?;
    let jobs = serde_json::from_str::<serde_json::Value>(&jobs_text)
        .map_err(|e| fail("OMEN_PREVIEW_CI_NOT_GREEN", e))?;
    let names = jobs
        .pointer("/jobs")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter(|j| j.get("conclusion").and_then(|x| x.as_str()) == Some("success"))
                .filter_map(|j| j.get("name").and_then(|x| x.as_str()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let required = [
        "check-and-lint",
        "rust-analyzer-acceptance",
        "matrix-test (ubuntu-latest)",
        "matrix-test (windows-latest)",
        "matrix-test (macos-latest)",
        "h2-gate-e2e (ubuntu-latest)",
        "h2-gate-e2e (windows-latest)",
        "windows-preview-candidate",
    ];
    if required.iter().any(|n| !names.contains(n)) {
        return Err(fail(
            "OMEN_PREVIEW_CI_NOT_GREEN",
            format!("required jobs not green: {required:?}"),
        ));
    }
    let dir = root.join("work").join(format!("cycle-{id}"));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    command_output(
        root,
        "gh",
        &[
            "run",
            "download",
            &id_text,
            "--name",
            "omen-preview-candidate-windows-x86_64",
            "--dir",
            dir.to_str().ok_or_else(|| {
                fail(
                    "OMEN_PREVIEW_ARTIFACT_NOT_FOUND",
                    "download path is not UTF-8",
                )
            })?,
        ],
    )?;
    let artifact = walk_zip(&dir)
        .ok_or_else(|| fail("OMEN_PREVIEW_ARTIFACT_NOT_FOUND", "candidate zip missing"))?;
    install(root, Some(artifact))?;
    let mut state = read_state();
    state.ci_green = true;
    write_state(&state)?;
    prove(root, mcp_protocol)
}
/// Main candidate zip under work/: the CI package, never the
/// gate-companion asset (read_dir order is OS-defined; the companion
/// name always contains "gate-companion").
fn find_main_package(dir: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries {
            let Ok(e) = e else { continue };
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|x| x.to_str()) == Some("zip") {
                out.push(p);
            }
        }
    }
    let mut zips = Vec::new();
    walk(dir, &mut zips);
    zips.sort();
    zips.into_iter().find(|p| {
        !p.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .contains("gate-companion")
    })
}
fn walk_zip(dir: &Path) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()? {
        let p = e.ok()?.path();
        if p.is_dir() {
            if let Some(x) = walk_zip(&p) {
                return Some(x);
            }
        } else if p.extension().and_then(|x| x.to_str()) == Some("zip") {
            return Some(p);
        }
    }
    None
}
fn rollback(_root: &Path) -> Result<(), String> {
    let mut s = read_state();
    let prev = s
        .previous_slot
        .clone()
        .ok_or_else(|| fail("OMEN_INSTALL_IDENTITY_MISMATCH", "no previous slot"))?;
    let dir = install_root().join("versions").join(&prev);
    if !dir.exists() {
        return Err(fail("OMEN_INSTALL_IDENTITY_MISMATCH", prev));
    }
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(dir.join("manifest.json")).map_err(|e| e.to_string())?)
            .map_err(|e| fail("OMEN_INSTALL_IDENTITY_MISMATCH", e))?;
    let candidate = dir.join(if cfg!(windows) { "omen.exe" } else { "omen" });
    if digest_file(&candidate)? != manifest.binary_sha256 {
        return Err(fail(
            "OMEN_INSTALL_IDENTITY_MISMATCH",
            "rollback executable digest differs from manifest",
        ));
    }
    fs::copy(&candidate, active_binary()).map_err(|e| fail("OMEN_INSTALL_BUSY", e))?;
    s.active_slot = Some(prev);
    s.previous_slot = None;
    s.active = Some(manifest);
    write_state(&s)?;
    println!("ROLLBACK: {}", s.active_slot.unwrap());
    Ok(())
}
fn promote(root: &Path, expected: &str) -> Result<(), String> {
    let mut s = read_state();
    let manifest = s
        .active
        .clone()
        .ok_or_else(|| fail("OMEN_PREVIEW_CI_NOT_GREEN", "no active candidate"))?;
    if manifest.git_sha != expected {
        return Err(fail(
            "OMEN_PREVIEW_SHA_MISMATCH",
            "active candidate differs from --expect-sha",
        ));
    }
    if !matches!(manifest.provenance, Provenance::Ci) {
        return Err(fail(
            "OMEN_PREVIEW_CI_NOT_GREEN",
            "only CI candidates can be promoted",
        ));
    }
    if s.last_proof.as_ref().map(|p| p.result.as_str()) != Some("PASS")
        || s.last_proof.as_ref().map(|p| p.external_trial_ready) != Some(true)
    {
        return Err(fail(
            "OMEN_PREVIEW_CI_NOT_GREEN",
            "candidate has no qualifying proof",
        ));
    }
    // Main package: the CI candidate zip (never the gate-companion
    // asset — walk_zip order is OS-defined, so select explicitly).
    let pkg = find_main_package(&root.join("work")).ok_or_else(|| {
        fail(
            "OMEN_PREVIEW_ARTIFACT_NOT_FOUND",
            "proven CI package not found",
        )
    })?;
    let tag = format!("v{}", manifest.preview_version);
    let tag_text = command_output(root, "git", &["tag", "--list", &tag])?;
    if !tag_text.trim().is_empty() {
        return Err(fail(
            "OMEN_PREVIEW_SHA_MISMATCH",
            format!("tag already exists: {tag}"),
        ));
    }
    // The canonical release manifest must exist beside the package: the
    // release publishes BOTH assets from this one authority.
    let release_manifest = root.join("work").join("omen-release.json");
    if !release_manifest.is_file() {
        return Err(fail(
            "OMEN_PREVIEW_ARTIFACT_NOT_FOUND",
            "work/omen-release.json missing; run preview package first",
        ));
    }
    command_output(root, "gh", &["--version"])?;
    command_output(root, "gh", &["auth", "status"])?;
    // Release notes carry the ARCHIVE identity from the release manifest
    // (verified against the packaged ZIP below) — never embedded fields.
    let rel_text = fs::read_to_string(&release_manifest).map_err(|e| e.to_string())?;
    let rel_json: serde_json::Value = serde_json::from_str(&rel_text).map_err(|e| e.to_string())?;
    let archive_sha = rel_json
        .get("package_sha256")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            fail(
                "OMEN_PREVIEW_ARTIFACT_NOT_FOUND",
                "release manifest lacks package_sha256",
            )
        })?;
    // The published bytes must BE the bytes the release manifest vouches
    // for: hash the packaged ZIP independently and require equality.
    // Sourcing the digest from anywhere else (notably the embedded
    // manifest) would reintroduce the payload/archive conflation.
    if digest_file(&pkg)? != archive_sha {
        return Err(fail(
            "OMEN_PREVIEW_SHA_MISMATCH",
            "packaged ZIP digest differs from release manifest package_sha256; refusing to publish",
        ));
    }
    let notes = format!(
        "Preview: {}\nSource SHA: {}\nCI run: {:?}\nArtifact: {:?}\nPackage SHA256: {}\nBinary SHA256: {}",
        manifest.preview_version,
        manifest.git_sha,
        manifest.ci_run_id,
        manifest.artifact_id,
        archive_sha,
        manifest.binary_sha256
    );
    command_output(
        root,
        "gh",
        &[
            "release",
            "create",
            &tag,
            pkg.to_str().ok_or_else(|| {
                fail(
                    "OMEN_PREVIEW_ARTIFACT_NOT_FOUND",
                    "package path is not UTF-8",
                )
            })?,
            release_manifest.to_str().ok_or_else(|| {
                fail(
                    "OMEN_PREVIEW_ARTIFACT_NOT_FOUND",
                    "release manifest path is not UTF-8",
                )
            })?,
            "--target",
            &manifest.git_sha,
            "--prerelease",
            "--title",
            &tag,
            "--notes",
            &notes,
        ],
    )?;
    // H2 companion asset: published beside the main package when the
    // release manifest binds one (built by `package --gate-companion`).
    // Reuses rel_json parsed above: single authority, single read.
    if let Some(companion_asset) = rel_json
        .get("gate_companion_asset")
        .and_then(|v| v.as_str())
    {
        let companion_path = root.join("work").join(companion_asset);
        if !companion_path.is_file() {
            return Err(fail(
                "OMEN_PREVIEW_ARTIFACT_NOT_FOUND",
                format!("bound companion asset missing: {companion_asset}"),
            ));
        }
        command_output(
            root,
            "gh",
            &[
                "release",
                "upload",
                &tag,
                companion_path.to_str().ok_or_else(|| {
                    fail(
                        "OMEN_PREVIEW_ARTIFACT_NOT_FOUND",
                        "companion path is not UTF-8",
                    )
                })?,
            ],
        )?;
        println!("COMPANION_PUBLISHED: {companion_asset}");
    }
    if let Some(active) = s.active.as_mut() {
        active.provenance = Provenance::Release;
    }
    if let Some(proof) = s.last_proof.as_mut() {
        proof.provenance = Some(Provenance::Release);
    }
    write_state(&s)?;
    println!("PROMOTED: {tag}\nREBUILD: NO");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_increment_is_deterministic() {
        assert_eq!(parse_preview("0.8.0-preview.3").unwrap() + 1, 4);
        assert_eq!(parse_preview("0.9.0-preview.1").unwrap(), 1);
    }
    #[test]
    fn malformed_version_refused() {
        assert!(parse_preview("0.8.0").is_err());
    }
    #[test]
    fn manifest_roundtrips_provenance() {
        let m = Manifest {
            schema_version: 1,
            provenance: Provenance::Local,
            preview_version: "0.8.0-preview.3".into(),
            git_sha: "abc".into(),
            contract_version: "0.8".into(),
            target: "x86_64".into(),
            profile: "release".into(),
            binary_sha256: "bin".into(),
            package_sha256: Some("pkg".into()),
            payload_sha256: None,
            ci_run_id: None,
            artifact_id: None,
            fixture_files: Default::default(),
            daemon_binary_sha256: Some("daemon".into()),
        };
        assert_eq!(
            serde_json::from_str::<Manifest>(&serde_json::to_string(&m).unwrap()).unwrap(),
            m
        );
        // Pre-Preview-8 manifests without the daemon field still parse.
        let legacy = serde_json::json!({
            "schema_version": 1,
            "provenance": "ci",
            "preview_version": "0.9.0-preview.7",
            "git_sha": "abc",
            "contract_version": "0.8",
            "target": "x86_64",
            "profile": "release",
            "binary_sha256": "bin",
            "package_sha256": "pkg",
            "ci_run_id": null,
            "artifact_id": null,
            "fixture_files": {},
        });
        let parsed: Manifest = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.daemon_binary_sha256, None);
        assert_eq!(parsed.payload_sha256, None);
    }

    fn synthetic_v2_manifest() -> (Manifest, Vec<u8>, Vec<u8>, Vec<u8>) {
        // Fixed synthetic payload: binary + daemon + one fixture.
        let binary = b"OMEN-BINARY".to_vec();
        let daemon = b"OMEND-BINARY".to_vec();
        let fixture = b"FIXTURE-BYTES".to_vec();
        let bin_hash = hex::encode(Sha256::digest(&binary));
        let daemon_hash = hex::encode(Sha256::digest(&daemon));
        let fixture_hash = hex::encode(Sha256::digest(&fixture));
        let mut fixtures = std::collections::BTreeMap::new();
        fixtures.insert("Cargo.toml".to_string(), fixture_hash);
        let m = new_v2_manifest(
            Provenance::Ci,
            "0.9.0-preview.23".into(),
            "a".repeat(40),
            "x86_64-pc-windows-msvc".into(),
            bin_hash,
            Some(daemon_hash),
            fixtures,
            Some(1),
            Some(2),
        );
        (m, binary, daemon, fixture)
    }

    fn synthetic_fixture_map(fixture: &[u8]) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut map = std::collections::BTreeMap::new();
        map.insert("Cargo.toml".to_string(), fixture.to_vec());
        map
    }

    // 1. New schema-v2 embedded manifest emits payload_sha256.
    #[test]
    fn v2_manifest_emits_payload_identity() {
        let (m, _, _, _) = synthetic_v2_manifest();
        assert_eq!(m.schema_version, EMBEDDED_MANIFEST_SCHEMA_V2);
        let id = m.payload_identity().unwrap();
        assert_eq!(id, m.payload_sha256.as_deref().unwrap());
        assert_eq!(
            id,
            &payload_digest_v2(
                &m.binary_sha256,
                m.daemon_binary_sha256.as_deref(),
                &m.fixture_files
            )
        );
    }

    // 2. New embedded manifest does NOT claim package_sha256.
    #[test]
    fn v2_manifest_carries_no_package_field() {
        let (m, _, _, _) = synthetic_v2_manifest();
        assert_eq!(m.package_sha256, None);
        let json = serde_json::to_value(&m).unwrap();
        assert!(
            json.get("package_sha256").is_none(),
            "v2 JSON must not contain package_sha256"
        );
        assert!(json.get("payload_sha256").is_some());
    }

    // 3. v2 payload digest verifies successfully.
    #[test]
    fn v2_payload_verifies() {
        let (m, binary, daemon, fixture) = synthetic_v2_manifest();
        verify_package_contents(&m, &binary, Some(&daemon), &synthetic_fixture_map(&fixture))
            .unwrap();
    }

    // 4. Mutated omen binary fails payload/binary verification.
    #[test]
    fn mutated_binary_fails_verification() {
        let (m, _, daemon, fixture) = synthetic_v2_manifest();
        let err = verify_package_contents(
            &m,
            b"TAMPERED-BINARY",
            Some(&daemon),
            &synthetic_fixture_map(&fixture),
        )
        .expect_err("mutated binary must fail");
        assert!(err.contains("OMEN_PREVIEW_HASH_MISMATCH"), "got: {err}");
    }

    // 5. Mutated omend binary fails verification.
    #[test]
    fn mutated_daemon_fails_verification() {
        let (m, binary, _, fixture) = synthetic_v2_manifest();
        let err = verify_package_contents(
            &m,
            &binary,
            Some(b"TAMPERED-DAEMON"),
            &synthetic_fixture_map(&fixture),
        )
        .expect_err("mutated daemon must fail");
        assert!(err.contains("OMEN_PREVIEW_HASH_MISMATCH"), "got: {err}");
    }

    // 6. Mutated fixture fails verification.
    #[test]
    fn mutated_fixture_fails_verification() {
        let (m, binary, daemon, _) = synthetic_v2_manifest();
        let err = verify_package_contents(
            &m,
            &binary,
            Some(&daemon),
            &synthetic_fixture_map(b"TAMPERED-FIXTURE"),
        )
        .expect_err("mutated fixture must fail");
        assert!(err.contains("OMEN_PREVIEW_HASH_MISMATCH"), "got: {err}");
    }

    // 7+8. Actual ZIP SHA equals the external release-manifest value and is
    // a different ontology from the payload digest: build a synthetic
    // package ZIP, hash its bytes, record the hash release-style, and
    // require equality — while the payload identity differs.
    #[test]
    fn archive_identity_is_release_hash_not_payload() {
        use std::io::Write;
        let (m, binary, daemon, fixture) = synthetic_v2_manifest();
        let payload_id = m.payload_identity().unwrap().to_string();
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("pkg.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("omen.exe", opts).unwrap();
            zip.write_all(&binary).unwrap();
            zip.start_file("omend.exe", opts).unwrap();
            zip.write_all(&daemon).unwrap();
            zip.start_file("manifest.json", opts).unwrap();
            zip.write_all(serde_json::to_vec(&m).unwrap().as_slice())
                .unwrap();
            zip.start_file("fixture/Cargo.toml", opts).unwrap();
            zip.write_all(&fixture).unwrap();
            zip.finish().unwrap();
        }
        let archive_sha = digest_file(&zip_path).unwrap();
        // Release-manifest law: package_sha256 IS the archive bytes digest.
        let release = serde_json::json!({
            "schema_version": 1,
            "package_sha256": archive_sha,
            "payload_sha256": payload_id,
        });
        assert_eq!(
            digest_file(&zip_path).unwrap(),
            release
                .get("package_sha256")
                .and_then(|v| v.as_str())
                .unwrap(),
            "actual ZIP SHA must equal release-manifest package_sha256"
        );
        // Ontology law: payload identity and archive identity differ for any
        // real package (different bytes hashed under different encodings).
        assert_ne!(
            payload_id, archive_sha,
            "payload and archive digests must be different values"
        );
    }

    // 9. Installer records actual ZIP SHA in InstallRecord.package_sha256.
    #[test]
    fn install_record_carries_archive_identity() {
        let mut record = omen_lifecycle::install::InstallRecord::new(
            omen_lifecycle::install::Ownership::Omen,
            omen_lifecycle::install::Channel::Preview,
            "0.9.0-preview.23",
            &"a".repeat(40),
        );
        assign_artifact_identity(&mut record, "ARCHIVE-SHA", "PAYLOAD-ID");
        assert_eq!(record.package_sha256.as_deref(), Some("ARCHIVE-SHA"));
        assert_eq!(record.payload_sha256.as_deref(), Some("PAYLOAD-ID"));
        assert_ne!(
            record.package_sha256, record.payload_sha256,
            "the two identities must never share one value by construction"
        );
    }

    // 10. Legacy schema-v1 Preview package remains installable: parses and
    // verifies through the preserved legacy path.
    #[test]
    fn legacy_v1_manifest_verifies_legacy_path() {
        let binary = b"OMEN-BINARY-V1".to_vec();
        let bin_hash = hex::encode(Sha256::digest(&binary));
        let mut fixtures = std::collections::BTreeMap::new();
        fixtures.insert("Cargo.toml".to_string(), hex::encode(Sha256::digest(b"F")));
        let legacy_field = payload_digest(&bin_hash, &fixtures);
        let json = serde_json::json!({
            "schema_version": 1,
            "provenance": "ci",
            "preview_version": "0.9.0-preview.22",
            "git_sha": "b".repeat(40),
            "contract_version": "0.8",
            "target": "x86_64-pc-windows-msvc",
            "profile": "release",
            "binary_sha256": bin_hash,
            "package_sha256": legacy_field,
            "ci_run_id": null,
            "artifact_id": null,
            "fixture_files": fixtures,
        });
        let m: Manifest = serde_json::from_value(json).unwrap();
        assert_eq!(m.payload_identity().unwrap(), legacy_field);
        // Legacy verification is content-agnostic for fixtures (exact v1
        // semantics): binary bytes must match, legacy field must match.
        verify_package_contents(&m, &binary, None, &std::collections::BTreeMap::new()).unwrap();
    }

    // 11. Legacy v1 `package_sha256` is payload identity, NOT archive
    // identity: the identity accessor returns the legacy field, which must
    // not equal an enclosing archive's digest.
    #[test]
    fn legacy_field_is_payload_not_archive() {
        let binary = b"OMEN-BINARY-V1".to_vec();
        let bin_hash = hex::encode(Sha256::digest(&binary));
        let mut fixtures = std::collections::BTreeMap::new();
        fixtures.insert("Cargo.toml".to_string(), hex::encode(Sha256::digest(b"F")));
        let legacy_field = payload_digest(&bin_hash, &fixtures);
        let m = Manifest {
            schema_version: EMBEDDED_MANIFEST_SCHEMA_V1,
            provenance: Provenance::Ci,
            preview_version: "0.9.0-preview.22".into(),
            git_sha: "b".repeat(40),
            contract_version: "0.8".into(),
            target: "x".into(),
            profile: "release".into(),
            binary_sha256: bin_hash,
            package_sha256: Some(legacy_field.clone()),
            payload_sha256: None,
            ci_run_id: None,
            artifact_id: None,
            fixture_files: fixtures,
            daemon_binary_sha256: None,
        };
        // The accessor exposes the legacy value AS payload identity...
        assert_eq!(m.payload_identity().unwrap(), legacy_field);
        // ...and that value is definitionally not an archive digest: it was
        // computed over digests-of-contents, never over ZIP bytes.
        let fake_archive_sha = hex::encode(Sha256::digest(b"ZIP-BYTES"));
        assert_ne!(m.payload_identity().unwrap(), fake_archive_sha);
    }

    // Digest-algorithm regression vectors (independent preimages hashed
    // outside this codebase): legacy v1 pins byte-exact preservation, v2
    // pins the canonical encoding. Inputs: binary "BINHASH", one fixture
    // "Cargo.toml" -> "CHASH", daemon "DAEMONHASH" where present.
    #[test]
    fn digest_regression_vectors() {
        let mut fixtures = std::collections::BTreeMap::new();
        fixtures.insert("Cargo.toml".to_string(), "CHASH".to_string());
        assert_eq!(
            payload_digest("BINHASH", &fixtures),
            "0f8b32c83fd25421ce6885c94427b085852461c91752eab23411f0462381e4ff",
            "legacy v1 algorithm must never change"
        );
        assert_eq!(
            payload_digest_v2("BINHASH", Some("DAEMONHASH"), &fixtures),
            "63fd219144f321ab181ca732efbe678a40e526624349016ffe38d5735e1b6e53",
            "v2 canonical encoding must never change silently"
        );
        assert_eq!(
            payload_digest_v2("BINHASH", None, &std::collections::BTreeMap::new()),
            "52ce1721df690fc808b2c48f52dd958ea8d215ca0003236bd3d14ab0c89e3dd8",
            "v2 daemon-absent encoding must never change silently"
        );
        assert_ne!(
            payload_digest("BINHASH", &fixtures),
            payload_digest_v2("BINHASH", Some("DAEMONHASH"), &fixtures),
            "v1 and v2 are distinct algorithms by design"
        );
    }

    // Unknown embedded schema versions fail closed instead of guessing.
    #[test]
    fn unknown_schema_version_refused() {
        let (mut m, binary, daemon, fixture) = synthetic_v2_manifest();
        m.schema_version = 99;
        assert!(m.payload_identity().is_err());
        assert!(
            verify_package_contents(&m, &binary, Some(&daemon), &synthetic_fixture_map(&fixture))
                .is_err()
        );
    }

    // v2 manifests smuggling the legacy archive-name field are refused:
    // an archive SHA can never truthfully sit inside its own archive.
    #[test]
    fn v2_with_package_field_refused() {
        let (mut m, _binary, _daemon, _fixture) = synthetic_v2_manifest();
        m.package_sha256 = Some("smuggled".into());
        assert!(m.payload_identity().is_err());
    }
    #[test]
    fn local_never_external() {
        assert!(matches!(Provenance::Local, Provenance::Local));
    }
}
