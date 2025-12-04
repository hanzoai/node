//! Hanzo AI Format - Universal .ai file format for AI artifacts
//!
//! This crate provides a standard format for packaging, distributing, and sharing
//! AI artifacts across the Hanzo/Zoo network. The .ai format supports:
//!
//! - Model weights and biases
//! - Fine-tuning deltas (LoRA, QLoRA)
//! - Quantized models (GGUF, AWQ, GPTQ)
//! - Datasets and evaluation data
//! - Embeddings and vector stores
//! - Agent state and memory
//! - Configuration and hyperparameters
//!
//! # File Structure
//!
//! ```text
//! artifact.ai (ZIP archive with .ai extension)
//! ├── manifest.json          # Artifact metadata and manifest
//! ├── data/                   # Primary artifact data
//! │   ├── weights/            # Model weights (safetensors, bin, etc.)
//! │   ├── config/             # Model configuration
//! │   └── tokenizer/          # Tokenizer files
//! ├── delta/                  # Fine-tuning deltas (optional)
//! ├── dataset/                # Training/eval datasets (optional)
//! ├── embeddings/             # Pre-computed embeddings (optional)
//! ├── state/                  # Agent state/memory (optional)
//! └── signatures/             # Cryptographic signatures
//! ```
//!
//! # Storage Backends
//!
//! The format supports multiple storage backends:
//! - Local filesystem
//! - HuggingFace Hub (fallback/mirror)
//! - P2P swarm (BitTorrent-style distribution)
//! - IPFS (content-addressed storage)
//!
//! # Example
//!
//! ```ignore
//! use hanzo_ai_format::{AiArtifact, ArtifactType, Storage};
//!
//! // Create a new artifact
//! let artifact = AiArtifact::builder()
//!     .name("my-model")
//!     .artifact_type(ArtifactType::Model)
//!     .add_weights("weights.safetensors", weights_data)
//!     .build()?;
//!
//! // Save to .ai file
//! artifact.save("my-model.ai").await?;
//!
//! // Upload to storage
//! let storage = Storage::new_with_hf_fallback();
//! storage.upload(&artifact).await?;
//! ```

pub mod artifact;
pub mod error;
pub mod manifest;
pub mod storage;

pub use artifact::*;
pub use error::*;
pub use manifest::*;
pub use storage::*;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Magic bytes for .ai file format
pub const AI_MAGIC: &[u8; 4] = b"HAIF"; // Hanzo AI Format

/// Current format version
pub const FORMAT_VERSION: u32 = 1;

/// File extension for AI artifacts
pub const AI_EXTENSION: &str = "ai";

/// Artifact identifier (content-addressed)
pub type ArtifactId = String;

/// Unique identifier for artifacts
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ArtifactRef {
    /// Content hash (Blake3)
    pub hash: String,
    /// Human-readable name
    pub name: String,
    /// Version
    pub version: String,
    /// Storage location hints
    pub locations: Vec<StorageLocation>,
}

impl ArtifactRef {
    pub fn new(hash: String, name: String, version: String) -> Self {
        Self {
            hash,
            name,
            version,
            locations: Vec::new(),
        }
    }

    pub fn id(&self) -> ArtifactId {
        format!("{}@{}", self.name, self.hash[..8].to_string())
    }

    pub fn with_location(mut self, location: StorageLocation) -> Self {
        self.locations.push(location);
        self
    }
}

/// Storage location for an artifact
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum StorageLocation {
    /// Local filesystem path
    Local(String),
    /// HuggingFace Hub repository
    HuggingFace { repo_id: String, revision: Option<String> },
    /// P2P swarm (BitTorrent-style)
    Swarm { info_hash: String, peers: Vec<String> },
    /// IPFS CID
    Ipfs(String),
    /// HTTP(S) URL
    Http(String),
    /// Node operator storage (peer ID + path)
    NodeStorage { peer_id: String, path: String },
}

impl StorageLocation {
    pub fn local(path: impl Into<String>) -> Self {
        Self::Local(path.into())
    }

    pub fn huggingface(repo_id: impl Into<String>) -> Self {
        Self::HuggingFace {
            repo_id: repo_id.into(),
            revision: None,
        }
    }

    pub fn swarm(info_hash: impl Into<String>) -> Self {
        Self::Swarm {
            info_hash: info_hash.into(),
            peers: Vec::new(),
        }
    }

    pub fn ipfs(cid: impl Into<String>) -> Self {
        Self::Ipfs(cid.into())
    }

    pub fn http(url: impl Into<String>) -> Self {
        Self::Http(url.into())
    }
}

/// Type of AI artifact
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ArtifactType {
    /// Complete model (weights + config + tokenizer)
    Model,
    /// Model weights only
    Weights,
    /// Quantized model (GGUF, AWQ, GPTQ, etc.)
    QuantizedModel {
        format: QuantFormat,
        bits: u8,
    },
    /// Fine-tuning delta (LoRA, QLoRA, etc.)
    Delta {
        base_model: String,
        method: DeltaMethod,
    },
    /// Training or evaluation dataset
    Dataset {
        task: DatasetTask,
        split: Option<String>,
    },
    /// Pre-computed embeddings
    Embeddings {
        model: String,
        dimensions: usize,
    },
    /// Vector store / index
    VectorStore {
        index_type: String,
        dimensions: usize,
    },
    /// Agent state / memory
    AgentState {
        agent_type: String,
    },
    /// Tokenizer only
    Tokenizer,
    /// Configuration only
    Config,
    /// Checkpoint during training
    Checkpoint {
        epoch: usize,
        step: usize,
    },
    /// Custom artifact type
    Custom(String),
}

