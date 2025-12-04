//! AI Artifact - Read/write .ai files

use crate::{
    error::{AiFormatError, Result},
    manifest::Manifest,
    ArtifactMetadata, ArtifactType, AI_EXTENSION, AI_MAGIC, FORMAT_VERSION,
};
use chrono::Utc;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use tokio::fs;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

/// An AI artifact that can be saved to a .ai file
#[derive(Debug, Clone)]
pub struct AiArtifact {
    /// Artifact metadata
    pub metadata: ArtifactMetadata,
    /// Manifest (detailed file listing)
    pub manifest: Manifest,
    /// Data files (path -> contents)
    data: HashMap<String, Vec<u8>>,
}

impl AiArtifact {
    /// Create a new artifact with the given metadata
    pub fn new(metadata: ArtifactMetadata) -> Self {
        Self {
            metadata,
            manifest: Manifest::new(),
            data: HashMap::new(),
        }
    }

    /// Create a builder for constructing artifacts
    pub fn builder() -> ArtifactBuilder {
        ArtifactBuilder::new()
    }

    /// Add a file to the artifact
    pub fn add_file(&mut self, path: impl Into<String>, data: Vec<u8>) {
        let path = path.into();
        let hash = blake3::hash(&data).to_hex().to_string();
        self.manifest.add_file(&path, data.len() as u64, &hash);
        self.data.insert(path, data);
    }

    /// Add weights to the artifact
    pub fn add_weights(&mut self, filename: impl Into<String>, data: Vec<u8>) {
        let path = format!("data/weights/{}", filename.into());
        self.add_file(path, data);
    }

    /// Add config to the artifact
    pub fn add_config(&mut self, filename: impl Into<String>, data: Vec<u8>) {
        let path = format!("data/config/{}", filename.into());
        self.add_file(path, data);
    }

    /// Add tokenizer to the artifact
    pub fn add_tokenizer(&mut self, filename: impl Into<String>, data: Vec<u8>) {
        let path = format!("data/tokenizer/{}", filename.into());
        self.add_file(path, data);
    }

    /// Add delta (LoRA, etc.) to the artifact
    pub fn add_delta(&mut self, filename: impl Into<String>, data: Vec<u8>) {
        let path = format!("delta/{}", filename.into());
        self.add_file(path, data);
    }

    /// Add dataset to the artifact
    pub fn add_dataset(&mut self, filename: impl Into<String>, data: Vec<u8>) {
        let path = format!("dataset/{}", filename.into());
        self.add_file(path, data);
    }

    /// Add embeddings to the artifact
    pub fn add_embeddings(&mut self, filename: impl Into<String>, data: Vec<u8>) {
        let path = format!("embeddings/{}", filename.into());
        self.add_file(path, data);
    }

    /// Add agent state to the artifact
    pub fn add_state(&mut self, filename: impl Into<String>, data: Vec<u8>) {
        let path = format!("state/{}", filename.into());
        self.add_file(path, data);
    }

    /// Get a file from the artifact
    pub fn get_file(&self, path: &str) -> Option<&[u8]> {
        self.data.get(path).map(|v| v.as_slice())
    }

    /// List all files in the artifact
    pub fn list_files(&self) -> Vec<&str> {
        self.data.keys().map(|s| s.as_str()).collect()
    }

    /// Calculate the content hash of the artifact
    pub fn calculate_hash(&self) -> String {
        let mut hasher = blake3::Hasher::new();

        // Hash all files in sorted order for determinism
        let mut paths: Vec<_> = self.data.keys().collect();
        paths.sort();

        for path in paths {
            if let Some(data) = self.data.get(path) {
                hasher.update(path.as_bytes());
                hasher.update(data);
            }
        }

        hasher.finalize().to_hex().to_string()
    }

    /// Save the artifact to a .ai file
    pub async fn save(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();

        // Update metadata
        self.metadata.updated_at = Utc::now();
        self.metadata.content_hash = self.calculate_hash();
        self.metadata.size_bytes = self.data.values().map(|v| v.len() as u64).sum();

        // Create the file
        let file = std::fs::File::create(path)?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // Write magic bytes and version as first file
        zip.start_file("_format", options)?;
        zip.write_all(AI_MAGIC)?;
        zip.write_all(&FORMAT_VERSION.to_le_bytes())?;

        // Write manifest.json
        let manifest_json = serde_json::to_vec_pretty(&self.manifest)?;
        zip.start_file("manifest.json", options)?;
        zip.write_all(&manifest_json)?;

        // Write metadata.json
        let metadata_json = serde_json::to_vec_pretty(&self.metadata)?;
        zip.start_file("metadata.json", options)?;
        zip.write_all(&metadata_json)?;

        // Write all data files
        for (file_path, data) in &self.data {
            zip.start_file(file_path, options)?;
            zip.write_all(data)?;
        }

        zip.finish()?;
        Ok(())
    }

    /// Load an artifact from a .ai file
    pub async fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = std::fs::File::open(path)?;
        let mut zip = ZipArchive::new(file)?;

        // Verify format
        {
            let mut format_file = zip.by_name("_format")?;
            let mut magic = [0u8; 4];
            format_file.read_exact(&mut magic)?;
            if &magic != AI_MAGIC {
                return Err(AiFormatError::InvalidMagic);
            }

            let mut version_bytes = [0u8; 4];
            format_file.read_exact(&mut version_bytes)?;
            let version = u32::from_le_bytes(version_bytes);
            if version > FORMAT_VERSION {
                return Err(AiFormatError::UnsupportedVersion(version));
            }
        }

