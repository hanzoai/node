use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::env;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub node: NodeConfig,
    pub network: NetworkConfig,
    pub logging: LoggingConfig,
    pub embeddings: EmbeddingsConfig,
    pub engine: EngineConfig,
    pub database: DatabaseConfig,
    pub security: SecurityConfig,
    pub llm_providers: LLMProvidersConfig,
    pub wallets: WalletsConfig,
    pub tools: ToolsConfig,
    pub performance: PerformanceConfig,
    pub development: DevelopmentConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NodeConfig {
    pub ip: String,
    pub port: u16,
    pub api_ip: String,
    pub api_port: u16,
    pub global_identity_name: String,
    pub starting_num_qr_profiles: u32,
    pub starting_num_qr_devices: u32,
    pub first_device_needs_registration_code: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NetworkConfig {
    pub ping_interval_secs: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LoggingConfig {
    pub rust_log: String,
    pub log_simple: bool,
    pub log_all: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EmbeddingsConfig {
    pub use_native_embeddings: bool,
    pub use_gpu: bool,
    pub default_embedding_model: String,
    pub reranker_model: Option<String>,
    pub embeddings_server_url: Option<String>,
    pub embeddings_server_api_key: Option<String>,
    pub native_model_path: Option<String>,
    pub reranker_model_path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EngineConfig {
    pub use_local_engine: bool,
    pub engine_pool_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabaseConfig {
    pub path: String,
    pub max_connections: u32,
    pub connection_timeout: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SecurityConfig {
    pub pqc_enabled: bool,
    pub privacy_tier: u8,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LLMProvidersConfig {
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub together_api_key: Option<String>,
    pub groq_api_key: Option<String>,
    pub ollama_base_url: String,
    pub lm_studio_base_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WalletsConfig {
    pub coinbase_mpc_enabled: Option<bool>,
    pub ethereum_enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolsConfig {
    pub mcp_enabled: bool,
    pub javascript_runtime: String,
    pub python_runtime: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PerformanceConfig {
    pub job_queue_workers: u32,
    pub max_concurrent_jobs: u32,
    pub request_timeout: u64,
    pub stream_timeout: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DevelopmentConfig {
    pub swagger_ui_enabled: bool,
    pub debug_mode: bool,
    pub mock_providers_enabled: bool,
}

impl Config {
    /// Load configuration from file or environment
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        // Try to load from config file first
        let config_path = env::var("HANZO_CONFIG_PATH")
            .unwrap_or_else(|_| "hanzo.toml".to_string());
        
        if Path::new(&config_path).exists() {
            let config_str = fs::read_to_string(config_path)?;
            let mut config: Config = toml::from_str(&config_str)?;
            
            // Override with environment variables
            config.override_with_env();
            
            Ok(config)
        } else {
            // Load from environment variables only
            Ok(Self::from_env())
        }
    }
    
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        Config {
            node: NodeConfig {
                ip: env::var("NODE_IP").unwrap_or_else(|_| "0.0.0.0".to_string()),
                port: env::var("NODE_PORT")
                    .unwrap_or_else(|_| "3691".to_string())
                    .parse()
                    .unwrap_or(3691),
                api_ip: env::var("NODE_API_IP").unwrap_or_else(|_| "0.0.0.0".to_string()),
                api_port: env::var("NODE_API_PORT")
                    .unwrap_or_else(|_| "3690".to_string())
                    .parse()
                    .unwrap_or(3690),
                global_identity_name: env::var("GLOBAL_IDENTITY_NAME")
                    .unwrap_or_else(|_| "@@localhost.sep-hanzo".to_string()),
                starting_num_qr_profiles: env::var("STARTING_NUM_QR_PROFILES")
                    .unwrap_or_else(|_| "1".to_string())
                    .parse()
                    .unwrap_or(1),
                starting_num_qr_devices: env::var("STARTING_NUM_QR_DEVICES")
                    .unwrap_or_else(|_| "1".to_string())
                    .parse()
                    .unwrap_or(1),
                first_device_needs_registration_code: env::var("FIRST_DEVICE_NEEDS_REGISTRATION_CODE")
                    .unwrap_or_else(|_| "false".to_string())
                    .parse()
                    .unwrap_or(false),
            },
            network: NetworkConfig {
                ping_interval_secs: env::var("PING_INTERVAL_SECS")
                    .unwrap_or_else(|_| "0".to_string())
                    .parse()
                    .unwrap_or(0),
            },
            logging: LoggingConfig {
                rust_log: env::var("RUST_LOG")
                    .unwrap_or_else(|_| "debug,error,info".to_string()),
                log_simple: env::var("LOG_SIMPLE")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .unwrap_or(true),
                log_all: env::var("LOG_ALL")
                    .unwrap_or_else(|_| "false".to_string())
                    .parse()
                    .unwrap_or(false),
            },
            embeddings: EmbeddingsConfig {
                use_native_embeddings: env::var("USE_NATIVE_EMBEDDINGS")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .unwrap_or(true),
                use_gpu: env::var("USE_GPU")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .unwrap_or(true),
                default_embedding_model: env::var("DEFAULT_EMBEDDING_MODEL")
                    .unwrap_or_else(|_| "qwen3-embedding-8b".to_string()),
                reranker_model: env::var("RERANKER_MODEL").ok(),
                embeddings_server_url: env::var("EMBEDDINGS_SERVER_URL").ok(),
                embeddings_server_api_key: env::var("EMBEDDINGS_SERVER_API_KEY").ok(),
                native_model_path: env::var("NATIVE_MODEL_PATH").ok(),
                reranker_model_path: env::var("RERANKER_MODEL_PATH").ok(),
            },
            engine: EngineConfig {
                use_local_engine: env::var("USE_LOCAL_ENGINE")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .unwrap_or(true),
                engine_pool_url: env::var("ENGINE_POOL_URL").ok(),
            },
            database: DatabaseConfig {
                path: env::var("DATABASE_PATH")
                    .unwrap_or_else(|_| "./storage/db.sqlite".to_string()),
                max_connections: 10,
                connection_timeout: 30,
            },
            security: SecurityConfig {
                pqc_enabled: false,
                privacy_tier: 0,
            },
            llm_providers: LLMProvidersConfig {
                openai_api_key: env::var("OPENAI_API_KEY").ok(),
                anthropic_api_key: env::var("ANTHROPIC_API_KEY").ok(),
                together_api_key: env::var("TOGETHER_API_KEY").ok(),
                groq_api_key: env::var("GROQ_API_KEY").ok(),
                ollama_base_url: env::var("OLLAMA_BASE_URL")
                    .unwrap_or_else(|_| "http://localhost:11434".to_string()),
                lm_studio_base_url: env::var("LM_STUDIO_BASE_URL")
                    .unwrap_or_else(|_| "http://localhost:1234".to_string()),
            },
            wallets: WalletsConfig {
                coinbase_mpc_enabled: None,
                ethereum_enabled: None,
            },
            tools: ToolsConfig {
                mcp_enabled: true,
                javascript_runtime: "deno".to_string(),
                python_runtime: "uv".to_string(),
            },
            performance: PerformanceConfig {
                job_queue_workers: 4,
                max_concurrent_jobs: 10,
                request_timeout: 60,
                stream_timeout: 300,
            },
            development: DevelopmentConfig {
                swagger_ui_enabled: cfg!(feature = "swagger-ui"),
                debug_mode: false,
                mock_providers_enabled: false,
            },
        }
    }
    
    /// Override configuration with environment variables
    fn override_with_env(&mut self) {
        // Override node settings
        if let Ok(ip) = env::var("NODE_IP") {
            self.node.ip = ip;
        }
        if let Ok(port) = env::var("NODE_PORT") {
            if let Ok(p) = port.parse() {
                self.node.port = p;
            }
        }
        
        // Override other settings as needed...
        // This is a simplified version - you can expand this to override all fields
    }
    
    /// Export configuration to environment variables
    pub fn export_to_env(&self) {
        env::set_var("NODE_IP", &self.node.ip);
        env::set_var("NODE_PORT", self.node.port.to_string());
        env::set_var("NODE_API_IP", &self.node.api_ip);
        env::set_var("NODE_API_PORT", self.node.api_port.to_string());
        env::set_var("GLOBAL_IDENTITY_NAME", &self.node.global_identity_name);
        env::set_var("RUST_LOG", &self.logging.rust_log);
        env::set_var("USE_NATIVE_EMBEDDINGS", self.embeddings.use_native_embeddings.to_string());
        env::set_var("USE_GPU", self.embeddings.use_gpu.to_string());
        env::set_var("DEFAULT_EMBEDDING_MODEL", &self.embeddings.default_embedding_model);
        env::set_var("USE_LOCAL_ENGINE", self.engine.use_local_engine.to_string());
        
        // Export optional values
        if let Some(url) = &self.embeddings.embeddings_server_url {
            env::set_var("EMBEDDINGS_SERVER_URL", url);
        }
        if let Some(key) = &self.llm_providers.openai_api_key {
            env::set_var("OPENAI_API_KEY", key);
        }
        // ... export other fields as needed
    }
}