//! Hanzo Node - AI Infrastructure Platform
//! 
//! This library provides the core functionality for running a Hanzo node,
//! which enables creation of AI agents without coding. It provides multi-LLM
//! provider support, job orchestration, and tool execution capabilities.

#![recursion_limit = "512"]

// Public modules for library consumers
pub mod config;
pub mod cron_tasks;
pub mod llm_provider;
pub mod managers;
pub mod network;
pub mod tools;
pub mod utils;
pub mod wallet;

// Re-export commonly used types
pub use config::Config;

// Version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Initialize a Hanzo node with the given configuration
pub async fn init(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    // Export config to environment for backward compatibility
    config.export_to_env();
    
    // Additional initialization logic would go here
    Ok(())
}

/// Run a Hanzo node with default configuration
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load()?;
    init(config).await
}