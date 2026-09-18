use omen_adapters::{CargoAdapter, ThreadMothAdapter};
use omen_core::{Assurance, CoreError, ValidityState};
use omen_engine::ProcessSupervisor;
use omen_knowledge::{ContentAddressedStore, Database, FactRegistry};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn auth_fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("auth_project")
}

#[tokio::test]
async fn proof_1_authoritative_lifecycle_and_refusal() {
    let fixture_src = auth_fixture_dir();
    let temp_workspace = tempdir().unwrap();
    let ws_path = temp_workspace.path();

    // Copy fixture to temporary workspace
    fs::create_dir_all(ws_path.join("src")).unwrap();
    fs::copy(fixture_src.join("Cargo.toml"), ws_path.join("Cargo.toml")).unwrap();
    fs::copy(
        fixture_src.join("src").join("lib.rs"),
        ws_path.join("src").join("lib.rs"),
    )
    .unwrap();

    let supervisor = ProcessSupervisor::new();
    let mut db = Database::open(&ws_path.join("state.sqlite")).unwrap();
    let cas = ContentAddressedStore::new(ws_path.join("cas"));

    // Step 1: Initial Cargo Test -> Passing, CURRENT, VERIFIED
    let test_run_1 = CargoAdapter::test(&supervisor, &cas, &mut db, ws_path, None, None)
        .await
        .expect("Initial cargo test should succeed");
    assert!(test_run_1.success, "Test should pass initially");
    assert_eq!(test_run_1.test_fact.value, "passing");
    assert_eq!(test_run_1.test_fact.validity, ValidityState::Current);
    assert_eq!(test_run_1.test_fact.assurance, Assurance::Verified);

    let fact_1 = FactRegistry::get_fact(&db, &test_run_1.test_fact.resource_uri, true)
        .expect("Should fetch current fact");
    assert_eq!(fact_1.value, "passing");

    // Step 2: ThreadMoth mutates auth_project to break the test
    let mutation_1 = ThreadMothAdapter::replace_exact(
        &supervisor,
        ws_path,
        "src/lib.rs",
        r#"token == "omen-valid-token""#,
        r#"token == "different-token""#,
        None,
    )
    .await
    .expect("ThreadMoth mutation 1 should succeed");
    assert!(mutation_1.is_applied());

    // Invalidate/increment generation for workspace filesystem dependency
    FactRegistry::increment_generation(&mut db, "fs:workspace")
        .expect("Increment generation should succeed");

    // Step 3: Lazy Pessimism Refusal Verification
    // Querying with require_current = true MUST refuse with CoreError::FactDirty
    let query_dirty = FactRegistry::get_fact(&db, &test_run_1.test_fact.resource_uri, true);
    match query_dirty {
        Err(CoreError::FactDirty(msg)) => {
            assert!(msg.contains("fact://test/status"));
            assert!(msg.contains("explicit revalidation required"));
        }
        other => panic!("Expected CoreError::FactDirty, got: {:?}", other),
    }

    // Querying with require_current = false returns the fact marked as Dirty
    let fact_dirty_read = FactRegistry::get_fact(&db, &test_run_1.test_fact.resource_uri, false)
        .expect("Reading dirty fact with require_current=false should succeed");
    assert_eq!(fact_dirty_read.validity, ValidityState::Dirty);

    // Step 4: Explicit Cargo test execution produces new Failing Fact
    let test_run_2 = CargoAdapter::test(&supervisor, &cas, &mut db, ws_path, None, None)
        .await
        .expect("Cargo test run 2 should execute (even though tests fail)");
    assert!(!test_run_2.success, "Test should fail after mutation");
    assert_eq!(test_run_2.test_fact.value, "failing");
    assert_eq!(test_run_2.test_fact.validity, ValidityState::Current);

    // Step 5: Provenance & History Verification
    let provenance = FactRegistry::why_fact(&db, &test_run_2.test_fact.resource_uri)
        .expect("why_fact should return provenance");
    assert_eq!(provenance.fact.value, "failing");
    assert!(!provenance.artifacts.is_empty());
    assert_eq!(provenance.history.len(), 1);
    assert_eq!(provenance.history[0].value, "passing");
    assert_eq!(provenance.history[0].validity, ValidityState::Superseded);

    // Read raw failure transcript slice from CAS
    let failure_art_uri = &test_run_2.artifact_uri;
    let failure_hash = failure_art_uri.path().strip_prefix("sha256/").unwrap();
    let slice = cas
        .read_slice(&mut db, failure_hash, 0, 500)
        .expect("Should read CAS slice of test failure");
    let slice_text = String::from_utf8_lossy(&slice);
    assert!(
        slice_text.contains("test tests::test_auth_success ... FAILED")
            || slice_text.contains("FAILED"),
        "Transcript slice must contain failure evidence"
    );

    // Step 6: ThreadMoth restores the code using post_hash
    let restore_cert = ThreadMothAdapter::replace_exact(
        &supervisor,
        ws_path,
        "src/lib.rs",
        r#"token == "different-token""#,
        r#"token == "omen-valid-token""#,
        mutation_1.post_hash.as_deref(),
    )
    .await
    .expect("ThreadMoth restore should succeed");
    assert!(restore_cert.is_applied());

    // File changed -> filesystem generation incremented -> fact becomes DIRTY
    FactRegistry::increment_generation(&mut db, "fs:workspace")
        .expect("Increment generation should succeed");

    let query_after_restore = FactRegistry::get_fact(&db, &test_run_2.test_fact.resource_uri, true);
    match query_after_restore {
        Err(CoreError::FactDirty(_)) => {} // Correct! Restore does NOT make it CURRENT
        other => panic!(
            "Restoring code must leave fact DIRTY until re-verified, got: {:?}",
            other
        ),
    }

    // Step 7: Explicit Cargo re-verification -> passing, CURRENT, VERIFIED
    let test_run_3 = CargoAdapter::test(&supervisor, &cas, &mut db, ws_path, None, None)
        .await
        .expect("Cargo re-verification should execute");
    assert!(test_run_3.success, "Test should pass after restore");
    assert_eq!(test_run_3.test_fact.value, "passing");
    assert_eq!(test_run_3.test_fact.validity, ValidityState::Current);
}
