//! Storage backends for AI artifacts
//!
//! Supports multiple storage backends:
//! - Local filesystem
//! - HuggingFace Hub (fallback/mirror)
//! - P2P swarm (BitTorrent-style)
//! - IPFS
//! - Node operator storage

use crate::{
    error::{AiFormatError, Result},
    AiArtifact, ArtifactId, ArtifactMetadata, ArtifactRef, StorageLocation,
};
use async_trait::async_trait;
use dashmap::DashMap;
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tracing::{debug, info, warn};

/// Storage backend trait
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Check if an artifact exists
    async fn exists(&self, id: &ArtifactId) -> Result<bool>;

    /// Get artifact metadata
    async fn get_metadata(&self, id: &ArtifactId) -> Result<ArtifactMetadata>;

    /// Download an artifact
    async fn download(&self, id: &ArtifactId, dest: &Path) -> Result<PathBuf>;

    /// Upload an artifact
    async fn upload(&self, artifact: &AiArtifact) -> Result<StorageLocation>;

    /// List available artifacts
    async fn list(&self) -> Result<Vec<ArtifactRef>>;

    /// Delete an artifact
    async fn delete(&self, id: &ArtifactId) -> Result<()>;

    /// Get the storage location type
    fn location_type(&self) -> &'static str;
}

/// Multi-backend storage manager
pub struct Storage {
    /// Local storage (primary)
    local: LocalStorage,
    /// HuggingFace storage (fallback)
    huggingface: Option<HuggingFaceStorage>,
    /// IPFS storage
    ipfs: Option<IpfsStorage>,
    /// Node operator storage endpoints
    node_storage: Vec<NodeStorage>,
    /// Artifact cache
    cache: Arc<DashMap<ArtifactId, PathBuf>>,
}

