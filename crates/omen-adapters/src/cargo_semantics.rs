use omen_core::{CoreError, ResourceUri, SemanticProviderId};
use omen_semantic::{
    BoxFuture, PackageDependency, PackageRecord, ProviderCapabilities, ProviderKind,
    SemanticProvider, TargetRecord, TaskRecord,
};
use serde_json::Value;
use std::path::PathBuf;

/// Semantic provider that extracts package graphs, targets, and dependencies from Cargo workspaces.
pub struct CargoSemanticProvider {
    id: SemanticProviderId,
    workspace_dir: PathBuf,
}

impl CargoSemanticProvider {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            id: SemanticProviderId::new("sprov_cargo").expect("valid id"),
            workspace_dir: workspace_dir.into(),
        }
    }

    pub fn parse_metadata(json_str: &str) -> Result<Vec<PackageRecord>, CoreError> {
        let root: Value = serde_json::from_str(json_str).map_err(|e| {
            CoreError::ExecutionFailed(format!("Failed to parse cargo metadata: {e}"))
        })?;

        let packages = root
            .get("packages")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                CoreError::ExecutionFailed("Missing 'packages' array in cargo metadata".into())
            })?;

        let mut records = Vec::new();
        for pkg in packages {
            let name = pkg
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let version = pkg
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let manifest_path = pkg
                .get("manifest_path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let mut dependencies = Vec::new();
            if let Some(deps) = pkg.get("dependencies").and_then(|v| v.as_array()) {
                for dep in deps {
                    let dep_name = dep
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let req = dep
                        .get("req")
                        .and_then(|v| v.as_str())
                        .unwrap_or("*")
                        .to_string();
                    let kind = dep
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    dependencies.push(PackageDependency {
                        name: dep_name,
                        req,
                        kind,
                    });
                }
            }

            let mut targets = Vec::new();
            if let Some(tgts) = pkg.get("targets").and_then(|v| v.as_array()) {
                for tgt in tgts {
                    let tgt_name = tgt
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let kind = tgt
                        .get("kind")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|v| v.as_str())
                        .unwrap_or("lib")
                        .to_string();
                    let src_path = tgt
                        .get("src_path")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    targets.push(TargetRecord {
                        name: tgt_name,
                        kind,
                        src_path,
                    });
                }
            }

            let tasks = vec![
                TaskRecord {
                    name: format!("build:{name}"),
                    command: format!("cargo build -p {name}"),
                    description: Some(format!("Build package {name}")),
                },
                TaskRecord {
                    name: format!("test:{name}"),
                    command: format!("cargo test -p {name}"),
                    description: Some(format!("Test package {name}")),
                },
                TaskRecord {
                    name: format!("check:{name}"),
                    command: format!("cargo check -p {name}"),
                    description: Some(format!("Check package {name}")),
                },
            ];

            let safe_name = name.replace(
                |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
                "_",
            );
            let uri = ResourceUri::parse(&format!("package://cargo/{safe_name}"))
                .map_err(|e| CoreError::ExecutionFailed(format!("Invalid package uri: {e}")))?;

            records.push(PackageRecord {
                name,
                version,
                ecosystem: "cargo".to_string(),
                manifest_path,
                dependencies,
                targets,
                tasks,
                uri,
            });
        }

        Ok(records)
    }

    pub fn workspace_witness_paths(&self) -> Vec<PathBuf> {
        vec![
            self.workspace_dir.join("Cargo.toml"),
            self.workspace_dir.join("Cargo.lock"),
        ]
    }
}

impl SemanticProvider for CargoSemanticProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "cargo-semantics"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Ecosystem
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            packages: true,
            ..Default::default()
        }
    }

    fn is_available(&self) -> bool {
        self.workspace_dir.join("Cargo.toml").exists()
    }

    fn packages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<PackageRecord>, CoreError>> {
        Box::pin(async move {
            let output = tokio::process::Command::new("cargo")
                .args(["metadata", "--format-version", "1", "--no-deps"])
                .current_dir(&self.workspace_dir)
                .output()
                .await
                .map_err(|e| {
                    CoreError::ExecutionFailed(format!("Failed to execute cargo metadata: {e}"))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(CoreError::ExecutionFailed(format!(
                    "cargo metadata exited with code {:?}: {stderr}",
                    output.status.code()
                )));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            Self::parse_metadata(&stdout)
        })
    }
}
