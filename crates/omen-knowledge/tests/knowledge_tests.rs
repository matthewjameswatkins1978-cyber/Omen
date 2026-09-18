use omen_core::{Assurance, BlobState, CoreError, ResourceUri, RetentionClass, ValidityState};
use omen_knowledge::{ContentAddressedStore, Database, FactRegistry, PublishFactRequest};
use tempfile::tempdir;

#[test]
fn persistence_and_lazy_pessimism() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("state.sqlite");
    let test_resource = ResourceUri::parse("fact://test/status").unwrap();
    let gen_name = "generation://cargo/build-inputs";

    // 1. Initial connection: establish generation 1 and publish passing fact
    {
        let mut db = Database::open(&db_path).unwrap();
        FactRegistry::set_generation(&mut db, gen_name, 1).unwrap();

        let deps = [(gen_name.to_string(), 1)];
        let fact = FactRegistry::publish_fact(
            &mut db,
            PublishFactRequest {
                resource: &test_resource,
                value: "passing",
                assurance: Assurance::Verified,
                producer: "omen://action/test-run-1",
                witness: Some("commit-abc"),
                dependencies: &deps,
                artifacts: &[],
            },
        )
        .unwrap();

        assert_eq!(fact.value, "passing");
        assert_eq!(fact.validity, ValidityState::Current);

        let query = FactRegistry::get_fact(&db, &test_resource, true).unwrap();
        assert_eq!(query.value, "passing");
        assert_eq!(query.validity, ValidityState::Current);
    }

    // 2. Reopen database (persistence across restart) and mutate generation
    {
        let mut db = Database::open(&db_path).unwrap();

        // Increment generation -> should dirty the fact
        let next_gen = FactRegistry::increment_generation(&mut db, gen_name).unwrap();
        assert_eq!(next_gen, 2);

        // Fetch without require_current: returns DIRTY fact
        let dirty_fact = FactRegistry::get_fact(&db, &test_resource, false).unwrap();
        assert_eq!(dirty_fact.validity, ValidityState::Dirty);

        // Fetch with require_current: must REFUSE with FactDirty
        let err = FactRegistry::get_fact(&db, &test_resource, true).unwrap_err();
        assert!(matches!(err, CoreError::FactDirty(_)));

        // 3. Explicit revalidation: publish failing fact
        let deps = [(gen_name.to_string(), 2)];
        let new_fact = FactRegistry::publish_fact(
            &mut db,
            PublishFactRequest {
                resource: &test_resource,
                value: "failing",
                assurance: Assurance::Verified,
                producer: "omen://action/test-run-2",
                witness: Some("commit-def"),
                dependencies: &deps,
                artifacts: &[],
            },
        )
        .unwrap();

        assert_eq!(new_fact.value, "failing");
        assert_eq!(new_fact.validity, ValidityState::Current);

        // Verify provenance and history
        let why = FactRegistry::why_fact(&db, &test_resource).unwrap();
        assert_eq!(why.fact.value, "failing");
        assert_eq!(why.history.len(), 1);
        assert_eq!(why.history[0].value, "passing");
        assert_eq!(why.history[0].validity, ValidityState::Superseded);
    }
}

#[test]
fn cas_storage_bounded_slice_and_gc() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("state.sqlite");
    let cas_dir = dir.path().join("cas");
    let mut db = Database::open(&db_path).unwrap();
    let cas = ContentAddressedStore::new(cas_dir);

    let payload = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let meta = cas
        .store(
            &mut db,
            payload,
            "text/plain",
            "omen://action/compiler",
            RetentionClass::Ephemeral,
        )
        .unwrap();

    assert_eq!(meta.size, payload.len() as u64);
    assert_eq!(meta.blob_state, BlobState::Present);

    // Read bounded slice: offset 10, length 16
    let slice = cas.read_slice(&mut db, &meta.digest, 10, 16).unwrap();
    assert_eq!(slice, b"ABCDEFGHIJKLMNOP");

    // Dry-run GC
    let dry_report = cas.gc(&mut db, true).unwrap();
    assert_eq!(dry_report.reclaimed_count, 1);
    assert_eq!(dry_report.reclaimed_bytes, payload.len() as u64);

    // After dry run, slice still readable
    let slice_after_dry = cas.read_slice(&mut db, &meta.digest, 0, 5).unwrap();
    assert_eq!(slice_after_dry, b"01234");

    // Live GC
    let live_report = cas.gc(&mut db, false).unwrap();
    assert_eq!(live_report.reclaimed_count, 1);

    // After live GC, metadata still inspectable with EVICTED state
    let inspected = cas.inspect(&db, &meta.digest).unwrap();
    assert_eq!(inspected.blob_state, BlobState::Evicted);

    // Reading evicted blob slice fails with NotFound
    let read_err = cas.read_slice(&mut db, &meta.digest, 0, 5).unwrap_err();
    assert!(matches!(read_err, CoreError::NotFound(_)));
}