impl Storage {
    /// Create a new storage manager with local storage only
    pub fn new(local_path: impl Into<PathBuf>) -> Self {
        Self {
            local: LocalStorage::new(local_path),
            huggingface: None,
            ipfs: None,
            node_storage: Vec::new(),
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Create storage with HuggingFace fallback
    pub fn with_hf_fallback(local_path: impl Into<PathBuf>, hf_token: Option<String>) -> Self {
        Self {
            local: LocalStorage::new(local_path),
            huggingface: Some(HuggingFaceStorage::new(hf_token)),
            ipfs: None,
            node_storage: Vec::new(),
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Add IPFS backend
    pub fn with_ipfs(mut self, gateway: impl Into<String>) -> Self {
        self.ipfs = Some(IpfsStorage::new(gateway));
        self
    }

    /// Add node operator storage
    pub fn with_node_storage(mut self, peer_id: impl Into<String>, endpoint: impl Into<String>) -> Self {
        self.node_storage.push(NodeStorage::new(peer_id, endpoint));
        self
    }

    /// Get artifact, trying backends in order
    pub async fn get(&self, id: &ArtifactId) -> Result<PathBuf> {
        // Check cache first
        if let Some(path) = self.cache.get(id) {
            if path.exists() {
                return Ok(path.clone());
            }
        }

        // Try local storage first
        if self.local.exists(id).await? {
            let path = self.local.get_path(id);
            self.cache.insert(id.clone(), path.clone());
            return Ok(path);
        }

        // Try node operator storage
        for node in &self.node_storage {
            if let Ok(true) = node.exists(id).await {
                let path = node.download(id, &self.local.base_path).await?;
                self.cache.insert(id.clone(), path.clone());
                return Ok(path);
            }
        }

        // Try HuggingFace fallback
        if let Some(hf) = &self.huggingface {
            if let Ok(true) = hf.exists(id).await {
                let path = hf.download(id, &self.local.base_path).await?;
                self.cache.insert(id.clone(), path.clone());
                return Ok(path);
            }
        }

        // Try IPFS
        if let Some(ipfs) = &self.ipfs {
            if let Ok(true) = ipfs.exists(id).await {
                let path = ipfs.download(id, &self.local.base_path).await?;
                self.cache.insert(id.clone(), path.clone());
                return Ok(path);
            }
        }

        Err(AiFormatError::artifact_not_found(id))
    }

    /// Store artifact locally and optionally replicate
    pub async fn store(&self, artifact: &AiArtifact, replicate: bool) -> Result<Vec<StorageLocation>> {
        let mut locations = Vec::new();

        // Always store locally
        let local_loc = self.local.upload(artifact).await?;
        locations.push(local_loc);

        if replicate {
            // Replicate to HuggingFace if available
            if let Some(hf) = &self.huggingface {
                match hf.upload(artifact).await {
                    Ok(loc) => locations.push(loc),
                    Err(e) => warn!("Failed to replicate to HuggingFace: {}", e),
                }
            }

            // Replicate to IPFS if available
            if let Some(ipfs) = &self.ipfs {
                match ipfs.upload(artifact).await {
                    Ok(loc) => locations.push(loc),
                    Err(e) => warn!("Failed to replicate to IPFS: {}", e),
                }
            }
        }

        Ok(locations)
    }

    /// Load an artifact from storage
    pub async fn load(&self, id: &ArtifactId) -> Result<AiArtifact> {
        let path = self.get(id).await?;
        AiArtifact::load(&path).await
    }

    /// Get local storage path
    pub fn local_path(&self) -> &Path {
        &self.local.base_path
    }

    /// Clear cache
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

/// Local filesystem storage
pub struct LocalStorage {
    base_path: PathBuf,
}

impl LocalStorage {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    pub fn get_path(&self, id: &ArtifactId) -> PathBuf {
        self.base_path.join(format!("{}.ai", id))
    }

    /// Get default storage path
    pub fn default_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("hanzo")
            .join("artifacts")
    }
}

#[async_trait]
impl StorageBackend for LocalStorage {
    async fn exists(&self, id: &ArtifactId) -> Result<bool> {
        Ok(self.get_path(id).exists())
    }

    async fn get_metadata(&self, id: &ArtifactId) -> Result<ArtifactMetadata> {
        let artifact = AiArtifact::load(self.get_path(id)).await?;
        Ok(artifact.metadata)
    }

    async fn download(&self, id: &ArtifactId, _dest: &Path) -> Result<PathBuf> {
        let path = self.get_path(id);
        if path.exists() {
            Ok(path)
        } else {
            Err(AiFormatError::artifact_not_found(id))
        }
    }

    async fn upload(&self, artifact: &AiArtifact) -> Result<StorageLocation> {
        fs::create_dir_all(&self.base_path).await?;
        let path = self.get_path(&artifact.metadata.id);
        let mut artifact = artifact.clone();
        artifact.save(&path).await?;
        info!("Saved artifact {} to {:?}", artifact.metadata.id, path);
        Ok(StorageLocation::local(path.to_string_lossy()))
    }

    async fn list(&self) -> Result<Vec<ArtifactRef>> {
        let mut refs = Vec::new();

        if !self.base_path.exists() {
            return Ok(refs);
        }

        let mut entries = fs::read_dir(&self.base_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "ai").unwrap_or(false) {
                if let Ok(artifact) = AiArtifact::load(&path).await {
                    refs.push(ArtifactRef::new(
                        artifact.metadata.content_hash,
                        artifact.metadata.name,
                        artifact.metadata.version,
                    ));
                }
            }
        }

        Ok(refs)
    }

    async fn delete(&self, id: &ArtifactId) -> Result<()> {
        let path = self.get_path(id);
        if path.exists() {
            fs::remove_file(path).await?;
        }
        Ok(())
    }

    fn location_type(&self) -> &'static str {
        "local"
    }
}

/// HuggingFace Hub storage (fallback)
pub struct HuggingFaceStorage {
    client: Client,
    token: Option<String>,
    api_base: String,
}

impl HuggingFaceStorage {
    pub fn new(token: Option<String>) -> Self {
        Self {
            client: Client::new(),
            token,
            api_base: "https://huggingface.co/api".to_string(),
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.api_base = endpoint.into();
        self
    }

    /// Parse artifact ID to repo_id format
    fn parse_repo_id(id: &ArtifactId) -> String {
        // Format: org/model or model -> hanzo-ai/model
        if id.contains('/') {
            id.clone()
        } else {
            format!("hanzo-ai/{}", id)
        }
    }
}

#[async_trait]
impl StorageBackend for HuggingFaceStorage {
    async fn exists(&self, id: &ArtifactId) -> Result<bool> {
        let repo_id = Self::parse_repo_id(id);
        let url = format!("{}/models/{}", self.api_base, repo_id);

        let mut req = self.client.head(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    async fn get_metadata(&self, id: &ArtifactId) -> Result<ArtifactMetadata> {
        let repo_id = Self::parse_repo_id(id);
        let url = format!("{}/models/{}", self.api_base, repo_id);

        let mut req = self.client.get(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(AiFormatError::artifact_not_found(id));
        }

        let data: serde_json::Value = resp.json().await?;

        // Convert HF metadata to our format
        let name = data["modelId"].as_str().unwrap_or(id).to_string();
        let metadata = ArtifactMetadata::new(&name, crate::ArtifactType::Model);

        Ok(metadata)
    }

    async fn download(&self, id: &ArtifactId, dest: &Path) -> Result<PathBuf> {
        let repo_id = Self::parse_repo_id(id);
        debug!("Downloading from HuggingFace: {}", repo_id);

        // Download the .ai file if it exists, or construct from repo
        let url = format!(
            "https://huggingface.co/{}/resolve/main/artifact.ai",
            repo_id
        );

        let mut req = self.client.get(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(AiFormatError::HuggingFace(format!(
                "Failed to download {}: {}",
                repo_id,
                resp.status()
            )));
        }

        let bytes = resp.bytes().await?;
        let dest_path = dest.join(format!("{}.ai", id.replace('/', "_")));

        fs::create_dir_all(dest).await?;
        fs::write(&dest_path, bytes).await?;

        info!("Downloaded {} to {:?}", repo_id, dest_path);
        Ok(dest_path)
    }

    async fn upload(&self, artifact: &AiArtifact) -> Result<StorageLocation> {
        let _token = self.token.as_ref().ok_or_else(|| {
            AiFormatError::HuggingFace("HuggingFace token required for upload".to_string())
        })?;

        let repo_id = format!("hanzo-ai/{}", artifact.metadata.name);

        // Create repo if needed (simplified - real impl would use HF API properly)
        debug!("Would upload to HuggingFace repo: {}", repo_id);

        // For now, just return the location hint
        Ok(StorageLocation::huggingface(&repo_id))
    }

    async fn list(&self) -> Result<Vec<ArtifactRef>> {
        // List artifacts from hanzo-ai organization
        let url = format!("{}/models?author=hanzo-ai", self.api_base);

        let mut req = self.client.get(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }

        let data: Vec<serde_json::Value> = resp.json().await?;

        let refs = data
            .iter()
            .filter_map(|model| {
                let id = model["modelId"].as_str()?;
                Some(ArtifactRef::new(
                    String::new(), // No hash from HF
                    id.to_string(),
                    "latest".to_string(),
                ))
            })
            .collect();

        Ok(refs)
    }

    async fn delete(&self, _id: &ArtifactId) -> Result<()> {
        Err(AiFormatError::HuggingFace(
            "Deletion not supported via API".to_string(),
        ))
    }

    fn location_type(&self) -> &'static str {
        "huggingface"
    }
}

/// IPFS storage backend
pub struct IpfsStorage {
    client: Client,
    gateway: String,
}

impl IpfsStorage {
    pub fn new(gateway: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            gateway: gateway.into(),
        }
    }

    pub fn default_gateway() -> Self {
        Self::new("https://ipfs.io")
    }
}

#[async_trait]
impl StorageBackend for IpfsStorage {
    async fn exists(&self, id: &ArtifactId) -> Result<bool> {
        // For IPFS, the id should be a CID
        let url = format!("{}/ipfs/{}", self.gateway, id);

        match self.client.head(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    async fn get_metadata(&self, id: &ArtifactId) -> Result<ArtifactMetadata> {
        // Download and parse the artifact to get metadata
        let artifact = self.download_artifact(id).await?;
        Ok(artifact.metadata)
    }

    async fn download(&self, id: &ArtifactId, dest: &Path) -> Result<PathBuf> {
        let url = format!("{}/ipfs/{}", self.gateway, id);
        debug!("Downloading from IPFS: {}", url);

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(AiFormatError::Network(format!(
                "IPFS download failed: {}",
                resp.status()
            )));
        }

        let bytes = resp.bytes().await?;
        let dest_path = dest.join(format!("{}.ai", id));

        fs::create_dir_all(dest).await?;
        fs::write(&dest_path, bytes).await?;

        info!("Downloaded IPFS {} to {:?}", id, dest_path);
        Ok(dest_path)
    }

    async fn upload(&self, _artifact: &AiArtifact) -> Result<StorageLocation> {
        // IPFS upload would require pinning service
        Err(AiFormatError::Network(
            "IPFS upload requires pinning service configuration".to_string(),
        ))
    }

    async fn list(&self) -> Result<Vec<ArtifactRef>> {
        // IPFS doesn't have a list operation
        Ok(Vec::new())
    }

    async fn delete(&self, _id: &ArtifactId) -> Result<()> {
        // IPFS content can't be deleted
        Err(AiFormatError::Network(
            "IPFS content is immutable".to_string(),
        ))
    }

    fn location_type(&self) -> &'static str {
        "ipfs"
    }
}

impl IpfsStorage {
    async fn download_artifact(&self, id: &ArtifactId) -> Result<AiArtifact> {
        let url = format!("{}/ipfs/{}", self.gateway, id);
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Err(AiFormatError::artifact_not_found(id));
        }

        let bytes = resp.bytes().await?;
        let temp_path = std::env::temp_dir().join(format!("{}.ai", id));
        fs::write(&temp_path, &bytes).await?;

        let artifact = AiArtifact::load(&temp_path).await?;
        let _ = fs::remove_file(&temp_path).await;

        Ok(artifact)
    }
}

/// Node operator storage (peer-to-peer)
pub struct NodeStorage {
    peer_id: String,
    endpoint: String,
    client: Client,
}

impl NodeStorage {
    pub fn new(peer_id: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            peer_id: peer_id.into(),
            endpoint: endpoint.into(),
            client: Client::new(),
        }
    }
}

#[async_trait]
impl StorageBackend for NodeStorage {
    async fn exists(&self, id: &ArtifactId) -> Result<bool> {
        let url = format!("{}/artifacts/{}/exists", self.endpoint, id);

        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    async fn get_metadata(&self, id: &ArtifactId) -> Result<ArtifactMetadata> {
        let url = format!("{}/artifacts/{}/metadata", self.endpoint, id);

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(AiFormatError::artifact_not_found(id));
        }

        let metadata: ArtifactMetadata = resp.json().await?;
        Ok(metadata)
    }

