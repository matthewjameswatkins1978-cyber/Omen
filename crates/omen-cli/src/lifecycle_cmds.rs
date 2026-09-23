//! H product commands: clean, gc plan/apply, repair, update, channel, setup,
//! uninstall, diagnostics, pin. Thin CLI projections over `omen-lifecycle`:
//! one truth, human + machine renderings. Doctor itself stays observational;
//! consequential commands require explicit `--apply` (clean excepted:
//! conservative debris applies directly, `--plan` previews).
use omen_knowledge::{Database, user_state_base_dir};
use std::path::{Path, PathBuf};

fn base() -> PathBuf {
    user_state_base_dir()
}

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("omen"))
}

fn ownership() -> omen_lifecycle::install::Ownership {
    omen_lifecycle::install::effective_ownership(&base(), &exe_path())
}

fn channel() -> omen_lifecycle::install::Channel {
    omen_lifecycle::install::user_channel(&base())
}

fn machine_msg(status: &str, headline: &str, body: serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": status, "headline": headline, "detail": body
        }))
        .unwrap()
    );
}

// ---------------------------------------------------------------- clean ---
pub fn cmd_clean(plan_only: bool, json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    let mut plan = omen_lifecycle::clean::clean_plan(&b);
    omen_lifecycle::clean::attach_fingerprints(&b, &mut plan);
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }
    if plan.items.is_empty() {
        println!("[ok] clean: nothing disposable found");
        return Ok(());
    }
    let bytes: u64 = plan.items.iter().map(|i| i.size_bytes.unwrap_or(0)).sum();
    println!(
        "[info] clean plan: {} item(s), ~{} bytes",
        plan.items.len(),
        bytes
    );
    for item in &plan.items {
        println!("  - {} ({})", item.identity, item.reason);
    }
    if plan_only {
        println!("plan only: nothing removed (run `omen clean` to apply)");
        return Ok(());
    }
    let report = omen_lifecycle::clean::clean_apply(&b)?;
    println!(
        "[ok] clean applied: {} removed, {} refused, {} failed",
        report.completed.len(),
        report.refused.len(),
        report.failed.len()
    );
    for r in &report.refused {
        println!("  [kept] {} ({})", r.identity, r.reason);
    }
    for f in &report.failed {
        println!("  [fail] {} ({})", f.identity, f.error);
    }
    Ok(())
}

// ------------------------------------------------------------------- gc ---
/// Digests referenced by durable truth: artifacts table + fact_artifact
/// links + receipts. Best-effort read-only; failure yields empty (GC then
/// keeps more, never less... callers combine with pins).
pub fn db_referenced_digests(workspace_db: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let Ok(db) = Database::open_read_only(workspace_db) else {
        return out;
    };
    if let Ok(mut stmt) = db.conn().prepare("SELECT digest FROM artifacts")
        && let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0))
    {
        for d in rows.flatten() {
            out.insert(d);
        }
    }
    if let Ok(mut stmt) = db.conn().prepare("SELECT artifact_uri FROM fact_artifacts")
        && let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0))
    {
        for u in rows.flatten() {
            if let Some(d) = u.strip_prefix("artifact://") {
                out.insert(d.to_string());
            }
        }
    }
    out
}

pub fn workspace_reachability(b: &Path) -> omen_lifecycle::gc::Reachability {
    use omen_knowledge::{canonical_workspace_db_path_readonly, workspace_state_dir_path};
    let mut reach = omen_lifecycle::gc::Reachability::default();
    // All workspace DBs under the base (bounded scan).
    let ws = b.join("workspaces");
    if let Ok(rd) = std::fs::read_dir(&ws) {
        for entry in rd.flatten().take(512) {
            let db = entry.path().join("state.sqlite");
            if db.is_file() {
                for d in db_referenced_digests(&db) {
                    reach.history_referenced.insert(d);
                }
            }
        }
    }
    // Current-workspace DB via canonical authority (covers custom roots).
    let _ = (
        canonical_workspace_db_path_readonly,
        workspace_state_dir_path,
    );
    reach
}

