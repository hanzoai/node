use super::network::Node;
use super::utils::environment::NodeEnvironment;
use crate::utils::args::parse_args;
use crate::utils::cli::cli_handle_create_message;
use crate::utils::environment::{fetch_llm_provider_env, fetch_node_environment};
use crate::utils::keys::generate_or_load_keys;
use crate::zap_server::start_zap_server;
use async_channel::{bounded, Receiver, Sender};
use ed25519_dalek::VerifyingKey;
use hanzo_embed::embedding_generator::RemoteEmbeddingGenerator;
use hanzo_http_api::node_api_router;
use hanzo_http_api::node_commands::NodeCommand;
use hanzo_messages::hanzo_utils::encryption::{
    encryption_public_key_to_string, encryption_secret_key_to_string,
};
use hanzo_messages::hanzo_utils::hanzo_logging::{hanzo_log, HanzoLogLevel, HanzoLogOption};
use hanzo_messages::hanzo_utils::signatures::{
    clone_signature_secret_key, hash_signature_public_key, signature_public_key_to_string,
    signature_secret_key_to_string,
};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::fmt;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Weak};
use std::{env, fs};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

#[derive(Debug)]
pub struct NodeRunnerError {
    pub source: Box<dyn StdError + Send + Sync>,
}

impl fmt::Display for NodeRunnerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl StdError for NodeRunnerError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

impl From<Box<dyn StdError + Send + Sync>> for NodeRunnerError {
    fn from(err: Box<dyn StdError + Send + Sync>) -> Self {
        Self { source: err }
    }
}

/// Checks if a port is available for binding
fn port_is_available(port: u16) -> bool {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Load + register the canonical inference/embedding engine used by EVM
/// precompiles `0x0201` (AI inference) and `0x0202` (AI embedding).
///
/// Configuration (preserved across the engine upgrade for forward-compat):
/// * `HANZO_MODEL_PATH` — path to a local GGUF or safetensors directory.
///   Preferred when set: avoids touching the network.
/// * `HANZO_MODEL_REPO` — Hugging Face repo (e.g. `Qwen/Qwen3-4B`). Used
///   when `HANZO_MODEL_PATH` is unset.
///
/// If neither variable is set, the precompiles revert at call time. This is
/// the documented "fail open" contract: a node without a model is still a
/// valid node — it just can't serve those two precompiles.
///
/// ============================ FLAG: HUMAN REVIEW ============================
/// BREAKING ENGINE API CHANGE (convergence 2026-06, engine 0.6.0 → 1.0.2).
///
/// The 0.6.0 integration registered a global `hanzo_engine::MistralEngine`
/// via `hanzo_engine::register_inference_engine` / `register_embedding_engine`
/// (a trait registry added in engine commit a12549984). That entire surface —
/// the `MistralEngine` type AND both `register_*` free functions — was REMOVED
/// in engine 1.0.2. The 1.0.2 public API is builder-based:
/// `hanzo_engine::{Hanzo, HanzoBuilder, EngineConfig, ModelSelected, Pipeline,
/// Request, Response, ...}` and exposes NO global inference/embedding registry.
///
/// Re-wiring the precompiles to the 1.0.2 API (build a `Hanzo` via
/// `HanzoBuilder` from MODEL_PATH/MODEL_REPO, then route `0x0201`/`0x0202`
/// through it) is a non-trivial port that also depends on how/where the
/// precompile handler now obtains its engine handle. It is intentionally NOT
/// guessed here. Until it is implemented, this fn is a compile-safe NO-OP that
/// honors the existing "fail open" contract (precompiles revert), so the rest
/// of the node still builds and runs against engine 1.0.2.
///
/// TODO(convergence): port to `HanzoBuilder` and restore real inference/embed.
/// ===========================================================================
async fn install_engine() {
    let configured = match (env::var("HANZO_MODEL_PATH"), env::var("HANZO_MODEL_REPO")) {
        (Ok(path), _) if !path.is_empty() => Some(format!("path={path}")),
        (_, Ok(repo)) if !repo.is_empty() => Some(format!("repo={repo}")),
        _ => None,
    };

    match configured {
        Some(source) => hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Error,
            &format!(
                "hanzo_engine: model is configured ({source}) but the inference/embedding \
                 wiring has NOT yet been ported from the removed 0.6.0 registry to the \
                 engine 1.0.2 HanzoBuilder API. AI precompiles (0x0201, 0x0202) will revert. \
                 See runner::install_engine FLAG."
            ),
        ),
        None => hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Info,
            "hanzo_engine: HANZO_MODEL_PATH and HANZO_MODEL_REPO unset; \
             AI precompiles (0x0201, 0x0202) will revert until an engine is registered",
        ),
    }
}

