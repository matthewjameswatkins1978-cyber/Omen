use clap::Parser;
use omen_mcp::McpServer;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "omen-mcp",
    about = "Omen Model Context Protocol (MCP) stdio adapter"
)]
struct Cli {
    /// Workspace root directory (defaults to current directory)
    #[arg(short, long)]
    workspace: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let ws_path = cli
        .workspace
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let client = omen_client::OmenClient::connect_default(None).await.ok();
    if let Some(ref c) = client {
        let path_str = ws_path.canonicalize().unwrap_or_else(|_| ws_path.clone());
        let _ = c
            .attach_workspace(path_str.to_string_lossy().to_string())
            .await;
    }

    let server = McpServer::new(ws_path, client);
    server.run_stdio().await?;
    Ok(())
}