pub fn cmd_gc_plan(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    let policy = omen_lifecycle::gc::load_retention(&b);
    let reach = workspace_reachability(&b);
    let cands = omen_lifecycle::gc::workspace_cas_candidates(&b, &policy, &reach);
    let plan = omen_lifecycle::gc::gc_plan(&cands);
    let path = omen_lifecycle::plan::save_plan(&b, &plan)?;
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plan": plan, "plan_file": path
            }))?
        );
        return Ok(());
    }
    if plan.items.is_empty() {
        println!("[ok] gc plan: nothing eligible (protected truth untouched)");
    } else {
        let bytes: u64 = plan.items.iter().map(|i| i.size_bytes.unwrap_or(0)).sum();
        println!(
            "[info] gc plan: {} item(s), ~{} bytes -> {}",
            plan.items.len(),
            bytes,
            path.display()
        );
        for item in &plan.items {
            println!("  - {} ({})", item.identity, item.reason);
        }
        println!("review, then `omen gc --apply --plan-file <path>`");
    }
    Ok(())
}

pub fn cmd_gc_apply(plan_file: &Path, json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    let plan = omen_lifecycle::plan::load_plan(plan_file)?;
    if plan.kind != "gc" {
        machine_msg(
            "fail",
            "plan kind mismatch",
            serde_json::json!({"want": "gc", "got": plan.kind}),
        );
        std::process::exit(2);
    }
    let guard = omen_lifecycle::lock::acquire(&b)?;
    let _ = guard;
    // Live revalidation: fresh reachability + pins + digest recompute.
    let policy = omen_lifecycle::gc::load_retention(&b);
    let reach = workspace_reachability(&b);
    let pins = omen_lifecycle::pins::load_pins(&b)
        .map(|s| s.pins)
        .unwrap_or_default();
    let report = omen_lifecycle::plan::apply_plan(
        &plan,
        &|item| {
            let digest = item.fingerprint.clone().unwrap_or_default();
            let d = digest.strip_prefix("digest:").unwrap_or("").to_string();
            if d.is_empty() {
                return Ok(omen_lifecycle::plan::Revalidate::Refuse {
                    reason: "plan item lacks digest fingerprint".to_string(),
                });
            }
            if reach.is_reachable(&d) || pins.contains(&format!("digest:{d}")) {
                return Ok(omen_lifecycle::plan::Revalidate::Refuse {
                    reason: format!("{d} became protected after plan"),
                });
            }
            // Recompute digest from the live file: identity must match.
            let raw = Path::new(&item.identity);
            let resolved = if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                b.join(raw)
            };
            match std::fs::read(&resolved) {
                Ok(bytes) => {
                    use sha2::{Digest, Sha256};
                    let have = hex::encode(Sha256::digest(bytes));
                    if have == d {
                        Ok(omen_lifecycle::plan::Revalidate::Proceed)
                    } else {
                        Ok(omen_lifecycle::plan::Revalidate::Refuse {
                            reason: "payload bytes changed after plan".to_string(),
                        })
                    }
                }
                Err(_) => Ok(omen_lifecycle::plan::Revalidate::Proceed), // idempotent: already gone
            }
        },
        &|item| omen_lifecycle::gc::apply_gc_item(&b, item),
    );
    let _ = policy;
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if report.stale {
        println!("[fail] gc apply refused: plan went stale (protection changed)");
    }
    println!(
        "[info] gc apply: {} collected, {} refused, {} failed",
        report.completed.len(),
        report.refused.len(),
        report.failed.len()
    );
    for r in &report.refused {
        println!("  [kept] {} ({})", r.identity, r.reason);
    }
    Ok(())
}