/// Quantization format
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum QuantFormat {
    /// GGUF (llama.cpp format)
    GGUF,
    /// AWQ (Activation-aware Weight Quantization)
    AWQ,
    /// GPTQ
    GPTQ,
    /// ExLlamaV2
    ExL2,
    /// BitsAndBytes
    BnB,
    /// MLX quantization
    MLX,
    /// Custom format
    Custom(String),
}

/// Fine-tuning delta method
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DeltaMethod {
    /// Low-Rank Adaptation
    LoRA { rank: usize, alpha: f32 },
    /// Quantized LoRA
    QLoRA { rank: usize, alpha: f32, bits: u8 },
    /// Full fine-tuning diff
    FullDiff,
    /// Adapter layers
    Adapter,
    /// Prefix tuning
    Prefix { num_tokens: usize },
    /// Custom method
    Custom(String),
}

/// Dataset task type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DatasetTask {
    TextGeneration,
    Conversation,
    Classification,
    QuestionAnswering,
    Summarization,
    Translation,
    CodeGeneration,
    Embedding,
    Custom(String),
}

/// License type for artifacts
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum License {
    MIT,
    Apache2,
    GPL3,
    Llama2,
    Llama3,
    Qwen,
    Gemma,
    CcBy4,
    CcByNc4,
    CcBySa4,
    Custom(String),
    Proprietary,
}

impl Default for License {
    fn default() -> Self {
        Self::Apache2
    }
}

/// Network where artifact is available
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Network {
    HanzoMainnet,
    HanzoTestnet,
    ZooMainnet,
    ZooTestnet,
    All,
}

/// Compute requirements for using this artifact
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComputeRequirements {
    /// Minimum VRAM in MB
    pub min_vram_mb: Option<u32>,
    /// Minimum RAM in MB
    pub min_ram_mb: Option<u32>,
    /// GPU required
    pub gpu_required: bool,
    /// Supported backends
    pub backends: Vec<String>,
    /// Recommended batch size
    pub recommended_batch_size: Option<usize>,
}

/// Metadata about the artifact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    /// Unique identifier
    pub id: ArtifactId,
    /// Human-readable name
    pub name: String,
    /// Version
    pub version: String,
    /// Description
    pub description: Option<String>,
    /// Artifact type
    pub artifact_type: ArtifactType,
    /// Author
    pub author: Option<String>,
    /// License
    pub license: License,
    /// Tags
    pub tags: Vec<String>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last modified timestamp
    pub updated_at: DateTime<Utc>,
    /// Total size in bytes
    pub size_bytes: u64,
    /// Content hash (Blake3)
    pub content_hash: String,
    /// Compute requirements
    pub requirements: ComputeRequirements,
    /// Networks where available
    pub networks: Vec<Network>,
    /// Storage locations
    pub locations: Vec<StorageLocation>,
    /// Dependencies (other artifacts)
    pub dependencies: Vec<ArtifactRef>,
    /// Custom metadata
    pub custom: HashMap<String, serde_json::Value>,
}

impl ArtifactMetadata {
    pub fn new(name: impl Into<String>, artifact_type: ArtifactType) -> Self {
        let name = name.into();
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            version: "1.0.0".to_string(),
            description: None,
            artifact_type,
            author: None,
            license: License::default(),
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            size_bytes: 0,
            content_hash: String::new(),
            requirements: ComputeRequirements::default(),
            networks: vec![Network::All],
            locations: Vec::new(),
            dependencies: Vec::new(),
            custom: HashMap::new(),
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    pub fn with_license(mut self, license: License) -> Self {
        self.license = license;
        self
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    pub fn with_network(mut self, network: Network) -> Self {
        self.networks.push(network);
        self
    }

    pub fn with_dependency(mut self, dep: ArtifactRef) -> Self {
        self.dependencies.push(dep);
        self
    }
}

/// Statistics about artifact usage across the network
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtifactStats {
    pub downloads: u64,
    pub unique_users: u64,
    pub compute_hours: f64,
    pub peer_count: usize,
    pub average_rating: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_ref() {
        let artifact_ref = ArtifactRef::new(
            "abc123def456".to_string(),
            "my-model".to_string(),
            "1.0.0".to_string(),
        );
        assert_eq!(artifact_ref.id(), "my-model@abc123de");
    }

    #[test]
    fn test_storage_locations() {
        let local = StorageLocation::local("/path/to/model");
        assert!(matches!(local, StorageLocation::Local(_)));

        let hf = StorageLocation::huggingface("hanzo-lm/Llama-3-8B");
        assert!(matches!(hf, StorageLocation::HuggingFace { .. }));

        let ipfs = StorageLocation::ipfs("QmXyz123");
        assert!(matches!(ipfs, StorageLocation::Ipfs(_)));
    }

    #[test]
    fn test_artifact_metadata() {
        let metadata = ArtifactMetadata::new("test-model", ArtifactType::Model)
            .with_version("2.0.0")
            .with_author("hanzo")
            .with_license(License::Apache2)
            .with_tag("llm")
            .with_tag("inference");

        assert_eq!(metadata.name, "test-model");
        assert_eq!(metadata.version, "2.0.0");
        assert_eq!(metadata.author, Some("hanzo".to_string()));
        assert_eq!(metadata.tags.len(), 2);
    }

    #[test]
    fn test_artifact_types() {
        let model = ArtifactType::Model;
        let quant = ArtifactType::QuantizedModel {
            format: QuantFormat::GGUF,
            bits: 4,
        };
        let delta = ArtifactType::Delta {
            base_model: "llama-3-8b".to_string(),
            method: DeltaMethod::LoRA { rank: 64, alpha: 32.0 },
        };

        assert!(matches!(model, ArtifactType::Model));
        assert!(matches!(quant, ArtifactType::QuantizedModel { .. }));
        assert!(matches!(delta, ArtifactType::Delta { .. }));
    }
}
