//! H hostile state matrices: clean, GC plan/apply, stale-plan race.
//!
//! Fixture: disposable cache + live history + protected evidence +
//! unprotected expired evidence + pinned evidence + unrelated sentinel
//! OUTSIDE the Omen root. Sentinel must remain pristine throughout.
use omen_lifecycle::{clean, gc, pins, plan};
use std::path::{Path, PathBuf};

const HEX_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEX_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const FAR_FUTURE: u64 = u64::MAX / 2;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    sentinel: PathBuf,
    sentinel_text: String,
}

fn cas_blob(cas_root: &Path, hex: &str, bytes: &[u8]) -> PathBuf {
    let dir = cas_root.join("sha256").join(&hex[..2]);
    std::fs::create_dir_all(&dir).unwrap();
    let blob = dir.join(&hex[2..]);
    std::fs::write(&blob, bytes).unwrap();
    blob
}

fn setup() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("omen-state");
    // Disposable debris.
    std::fs::create_dir_all(base.join("tmp")).unwrap();
    std::fs::write(base.join("tmp").join("scratch.tmp"), b"scratch").unwrap();
    std::fs::create_dir_all(base.join("cache")).unwrap();
    std::fs::write(base.join("cache").join("accel.bin"), b"accel").unwrap();
    std::fs::create_dir_all(base.join("update").join("downloads")).unwrap();
    std::fs::write(
        base.join("update").join("downloads").join("dl.pkg.part"),
        b"part",
    )
    .unwrap();
    // Live history truth.
    std::fs::create_dir_all(base.join("workspaces").join("ws_live")).unwrap();
    std::fs::write(
        base.join("workspaces").join("ws_live").join("state.sqlite"),
        b"db-bytes",
    )
    .unwrap();
    // CAS: one protected (reachable), one pinned, one expired-eligible.
    let cas = base.join("workspaces").join("ws_live").join("cas");
    cas_blob(&cas, HEX_A, b"protected-payload");
    cas_blob(&cas, HEX_B, b"pinned-payload");
    let hex_c = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    cas_blob(&cas, hex_c, b"expired-payload");
    // Evidence dir file (retained proof).
    std::fs::create_dir_all(base.join("evidence")).unwrap();
    std::fs::write(base.join("evidence").join("proof.json"), b"proof").unwrap();
    // Sentinel OUTSIDE the root.
    let sentinel = tmp.path().join("OUTSIDE-sentinel.txt");
    let sentinel_text = "untouched-user-data".to_string();
    std::fs::write(&sentinel, &sentinel_text).unwrap();
    Fixture {
        _tmp: tmp,
        base,
        sentinel,
        sentinel_text,
    }
}

fn reachability(hex_a: &str) -> gc::Reachability {
    let mut r = gc::Reachability::default();
    r.protected_digests.insert(hex_a.to_string());
    r
}

#[test]
fn hostile_clean_removes_only_debris() {
    let fx = setup();
    // Pin the pinned blob BEFORE clean (pins are retention, respected by gc;
    // clean never touches evidence anyway).
    pins::pin(&fx.base, &format!("digest:{HEX_B}")).unwrap();

    let report = clean::clean_apply(&fx.base).unwrap();
    assert!(report.failed.is_empty());
    assert!(report.refused.is_empty());
    assert!(!report.completed.is_empty());

    // Debris gone.
    assert!(!fx.base.join("tmp").join("scratch.tmp").exists());
    assert!(!fx.base.join("cache").join("accel.bin").exists());
    assert!(
        !fx.base
            .join("update")
            .join("downloads")
            .join("dl.pkg.part")
            .exists()
    );
    // Truth preserved.
    assert!(
        fx.base
            .join("workspaces")
            .join("ws_live")
            .join("state.sqlite")
            .is_file()
    );
    assert!(fx.base.join("evidence").join("proof.json").is_file());
    for hex in [HEX_A, HEX_B] {
        let blob = fx
            .base
            .join("workspaces")
            .join("ws_live")
            .join("cas")
            .join("sha256")
            .join(&hex[..2])
            .join(&hex[2..]);
        assert!(blob.is_file(), "{hex}");
    }
    // Sentinel pristine.
    assert_eq!(
        std::fs::read_to_string(&fx.sentinel).unwrap(),
        fx.sentinel_text
    );
}