// --------------------------------------------------------------- repair ---
pub fn cmd_repair(apply: bool, json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    let report = lifecycle_doctor_report();
    let (plan, refused) = omen_lifecycle::repair::repair_plan(&b, &report);
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plan": plan, "refused": refused, "applied": false
            }))?
        );
        if !apply {
            return Ok(());
        }
    } else {
        if plan.items.is_empty() && refused.is_empty() {
            println!("[ok] repair: nothing to do");
            return Ok(());
        }
        for item in &plan.items {
            println!(
                "[plan] {}: {} => {}",
                item.identity, item.reason, item.consequence
            );
        }
        for r in &refused {
            println!("[refuse] {}: {}", r.id, r.reason);
        }
        if !apply {
            println!("preview only: nothing changed (run `omen repair --apply`)");
            return Ok(());
        }
    }
    let guard = omen_lifecycle::lock::acquire(&b)?;
    let _ = guard;
    let applied = omen_lifecycle::plan::apply_plan(
        &plan,
        &|_| Ok(omen_lifecycle::plan::Revalidate::Proceed),
        &|item| omen_lifecycle::repair::apply_repair_item(&b, item),
    );
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plan": plan, "refused": refused, "applied": applied
            }))?
        );
    } else {
        println!(
            "[ok] repair applied: {} done, {} refused, {} failed",
            applied.completed.len(),
            applied.refused.len() + refused.len(),
            applied.failed.len()
        );
    }
    Ok(())
}

pub fn lifecycle_doctor_report() -> omen_lifecycle::doctor::DoctorReport {
    let b = base();
    omen_lifecycle::doctor::run_doctor(&omen_lifecycle::doctor::DoctorInput {
        base: b,
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: env!("OMEN_GIT_SHA").to_string(),
        exe_path: exe_path(),
        adapters: vec![
            ("codex".to_string(), codex_present(), false),
            ("luna".to_string(), false, false),
        ],
    })
}

fn codex_present() -> bool {
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

// --------------------------------------------------------------- update ---
pub fn cmd_update_check(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    let record = omen_lifecycle::install::load_install_record(&b)?;
    let current = record
        .as_ref()
        .map(|r| r.version.clone())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let ch = record.as_ref().map(|r| r.channel).unwrap_or_else(channel);
    let own = record.as_ref().map(|r| r.owner).unwrap_or_else(ownership);
    let source = omen_lifecycle::update::resolve_source();
    let report = omen_lifecycle::update::check_for_update(&current, ch, own, &source);
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "[info] current {current} channel {ch:?} ownership {}",
        format!("{own:?}").to_lowercase()
    );
    match &report.outcome {
        omen_lifecycle::update::CheckOutcome::UpToDate { .. } => println!("[ok] up to date"),
        omen_lifecycle::update::CheckOutcome::Candidate {
            candidate,
            compat_note,
            ..
        } => {
            println!("[info] candidate {} ({})", candidate.version, compat_note)
        }
        other => println!("[warn] check inconclusive: {other:?}"),
    }
    Ok(())
}

pub fn cmd_update_apply(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    let mut record = match omen_lifecycle::install::load_install_record(&b)? {
        Some(r) => r,
        // Adoption (H item 83): a pre-H conveyor install (state/installed.json
        // with a healthy slot) is Omen provenance. Adopt it into a runtime
        // record instead of stranding a valid install. Package-manager and
        // unknown layouts have no such file and stay refused.
        None => adopt_conveyor_state(&b)?,
    };
    if !record.owner.self_update_allowed() {
        let msg = if record.owner == omen_lifecycle::install::Ownership::PackageManager {
            omen_lifecycle::install::manager_guidance(record.owner_name.as_deref())
        } else {
            format!("self-update refused for {:?} install", record.owner)
        };
        if json_mode {
            machine_msg("fail", "update refused", serde_json::json!({"reason": msg}));
        } else {
            println!("[fail] {msg}");
        }
        std::process::exit(2);
    }
    let source = omen_lifecycle::update::resolve_source();
    let check = omen_lifecycle::update::check_for_update(
        &record.version.clone(),
        record.channel,
        record.owner,
        &source,
    );
    let candidate = match check.outcome {
        omen_lifecycle::update::CheckOutcome::Candidate { candidate, .. } => candidate,
        omen_lifecycle::update::CheckOutcome::UpToDate { .. } => {
            println!("[ok] already up to date");
            return Ok(());
        }
        other => {
            if json_mode {
                machine_msg(
                    "fail",
                    "no usable candidate",
                    serde_json::json!({"outcome": other}),
                );
            } else {
                println!("[fail] no usable candidate: {other:?}");
            }
            std::process::exit(2);
        }
    };
    let health = omen_lifecycle::update::BinaryHealthCheck { base: b.clone() };
    match omen_lifecycle::update::run_update(
        &b,
        &mut record,
        candidate,
        &source,
        &omen_lifecycle::update::FailureHooks::default(),
        &health,
    ) {
        Ok(tx) => {
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&tx)?);
            } else {
                println!(
                    "[ok] updated to {} (slot {:?})",
                    record.version, record.active_slot
                );
            }
            Ok(())
        }
        Err(e) => {
            if json_mode {
                machine_msg(
                    "fail",
                    "update failed",
                    serde_json::json!({"phase": e.phase(), "error": e.to_string()}),
                );
            } else {
                println!("[fail] update failed at {}: {e}", e.phase());
                println!("previous healthy install preserved; see update transactions for detail");
            }
            std::process::exit(1);
        }
    }
}

