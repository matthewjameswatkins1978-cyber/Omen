use omen_adapters::CargoAdapter;
use omen_engine::ProcessSupervisor;
use omen_knowledge::{ContentAddressedStore, Database};
use std::path::Path;
use std::{fs, path::PathBuf};
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

    let fixture_source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("cargo_adapter");
    let fixture_dir = dir.path().join("cargo-adapter-fixture");
    copy_fixture(&fixture_source, &fixture_dir);

    // Run the production adapter against a real but deliberately tiny crate.
    // Cargo's generated target and lockfile stay inside this test's tempdir.
    let check_res = CargoAdapter::check(
        &supervisor,
        &cas,
        &mut db,
        &fixture_dir,
        Some("omen-cargo-adapter-fixture"),
    )
    .await;

    assert!(
        check_res.is_ok(),
        "Cargo check failed: {:?}",
        check_res.err()
    );
    let check_res = check_res.unwrap();
    assert!(check_res.success);
    assert!(check_res.diagnostics.is_empty());
    assert_eq!(check_res.compiler_fact.value, "none");
    assert_eq!(
        check_res.compiler_fact.resource_uri.as_str(),
        "fact://compiler/errors"
    );
    let check_hash = check_res
        .artifact_uri
        .path()
        .strip_prefix("sha256/")
        .unwrap();
    assert!(cas.inspect(&db, check_hash).unwrap().size > 0);

    // Prove real Cargo test execution, result parsing, fact publication and CAS evidence.
    let test_res = CargoAdapter::test(
        &supervisor,
        &cas,
        &mut db,
        &fixture_dir,
        Some("omen-cargo-adapter-fixture"),
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
    assert_eq!(test_res.exit_code, Some(0));
    let test_hash = test_res
        .artifact_uri
        .path()
        .strip_prefix("sha256/")
        .unwrap();
    assert!(cas.inspect(&db, test_hash).unwrap().size > 0);
}

fn copy_fixture(source: &Path, destination: &Path) {
    fs::create_dir_all(destination.join("src")).unwrap();
    fs::create_dir_all(destination.join("tests")).unwrap();
    for relative in [
        PathBuf::from("Cargo.toml"),
        PathBuf::from("src/lib.rs"),
        PathBuf::from("tests/threadmoth_tests.rs"),
    ] {
        fs::copy(source.join(&relative), destination.join(relative)).unwrap();
    }
}