pub async fn initialize_node() -> Result<
    (Sender<NodeCommand>, JoinHandle<()>, JoinHandle<()>, JoinHandle<()>, Weak<Mutex<Node>>),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let main_db: &str = "main_db";
    let _vector_fs_db: &str = "vector_fs_db";
    let secrets_file: &str = ".secret";

    // Fetch Env vars/args
    let args = parse_args();
    let node_env = fetch_node_environment();

    // Check if required ports are available
    let api_port = node_env.api_listen_address.port();
    let node_port = node_env.listen_address.port();
    let ws_port = node_env.ws_address.map(|addr| addr.port());
    let https_port = node_env.api_https_listen_address.port();

    if !port_is_available(api_port) {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("API port {} is already in use", api_port),
        )));
    }

    if !port_is_available(node_port) {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("Node port {} is already in use", node_port),
        )));
    }

    if let Some(port) = ws_port {
        if !port_is_available(port) {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("WebSocket port {} is already in use", port),
            )));
        }
    }

    if !port_is_available(https_port) {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("HTTPS port {} is already in use", https_port),
        )));
    }

    let zap_port = node_env.zap_address.port();
    if !port_is_available(zap_port) {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("ZAP port {} is already in use", zap_port),
        )));
    }

    // TODO:
    // Read file encryption key from ENV variable and decrypt the secrets file
    // Store in memory this file encryption key, which is used to encrypt / decrypt other information
    // such as wallet information (private key, mnemonic, etc).

    let node_storage_path = node_env.node_storage_path.clone();

    let secrets_file_path = get_secrets_file_path(secrets_file, node_storage_path.clone());
    let node_keys = generate_or_load_keys(&secrets_file_path);

    // Storage db filesystem
    let main_db_path = get_main_db_path(main_db, &node_keys.identity_public_key, node_storage_path.clone());

    // Acquire the Node's keys.
    // TODO: Should check with on and then it's with onchain data for matching with the keys provided
    let secrets = parse_secrets_file(&secrets_file_path);
    let global_identity_name = secrets
        .get("GLOBAL_IDENTITY_NAME")
        .cloned()
        .unwrap_or_else(|| env::var("GLOBAL_IDENTITY_NAME").unwrap_or("@@localhost.sep-hanzo".to_string()));

    let global_identity_name = if global_identity_name.is_empty() {
        "@@localhost.sep-hanzo".to_string()
    } else {
        global_identity_name
    };

    // Initialization, creating Tokio runtime and fetching needed startup data
    let initial_llm_providers = fetch_llm_provider_env(global_identity_name.clone());
    let identity_secret_key_string =
        signature_secret_key_to_string(clone_signature_secret_key(&node_keys.identity_secret_key));
    let identity_public_key_string = signature_public_key_to_string(node_keys.identity_public_key);
    let encryption_secret_key_string = encryption_secret_key_to_string(node_keys.encryption_secret_key.clone());
    let encryption_public_key_string = encryption_public_key_to_string(node_keys.encryption_public_key);

    // Initialize Embedding Generator
    let embedding_generator = init_embedding_generator(&node_env);

    // Log the address, port, and public_key
    hanzo_log(
        HanzoLogOption::Node,
        HanzoLogLevel::Info,
        format!(
            "Starting node with address: {}, main db path: {}",
            node_env.api_listen_address, main_db_path
        )
        .as_str(),
    );
    hanzo_log(
        HanzoLogOption::Node,
        HanzoLogLevel::Info,
        format!(
            "identity sk: {} pk: {} encryption sk: {} pk: {}",
            identity_secret_key_string,
            identity_public_key_string,
            encryption_secret_key_string,
            encryption_public_key_string,
        )
        .as_str(),
    );
    hanzo_log(
        HanzoLogOption::Node,
        HanzoLogLevel::Info,
        format!("Initial LLM Provider: {:?}", initial_llm_providers).as_str(),
    );

    // CLI check
    if args.create_message {
        cli_handle_create_message(args, &node_keys, &global_identity_name);
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Node not started due to CLI message creation",
        )));
    }

    // Store secrets into machine filesystem `db.secret` file (needed if new secrets were generated)
    let identity_secret_key_string =
        signature_secret_key_to_string(clone_signature_secret_key(&node_keys.identity_secret_key));
    let encryption_secret_key_string = encryption_secret_key_to_string(node_keys.encryption_secret_key.clone());

    // Add the HTTPS certificates to the secret content
    let private_cert = node_keys
        .private_https_certificate
        .clone()
        .unwrap_or_default()
        .replace('\n', "\\n");

    let public_cert = node_keys
        .public_https_certificate
        .clone()
        .unwrap_or_default()
        .replace('\n', "\\n");

    let secret_content = format!(
        "GLOBAL_IDENTITY_NAME={}\nIDENTITY_SECRET_KEY={}\nENCRYPTION_SECRET_KEY={}\nPRIVATE_HTTPS_CERTIFICATE={}\nPUBLIC_HTTPS_CERTIFICATE={}",
        global_identity_name,
        identity_secret_key_string,
        encryption_secret_key_string,
        private_cert,
        public_cert
    );

    if !node_env.no_secrets_file {
        std::fs::create_dir_all(Path::new(&secrets_file_path.clone()).parent().unwrap())
            .expect("Failed to create .secret dir");
        std::fs::write(secrets_file_path.clone(), secret_content).expect("Unable to write to .secret file");
    }

    // Now that all core init data acquired, start running the node itself
    let (node_commands_sender, node_commands_receiver): (Sender<NodeCommand>, Receiver<NodeCommand>) = bounded(100);
    let node = Node::new(
        global_identity_name.clone().to_string(),
        node_env.listen_address,
        clone_signature_secret_key(&node_keys.identity_secret_key),
        node_keys.encryption_secret_key.clone(),
        node_keys.private_https_certificate.clone(),
        node_keys.public_https_certificate.clone(),
        node_env.ping_interval,
        node_commands_receiver,
        main_db_path.clone(),
        secrets_file_path.clone(),
        node_env.proxy_identity.clone(),
        node_env.first_device_needs_registration_code,
        initial_llm_providers,
        Some(embedding_generator),
        node_env.ws_address,
        node_env.default_embedding_model.clone(),
        node_env.supported_embedding_models.clone(),
        node_env.api_v2_key.clone(),
    )
    .await;

    // Install the canonical inference + embedding engine before any
    // request can fan out through the EVM precompiles (`0x0201` /
    // `0x0202`). NOTE: currently a no-op pending the engine 0.6.0→1.0.2
    // API port — the precompiles revert at call time ("fail open").
    // See `install_engine` FLAG.
    install_engine().await;

    // Put the Node in an Arc<Mutex<Node>> for use in a task
    let start_node = Arc::clone(&node);
    let node_copy = Arc::downgrade(&start_node.clone());

    // Copy of node commands center
    let node_commands_sender_copy = node_commands_sender.clone();

    // Setup API Server task
    let api_listen_address = node_env.clone().api_listen_address;
    let api_https_listen_address = node_env.clone().api_https_listen_address;
    let ws_listen_address = node_env.clone().ws_address.unwrap();
    let api_server = tokio::spawn(async move {
        match node_api_router::run_api(
            node_commands_sender,
            api_listen_address,
            api_https_listen_address,
            ws_listen_address,
            global_identity_name.clone().to_string(),
            node_keys.private_https_certificate.clone(),
            node_keys.public_https_certificate.clone(),
        )
        .await
        {
            Ok(_) => {
                hanzo_log(
                    HanzoLogOption::Node,
                    HanzoLogLevel::Info,
                    "API server started successfully",
                );
            }
            Err(e) => {
                hanzo_log(
                    HanzoLogOption::Node,
                    HanzoLogLevel::Error,
                    &format!("API server failed to start: {}", e),
                );
                panic!("API server failed to start: {}", e);
            }
        }
    });

    // Node task
    let node_task = tokio::spawn(async move { start_node.lock().await.start().await.unwrap() });

    // ZAP binary protocol server task
    let zap_listen_address = node_env.zap_address;
    let zap_commands_sender = node_commands_sender_copy.clone();
    let zap_task = tokio::spawn(async move {
        start_zap_server(zap_listen_address, zap_commands_sender).await;
    });

    print_node_info(
        &node_env,
        &encryption_public_key_string,
        &identity_public_key_string,
        &main_db_path,
    );

    // Return the node_commands_sender_copy and the tasks
    Ok((node_commands_sender_copy, api_server, node_task, zap_task, node_copy))
}

