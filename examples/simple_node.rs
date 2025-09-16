/// Example: Running a simple Hanzo node
/// 
/// This example shows how to run a basic Hanzo node with default configuration.
/// To run: cargo run --example simple_node

use hanzo_node::config::Config;
use hanzo_message_primitives::hanzo_utils::hanzo_logging::init_default_tracing;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    init_default_tracing();
    
    // Load configuration
    let config = Config::load()?;
    
    println!("Starting Hanzo Node...");
    println!("Node IP: {}:{}", config.node.ip, config.node.port);
    println!("API: http://{}:{}", config.node.api_ip, config.node.api_port);
    println!("Embeddings: {}", config.embeddings.default_embedding_model);
    
    // Export config to environment for backward compatibility
    config.export_to_env();
    
    // Initialize and run the node
    // Note: This would use the actual runner module in a real implementation
    println!("Node configuration loaded successfully!");
    println!("To start the actual node, use: cargo run");
    
    Ok(())
}