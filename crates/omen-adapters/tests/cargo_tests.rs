use omen_adapters::CargoAdapter;
use omen_engine::ProcessSupervisor;
use omen_knowledge::{ContentAddressedStore, Database};
use std::path::Path;
use tempfile::tempdir;

#[tokio::test]
async fn test_cargo_metadata() {
    let supervisor = ProcessSupervisor::new();
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();

    let meta = CargoAdapter::metadata(&supervisor, root_dir).await;
    assert!(meta.is_ok(), "Cargo metadata failed: {:?}", meta.err());
    let val = meta.unwrap();
    assert!(val.get("packages").is_some());
    assert!(val.get("workspace_root").is_some());
}

#[tokio::test]
async fn test_cargo_check_and_test() {
    let supervisor = ProcessSupervisor::new();
    let dir = tempdir().unwrap();
    let mut db = Database::open(&dir.path().join("state.sqlite")).unwrap();
    let cas = ContentAddressedStore::new(dir.path().join("cas"));

    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();

    // 1. Check omen-core
    let check_res =
        CargoAdapter::check(&supervisor, &cas, &mut db, root_dir, Some("omen-core")).await;

    assert!(
        check_res.is_ok(),
        "Cargo check failed: {:?}",
        check_res.err()
    );
    let check_res = check_res.unwrap();
    assert!(check_res.success);
    assert_eq!(check_res.compiler_fact.value, "none");
    assert_eq!(
        check_res.compiler_fact.resource_uri.as_str(),
        "fact://compiler/errors"
    );

    // 2. Test threadmoth_tests target
    let test_res = CargoAdapter::test(
        &supervisor,
        &cas,
        &mut db,
        root_dir,
        Some("omen-adapters"),
        Some("threadmoth_tests"),
    )
    .await;

    assert!(test_res.is_ok(), "Cargo test failed: {:?}", test_res.err());
    let test_res = test_res.unwrap();
    assert!(test_res.success);
    assert_eq!(test_res.test_fact.value, "passing");
    assert_eq!(
        test_res.test_fact.resource_uri.as_str(),
        "fact://test/status"
    );
}