    async fn download(&self, id: &ArtifactId, dest: &Path) -> Result<PathBuf> {
        let url = format!("{}/artifacts/{}/download", self.endpoint, id);
        debug!("Downloading from node {}: {}", self.peer_id, id);

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(AiFormatError::PeerNotFound(self.peer_id.clone()));
        }

        let bytes = resp.bytes().await?;
        let dest_path = dest.join(format!("{}.ai", id));

        fs::create_dir_all(dest).await?;
        fs::write(&dest_path, bytes).await?;

        info!("Downloaded {} from node {}", id, self.peer_id);
        Ok(dest_path)
    }

    async fn upload(&self, artifact: &AiArtifact) -> Result<StorageLocation> {
        let url = format!("{}/artifacts/upload", self.endpoint);

        // Serialize artifact to bytes
        let temp_path = std::env::temp_dir().join(format!("{}.ai", artifact.metadata.id));
        let mut artifact = artifact.clone();
        artifact.save(&temp_path).await?;
        let bytes = fs::read(&temp_path).await?;
        let _ = fs::remove_file(&temp_path).await;

        let resp = self.client
            .post(&url)
            .body(bytes)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(AiFormatError::storage("Node upload failed"));
        }

        Ok(StorageLocation::NodeStorage {
            peer_id: self.peer_id.clone(),
            path: artifact.metadata.id.clone(),
        })
    }

    async fn list(&self) -> Result<Vec<ArtifactRef>> {
        let url = format!("{}/artifacts", self.endpoint);

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }

        let refs: Vec<ArtifactRef> = resp.json().await?;
        Ok(refs)
    }

    async fn delete(&self, id: &ArtifactId) -> Result<()> {
        let url = format!("{}/artifacts/{}", self.endpoint, id);

        let resp = self.client.delete(&url).send().await?;
        if !resp.status().is_success() {
            return Err(AiFormatError::storage("Node delete failed"));
        }

        Ok(())
    }

    fn location_type(&self) -> &'static str {
        "node"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_local_storage() {
        let dir = tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());

        // Create and save artifact
        let artifact = crate::AiArtifact::builder()
            .name("test-model")
            .artifact_type(crate::ArtifactType::Model)
            .add_config("config.json", b"{}".to_vec())
            .build()
            .unwrap();

        let id = artifact.metadata.id.clone();
        storage.upload(&artifact).await.unwrap();

        // Verify it exists
        assert!(storage.exists(&id).await.unwrap());

        // List should include it
        let list = storage.list().await.unwrap();
        assert!(!list.is_empty());
    }

    #[tokio::test]
    async fn test_storage_manager() {
        let dir = tempdir().unwrap();
        let storage = Storage::new(dir.path());

        let artifact = crate::AiArtifact::builder()
            .name("test-artifact")
            .artifact_type(crate::ArtifactType::Weights)
            .add_file("weights.bin", vec![1, 2, 3, 4])
            .build()
            .unwrap();

        let id = artifact.metadata.id.clone();

        // Store artifact
        let locations = storage.store(&artifact, false).await.unwrap();
        assert_eq!(locations.len(), 1);
        assert!(matches!(locations[0], StorageLocation::Local(_)));

        // Retrieve artifact
        let path = storage.get(&id).await.unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_hf_repo_id_parsing() {
        assert_eq!(
            HuggingFaceStorage::parse_repo_id(&"meta-llama/Llama-2-7b".to_string()),
            "meta-llama/Llama-2-7b"
        );
        assert_eq!(
            HuggingFaceStorage::parse_repo_id(&"my-model".to_string()),
            "hanzo-ai/my-model"
        );
    }
}
