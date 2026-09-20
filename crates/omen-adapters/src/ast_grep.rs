use omen_atlas::find_binary_on_path;
use omen_core::{CoreError, RequiredAssurance, SemanticProviderId, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_semantic::provider::{BoxFuture, ProviderCapabilities, ProviderKind, SemanticProvider};
use omen_semantic::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepMatchJson {
    pub text: String,
    pub range: AstGrepRangeJson,
    pub file: String,
    #[serde(default)]
    pub meta_variables: Option<AstGrepMetaVariablesJson>,
    #[serde(default)]
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepRangeJson {
    pub start: AstGrepPositionJson,
    pub end: AstGrepPositionJson,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepPositionJson {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepMetaVariablesJson {
    #[serde(default)]
    pub single: Option<HashMap<String, AstGrepMetaValJson>>,
    #[serde(default)]
    pub multi: Option<HashMap<String, Vec<AstGrepMetaValJson>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstGrepMetaValJson {
    pub text: String,
    pub range: Option<AstGrepRangeJson>,
}

#[derive(Debug, Clone)]
pub struct AstGrepRewriteCandidate {
    pub file: String,
    pub range: SourceRange,
    pub original_text: String,
    pub replacement_text: String,
}

pub struct AstGrepAdapter {
    id: SemanticProviderId,
    binary_path: Option<PathBuf>,
    workspace_root: PathBuf,
}

impl AstGrepAdapter {
    pub fn new(workspace_root: PathBuf) -> Self {
        let binary_path = find_binary_on_path("ast-grep").or_else(|| find_binary_on_path("sg"));
        let id = SemanticProviderId::new("ast-grep").unwrap();
        Self {
            id,
            binary_path,
            workspace_root,
        }
    }

    pub fn with_binary(workspace_root: PathBuf, binary_path: PathBuf) -> Self {
        let id = SemanticProviderId::new("ast-grep").unwrap();
        Self {
            id,
            binary_path: Some(binary_path),
            workspace_root,
        }
    }

    pub fn is_available(&self) -> bool {
        self.binary_path.is_some()
    }

    pub async fn run_structural_search(
        &self,
        pattern: &str,
        language: &str,
        limit: usize,
    ) -> Result<Vec<StructuralMatch>, CoreError> {
        let Some(bin) = &self.binary_path else {
            return Err(CoreError::ExecutionFailed(
                "ast-grep binary not found".into(),
            ));
        };

        let supervisor = ProcessSupervisor::new();
        let req = ExecutionRequest {
            argv: vec![
                bin.to_string_lossy().to_string(),
                "run".into(),
                "--pattern".into(),
                pattern.into(),
                "--lang".into(),
                language.into(),
                "--json".into(),
            ],
            cwd: self.workspace_root.clone(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 5000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
            secrets: vec![],
        };

        let output = supervisor.execute(req).await?;
        if !output.process_exit.is_zero() && output.stdout_all.is_empty() {
            return Err(CoreError::ExecutionFailed(format!(
                "ast-grep scan failed with exit code {:?}: {}",
                output.process_exit.code,
                String::from_utf8_lossy(&output.stderr_all)
            )));
        }

        Self::parse_matches(&output.stdout_all, &self.workspace_root, limit)
    }

    pub async fn derive_rewrite_candidates(
        &self,
        pattern: &str,
        rewrite: &str,
        language: &str,
    ) -> Result<Vec<AstGrepRewriteCandidate>, CoreError> {
        let Some(bin) = &self.binary_path else {
            return Err(CoreError::ExecutionFailed(
                "ast-grep binary not found".into(),
            ));
        };

        let supervisor = ProcessSupervisor::new();
        let req = ExecutionRequest {
            argv: vec![
                bin.to_string_lossy().to_string(),
                "run".into(),
                "--pattern".into(),
                pattern.into(),
                "--rewrite".into(),
                rewrite.into(),
                "--lang".into(),
                language.into(),
                "--json".into(),
            ],
            cwd: self.workspace_root.clone(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 5000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
            secrets: vec![],
        };

        let output = supervisor.execute(req).await?;
        Self::parse_rewrite_candidates(&output.stdout_all, &self.workspace_root)
    }

    pub fn parse_matches(
        raw_json: &[u8],
        workspace_root: &Path,
        limit: usize,
    ) -> Result<Vec<StructuralMatch>, CoreError> {
        if raw_json.is_empty() {
            return Ok(Vec::new());
        }

        let parsed: Vec<AstGrepMatchJson> = serde_json::from_slice(raw_json).map_err(|e| {
            CoreError::ExecutionFailed(format!("Failed to parse ast-grep JSON output: {e}"))
        })?;

        let mut matches = Vec::new();
        for item in parsed.into_iter().take(limit) {
            let file_rel = if let Ok(rel) = Path::new(&item.file).strip_prefix(workspace_root) {
                rel.to_string_lossy().replace('\\', "/")
            } else {
                item.file.replace('\\', "/")
            };

            let range = SourceRange::new(
                item.range.start.line,
                item.range.start.column,
                item.range.end.line,
                item.range.end.column,
            );

            let mut metavars = HashMap::new();
            if let Some(meta) = item.meta_variables
                && let Some(single) = meta.single
            {
                for (k, v) in single {
                    metavars.insert(format!("${k}"), v.text);
                }
            }

            matches.push(StructuralMatch {
                file: file_rel,
                range,
                matched_text: item.text,
                metavariables: metavars,
                rule_id: None,
            });
        }

        Ok(matches)
    }

    pub fn parse_rewrite_candidates(
        raw_json: &[u8],
        workspace_root: &Path,
    ) -> Result<Vec<AstGrepRewriteCandidate>, CoreError> {
        if raw_json.is_empty() {
            return Ok(Vec::new());
        }

        let parsed: Vec<AstGrepMatchJson> = serde_json::from_slice(raw_json).map_err(|e| {
            CoreError::ExecutionFailed(format!("Failed to parse ast-grep rewrite JSON: {e}"))
        })?;

        let mut candidates = Vec::new();
        for item in parsed {
            let Some(replacement) = item.replacement else {
                continue;
            };

            let file_rel = if let Ok(rel) = Path::new(&item.file).strip_prefix(workspace_root) {
                rel.to_string_lossy().replace('\\', "/")
            } else {
                item.file.replace('\\', "/")
            };

            let range = SourceRange::new(
                item.range.start.line,
                item.range.start.column,
                item.range.end.line,
                item.range.end.column,
            );

            candidates.push(AstGrepRewriteCandidate {
                file: file_rel,
                range,
                original_text: item.text,
                replacement_text: replacement,
            });
        }

        Ok(candidates)
    }
}

impl SemanticProvider for AstGrepAdapter {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "ast-grep"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Structural
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            symbol_search: false,
            definition: false,
            references: false,
            structural_search: true,
            diagnostics: false,
            packages: false,
        }
    }

    fn is_available(&self) -> bool {
        self.is_available()
    }

    fn structural_search<'a>(
        &'a self,
        pattern: &'a str,
        language: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StructuralMatch>, CoreError>> {
        Box::pin(async move { self.run_structural_search(pattern, language, limit).await })
    }
}