pub async fn run_node_tasks(
    api_server: JoinHandle<()>,
    node_task: JoinHandle<()>,
    zap_task: JoinHandle<()>,
    _: Weak<Mutex<Node>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_server_abort = api_server.abort_handle();
    let node_task_abort = node_task.abort_handle();
    let zap_task_abort = zap_task.abort_handle();

    match tokio::try_join!(api_server, node_task, zap_task) {
        Ok(_) => {
            hanzo_log(HanzoLogOption::Node, HanzoLogLevel::Info, "All tasks completed");
            Ok(())
        }
        Err(e) => {
            api_server_abort.abort();
            node_task_abort.abort();
            zap_task_abort.abort();

            Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))
        }
    }
}

/// Machine filesystem path to the main HanzoDB database, pub key based.
fn get_main_db_path(main_db: &str, identity_public_key: &VerifyingKey, node_storage_path: Option<String>) -> String {
    if let Some(path) = node_storage_path {
        Path::new(&path)
            .join(main_db)
            .join(hash_signature_public_key(identity_public_key))
            .to_str()
            .expect("Invalid NODE_STORAGE_PATH")
            .to_string()
    } else {
        Path::new(main_db)
            .join(hash_signature_public_key(identity_public_key))
            .into_os_string()
            .into_string()
            .unwrap()
    }
}

