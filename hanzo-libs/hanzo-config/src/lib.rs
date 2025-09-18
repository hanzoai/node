use std::path::{Path, PathBuf};
use std::fs;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Unified configuration for all Hanzo components
/// This ensures consistency across:
/// - hanzoai (engine CLI)
/// - hanzod (network node with web exposure)
/// - app (~/work/hanzo/app)
/// - All other Hanzo tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HanzoConfig {
    // Paths
    pub hanzo_home: PathBuf,
    pub models_dir: PathBuf,
    pub embeddings_dir: PathBuf,
    pub rerankers_dir: PathBuf,
    pub llms_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub config_dir: PathBuf,

    // Storage settings
    pub max_cache_size_gb: f64,
    pub max_models_size_gb: f64,
    pub auto_cleanup: bool,

    // Vector DB settings (LanceDB for RAG)
    pub lancedb_path: PathBuf,
    pub lancedb_max_size_gb: f64,
    pub enable_vector_search: bool,
    pub enable_full_text_search: bool,

    // Web exposure settings (for hanzod)
    pub web_enabled: bool,
    pub web_host: String,
    pub web_port: u16,
    pub api_host: String,
    pub api_port: u16,
    pub ws_enabled: bool,      // WebSocket support
    pub ws_port: Option<u16>,  // WebSocket port (if different from api_port)
    pub p2p_port: u16,         // P2P consensus port
    pub public_url: Option<String>,
    pub enable_cors: bool,
    pub allowed_origins: Vec<String>,

    // Engine settings
    pub engine_binary: PathBuf,
    pub engine_threads: usize,
    pub engine_gpu_layers: Option<u32>,
    pub engine_batch_size: usize,

    // Model defaults
    pub default_embedding_model: String,
    pub default_reranker_model: String,
    pub default_llm_model: String,

    // API keys (encrypted storage)
    pub api_keys_file: PathBuf,
}

impl Default for HanzoConfig {
    fn default() -> Self {
        let home = dirs::home_dir()
            .expect("Could not find home directory")
            .join(".hanzo");

        Self {
            hanzo_home: home.clone(),
            models_dir: home.join("models"),
            embeddings_dir: home.join("models/embeddings"),
            rerankers_dir: home.join("models/rerankers"),
            llms_dir: home.join("models/llms"),
            cache_dir: home.join("cache"),
            data_dir: home.join("data"),
            logs_dir: home.join("logs"),
            config_dir: home.join("config"),

            // Storage: Default to 100GB for models, 10GB for cache
            max_cache_size_gb: 10.0,
            max_models_size_gb: 100.0,
            auto_cleanup: true,

            // LanceDB for vector storage (RAG backend)
            lancedb_path: home.join("data/lancedb"),
            lancedb_max_size_gb: 50.0,
            enable_vector_search: true,
            enable_full_text_search: true,

            // Web exposure (for public access via hanzod)
            web_enabled: true,
            web_host: "0.0.0.0".to_string(),
            web_port: 3692,  // Web interface port (3690 + 2)
            api_host: "0.0.0.0".to_string(),
            api_port: 3690,  // Main hanzod port (API + WebSocket)
            ws_enabled: true,
            ws_port: None,  // Use same port as API (3690) for WebSocket
            p2p_port: 3691,  // P2P consensus port (3690 + 1)
            public_url: None,
            enable_cors: true,
            allowed_origins: vec!["*".to_string()],

            // Engine settings
            engine_binary: home.join("bin/hanzo-engine"),
            engine_threads: num_cpus::get(),
            engine_gpu_layers: None,
            engine_batch_size: 32,

            // Model defaults - prioritize 8B models
            default_embedding_model: "qwen3-embedding-8b".to_string(),
            default_reranker_model: "qwen3-reranker-8b".to_string(),
            default_llm_model: "qwen3-8b-instruct".to_string(),

            // Security
            api_keys_file: home.join("config/api_keys.encrypted"),
        }
    }
}

impl HanzoConfig {
    /// Load config from ~/.hanzo/config/hanzo.toml or create default
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = Self::default().config_dir.join("hanzo.toml");

        if config_path.exists() {
            let contents = fs::read_to_string(&config_path)?;
            let config: Self = toml::from_str(&contents)?;
            Ok(config)
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }

    /// Save config to ~/.hanzo/config/hanzo.toml
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(&self.config_dir)?;
        let config_path = self.config_dir.join("hanzo.toml");
        let contents = toml::to_string_pretty(self)?;
        fs::write(&config_path, contents)?;
        Ok(())
    }

    /// Ensure all directories exist
    pub fn ensure_directories(&self) -> Result<(), Box<dyn std::error::Error>> {
        let dirs = vec![
            &self.hanzo_home,
            &self.models_dir,
            &self.embeddings_dir,
            &self.rerankers_dir,
            &self.llms_dir,
            &self.cache_dir,
            &self.data_dir,
            &self.logs_dir,
            &self.config_dir,
            &self.lancedb_path,
        ];

        for dir in dirs {
            fs::create_dir_all(dir)?;
        }

        Ok(())
    }

    /// Get storage usage statistics
    pub fn get_storage_stats(&self) -> StorageStats {
        StorageStats {
            models_size_gb: get_dir_size_gb(&self.models_dir),
            cache_size_gb: get_dir_size_gb(&self.cache_dir),
            lancedb_size_gb: get_dir_size_gb(&self.lancedb_path),
            total_size_gb: get_dir_size_gb(&self.hanzo_home),
        }
    }

    /// Clean up old cache files if over limit
    pub fn cleanup_cache(&self) -> Result<usize, Box<dyn std::error::Error>> {
        if !self.auto_cleanup {
            return Ok(0);
        }

        let current_size = get_dir_size_gb(&self.cache_dir);
        if current_size <= self.max_cache_size_gb {
            return Ok(0);
        }

        // Clean oldest files first
        let mut entries: Vec<_> = fs::read_dir(&self.cache_dir)?
            .filter_map(|e| e.ok())
            .collect();

        entries.sort_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or_else(|_| std::time::SystemTime::UNIX_EPOCH)
        });

        let mut deleted = 0;
        let mut current_size = current_size;

        for entry in entries {
            if current_size <= self.max_cache_size_gb * 0.8 {
                break;
            }

            if let Ok(metadata) = entry.metadata() {
                let size_gb = metadata.len() as f64 / 1_073_741_824.0;
                fs::remove_file(entry.path())?;
                current_size -= size_gb;
                deleted += 1;
            }
        }

        Ok(deleted)
    }

    /// Get path for a specific model
    pub fn get_model_path(&self, model_name: &str) -> PathBuf {
        if model_name.contains("embed") {
            self.embeddings_dir.join(model_name)
        } else if model_name.contains("rerank") {
            self.rerankers_dir.join(model_name)
        } else {
            self.llms_dir.join(model_name)
        }
    }

    /// Check if model is downloaded
    pub fn is_model_downloaded(&self, model_name: &str) -> bool {
        let path = self.get_model_path(model_name);
        path.exists() && path.is_dir()
    }

    /// Get public URL for web exposure
    pub fn get_public_url(&self) -> String {
        self.public_url.clone().unwrap_or_else(|| {
            format!("http://{}:{}", self.web_host, self.web_port)
        })
    }

    /// Get API URL
    pub fn get_api_url(&self) -> String {
        format!("http://{}:{}", self.api_host, self.api_port)
    }

    /// Get WebSocket URL
    pub fn get_ws_url(&self) -> String {
        let port = self.ws_port.unwrap_or(self.api_port);
        format!("ws://{}:{}", self.api_host, port)
    }

    /// Get public WebSocket URL
    pub fn get_public_ws_url(&self) -> String {
        if let Some(ref public_url) = self.public_url {
            // Convert http/https to ws/wss
            public_url
                .replace("https://", "wss://")
                .replace("http://", "ws://")
        } else {
            self.get_ws_url()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub models_size_gb: f64,
    pub cache_size_gb: f64,
    pub lancedb_size_gb: f64,
    pub total_size_gb: f64,
}

fn get_dir_size_gb(path: &Path) -> f64 {
    if !path.exists() {
        return 0.0;
    }

    let size = walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum::<u64>();

    size as f64 / 1_073_741_824.0
}

/// Global config instance for all Hanzo components
pub struct GlobalConfig {
    inner: Arc<RwLock<HanzoConfig>>,
}

impl GlobalConfig {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HanzoConfig::default())),
        }
    }

    pub async fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let config = HanzoConfig::load()?;
        Ok(Self {
            inner: Arc::new(RwLock::new(config)),
        })
    }

    pub async fn get(&self) -> HanzoConfig {
        self.inner.read().await.clone()
    }

    pub async fn update<F>(&self, f: F) -> Result<(), Box<dyn std::error::Error>>
    where
        F: FnOnce(&mut HanzoConfig),
    {
        let mut config = self.inner.write().await;
        f(&mut *config);
        config.save()?;
        Ok(())
    }
}

/// Initialize Hanzo environment for all tools
pub async fn init_hanzo_environment() -> Result<GlobalConfig, Box<dyn std::error::Error>> {
    let config = GlobalConfig::load().await?;
    let cfg = config.get().await;
    cfg.ensure_directories()?;

    // Create initial config files if they don't exist
    let hanzo_toml = cfg.config_dir.join("hanzo.toml");
    if !hanzo_toml.exists() {
        cfg.save()?;
    }

    // Initialize LanceDB directory structure
    if cfg.enable_vector_search {
        fs::create_dir_all(&cfg.lancedb_path)?;
    }

    // Clean cache if needed
    if cfg.auto_cleanup {
        let _ = cfg.cleanup_cache();
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HanzoConfig::default();
        assert_eq!(config.default_embedding_model, "qwen3-embedding-8b");
        assert_eq!(config.default_reranker_model, "qwen3-reranker-8b");
        assert!(config.hanzo_home.ends_with(".hanzo"));
    }

    #[tokio::test]
    async fn test_global_config() {
        let config = GlobalConfig::new();
        let cfg = config.get().await;
        assert_eq!(cfg.web_port, 3692);
        assert_eq!(cfg.api_port, 3690);
    }
}