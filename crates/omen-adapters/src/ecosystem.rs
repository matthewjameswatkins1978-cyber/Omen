use omen_core::{CoreError, ResourceUri, SemanticProviderId};
use omen_semantic::{
    BoxFuture, PackageDependency, PackageRecord, ProviderCapabilities, ProviderKind,
    SemanticProvider, TargetRecord, TaskRecord,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

// -----------------------------------------------------------------------------
// NPM Semantic Provider
// -----------------------------------------------------------------------------

pub struct NpmSemanticProvider {
    id: SemanticProviderId,
    workspace_dir: PathBuf,
}

impl NpmSemanticProvider {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            id: SemanticProviderId::new("sprov_npm").expect("valid id"),
            workspace_dir: workspace_dir.into(),
        }
    }

    pub fn parse_package_json(
        json_str: &str,
        manifest_path: &Path,
    ) -> Result<PackageRecord, CoreError> {
        let v: Value = serde_json::from_str(json_str)
            .map_err(|e| CoreError::ExecutionFailed(format!("Invalid package.json: {e}")))?;

        let name = v
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("unnamed")
            .to_string();
        let version = v
            .get("version")
            .and_then(|ver| ver.as_str())
            .unwrap_or("0.0.0")
            .to_string();

        let mut dependencies = Vec::new();
        if let Some(deps) = v.get("dependencies").and_then(|d| d.as_object()) {
            for (dep_name, req_val) in deps {
                let req = req_val.as_str().unwrap_or("*").to_string();
                dependencies.push(PackageDependency {
                    name: dep_name.clone(),
                    req,
                    kind: Some("prod".into()),
                });
            }
        }
        if let Some(dev_deps) = v.get("devDependencies").and_then(|d| d.as_object()) {
            for (dep_name, req_val) in dev_deps {
                let req = req_val.as_str().unwrap_or("*").to_string();
                dependencies.push(PackageDependency {
                    name: dep_name.clone(),
                    req,
                    kind: Some("dev".into()),
                });
            }
        }

        let mut tasks = Vec::new();
        if let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) {
            for (script_name, script_cmd) in scripts {
                let cmd = script_cmd.as_str().unwrap_or_default().to_string();
                tasks.push(TaskRecord {
                    name: script_name.clone(),
                    command: format!("npm run {script_name}"),
                    description: Some(cmd),
                });
            }
        }

        let mut targets = Vec::new();
        if let Some(main) = v.get("main").and_then(|m| m.as_str()) {
            targets.push(TargetRecord {
                name: "main".into(),
                kind: "entrypoint".into(),
                src_path: Some(main.to_string()),
            });
        }
        if let Some(bin) = v.get("bin") {
            if let Some(bin_str) = bin.as_str() {
                targets.push(TargetRecord {
                    name: name.clone(),
                    kind: "bin".into(),
                    src_path: Some(bin_str.to_string()),
                });
            } else if let Some(bin_map) = bin.as_object() {
                for (b_name, b_path) in bin_map {
                    targets.push(TargetRecord {
                        name: b_name.clone(),
                        kind: "bin".into(),
                        src_path: b_path.as_str().map(|s| s.to_string()),
                    });
                }
            }
        }

        let safe_name = name.replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
            "_",
        );
        let uri = ResourceUri::parse(&format!("package://npm/{safe_name}"))
            .map_err(|e| CoreError::ExecutionFailed(format!("Invalid uri: {e}")))?;

        Ok(PackageRecord {
            name,
            version,
            ecosystem: "npm".to_string(),
            manifest_path: manifest_path.to_string_lossy().replace('\\', "/"),
            dependencies,
            targets,
            tasks,
            uri,
        })
    }

    pub fn witness_paths(&self) -> Vec<PathBuf> {
        vec![
            self.workspace_dir.join("package.json"),
            self.workspace_dir.join("package-lock.json"),
        ]
    }
}

impl SemanticProvider for NpmSemanticProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "npm-semantics"
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
        self.workspace_dir.join("package.json").exists()
    }

    fn packages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<PackageRecord>, CoreError>> {
        Box::pin(async move {
            let p = self.workspace_dir.join("package.json");
            if !p.exists() {
                return Ok(Vec::new());
            }
            let contents = tokio::fs::read_to_string(&p)
                .await
                .map_err(|e| CoreError::ExecutionFailed(e.to_string()))?;
            let record = Self::parse_package_json(&contents, &p)?;
            Ok(vec![record])
        })
    }
}

// -----------------------------------------------------------------------------
// Python / uv Semantic Provider
// -----------------------------------------------------------------------------

pub struct PythonUvSemanticProvider {
    id: SemanticProviderId,
    workspace_dir: PathBuf,
}

