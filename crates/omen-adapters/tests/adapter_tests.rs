use omen_adapters::{GitAdapter, RipgrepAdapter};
use omen_core::ValidityState;
use omen_engine::ProcessSupervisor;
use omen_knowledge::{ContentAddressedStore, Database, FactRegistry};
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[tokio::test]
async fn git_adapter_status_and_fact_publishing() {
    let supervisor = ProcessSupervisor::new();
    let root = repo_root();

    let status = GitAdapter::query_status(&supervisor, &root).await.unwrap();
    assert_eq!(status.branch, "feature/omen-0.2-runtime-proof");

    let dir = tempdir().unwrap();
    let mut db = Database::open(&dir.path().join("state.sqlite")).unwrap();

    let (branch_fact, clean_fact) = GitAdapter::publish_facts(&supervisor, &root, &mut db)
        .await
        .unwrap();

    assert_eq!(branch_fact.value, "feature/omen-0.2-runtime-proof");
    assert_eq!(branch_fact.validity, ValidityState::Current);

    assert_eq!(clean_fact.validity, ValidityState::Current);

    // Verify registry can retrieve branch fact
    let retrieved = FactRegistry::get_fact(&db, &branch_fact.resource_uri, true).unwrap();
    assert_eq!(retrieved.value, "feature/omen-0.2-runtime-proof");
}

#[tokio::test]
async fn ripgrep_adapter_json_and_cas_spooling() {
    let supervisor = ProcessSupervisor::new();
    let root = repo_root();
    let dir = tempdir().unwrap();
    let mut db = Database::open(&dir.path().join("state.sqlite")).unwrap();
    let cas = ContentAddressedStore::new(dir.path().join("cas"));

    let result = RipgrepAdapter::search(
        &supervisor,
        &cas,
        &mut db,
        &root,
        "substrate, not sovereign",
    )
    .await
    .unwrap();

    assert!(
        result.match_count > 0,
        "Expected to find matches for core doctrine"
    );
    assert!(result.file_count > 0);
    assert!(!result.preview.is_empty());

    // Verify artifact is stored in CAS and inspectable
    let hash = result.artifact_uri.path().strip_prefix("sha256/").unwrap();
    let inspected = cas.inspect(&db, hash).unwrap();
    assert!(inspected.size > 0);

    // Verify slice read
    let slice = cas.read_slice(&mut db, hash, 0, 50).unwrap();
    assert!(!slice.is_empty());
}
