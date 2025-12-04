//! Manifest - File listing and metadata for .ai archives

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Manifest for an AI artifact archive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Format version
    pub version: u32,
    /// File entries
    pub files: HashMap<String, FileEntry>,
    /// Total size of all files
    pub total_size: u64,
    /// Total number of files
    pub file_count: usize,
}

impl Manifest {
    pub fn new() -> Self {
        Self {
            version: crate::FORMAT_VERSION,
            files: HashMap::new(),
            total_size: 0,
            file_count: 0,
        }
    }

    pub fn add_file(&mut self, path: &str, size: u64, hash: &str) {
        self.files.insert(
            path.to_string(),
            FileEntry {
                size,
                hash: hash.to_string(),
                compressed_size: None,
            },
        );
        self.total_size += size;
        self.file_count += 1;
    }

    pub fn get_file(&self, path: &str) -> Option<&FileEntry> {
        self.files.get(path)
    }

    pub fn list_files(&self) -> Vec<&str> {
        self.files.keys().map(|s| s.as_str()).collect()
    }

    pub fn list_by_prefix(&self, prefix: &str) -> Vec<&str> {
        self.files
            .keys()
            .filter(|k| k.starts_with(prefix))
            .map(|s| s.as_str())
            .collect()
    }

    /// Get all weight files
    pub fn get_weights(&self) -> Vec<&str> {
        self.list_by_prefix("data/weights/")
    }

    /// Get all config files
    pub fn get_configs(&self) -> Vec<&str> {
        self.list_by_prefix("data/config/")
    }

    /// Get all tokenizer files
    pub fn get_tokenizers(&self) -> Vec<&str> {
        self.list_by_prefix("data/tokenizer/")
    }

    /// Get all delta files
    pub fn get_deltas(&self) -> Vec<&str> {
        self.list_by_prefix("delta/")
    }

    /// Get all dataset files
    pub fn get_datasets(&self) -> Vec<&str> {
        self.list_by_prefix("dataset/")
    }

    /// Get all embedding files
    pub fn get_embeddings(&self) -> Vec<&str> {
        self.list_by_prefix("embeddings/")
    }

    /// Get all state files
    pub fn get_states(&self) -> Vec<&str> {
        self.list_by_prefix("state/")
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self::new()
    }
}

/// Entry for a single file in the manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Uncompressed size in bytes
    pub size: u64,
    /// Blake3 hash of the file contents
    pub hash: String,
    /// Compressed size (if different from uncompressed)
    pub compressed_size: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest() {
        let mut manifest = Manifest::new();

        manifest.add_file("data/weights/model.bin", 1000, "abc123");
        manifest.add_file("data/config/config.json", 100, "def456");

        assert_eq!(manifest.file_count, 2);
        assert_eq!(manifest.total_size, 1100);
        assert_eq!(manifest.get_weights().len(), 1);
        assert_eq!(manifest.get_configs().len(), 1);
    }
}