#[test]
fn hostile_gc_plan_apply_with_tombstone() {
    let fx = setup();
    pins::pin(&fx.base, &format!("digest:{HEX_B}")).unwrap();
    let policy = gc::RetentionPolicy {
        schema_version: 1,
        keep_unprotected_evidence_days: 0,
        max_unprotected_evidence_bytes: None,
    };
    let reach = reachability(HEX_A);
    let ws_cas = fx.base.join("workspaces").join("ws_live").join("cas");
    let cands = gc::enumerate_candidates(&ws_cas, &fx.base, &policy, &reach, FAR_FUTURE);
    // Only the expired unprotected blob: A reachable, B pinned.
    assert_eq!(cands.len(), 1);
    assert_eq!(
        cands[0].digest,
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
    );

    let plan_doc = gc::gc_plan(&cands);
    assert_eq!(plan_doc.items.len(), 1);
    // Apply with live revalidation: recompute reachability at apply time.
    let report = plan::apply_plan(
        &plan_doc,
        &|item| {
            let digest = item.fingerprint.clone().unwrap();
            let d = digest.strip_prefix("digest:").unwrap();
            let live = gc::enumerate_candidates(
                &ws_cas,
                &fx.base,
                &policy,
                &reachability(HEX_A),
                FAR_FUTURE,
            );
            if live.iter().any(|c| c.digest == d) {
                Ok(plan::Revalidate::Proceed)
            } else {
                Ok(plan::Revalidate::Refuse {
                    reason: "no longer eligible".to_string(),
                })
            }
        },
        &|item| gc::apply_gc_item(&fx.base, item),
    );
    assert!(report.fully_applied());
    // Blob gone, tombstone recorded, history truth (db + proof) intact.
    assert!(!Path::new(&cands[0].path).exists());
    let tomb = std::fs::read_to_string(gc::tombstone_log_path(&fx.base)).unwrap();
    assert!(tomb.contains(&cands[0].digest));
    assert!(tomb.contains("payload unavailable / collected"));
    assert!(
        fx.base
            .join("workspaces")
            .join("ws_live")
            .join("state.sqlite")
            .is_file()
    );
    assert!(fx.base.join("evidence").join("proof.json").is_file());
    assert_eq!(
        std::fs::read_to_string(&fx.sentinel).unwrap(),
        fx.sentinel_text
    );
}

#[test]
fn stale_plan_cannot_delete_newly_protected() {
    let fx = setup();
    let policy = gc::RetentionPolicy {
        schema_version: 1,
        keep_unprotected_evidence_days: 0,
        max_unprotected_evidence_bytes: None,
    };
    let ws_cas = fx.base.join("workspaces").join("ws_live").join("cas");
    // Plan while blob C is unprotected...
    let cands = gc::enumerate_candidates(
        &ws_cas,
        &fx.base,
        &policy,
        &gc::Reachability::default(),
        FAR_FUTURE,
    );
    assert_eq!(cands.len(), 3); // A, B, C all unprotected at plan time
    let plan_doc = gc::gc_plan(&cands);
    // ...then protect A (reachable) and pin B before apply.
    let reach = reachability(HEX_A);
    pins::pin(&fx.base, &format!("digest:{HEX_B}")).unwrap();
    let report = plan::apply_plan(
        &plan_doc,
        &|item| {
            let digest = item.fingerprint.clone().unwrap();
            let d = digest.strip_prefix("digest:").unwrap().to_string();
            // Live revalidation consults CURRENT protection.
            let live = gc::enumerate_candidates(&ws_cas, &fx.base, &policy, &reach, FAR_FUTURE);
            if live.iter().any(|c| c.digest == d) {
                Ok(plan::Revalidate::Proceed)
            } else {
                Ok(plan::Revalidate::Refuse {
                    reason: format!("{d} became protected after plan"),
                })
            }
        },
        &|item| gc::apply_gc_item(&fx.base, item),
    );
    // First item alphabetically is A (protected now) -> whole apply refuses.
    assert!(report.stale);
    assert!(report.completed.is_empty());
    assert!(!report.refused.is_empty());
    // Newly protected evidence survives.
    for hex in [HEX_A, HEX_B] {
        let blob = ws_cas.join("sha256").join(&hex[..2]).join(&hex[2..]);
        assert!(blob.is_file(), "{hex}");
    }
    assert_eq!(
        std::fs::read_to_string(&fx.sentinel).unwrap(),
        fx.sentinel_text
    );
}
