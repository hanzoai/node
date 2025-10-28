//! # Hanzo Migration Tool
//!
//! CLI tool for migrating SQLite databases to LanceDB.

use anyhow::Result;
use clap::Parser;
use hanzo_db::migration::run_migration_cli;

/// SQLite to LanceDB migration tool for Hanzo Node
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the SQLite database file
    #[arg(long, default_value = "./storage/db.sqlite")]
    sqlite_path: String,
    
    /// Path to the LanceDB directory
    #[arg(long, default_value = "./storage/lancedb")]
    lancedb_path: String,
    
    /// Verify data after migration
    #[arg(long, default_value_t = true)]
    verify: bool,
    
    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    // Initialize logging
    let log_level = if args.verbose {
        "debug"
    } else {
        "info"
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .init();
    
    // Run migration
    run_migration_cli(&args.sqlite_path, &args.lancedb_path).await?;
    
    Ok(())
}