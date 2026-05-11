// main.rs
#![recursion_limit = "512"]
mod cron_tasks;
mod llm_provider;
#[cfg(feature = "machine")]
mod machine;
mod managers;
mod network;
mod runner;
mod tools;
mod utils;
mod wallet;
mod zap_server;

use runner::{initialize_node, run_node_tasks};
use hanzo_messages::hanzo_utils::hanzo_logging::init_default_tracing;

#[cfg(feature = "console")]
use console_subscriber;

#[tokio::main]
pub async fn main() {
    // WHY: short-circuit `hanzod machine ...` before the heavy node init so
    // CLI calls don't bind ports or load the DB. Pure FFI into libluxmachine.
    #[cfg(feature = "machine")]
    {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        if let Some(code) = machine::try_handle_cli(&argv) {
            std::process::exit(code);
        }
    }
    // Initialize crypto provider for rustls (required by ngrok)
    #[cfg(feature = "ngrok")]
    {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    // Initialize logging based on features
    #[cfg(feature = "console")]
    {
        // When using console subscriber, we don't need env_logger
        console_subscriber::init();
        eprintln!("> tokio-console is enabled");
    }
    #[cfg(not(feature = "console"))]
    {
        // When not using console subscriber, use the default logging setup
        env_logger::Builder::from_env(env_logger::Env::default())
            .format_timestamp_millis()
            .init();
        init_default_tracing();
    }

    println!("Starting Hanzo Node...");

    let result = initialize_node().await.unwrap();
    let _ = run_node_tasks(result.1, result.2, result.3, result.4).await;
}