/// Machine filesystem path for .secret.
fn get_secrets_file_path(secrets_file: &str, node_storage_path: Option<String>) -> String {
    if let Some(path) = node_storage_path {
        Path::new(&path)
            .join(secrets_file)
            .to_str()
            .expect("Invalid NODE_STORAGE_PATH")
            .to_string()
    } else {
        Path::new(secrets_file).to_str().unwrap().to_string()
    }
}

/// Parses the secrets file ( `.secret`) from the machine's filesystem
/// This file holds the user's keys.
fn parse_secrets_file(secrets_file_path: &str) -> HashMap<String, String> {
    let contents = fs::read_to_string(secrets_file_path).unwrap_or_default();

    let mut map = HashMap::new();

    for line in contents.lines() {
        if let Some((key, value)) = line.split_once('=') {
            // Handle migration of old identity format for GLOBAL_IDENTITY_NAME
            if key == "GLOBAL_IDENTITY_NAME" {
                let updated_value = if value.contains(".arb-sep-hanzo") {
                    value.replace(".arb-sep-hanzo", ".sep-hanzo")
                } else {
                    value.to_string()
                };
                map.insert(key.to_string(), updated_value);
            } else {
                map.insert(key.to_string(), value.to_string());
            }
        }
    }

    map
}

/// Initializes RemoteEmbeddingGenerator struct using node environment/default embedding model for now
fn init_embedding_generator(node_env: &NodeEnvironment) -> RemoteEmbeddingGenerator {
    let api_url = node_env
        .embeddings_server_url
        .clone()
        .expect("EMBEDDINGS_SERVER_URL not found in node_env");
    let api_key = node_env.embeddings_server_api_key.clone();
    RemoteEmbeddingGenerator::new(node_env.default_embedding_model.clone(), &api_url, api_key)
}

/// Prints Useful Node information at startup
pub fn print_node_info(node_env: &NodeEnvironment, encryption_pk: &str, signature_pk: &str, main_db_path: &str) {
    println!("---------------------------------------------------------------");
    println!("Node API address: {}", node_env.api_listen_address);
    println!("Node API HTTPS address: {}", node_env.api_https_listen_address);
    println!("Node ZAP address: {}", node_env.zap_address);
    println!("Node TCP address: {}", node_env.listen_address);
    println!("Node WS address: {:?}", node_env.ws_address);
    println!(
        "Node Relayer address: {}",
        node_env.proxy_identity.as_deref().unwrap_or("None")
    );
    println!("Node Hanzo identity: {}", node_env.global_identity_name);
    println!("Node Main Profile: main (assumption)"); // Assuming "main" as the main profile
    println!("Node encryption pk: {}", encryption_pk);
    println!("Node signature pk: {}", signature_pk);
    println!("Main DB path: {}", main_db_path);
    println!("---------------------------------------------------------------");
}
