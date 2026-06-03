use super::network::Node;
use super::utils::environment::NodeEnvironment;
use crate::managers::federation::{FederationConfig, FederationManager};
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

/// Load + register the canonical [`hanzo_engine::MistralEngine`] used by
/// EVM precompiles `0x0201` (AI inference) and `0x0202` (AI embedding).
///
/// Configuration:
/// * `HANZO_MODEL_PATH` — path to a local GGUF or safetensors directory.
///   Preferred when set: avoids touching the network.
/// * `HANZO_MODEL_REPO` — Hugging Face repo (e.g. `Qwen/Qwen3-4B`). Used
///   when `HANZO_MODEL_PATH` is unset. The model is downloaded by
///   `mistralrs` the first time and cached.
///
/// If neither variable is set, or the load fails, the function logs and
/// returns. The precompiles then revert with
/// `EngineError::NoInferenceEngine` / `NoEmbeddingEngine` at call time.
/// This is the documented "fail open" contract: a node without a model
/// is still a valid node — it just can't serve those two precompiles.
async fn install_engine() {
    use std::sync::Arc;

    let load_result = match (env::var("HANZO_MODEL_PATH"), env::var("HANZO_MODEL_REPO")) {
        (Ok(path), _) if !path.is_empty() => {
            hanzo_log(
                HanzoLogOption::Node,
                HanzoLogLevel::Info,
                &format!("hanzo_engine: loading MistralEngine from path: {path}"),
            );
            hanzo_engine::MistralEngine::from_model_path(std::path::Path::new(&path))
                .await
                .map(|engine| (engine, format!("path={path}")))
        }
        (_, Ok(repo)) if !repo.is_empty() => {
            hanzo_log(
                HanzoLogOption::Node,
                HanzoLogLevel::Info,
                &format!("hanzo_engine: loading MistralEngine from HF repo: {repo}"),
            );
            hanzo_engine::MistralEngine::from_hf_repo(&repo)
                .await
                .map(|engine| (engine, format!("repo={repo}")))
        }
        _ => {
            hanzo_log(
                HanzoLogOption::Node,
                HanzoLogLevel::Info,
                "hanzo_engine: HANZO_MODEL_PATH and HANZO_MODEL_REPO unset; \
                 AI precompiles (0x0201, 0x0202) will revert until an engine is registered",
            );
            return;
        }
    };

    let (engine, source) = match load_result {
        Ok(pair) => pair,
        Err(e) => {
            hanzo_log(
                HanzoLogOption::Node,
                HanzoLogLevel::Error,
                &format!("hanzo_engine: model load failed ({e}); AI precompiles will revert"),
            );
            return;
        }
    };

    let arc: Arc<hanzo_engine::MistralEngine> = Arc::new(engine);

    // `MistralEngine` implements both `InferenceEngine` and `EmbeddingEngine`,
    // so the same `Arc` registers under both surfaces.
    if let Err(e) = hanzo_engine::register_inference_engine(arc.clone()) {
        hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Error,
            &format!("hanzo_engine: inference registration failed ({e})"),
        );
    }
    if let Err(e) = hanzo_engine::register_embedding_engine(arc) {
        hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Error,
            &format!("hanzo_engine: embedding registration failed ({e})"),
        );
    }

    hanzo_log(
        HanzoLogOption::Node,
        HanzoLogLevel::Info,
        &format!("hanzo_engine: MistralEngine registered ({source})"),
    );
}

/// `[zen5]` block in `hanzo.toml`. Opt-in. When `enabled = true` and the
/// `weights_dir` contains one or more `<variant>.gguf` files, hanzod
/// registers a [`hanzo_engine::Zen5Registry`] as the process inference
/// engine. Each variant's `model_id` is `sha256("<variant>:<weights_dir>/<variant>.gguf")`
/// — stable across restarts, so EVM precompiles can pin a specific model.
///
/// Coexists with `install_engine` (MistralEngine) on a first-writer-wins
/// basis: Zen5 runs first when enabled, so MistralEngine becomes a no-op
/// register-fails-and-logs path. The opposite layering is intentional —
/// Zen5 is the native fast-path; MistralEngine is the catch-all for HF
/// repos.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Zen5Config {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_zen5_weights_dir")]
    pub weights_dir: String,
    #[serde(default = "default_zen5_models")]
    pub models: Vec<String>,
    #[serde(default = "default_zen5_backend")]
    pub backend: String,
}

fn default_zen5_weights_dir() -> String {
    "/var/lib/hanzo/zen5".to_string()
}