impl PythonUvSemanticProvider {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            id: SemanticProviderId::new("sprov_uv").expect("valid id"),
            workspace_dir: workspace_dir.into(),
        }
    }

    pub fn parse_pyproject(
        toml_str: &str,
        manifest_path: &Path,
    ) -> Result<PackageRecord, CoreError> {
        let v: toml::Value = toml::from_str(toml_str)
            .map_err(|e| CoreError::ExecutionFailed(format!("Invalid pyproject.toml: {e}")))?;

        let project = v.get("project");
        let name = project
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unnamed-python")
            .to_string();
        let version = project
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("0.1.0")
            .to_string();

        let mut dependencies = Vec::new();
        if let Some(deps) = project
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_array())
        {
            for dep in deps {
                if let Some(dep_str) = dep.as_str() {
                    dependencies.push(PackageDependency {
                        name: dep_str.to_string(),
                        req: "*".into(),
                        kind: Some("runtime".into()),
                    });
                }
            }
        }

        let mut tasks = Vec::new();
        tasks.push(TaskRecord {
            name: "run".into(),
            command: "uv run python -m app".into(),
            description: Some("Run project via uv".into()),
        });
        tasks.push(TaskRecord {
            name: "test".into(),
            command: "uv run pytest".into(),
            description: Some("Run pytest via uv".into()),
        });

        if let Some(scripts) = project
            .and_then(|p| p.get("scripts"))
            .and_then(|s| s.as_table())
        {
            for (script_name, script_val) in scripts {
                tasks.push(TaskRecord {
                    name: script_name.clone(),
                    command: format!("uv run {script_name}"),
                    description: script_val.as_str().map(|s| s.to_string()),
                });
            }
        }

        let safe_name = name.replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
            "_",
        );
        let uri = ResourceUri::parse(&format!("package://python/{safe_name}"))
            .map_err(|e| CoreError::ExecutionFailed(format!("Invalid uri: {e}")))?;

        Ok(PackageRecord {
            name,
            version,
            ecosystem: "python".to_string(),
            manifest_path: manifest_path.to_string_lossy().replace('\\', "/"),
            dependencies,
            targets: vec![TargetRecord {
                name: "default".into(),
                kind: "module".into(),
                src_path: None,
            }],
            tasks,
            uri,
        })
    }

    pub fn witness_paths(&self) -> Vec<PathBuf> {
        vec![
            self.workspace_dir.join("pyproject.toml"),
            self.workspace_dir.join("uv.lock"),
        ]
    }
}

impl SemanticProvider for PythonUvSemanticProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "python-uv-semantics"
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
        self.workspace_dir.join("pyproject.toml").exists()
    }

    fn packages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<PackageRecord>, CoreError>> {
        Box::pin(async move {
            let p = self.workspace_dir.join("pyproject.toml");
            if !p.exists() {
                return Ok(Vec::new());
            }
            let contents = tokio::fs::read_to_string(&p)
                .await
                .map_err(|e| CoreError::ExecutionFailed(e.to_string()))?;
            let record = Self::parse_pyproject(&contents, &p)?;
            Ok(vec![record])
        })
    }
}

// -----------------------------------------------------------------------------
// Go Semantic Provider
// -----------------------------------------------------------------------------

pub struct GoSemanticProvider {
    id: SemanticProviderId,
    workspace_dir: PathBuf,
}

impl GoSemanticProvider {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            id: SemanticProviderId::new("sprov_go").expect("valid id"),
            workspace_dir: workspace_dir.into(),
        }
    }

    pub fn parse_go_mod(contents: &str, manifest_path: &Path) -> Result<PackageRecord, CoreError> {
        let mut module_name = "unknown_go_module".to_string();
        let mut go_version = "1.21".to_string();
        let mut dependencies = Vec::new();

        let mut in_require_block = false;

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.is_empty() {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("module ") {
                module_name = rest.trim().to_string();
            } else if let Some(rest) = trimmed.strip_prefix("go ") {
                go_version = rest.trim().to_string();
            } else if trimmed == "require (" {
                in_require_block = true;
            } else if in_require_block && trimmed == ")" {
                in_require_block = false;
            } else if in_require_block {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    dependencies.push(PackageDependency {
                        name: parts[0].to_string(),
                        req: parts[1].to_string(),
                        kind: Some("direct".into()),
                    });
                }
            } else if let Some(rest) = trimmed.strip_prefix("require ") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() >= 2 {
                    dependencies.push(PackageDependency {
                        name: parts[0].to_string(),
                        req: parts[1].to_string(),
                        kind: Some("direct".into()),
                    });
                }
            }
        }

        let safe_name = module_name.replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
            "_",
        );
        let uri = ResourceUri::parse(&format!("package://go/{safe_name}"))
            .map_err(|e| CoreError::ExecutionFailed(format!("Invalid uri: {e}")))?;

        let tasks = vec![
            TaskRecord {
                name: "build".into(),
                command: "go build ./...".into(),
                description: Some("Build Go module".into()),
            },
            TaskRecord {
                name: "test".into(),
                command: "go test ./...".into(),
                description: Some("Test Go module".into()),
            },
        ];

        Ok(PackageRecord {
            name: module_name,
            version: go_version,
            ecosystem: "go".to_string(),
            manifest_path: manifest_path.to_string_lossy().replace('\\', "/"),
            dependencies,
            targets: vec![TargetRecord {
                name: "main".into(),
                kind: "go_package".into(),
                src_path: None,
            }],
            tasks,
            uri,
        })
    }

    pub fn witness_paths(&self) -> Vec<PathBuf> {
        vec![
            self.workspace_dir.join("go.mod"),
            self.workspace_dir.join("go.sum"),
        ]
    }
}

