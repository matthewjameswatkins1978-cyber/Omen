use crate::ast_grep::AstGrepRewriteCandidate;
use crate::threadmoth::{ThreadMothAdapter, ThreadMothCertificate};
use omen_core::CoreError;
use omen_engine::ProcessSupervisor;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub struct ThreadMothBridge;

impl ThreadMothBridge {
    /// Applies an ast-grep structural rewrite candidate through ThreadMoth's deterministic
    /// mutation engine, computing pre-image hash, capturing post-image hash, and returning
    /// the cryptographic mutation certificate.
    pub async fn apply_structural_rewrite(
        supervisor: &ProcessSupervisor,
        workspace_root: &Path,
        candidate: &AstGrepRewriteCandidate,
    ) -> Result<ThreadMothCertificate, CoreError> {
        let full_path = workspace_root.join(&candidate.file);
        let bytes = fs::read(&full_path).map_err(|e| {
            CoreError::NotFound(format!("File not found for structural mutation: {e}"))
        })?;

        // Compute pre-image hash
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let pre_hash = hex::encode(hasher.finalize());

        // Dispatch through ThreadMoth with expected pre-image hash
        let cert = ThreadMothAdapter::replace_exact(
            supervisor,
            workspace_root,
            &candidate.file,
            &candidate.original_text,
            &candidate.replacement_text,
            Some(&pre_hash),
        )
        .await?;

        if !cert.is_applied() {
            return Err(CoreError::ExecutionFailed(format!(
                "ThreadMoth refused mutation on {}: outcome={}, reason={:?}",
                candidate.file, cert.outcome, cert.reason_code
            )));
        }

        Ok(cert)
    }
}
