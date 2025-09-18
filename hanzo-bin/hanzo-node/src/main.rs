// main.rs
#![recursion_limit = "512"]
mod config;
mod llm_provider;
mod cron_tasks;
mod managers;
mod network;
mod runner;
mod utils;
mod wallet;
mod tools;
mod cli;

use clap::Parser;
use runner::{initialize_node, run_node_tasks};
use hanzo_message_primitives::hanzo_utils::hanzo_logging::init_default_tracing;
use cli::{Cli, Commands};

#[cfg(feature = "console")]
use console_subscriber;

#[tokio::main]
pub async fn main() {
    let cli = Cli::parse();
    // Initialize crypto provider for rustls (required by ngrok)
    #[cfg(feature = "ngrok")]
    {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    
    // Initialize logging based on features
    #[cfg(feature = "console")] {
        // When using console subscriber, we don't need env_logger
        console_subscriber::init();
        eprintln!("> tokio-console is enabled");
    }
    #[cfg(not(feature = "console"))] {
        // When not using console subscriber, use the default logging setup
        env_logger::Builder::from_env(env_logger::Env::default())
            .format_timestamp_millis()
            .init();
        init_default_tracing();
    }

    // Handle CLI commands
    match cli.command {
        Some(Commands::Keys { ref subcommand }) => {
            // Handle key management commands
            if let Err(e) = cli::keys::handle_keys_command(subcommand).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Version) => {
            println!("Hanzo Node v{}", env!("CARGO_PKG_VERSION"));
            println!("Build: {}", env!("CARGO_PKG_NAME"));
        }
        Some(Commands::Init { force: _ }) => {
            println!("Node initialization not yet implemented");
            // TODO: Implement node initialization
        }
        Some(Commands::Run { .. }) | None => {
            // Default behavior: run the node
            println!("Starting Hanzo Node...");
            let result = initialize_node().await.unwrap();
            let _ = run_node_tasks(result.1, result.2, result.3).await;
        }
    }
}