impl SemanticProvider for GoSemanticProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "go-semantics"
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
        self.workspace_dir.join("go.mod").exists()
    }

    fn packages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<PackageRecord>, CoreError>> {
        Box::pin(async move {
            let p = self.workspace_dir.join("go.mod");
            if !p.exists() {
                return Ok(Vec::new());
            }
            let contents = tokio::fs::read_to_string(&p)
                .await
                .map_err(|e| CoreError::ExecutionFailed(e.to_string()))?;
            let record = Self::parse_go_mod(&contents, &p)?;
            Ok(vec![record])
        })
    }
}

// -----------------------------------------------------------------------------
// Docker Semantic Provider
// -----------------------------------------------------------------------------

pub struct DockerSemanticProvider {
    id: SemanticProviderId,
    workspace_dir: PathBuf,
}

impl DockerSemanticProvider {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            id: SemanticProviderId::new("sprov_docker").expect("valid id"),
            workspace_dir: workspace_dir.into(),
        }
    }
}

impl SemanticProvider for DockerSemanticProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "docker-semantics"
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
        self.workspace_dir.join("Dockerfile").exists()
            || self.workspace_dir.join("docker-compose.yml").exists()
            || self.workspace_dir.join("compose.yaml").exists()
    }

    fn packages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<PackageRecord>, CoreError>> {
        Box::pin(async move {
            let mut records = Vec::new();
            let dockerfile = self.workspace_dir.join("Dockerfile");
            if dockerfile.exists() {
                let uri = ResourceUri::parse("package://docker/dockerfile")
                    .map_err(|e| CoreError::ExecutionFailed(e.to_string()))?;
                records.push(PackageRecord {
                    name: "dockerfile".to_string(),
                    version: "latest".to_string(),
                    ecosystem: "docker".to_string(),
                    manifest_path: dockerfile.to_string_lossy().replace('\\', "/"),
                    dependencies: Vec::new(),
                    targets: vec![TargetRecord {
                        name: "image".into(),
                        kind: "docker_image".into(),
                        src_path: Some(dockerfile.to_string_lossy().into()),
                    }],
                    tasks: vec![TaskRecord {
                        name: "docker:build".into(),
                        command: "docker build -t app .".into(),
                        description: Some("Build Docker image".into()),
                    }],
                    uri,
                });
            }
            Ok(records)
        })
    }
}

// -----------------------------------------------------------------------------
// GitHub CLI Semantic Provider
// -----------------------------------------------------------------------------

pub struct GitHubCliSemanticProvider {
    id: SemanticProviderId,
    workspace_dir: PathBuf,
}

impl GitHubCliSemanticProvider {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            id: SemanticProviderId::new("sprov_github").expect("valid id"),
            workspace_dir: workspace_dir.into(),
        }
    }
}

impl SemanticProvider for GitHubCliSemanticProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "github-cli-semantics"
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
        self.workspace_dir.join(".git").exists()
    }

    fn packages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<PackageRecord>, CoreError>> {
        Box::pin(async move {
            let uri = ResourceUri::parse("package://github/repo")
                .map_err(|e| CoreError::ExecutionFailed(e.to_string()))?;
            Ok(vec![PackageRecord {
                name: "workspace-repo".to_string(),
                version: "head".to_string(),
                ecosystem: "github".to_string(),
                manifest_path: ".git".to_string(),
                dependencies: Vec::new(),
                targets: Vec::new(),
                tasks: vec![
                    TaskRecord {
                        name: "gh:pr:list".to_string(),
                        command: "gh pr list".to_string(),
                        description: Some("List open pull requests".into()),
                    },
                    TaskRecord {
                        name: "gh:issue:list".to_string(),
                        command: "gh issue list".to_string(),
                        description: Some("List open issues".into()),
                    },
                ],
                uri,
            }])
        })
    }
}
