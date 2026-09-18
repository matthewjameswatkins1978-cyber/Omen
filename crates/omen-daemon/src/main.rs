use clap::Parser;
use omen_daemon::DaemonServer;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "omend", about = "Omen shared runtime daemon")]
struct Cli {
    /// Custom IPC endpoint override
    #[arg(long)]
    endpoint: Option<String>,

    /// Run daemon in foreground
    #[arg(long, default_value_t = true)]
    foreground: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let server = DaemonServer::new(cli.endpoint);
    info!(
        "Starting omend version {} (daemon_id: {}, endpoint: {})",
        env!("CARGO_PKG_VERSION"),
        server.instance_id(),
        server.endpoint()
    );

    tokio::select! {
        res = server.run() => {
            if let Err(e) = res {
                eprintln!("omend error: {e}");
                std::process::exit(1);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("omend received Ctrl-C, shutting down");
            server.shutdown();
        }
    }

    Ok(())
}
