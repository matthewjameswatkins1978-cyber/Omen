use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "omen",
    author,
    version,
    about = "Agent-native developer runtime"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Inspect environment and health
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Describe Omen runtime capabilities
    Describe {
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Doctor { json }) => {
            if json {
                println!(r#"{{"status":"ok","version":"0.2.0"}}"#);
            } else {
                println!("Omen 0.2.0: OK");
            }
        }
        Some(Commands::Describe { json }) => {
            if json {
                println!(
                    r#"{{"name":"omen","doctrine":"substrate, not sovereign","version":"0.2.0"}}"#
                );
            } else {
                println!("Omen: substrate, not sovereign (v0.2.0)");
            }
        }
        None => {
            println!("Omen: substrate, not sovereign. Run 'omen --help' for usage.");
        }
    }
}
