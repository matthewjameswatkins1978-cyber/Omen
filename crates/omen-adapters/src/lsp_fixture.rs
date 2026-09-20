use crate::lsp::LspClient;
use omen_core::{CoreError, SemanticProviderId};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostileLspMode {
    Valid,
    SlowInit,
    NeverInit,
    Malformed,
    WrongId,
    LateResponse,
    HugeResponse,
    Flood,
    ExitMid,
}

impl HostileLspMode {
    pub fn as_arg(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::SlowInit => "slow-init",
            Self::NeverInit => "never-init",
            Self::Malformed => "malformed",
            Self::WrongId => "wrong-id",
            Self::LateResponse => "late-response",
            Self::HugeResponse => "huge-response",
            Self::Flood => "flood",
            Self::ExitMid => "exit-mid",
        }
    }
}

pub struct HostileLspServer;

impl HostileLspServer {
    pub async fn spawn_client(
        gremlin_exe: &Path,
        mode: HostileLspMode,
        workspace_root: PathBuf,
    ) -> Result<LspClient, CoreError> {
        let id = SemanticProviderId::new(format!("hostile-lsp-{}", mode.as_arg())).unwrap();
        LspClient::spawn(
            gremlin_exe,
            &["--lsp-mode".into(), mode.as_arg().into()],
            workspace_root,
            id,
        )
        .await
    }
}