        // Read manifest
        let manifest: Manifest = {
            let mut file = zip.by_name("manifest.json")?;
            let mut contents = String::new();
            file.read_to_string(&mut contents)?;
            serde_json::from_str(&contents)?
        };

        // Read metadata
        let metadata: ArtifactMetadata = {
            let mut file = zip.by_name("metadata.json")?;
            let mut contents = String::new();
            file.read_to_string(&mut contents)?;
            serde_json::from_str(&contents)?
        };

        // Read all data files
        let mut data = HashMap::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            let name = file.name().to_string();

            // Skip metadata files
            if name == "_format" || name == "manifest.json" || name == "metadata.json" {
                continue;
            }

            let mut contents = Vec::new();
            file.read_to_end(&mut contents)?;
            data.insert(name, contents);
        }

        let artifact = Self {
            metadata,
            manifest,
            data,
        };

        // Verify checksum
        let hash = artifact.calculate_hash();
        if hash != artifact.metadata.content_hash {
            return Err(AiFormatError::ChecksumMismatch {
                expected: artifact.metadata.content_hash.clone(),
                actual: hash,
            });
        }

        Ok(artifact)
    }

    /// Extract the artifact to a directory
    pub async fn extract(&self, dir: impl AsRef<Path>) -> Result<()> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir).await?;

        for (path, data) in &self.data {
            let file_path = dir.join(path);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(&file_path, data).await?;
        }

        // Write metadata
        let metadata_json = serde_json::to_vec_pretty(&self.metadata)?;
        fs::write(dir.join("metadata.json"), metadata_json).await?;

        Ok(())
    }

    /// Get the file extension for AI artifacts
    pub fn extension() -> &'static str {
        AI_EXTENSION
    }
}

/// Builder for creating AI artifacts
#[derive(Debug, Default)]
pub struct ArtifactBuilder {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    author: Option<String>,
    artifact_type: Option<ArtifactType>,
    files: HashMap<String, Vec<u8>>,
    tags: Vec<String>,
}

impl ArtifactBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    pub fn artifact_type(mut self, artifact_type: ArtifactType) -> Self {
        self.artifact_type = Some(artifact_type);
        self
    }

    pub fn add_weights(mut self, filename: impl Into<String>, data: Vec<u8>) -> Self {
        let path = format!("data/weights/{}", filename.into());
        self.files.insert(path, data);
        self
    }

    pub fn add_config(mut self, filename: impl Into<String>, data: Vec<u8>) -> Self {
        let path = format!("data/config/{}", filename.into());
        self.files.insert(path, data);
        self
    }

    pub fn add_file(mut self, path: impl Into<String>, data: Vec<u8>) -> Self {
        self.files.insert(path.into(), data);
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    pub fn build(self) -> Result<AiArtifact> {
        let name = self.name.ok_or_else(|| AiFormatError::missing_field("name"))?;
        let artifact_type = self.artifact_type.ok_or_else(|| AiFormatError::missing_field("artifact_type"))?;

        let mut metadata = ArtifactMetadata::new(&name, artifact_type);

        if let Some(version) = self.version {
            metadata = metadata.with_version(version);
        }
        if let Some(desc) = self.description {
            metadata = metadata.with_description(desc);
        }
        if let Some(author) = self.author {
            metadata = metadata.with_author(author);
        }
        for tag in self.tags {
            metadata = metadata.with_tag(tag);
        }

        let mut artifact = AiArtifact::new(metadata);

        for (path, data) in self.files {
            artifact.add_file(path, data);
        }

        Ok(artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_artifact_creation() {
        let artifact = AiArtifact::builder()
            .name("test-model")
            .artifact_type(ArtifactType::Model)
            .add_weights("model.safetensors", vec![1, 2, 3, 4])
            .add_config("config.json", b"{}".to_vec())
            .build()
            .unwrap();

        assert_eq!(artifact.metadata.name, "test-model");
        assert_eq!(artifact.list_files().len(), 2);
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.ai");

        // Create and save
        let mut artifact = AiArtifact::builder()
            .name("test-model")
            .version("1.0.0")
            .artifact_type(ArtifactType::Model)
            .add_weights("weights.bin", vec![1, 2, 3, 4, 5])
            .build()
            .unwrap();

        artifact.save(&file_path).await.unwrap();

        // Load and verify
        let loaded = AiArtifact::load(&file_path).await.unwrap();
        assert_eq!(loaded.metadata.name, "test-model");
        assert_eq!(loaded.metadata.version, "1.0.0");
        assert_eq!(
            loaded.get_file("data/weights/weights.bin"),
            Some([1u8, 2, 3, 4, 5].as_slice())
        );
    }

    #[tokio::test]
    async fn test_extract() {
        let dir = tempdir().unwrap();
        let extract_dir = dir.path().join("extracted");

        let artifact = AiArtifact::builder()
            .name("test-model")
            .artifact_type(ArtifactType::Model)
            .add_file("config.json", b"{}".to_vec())
            .build()
            .unwrap();

        artifact.extract(&extract_dir).await.unwrap();

        assert!(extract_dir.join("config.json").exists());
        assert!(extract_dir.join("metadata.json").exists());
    }
}
