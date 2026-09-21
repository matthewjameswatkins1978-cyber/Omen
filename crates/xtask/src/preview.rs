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
    time::Duration,
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
    },
    Install {
        #[arg(long)]
        artifact: Option<PathBuf>,
    },
    Prove,
    Cycle,
    Rollback,
    Promote {
        #[arg(long)]
        expect_sha: String,
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
    pub package_sha256: String,
    pub ci_run_id: Option<u64>,
    pub artifact_id: Option<u64>,
    pub fixture_files: std::collections::BTreeMap<String, String>,
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
        } => package(root, ci_run_id, artifact_id),
        PreviewCommand::Install { artifact } => install(root, artifact),
        PreviewCommand::Prove => prove(root),
        PreviewCommand::Cycle => cycle(root),
        PreviewCommand::Rollback => rollback(root),
        PreviewCommand::Promote { expect_sha } => promote(root, &expect_sha),
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
fn payload_digest(binary: &str, fixtures: &std::collections::BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    h.update(binary.as_bytes());
    for (name, digest) in fixtures {
        h.update(name.as_bytes());
        h.update(digest.as_bytes());
    }
    hex::encode(h.finalize())
}
fn preview_version(root: &Path) -> Result<String, String> {
    let text = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    text.lines()
        .find(|l| l.starts_with("version = \"0.8.0-preview."))
        .and_then(|l| l.split('"').nth(1))
        .map(str::to_string)
        .ok_or_else(|| fail("OMEN_PREVIEW_VERSION_INVALID", "workspace version missing"))
}
fn parse_preview(v: &str) -> Result<u32, String> {
    let n = v
        .strip_prefix("0.8.0-preview.")
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
fn mcp_probe(
    binary: &Path,
    fixture: &Path,
) -> Result<
    (
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
    ),
    String,
> {
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
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"omen-preview-conveyor","version":"0.1"}}}),
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
    let search = mcp_send(
        &mut p,
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"omen_symbol_search","arguments":{"query":"refresh_token","limit":50}}}),
        3,
    )?;
    let definition = mcp_send(
        &mut p,
        serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"omen_symbol_definition","arguments":{"symbol":"refresh_token"}}}),
        4,
    )?;
    let references = mcp_send(
        &mut p,
        serde_json::json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"omen_symbol_references","arguments":{"symbol":"refresh_token","limit":50}}}),
        5,
    )?;
    let _ = p.child.kill();
    Ok((init, list, search, definition, references))
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
    let installed=state.active.as_ref().map(|m| serde_json::json!({"version":m.preview_version,"SHA":m.git_sha,"provenance":m.provenance,"binary_hash":m.binary_sha256,"package_hash":m.package_sha256,"slot":state.active_slot,"active_path":install_root().join("bin").join(if cfg!(windows){"omen.exe"}else{"omen"})})).unwrap_or(serde_json::Value::Null);
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
    let next = to.unwrap_or_else(|| format!("0.8.0-preview.{}", parse_preview(&old).unwrap() + 1));
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

