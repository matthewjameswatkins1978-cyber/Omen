use omen_adapters::ThreadMothAdapter;
use omen_engine::ProcessSupervisor;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_threadmoth_doctor() {
    let supervisor = ProcessSupervisor::new();
    let doc = match ThreadMothAdapter::doctor(&supervisor).await {
        Ok(doc) => doc,
        Err(omen_core::CoreError::ExecutionFailed(err))
            if err.contains("No such file or directory")
                || err.contains("program not found")
                || err.contains("os error 2") =>
        {
            eprintln!("Skipping test_threadmoth_doctor: 'threadmoth' binary not found on host");
            return;
        }
        Err(e) => panic!("ThreadMoth doctor failed: {e}"),
    };
    assert!(doc.get("status").is_some() || doc.get("version").is_some());
}

#[tokio::test]
async fn test_threadmoth_capabilities() {
    let supervisor = ProcessSupervisor::new();
    let caps = match ThreadMothAdapter::capabilities(&supervisor).await {
        Ok(caps) => caps,
        Err(omen_core::CoreError::ExecutionFailed(err))
            if err.contains("No such file or directory")
                || err.contains("program not found")
                || err.contains("os error 2") =>
        {
            eprintln!(
                "Skipping test_threadmoth_capabilities: 'threadmoth' binary not found on host"
            );
            return;
        }
        Err(e) => panic!("ThreadMoth capabilities failed: {e}"),
    };
    assert!(caps.get("threadmoth_version").is_some() || caps.get("targets").is_some());
}

#[tokio::test]
async fn test_threadmoth_mutation_and_refusal() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("sample.txt");
    fs::write(&file_path, "Hello, world!\n").unwrap();

    let supervisor = ProcessSupervisor::new();

    // 1. Successful exact replacement
    let cert = match ThreadMothAdapter::replace_exact(
        &supervisor,
        dir.path(),
        "sample.txt",
        "world",
        "Omen",
        None,
    )
    .await
    {
        Ok(cert) => cert,
        Err(omen_core::CoreError::ExecutionFailed(err))
            if err.contains("No such file or directory")
                || err.contains("program not found")
                || err.contains("os error 2") =>
        {
            eprintln!(
                "Skipping test_threadmoth_mutation_and_refusal: 'threadmoth' binary not found on host"
            );
            return;
        }
        Err(e) => panic!("ThreadMoth replace_exact failed: {e}"),
    };
    assert_eq!(cert.outcome, "APPLIED");
    assert!(cert.is_applied());
    assert!(cert.pre_hash.is_some());
    assert!(cert.post_hash.is_some());

    let content = fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "Hello, Omen!\n");

    let post_hash = cert.post_hash.unwrap();

    // 2. Refusal when expected pre_hash does not match
    let refused = ThreadMothAdapter::replace_exact(
        &supervisor,
        dir.path(),
        "sample.txt",
        "Omen",
        "World",
        Some("wrong_hash_0000000000000000000000000000000000000000000000000000000000000000"),
    )
    .await;

    assert!(
        refused.is_ok(),
        "Should successfully parse refusal certificate: {:?}",
        refused.err()
    );
    let refused_cert = refused.unwrap();
    assert_eq!(refused_cert.outcome, "REFUSED");
    assert!(refused_cert.is_refused());
    assert!(refused_cert.reason_code.is_some());

    // Content should remain untouched
    let content_after = fs::read_to_string(&file_path).unwrap();
    assert_eq!(content_after, "Hello, Omen!\n");

    // 3. Success when expected pre_hash matches
    let success_hash = ThreadMothAdapter::replace_exact(
        &supervisor,
        dir.path(),
        "sample.txt",
        "Omen",
        "Substrate",
        Some(&post_hash),
    )
    .await;

    assert!(success_hash.is_ok());
    let success_cert = success_hash.unwrap();
    assert_eq!(success_cert.outcome, "APPLIED");

    let final_content = fs::read_to_string(&file_path).unwrap();
    assert_eq!(final_content, "Hello, Substrate!\n");
}