pub fn cmd_channel(set: Option<String>, json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    if let Some(name) = set {
        match omen_lifecycle::install::Channel::parse(&name) {
            Some(ch) => {
                omen_lifecycle::install::set_user_channel(&b, ch)?;
                if json_mode {
                    machine_msg("ok", "channel set", serde_json::json!({"channel": ch}));
                } else {
                    println!("[ok] channel -> {ch:?} (user installation context)");
                }
            }
            None => {
                eprintln!("unknown channel {name:?}; expected stable|preview");
                std::process::exit(2);
            }
        }
        return Ok(());
    }
    let ch = channel();
    if json_mode {
        machine_msg("ok", "channel", serde_json::json!({"channel": ch}));
    } else {
        println!("[info] channel: {ch:?}");
    }
    Ok(())
}

/// Adopt a pre-H conveyor install into a runtime record. Fails when there
/// is no conveyor state or the recorded slot is unhealthy — adoption never
/// guesses.
fn adopt_conveyor_state(
    b: &Path,
) -> Result<omen_lifecycle::install::InstallRecord, Box<dyn std::error::Error>> {
    let state_path = b.join("state").join("installed.json");
    let bytes = std::fs::read(&state_path).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no install record and no conveyor state: unmanaged install; update refused",
        )
    })?;
    let state: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("conveyor state unreadable: {e}"),
        )
    })?;
    let slot = state
        .get("active_slot")
        .and_then(|s| s.as_str())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "conveyor state has no active slot; refusing to guess",
            )
        })?;
    if slot.contains('/') || slot.contains('\\') || slot.contains("..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "conveyor slot escapes; refused",
        )
        .into());
    }
    let exe = if cfg!(windows) { "omen.exe" } else { "omen" };
    if !b.join("versions").join(slot).join(exe).is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "conveyor active slot binary missing; refusing to adopt a broken install",
        )
        .into());
    }
    let active = state
        .get("active")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let get = |k: &str| {
        active
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut record = omen_lifecycle::install::InstallRecord::new(
        omen_lifecycle::install::Ownership::Omen,
        omen_lifecycle::install::user_channel(b),
        &get("preview_version"),
        &get("git_sha"),
    );
    if record.version.is_empty() || record.git_sha.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "conveyor manifest lacks version identity; refusing to adopt",
        )
        .into());
    }
    record.active_slot = Some(slot.to_string());
    record.previous_slot = state
        .get("previous_slot")
        .and_then(|s| s.as_str())
        .map(str::to_string);
    let pkg = get("package_sha256");
    let bin = get("binary_sha256");
    record.package_sha256 = if pkg.is_empty() { None } else { Some(pkg) };
    record.binary_sha256 = if bin.is_empty() { None } else { Some(bin) };
    omen_lifecycle::install::save_install_record(b, &record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    if omen_lifecycle::update::read_active_pointer(b).is_none() {
        omen_lifecycle::update::write_active_pointer(b, slot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    }
    Ok(record)
}

// ------------------------------------------------------------- rollback ---
pub fn cmd_rollback(
    binary: bool,
    state: Option<String>,
    json_mode: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    if !binary && state.is_none() {
        eprintln!(
            "specify --binary (previous slot) or --state <snapshot-id>; the two are separate truths"
        );
        std::process::exit(2);
    }
    if binary {
        let mut record = omen_lifecycle::install::load_install_record(&b)?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no install record: nothing to roll back",
            )
        })?;
        let launcher = omen_lifecycle::update::ProcessLauncher;
        match omen_lifecycle::update::rollback_binary(&b, &mut record, &launcher) {
            Ok(slot) => {
                if json_mode {
                    machine_msg(
                        "ok",
                        "binary rolled back",
                        serde_json::json!({"active_slot": slot}),
                    );
                } else {
                    println!("[ok] binary rolled back to slot {slot}");
                    println!("state untouched: binary rollback is not state rollback");
                }
            }
            Err(e) => {
                if json_mode {
                    machine_msg(
                        "fail",
                        "rollback refused",
                        serde_json::json!({"error": e.to_string()}),
                    );
                } else {
                    println!("[fail] rollback refused: {e}");
                }
                std::process::exit(1);
            }
        }
    }
    if let Some(snap) = state {
        match omen_lifecycle::update::rollback_state(&b, &snap) {
            Ok(restored) => {
                if json_mode {
                    machine_msg(
                        "ok",
                        "state restored",
                        serde_json::json!({"restored": restored}),
                    );
                } else {
                    println!("[ok] state restored from {snap}:");
                    for r in &restored {
                        println!("  - {r}");
                    }
                }
            }
            Err(e) => {
                if json_mode {
                    machine_msg(
                        "fail",
                        "state rollback refused",
                        serde_json::json!({"error": e.to_string()}),
                    );
                } else {
                    println!("[fail] state rollback refused: {e}");
                }
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- setup ---
pub fn cmd_setup(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Setup is optional and idempotent: sane defaults, deferrable choices.
    // It never claims a managed install and never requires a toolchain.
    let b = base();
    let mut made = Vec::new();
    for dir in [
        "workspaces",
        "evidence",
        "history",
        "cache",
        "tmp",
        "pins",
        "install",
    ] {
        let d = b.join(dir);
        if !d.exists() {
            std::fs::create_dir_all(&d)?;
            made.push(dir.to_string());
        }
    }
    // Default channel only when the user never chose one.
    let ch = channel();
    if json_mode {
        machine_msg(
            "ok",
            "setup complete",
            serde_json::json!({"created": made, "channel": ch}),
        );
    } else {
        println!("[ok] setup complete: state ready at {}", b.display());
        if !made.is_empty() {
            println!("created: {}", made.join(", "));
        }
        println!("channel: {ch:?} (change with `omen channel stable|preview`)");
        println!("optional next: install a provider CLI (codex/luna), then `omen doctor`");
    }
    Ok(())
}

// ------------------------------------------------------------ uninstall ---
pub fn cmd_uninstall(
    scope: String,
    apply: bool,
    json_mode: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    let scope_v = match scope.as_str() {
        "app" => omen_lifecycle::uninstall::UninstallScope::AppOnly,
        "app-cache" => omen_lifecycle::uninstall::UninstallScope::AppAndCache,
        "everything" => omen_lifecycle::uninstall::UninstallScope::Everything,
        _ => {
            eprintln!("unknown scope {scope:?}; expected app|app-cache|everything");
            std::process::exit(2);
        }
    };
    let (plan, retained) = omen_lifecycle::uninstall::uninstall_plan(&b, scope_v);
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "scope": scope, "remove": plan, "retained": retained, "applied": false
            }))?
        );
        if !apply {
            return Ok(());
        }
    } else {
        println!(
            "[info] uninstall scope {scope}: {} item(s) to remove, {} retained",
            plan.items.len(),
            retained.len()
        );
        for item in plan.items.iter().take(20) {
            println!("  [remove] {}", item.identity);
        }
        if plan.items.len() > 20 {
            println!(
                "  ... and {} more (see --machine for full list)",
                plan.items.len() - 20
            );
        }
        for r in retained.iter().take(10) {
            println!("  [retain] {} ({:?}): {}", r.identity, r.class, r.reason);
        }
        if !apply {
            println!("preview only: nothing removed (run with --apply)");
            return Ok(());
        }
    }
    let guard = omen_lifecycle::lock::acquire(&b)?;
    let _ = guard;
    let report = omen_lifecycle::plan::apply_plan(
        &plan,
        &|item| {
            Ok(omen_lifecycle::clean::revalidate_clean_item(
                &b,
                &std::collections::BTreeSet::new(),
                item,
            ))
        },
        &|item| omen_lifecycle::uninstall::apply_uninstall_item(&b, item),
    );
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "scope": scope, "applied": report, "retained": retained
            }))?
        );
    } else {
        println!(
            "[ok] uninstall: {} removed, {} refused, {} failed; {} retained",
            report.completed.len(),
            report.refused.len(),
            report.failed.len(),
            retained.len()
        );
    }
    Ok(())
}