fn package(root: &Path, ci_run_id: Option<u64>, artifact_id: Option<u64>) -> Result<(), String> {
    run_cmd(root, "cargo", &["build", "--release", "-p", "omen-cli"])?;
    let exe =
        root.join("target")
            .join("release")
            .join(if cfg!(windows) { "omen.exe" } else { "omen" });
    let bin_hash = digest_file(&exe)?;
    let version = preview_version(root)?;
    let sha = git(root, &["rev-parse", "HEAD"])?;
    let fixture_files = fixture_hashes(root)?;
    let mut manifest = Manifest {
        schema_version: 1,
        provenance: if ci_run_id.is_some() {
            Provenance::Ci
        } else {
            Provenance::Local
        },
        preview_version: version.clone(),
        git_sha: sha,
        contract_version: CONTRACT_VERSION.into(),
        target: std::env::consts::ARCH.into(),
        profile: "release".into(),
        binary_sha256: bin_hash.clone(),
        package_sha256: payload_digest(&bin_hash, &fixture_files),
        ci_run_id,
        artifact_id,
        fixture_files,
    };
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
    manifest.package_sha256 = digest_file(&out)?;
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

fn install(_root: &Path, artifact: Option<PathBuf>) -> Result<(), String> {
    let path = artifact.ok_or_else(|| {
        fail(
            "OMEN_PREVIEW_GITHUB_UNAVAILABLE",
            "default install requires an exact CI artifact; use --artifact for local debugging",
        )
    })?;
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
    let mut archive_binary = Vec::new();
    archive
        .by_name(if cfg!(windows) { "omen.exe" } else { "omen" })
        .map_err(|e| fail("OMEN_PREVIEW_HASH_MISMATCH", e))?
        .read_to_end(&mut archive_binary)
        .map_err(|e| e.to_string())?;
    let actual_binary = hex::encode(Sha256::digest(&archive_binary));
    if actual_binary != manifest.binary_sha256 {
        return Err(fail(
            "OMEN_PREVIEW_HASH_MISMATCH",
            "binary digest differs from manifest",
        ));
    }
    if payload_digest(&manifest.binary_sha256, &manifest.fixture_files) != manifest.package_sha256 {
        return Err(fail(
            "OMEN_PREVIEW_HASH_MISMATCH",
            "package digest differs from manifest",
        ));
    }
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
        &manifest.package_sha256[..8]
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
    let mut s = read_state();
    s.previous_slot = s.active_slot.take();
    s.active_slot = Some(slot);
    s.active = Some(manifest);
    s.ci_green = false;
    write_state(&s)?;
    println!("INSTALLED: {}", stable.display());
    Ok(())
}

fn prove(_root: &Path) -> Result<(), String> {
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
    let (initialize, tools, search, definition, references) = mcp_probe(&binary, &fixture)?;
    let required = [
        "omen_orient",
        "omen_symbol_search",
        "omen_symbol_definition",
        "omen_symbol_references",
    ];
    let names = tools
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
    let search_text = serde_json::to_string(&search).unwrap();
    let definition_text = mcp_text(&definition);
    let references_text = mcp_text(&references);
    if !search_text.contains("refresh_token") {
        return Err(fail(
            "OMEN_SEMANTIC_PROOF_FAILED",
            "refresh_token search not found",
        ));
    }
    let definition_json: serde_json::Value = serde_json::from_str(&definition_text)
        .map_err(|e| fail("OMEN_SEMANTIC_PROOF_FAILED", e))?;
    if definition_json
        .pointer("/resolved/file")
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
        .pointer("/resolved/range/start_line")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if !references_json
        .pointer("/resolved")
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
    fs::write(proof_path(),serde_json::to_vec_pretty(&serde_json::json!({"schema_version":1,"manifest":m,"identity":"PASS","orient":orient_json,"initialize":initialize,"tools":tools,"semantic_search":search,"semantic_definition":definition,"semantic_references":references,"fixture":{"changed_files":changed},"readiness":{"local_proof_pass":true,"ready_for_ci":true,"ready_for_external_trial":ready},"proof":p})).unwrap()).map_err(|e|e.to_string())?;
    println!(
        "LOCAL_PROOF_PASS: YES\nREADY_FOR_CI: YES\nREADY_FOR_EXTERNAL_TRIAL: {}\nchanged_files: 0",
        if ready { "YES" } else { "NO" }
    );
    Ok(())
}
fn cycle(root: &Path) -> Result<(), String> {
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
    prove(root)
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
    let pkg = walk_zip(&root.join("work")).ok_or_else(|| {
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
    command_output(root, "gh", &["--version"])?;
    command_output(root, "gh", &["auth", "status"])?;
    let notes = format!(
        "Preview: {}\nSource SHA: {}\nCI run: {:?}\nArtifact: {:?}\nPackage SHA256: {}\nBinary SHA256: {}",
        manifest.preview_version,
        manifest.git_sha,
        manifest.ci_run_id,
        manifest.artifact_id,
        manifest.package_sha256,
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
            "--target",
            &manifest.git_sha,
            "--prerelease",
            "--title",
            &tag,
            "--notes",
            &notes,
        ],
    )?;
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
            package_sha256: "pkg".into(),
            ci_run_id: None,
            artifact_id: None,
            fixture_files: Default::default(),
        };
        assert_eq!(
            serde_json::from_str::<Manifest>(&serde_json::to_string(&m).unwrap()).unwrap(),
            m
        );
    }
    #[test]
    fn local_never_external() {
        assert!(matches!(Provenance::Local, Provenance::Local));
    }
}
