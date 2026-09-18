use omen_core::{Assurance, CoreError, RequiredAssurance, ResourceUri, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::{Database, FactRecord, FactRegistry, PublishFactRequest};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusResult {
    pub branch: String,
    pub head_oid: String,
    pub is_clean: bool,
    pub changed_files_count: usize,
    pub witness: String,
}

pub struct GitAdapter;

impl GitAdapter {
    pub async fn query_status(
        supervisor: &ProcessSupervisor,
        repo_path: &Path,
    ) -> Result<GitStatusResult, CoreError> {
        let req = ExecutionRequest {
            argv: vec![
                "git".into(),
                "status".into(),
                "--porcelain=v2".into(),
                "--branch".into(),
            ],
            cwd: repo_path.to_path_buf(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 10000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
        };

        let output = supervisor.execute(req).await?;
        if output.runtime_status != omen_core::RuntimeStatus::Completed
            || !output.process_exit.is_zero()
        {
            return Err(CoreError::ExecutionFailed(format!(
                "git status probe failed with exit {:?}",
                output.process_exit.code
            )));
        }

        let raw = String::from_utf8_lossy(&output.stdout_all);
        let mut branch = "unknown".to_string();
        let mut head_oid = "unknown".to_string();
        let mut changed_count = 0;

        for line in raw.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("# branch.head ") {
                branch = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("# branch.oid ") {
                head_oid = rest.to_string();
            } else if line.starts_with("1 ")
                || line.starts_with("2 ")
                || line.starts_with("u ")
                || line.starts_with("? ")
            {
                changed_count += 1;
            }
        }

        let is_clean = changed_count == 0;

        // Compute adapter-owned witness: SHA256 of branch, head_oid, and changed_count
        let mut hasher = Sha256::new();
        hasher.update(branch.as_bytes());
        hasher.update(head_oid.as_bytes());
        hasher.update(changed_count.to_string().as_bytes());
        let witness = hex::encode(hasher.finalize());

        Ok(GitStatusResult {
            branch,
            head_oid,
            is_clean,
            changed_files_count: changed_count,
            witness,
        })
    }

    pub async fn publish_facts(
        supervisor: &ProcessSupervisor,
        repo_path: &Path,
        db: &mut Database,
    ) -> Result<(FactRecord, FactRecord), CoreError> {
        let status = Self::query_status(supervisor, repo_path).await?;

        let branch_resource = ResourceUri::parse("fact://git/branch")?;
        let clean_resource = ResourceUri::parse("fact://git/clean")?;

        let branch_fact = FactRegistry::publish_fact(
            db,
            PublishFactRequest {
                resource: &branch_resource,
                value: &status.branch,
                assurance: Assurance::Verified,
                producer: "omen://adapter/git",
                witness: Some(&status.head_oid),
                dependencies: &[],
                artifacts: &[],
            },
        )?;

        let clean_str = if status.is_clean { "clean" } else { "dirty" };
        let clean_fact = FactRegistry::publish_fact(
            db,
            PublishFactRequest {
                resource: &clean_resource,
                value: clean_str,
                assurance: Assurance::Verified,
                producer: "omen://adapter/git",
                witness: Some(&status.witness),
                dependencies: &[],
                artifacts: &[],
            },
        )?;

        Ok((branch_fact, clean_fact))
    }
}
