use clap::{Command, Arg};
use dotenv::dotenv;
use hanzo_libp2p_relayer::{LibP2PProxy, LibP2PRelayError};
use hanzo_messages::hanzo_utils::{
    encryption::string_to_encryption_static_key, signatures::string_to_signature_secret_key,
};
use std::env;

#[tokio::main]
async fn main() -> Result<(), LibP2PRelayError> {
    dotenv().ok();

    let matches = Command::new("Hanzo LibP2P Relayer")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Hanzo Team <team@hanzo.ai>")
        .about("Relays LibP2P connections for Hanzo")
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .value_name("PORT")
                .help("Sets the port to bind the server")
                .num_args(1)
                .default_value("8080")
                .env("PORT"),
        )
        .arg(
            Arg::new("rpc_url")
                .long("rpc-url")
                .value_name("RPC_URL")
                .help("RPC URL for the registry")
                .num_args(1)
                .env("RPC_URL"),
        )
        .arg(
            Arg::new("contract_address")
                .long("contract-address")
                .value_name("CONTRACT_ADDRESS")
                .help("Contract address for the registry")
                .num_args(1)
                .env("CONTRACT_ADDRESS"),
        )
        .arg(
            Arg::new("identity_secret_key")
                .long("identity-secret-key")
                .value_name("IDENTITY_SECRET_KEY")
                .help("Identity secret key")
                .num_args(1)
                .required(true)
                .env("IDENTITY_SECRET_KEY"),
        )
        .arg(
            Arg::new("encryption_secret_key")
                .long("encryption-secret-key")
                .value_name("ENCRYPTION_SECRET_KEY")
                .help("Encryption secret key")
                .num_args(1)
                .required(true)
                .env("ENCRYPTION_SECRET_KEY"),
        )
        .arg(
            Arg::new("node_name")
                .long("node-name")
                .value_name("NODE_NAME")
                .help("Node name")
                .num_args(1)
                .required(true)
                .env("NODE_NAME"),
        )
        .arg(
            Arg::new("max_connections")
                .long("max-connections")
                .value_name("MAX_CONNECTIONS")
                .help("Maximum number of concurrent connections")
                .num_args(1)
                .env("MAX_CONNECTIONS"),
        )
        .get_matches();

    let port = matches
        .get_one::<String>("port")
        .unwrap()
        .parse::<u16>()
        .map_err(|e| LibP2PRelayError::ConfigurationError(format!("Invalid port: {}", e)))?;
    let rpc_url = matches.get_one::<String>("rpc_url").map(|s| s.clone());
    let contract_address = matches.get_one::<String>("contract_address").map(|s| s.clone());
    let identity_secret_key = matches.get_one::<String>("identity_secret_key").unwrap().to_string();
    let encryption_secret_key = matches.get_one::<String>("encryption_secret_key").unwrap().to_string();
    let node_name = matches.get_one::<String>("node_name").unwrap().to_string();
    let max_connections = matches.get_one::<String>("max_connections").map(|v| v.parse().unwrap_or(20));

    let identity_secret_key = string_to_signature_secret_key(&identity_secret_key)
        .map_err(|e| LibP2PRelayError::ConfigurationError(format!("Invalid IDENTITY_SECRET_KEY: {}", e)))?;
    let encryption_secret_key = string_to_encryption_static_key(&encryption_secret_key)
        .map_err(|e| LibP2PRelayError::ConfigurationError(format!("Invalid ENCRYPTION_SECRET_KEY: {}", e)))?;

    println!("Initializing LibP2P Relay Server on port {}", port);

    let proxy = LibP2PProxy::new(
        Some(identity_secret_key),
        Some(encryption_secret_key),
        Some(node_name),
        rpc_url,
        contract_address,
        max_connections,
        Some(port),
    )
    .await?;

    // Start the relay server (this will run indefinitely)
    proxy.start().await?;

    Ok(())
}
