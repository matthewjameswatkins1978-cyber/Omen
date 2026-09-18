use omen_atlas::{
    RuntimeProfile, ToolInstance, ToolUnderstanding, ToolValidator, ValidationState,
    compute_binary_fingerprint, find_binary_on_path,
};
use omen_core::ToolId;
use std::path::{Path, PathBuf};

fn profiles_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("profiles")
}

#[test]
fn load_committed_runtime_profiles() {
    let dir = profiles_dir();
    let threadmoth = RuntimeProfile::from_file(&dir.join("threadmoth.toml")).unwrap();
    assert_eq!(threadmoth.tool_id, "tool://threadmoth");
    assert_eq!(threadmoth.binary_name, "threadmoth");

    let cargo = RuntimeProfile::from_file(&dir.join("cargo.toml")).unwrap();
    assert_eq!(cargo.tool_id, "tool://cargo");

    let git = RuntimeProfile::from_file(&dir.join("git.toml")).unwrap();
    assert_eq!(git.tool_id, "tool://git");

    let rg = RuntimeProfile::from_file(&dir.join("ripgrep.toml")).unwrap();
    assert_eq!(rg.tool_id, "tool://rg");
}

#[tokio::test]
async fn tool_discovery_and_active_validation() {
    let cargo_path =
        find_binary_on_path("cargo").expect("cargo binary must be present in environment");
    assert!(cargo_path.exists());

    let fingerprint = compute_binary_fingerprint(&cargo_path).unwrap();
    assert!(!fingerprint.is_empty());

    let profiles = profiles_dir();
    let profile = RuntimeProfile::from_file(&profiles.join("cargo.toml")).ok();

    let mut instance = ToolInstance {
        tool_id: ToolId::new("tool://cargo").unwrap(),
        binary_path: cargo_path,
        binary_fingerprint: fingerprint,
        version: None,
        platform: std::env::consts::OS.to_string(),
        validation_state: ValidationState::Unvalidated,
        understanding: ToolUnderstanding::Adapted,
        profile,
        last_validated_at: None,
    };

    assert_eq!(instance.validation_state, ValidationState::Unvalidated);

    let validator = ToolValidator::new();
    validator.validate(&mut instance).await.unwrap();

    assert_eq!(instance.validation_state, ValidationState::Validated);
    assert!(instance.version.is_some());
    assert!(instance.version.as_ref().unwrap().contains("cargo"));
    assert!(instance.last_validated_at.is_some());
}

#[test]
fn stale_validation_detection() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mock_bin = temp_dir
        .path()
        .join(if cfg!(windows) { "tool.exe" } else { "tool" });

    // Step 1: Write initial version
    std::fs::write(&mock_bin, b"version-1-content").unwrap();
    let hash1 = compute_binary_fingerprint(&mock_bin).unwrap();

    let mut instance = ToolInstance {
        tool_id: ToolId::new("tool://mock").unwrap(),
        binary_path: mock_bin.clone(),
        binary_fingerprint: hash1,
        version: Some("1.0".into()),
        platform: "test".into(),
        validation_state: ValidationState::Validated,
        understanding: ToolUnderstanding::Observed,
        profile: None,
        last_validated_at: Some("now".into()),
    };

    // No change yet
    assert!(!instance.check_stale().unwrap());
    assert_eq!(instance.validation_state, ValidationState::Validated);

    // Step 2: Upgrade/modify the binary on disk
    std::fs::write(&mock_bin, b"version-2-upgraded-content").unwrap();

    // Now check_stale must detect modification and transition to STALE_VALIDATION
    let changed = instance.check_stale().unwrap();
    assert!(changed);
    assert_eq!(instance.validation_state, ValidationState::StaleValidation);
}
