use omen_core::{CoreError, ResourceUri, RetentionClass, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::{ContentAddressedStore, Database};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RipgrepMatch {
    pub path: String,
    pub line_number: Option<u64>,
    pub line_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RipgrepSearchResult {
    pub match_count: usize,
    pub file_count: usize,
    pub preview: Vec<RipgrepMatch>,
    pub artifact_uri: ResourceUri,
}

pub struct RipgrepAdapter;

impl RipgrepAdapter {
    pub async fn search(
        supervisor: &ProcessSupervisor,
        cas: &ContentAddressedStore,
        db: &mut Database,
        search_dir: &Path,
        query: &str,
    ) -> Result<RipgrepSearchResult, CoreError> {
        let req = ExecutionRequest {
            argv: vec![
                "rg".into(),
                "--json".into(),
                query.into(),
                search_dir.to_string_lossy().to_string(),
            ],
            cwd: search_dir.to_path_buf(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 15000,
            inline_budget: 65536,
            required_assurance: omen_core::RequiredAssurance::default(),
            secrets: vec![],
        };

        let output = supervisor.execute(req).await?;

        // Spool complete unreduced output into CAS
        let artifact_meta = cas.store(
            db,
            &output.stdout_all,
            "application/x-ndjson",
            "omen://adapter/ripgrep",
            RetentionClass::Referenced,
        )?;

        let raw = String::from_utf8_lossy(&output.stdout_all);
        let mut matches = Vec::new();
        let mut files = std::collections::HashSet::new();

        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                continue;
            };

            if v["type"] == "match" {
                let path = v["data"]["path"]["text"].as_str().unwrap_or("").to_string();
                let line_number = v["data"]["line_number"].as_u64();
                let line_content = v["data"]["lines"]["text"]
                    .as_str()
                    .unwrap_or("")
                    .trim_end()
                    .to_string();

                files.insert(path.clone());

                if matches.len() < 20 {
                    matches.push(RipgrepMatch {
                        path,
                        line_number,
                        line_content,
                    });
                }
            }
        }

        let match_count = if matches.len() < 20 {
            matches.len()
        } else {
            // Count total matches in the JSON lines
            raw.lines()
                .filter(|l| l.contains(r#""type":"match""#))
                .count()
        };

        Ok(RipgrepSearchResult {
            match_count,
            file_count: files.len(),
            preview: matches,
            artifact_uri: artifact_meta.uri,
        })
    }
}
