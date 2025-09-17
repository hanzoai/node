use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[clap(
    name = "hanzod",
    version = env!("CARGO_PKG_VERSION"),
    author = "Hanzo AI",
    about = "Hanzo Node - Decentralized AI Infrastructure"
)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Option<Commands>,

    /// Path to configuration file
    #[clap(short, long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Enable verbose logging
    #[clap(short, long)]
    pub verbose: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run the Hanzo node
    Run {
        /// Node IP address
        #[clap(long, env = "NODE_IP", default_value = "0.0.0.0")]
        node_ip: String,

        /// Node port
        #[clap(long, env = "NODE_PORT", default_value = "9452")]
        node_port: u16,

        /// API IP address
        #[clap(long, env = "NODE_API_IP", default_value = "0.0.0.0")]
        api_ip: String,

        /// API port
        #[clap(long, env = "NODE_API_PORT", default_value = "9450")]
        api_port: u16,
    },

    /// Manage cryptographic keys
    Keys {
        #[clap(subcommand)]
        subcommand: KeyCommands,
    },

    /// Initialize node configuration
    Init {
        /// Force overwrite existing configuration
        #[clap(short, long)]
        force: bool,
    },

    /// Display version information
    Version,
}

#[derive(Subcommand, Debug)]
pub enum KeyCommands {
    /// Generate new keys
    Generate {
        /// Type of keys to generate
        #[clap(subcommand)]
        key_type: KeyType,

        /// Output format
        #[clap(short, long, value_enum, default_value = "env")]
        format: OutputFormat,

        /// Save keys to ~/.hanzod/keys/ directory
        #[clap(short, long)]
        save: bool,
    },

    /// List existing keys
    List {
        /// Show full key values (default: show only addresses)
        #[clap(short, long)]
        show_private: bool,
    },

    /// Import keys from file or stdin
    Import {
        /// Path to key file (or stdin if not provided)
        #[clap(value_name = "FILE")]
        file: Option<PathBuf>,
    },

    /// Export keys to file or stdout
    Export {
        /// Output file (or stdout if not provided)
        #[clap(value_name = "FILE")]
        file: Option<PathBuf>,

        /// Include private keys in export
        #[clap(long)]
        include_private: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum KeyType {
    /// Generate wallet keys for transactions
    Wallet {
        /// Network to generate keys for
        #[clap(long, default_value = "sepolia")]
        network: String,
    },

    /// Generate node identity keys
    Identity,

    /// Generate X402 payment protocol keys
    X402,

    /// Generate all test keys for CI/testing
    Test,

    /// Generate staking/validator keys
    Staking {
        /// Amount to stake (in ETH)
        #[clap(long)]
        amount: Option<String>,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum OutputFormat {
    /// Environment variable format
    Env,
    /// JSON format
    Json,
    /// YAML format
    Yaml,
    /// TOML format
    Toml,
}