fn default_zen5_models() -> Vec<String> {
    hanzo_engine::DEFAULT_VARIANTS
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn default_zen5_backend() -> String {
    "auto".to_string()
}

impl Default for Zen5Config {
    fn default() -> Self {
        Self {
            enabled: false,
            weights_dir: default_zen5_weights_dir(),
            models: default_zen5_models(),
            backend: default_zen5_backend(),
        }
    }
}

impl Zen5Config {
    /// Load from the `[zen5]` table of `hanzo.toml`. Returns a disabled
    /// default if the file or section is absent.
    pub fn load_from(path: &Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        let value: toml::Value = match toml::from_str(&raw) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let Some(table) = value.get("zen5") else {
            return Self::default();
        };
        match table.clone().try_into::<Zen5Config>() {
            Ok(c) => c,
            Err(e) => {
                hanzo_log(
                    HanzoLogOption::Node,
                    HanzoLogLevel::Error,
                    &format!("[zen5] parse error in {}: {e}", path.display()),
                );
                Self::default()
            }
        }
    }
}

/// Register the Zen5 family with `hanzo_engine` if `[zen5]` is enabled in
/// `hanzo.toml`. No-op when disabled. See [`Zen5Config`] for layering rules.
fn install_zen5_engines() {
    let config_path = env::var("HANZO_CONFIG_PATH").unwrap_or_else(|_| "hanzo.toml".to_string());
    let cfg = Zen5Config::load_from(Path::new(&config_path));
    if !cfg.enabled {
        hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Debug,
            "[zen5] disabled (no [zen5] block or enabled=false)",
        );
        return;
    }
    let weights_dir = Path::new(&cfg.weights_dir);
    if !weights_dir.is_dir() {
        hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Info,
            &format!(
                "[zen5] enabled but weights_dir {} not found; nothing registered",
                weights_dir.display()
            ),
        );
        return;
    }
    let variants: Vec<&str> = cfg.models.iter().map(|s| s.as_str()).collect();
    match hanzo_engine::register_zen5_engines_at_startup(weights_dir, &variants) {
        Ok(registry) => hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Info,
            &format!(
                "[zen5] registered {} models from {} (backend={})",
                registry.len(),
                weights_dir.display(),
                cfg.backend,
            ),
        ),
        Err(e) => hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Error,
            &format!("[zen5] register failed: {e}"),
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

    // Inference engine boot. Two paths share the process-wide registry
    // (first writer wins):
    //   1. Zen5 family registry (zen-5-flash/pro/mini/coder) — native,
    //      gated by `[zen5] enabled = true` in hanzo.toml. Runs first.
    //   2. MistralEngine — fallback / HF-repo path. Runs second; if
    //      Zen5 already claimed the slot the register call is a no-op
    //      that we log + ignore.
    install_zen5_engines();
    install_engine().await;

    // Federation (lab mode) — opt-in via `[federation]` in hanzo.toml.
    // Loads config, elects role, spawns coordinator OR worker. When the
    // `federation-runtime` feature is off we log and park; the rest of
    // the node keeps running. The manager is leaked into a static slot
    // so the spawned task lives for the process lifetime; node tasks
    // own their own teardown and shut us down via Drop on process exit.
    init_federation();

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

/// Boot the federation manager from `hanzo.toml` (or `HANZO_CONFIG_PATH`).
///
/// The manager is opt-in: if `[federation]` is absent or `enabled = false`,
/// this is a no-op log line. The manager is stored in a process-lifetime
/// `OnceLock` so its background task survives until process exit; Drop
/// will signal the spawned coordinator/worker if the slot is ever cleared.
fn init_federation() {
    use std::sync::OnceLock;
    static FEDERATION: OnceLock<tokio::sync::Mutex<FederationManager>> = OnceLock::new();

    let config_path = env::var("HANZO_CONFIG_PATH").unwrap_or_else(|_| "hanzo.toml".to_string());
    let cfg = FederationConfig::load_from(Path::new(&config_path));
    if !cfg.enabled {
        hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Debug,
            "[federation] disabled (no [federation] block or enabled=false)",
        );
        return;
    }
    let mgr = FEDERATION.get_or_init(|| tokio::sync::Mutex::new(FederationManager::new(cfg)));
    // Use try_lock — at boot nothing else holds it. If contended (unexpected),
    // log and skip rather than block the runner.
    match mgr.try_lock() {
        Ok(mut guard) => guard.start(),
        Err(_) => hanzo_log(
            HanzoLogOption::Node,
            HanzoLogLevel::Error,
            "[federation] manager already locked at boot; not starting",
        ),
    }
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