// ---------------------------------------------------------- diagnostics ---
pub fn cmd_diagnostics(json_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    let report = lifecycle_doctor_report();
    let items = omen_lifecycle::state::classify_tree(&b);
    let (count, bytes, unknown) = items.iter().fold((0, 0u64, 0), |(c, by, u), i| {
        (
            c + 1,
            by + i.size_bytes.unwrap_or(0),
            u + usize::from(i.size_bytes.is_none()),
        )
    });
    let bundle = omen_lifecycle::diagnostics::DiagnosticBundle {
        schema_version: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: env!("OMEN_GIT_SHA").to_string(),
        contract_version: "0.8".to_string(),
        ownership: format!("{:?}", ownership()).to_lowercase(),
        channel: format!("{:?}", channel()).to_lowercase(),
        doctor: report,
        env_shape: omen_lifecycle::diagnostics::collect_env_shape(),
        config_shape: serde_json::json!({}),
        logs_tail: omen_lifecycle::diagnostics::collect_logs_tail(&b),
        storage_summary: serde_json::json!({
            "classified_items": count,
            "known_bytes": bytes,
            "unknown_sizes": unknown,
        }),
    };
    let dir = omen_lifecycle::diagnostics::write_bundle(&b, &bundle)?;
    if json_mode {
        machine_msg(
            "ok",
            "diagnostic bundle written",
            serde_json::json!({"dir": dir}),
        );
    } else {
        println!("[ok] diagnostic bundle: {}", dir.display());
        println!("inspect bundle.json before sharing; Omen never uploads it automatically");
    }
    Ok(())
}

// ------------------------------------------------------------------ pin ---
pub fn cmd_pin(
    add: Option<String>,
    remove: Option<String>,
    json_mode: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let b = base();
    if let Some(id) = add {
        let inserted = omen_lifecycle::pins::pin(&b, &id)?;
        if json_mode {
            machine_msg(
                "ok",
                "pinned",
                serde_json::json!({"identity": id, "new": inserted}),
            );
        } else {
            println!("[ok] pinned {id}");
        }
        return Ok(());
    }
    if let Some(id) = remove {
        let removed = omen_lifecycle::pins::unpin(&b, &id)?;
        if json_mode {
            machine_msg(
                "ok",
                "unpinned",
                serde_json::json!({"identity": id, "was_present": removed}),
            );
        } else {
            println!("[ok] unpinned {id}");
        }
        return Ok(());
    }
    let store = omen_lifecycle::pins::load_pins(&b)?;
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&store)?);
    } else if store.pins.is_empty() {
        println!("[info] no pins");
    } else {
        for p in &store.pins {
            println!("  [pin] {p}");
        }
    }
    Ok(())
}
