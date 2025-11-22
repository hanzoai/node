use std::path::PathBuf;
use std::fs;
use crate::hanzo_embedding_errors::HanzoEmbeddingError;
use crate::model_type::OllamaTextEmbeddingsInference;

/// Central model storage manager for all Hanzo models
/// All models are stored in ~/.hanzo/models for consistency across:
/// - hanzoai (engine CLI)
/// - hanzod (network node)
/// - app (~/work/hanzo/app)
pub struct ModelStorage;

impl ModelStorage {
    /// Get the base directory for all Hanzo models
    pub fn hanzo_home() -> PathBuf {
        dirs::home_dir()
            .expect("Could not find home directory")
            .join(".hanzo")
    }

    /// Get the models directory (~/.hanzo/models)
    pub fn models_dir() -> PathBuf {
        Self::hanzo_home().join("models")
    }

    /// Get the embeddings models directory (~/.hanzo/models/embeddings)
    pub fn embeddings_dir() -> PathBuf {
        Self::models_dir().join("embeddings")
    }

    /// Get the reranker models directory (~/.hanzo/models/rerankers)
    pub fn rerankers_dir() -> PathBuf {
        Self::models_dir().join("rerankers")
    }

    /// Get the LLM models directory (~/.hanzo/models/llms)
    pub fn llms_dir() -> PathBuf {
        Self::models_dir().join("llms")
    }

    /// Get the cache directory (~/.hanzo/cache)
    pub fn cache_dir() -> PathBuf {
        Self::hanzo_home().join("cache")
    }

    /// Ensure all required directories exist
    pub fn ensure_directories() -> Result<(), HanzoEmbeddingError> {
        let dirs = vec![
            Self::models_dir(),
            Self::embeddings_dir(),
            Self::rerankers_dir(),
            Self::llms_dir(),
            Self::cache_dir(),
        ];

        for dir in dirs {
            fs::create_dir_all(&dir)
                .map_err(|e| HanzoEmbeddingError::FailedEmbeddingGeneration(
                    format!("Failed to create directory {dir:?}: {e}")
                ))?;
        }

        Ok(())
    }

    /// Get the path for a specific model
    pub fn get_model_path(model: &OllamaTextEmbeddingsInference) -> PathBuf {
        match model {
            // Embedding models go in ~/.hanzo/models/embeddings/
            OllamaTextEmbeddingsInference::Qwen3Embedding8B => {
                Self::embeddings_dir().join("qwen3-embedding-8b")
            }
            OllamaTextEmbeddingsInference::Qwen3Embedding4B => {
                Self::embeddings_dir().join("qwen3-embedding-4b")
            }
            OllamaTextEmbeddingsInference::Qwen3Next => {
                Self::embeddings_dir().join("qwen3-next")
            }

            // Reranker models go in ~/.hanzo/models/rerankers/
            OllamaTextEmbeddingsInference::Qwen3Reranker8B => {
                Self::rerankers_dir().join("qwen3-reranker-8b")
            }
            OllamaTextEmbeddingsInference::Qwen3Reranker4B => {
                Self::rerankers_dir().join("qwen3-reranker-4b")
            }

            // Other embedding models
            OllamaTextEmbeddingsInference::EmbeddingGemma300M => {
                Self::embeddings_dir().join("gemma-300m")
            }
            OllamaTextEmbeddingsInference::AllMiniLML6v2 => {
                Self::embeddings_dir().join("all-minilm-l6-v2")
            }
            OllamaTextEmbeddingsInference::SnowflakeArcticEmbedM => {
                Self::embeddings_dir().join("snowflake-arctic-embed-m")
            }
            OllamaTextEmbeddingsInference::JinaEmbeddingsV2BaseEs => {
                Self::embeddings_dir().join("jina-embeddings-v2-base-es")
            }

            // Unknown models
            OllamaTextEmbeddingsInference::Other(name) => {
                Self::models_dir().join(name.replace("/", "--"))
            }
        }
    }

    /// Check if a model is already downloaded
    pub fn is_model_downloaded(model: &OllamaTextEmbeddingsInference) -> bool {
        let path = Self::get_model_path(model);
        path.exists() && path.is_dir()
    }

    /// Get the GGUF file path for a model
    pub fn get_gguf_path(model: &OllamaTextEmbeddingsInference) -> Option<PathBuf> {
        let model_dir = Self::get_model_path(model);
        if !model_dir.exists() {
            return None;
        }

        // Look for GGUF files in the model directory
        let gguf_files: Vec<_> = fs::read_dir(&model_dir)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .map(|ext| ext == "gguf")
                    .unwrap_or(false)
            })
            .collect();

        // Prefer Q5_K_M quantization for Qwen models
        gguf_files.iter()
            .find(|p| p.to_string_lossy().contains("Q5_K_M"))
            .or_else(|| gguf_files.first())
            .cloned()
    }

    /// Get download URLs for models
    pub fn get_download_url(model: &OllamaTextEmbeddingsInference) -> String {
        match model {
            // Main 8B embedding model - this is what we prioritize
            OllamaTextEmbeddingsInference::Qwen3Embedding8B => {
                "https://huggingface.co/dengcao/Qwen3-Embedding-8B-GGUF/resolve/main/qwen3-embedding-8b-Q5_K_M.gguf".to_string()
            }
            OllamaTextEmbeddingsInference::Qwen3Embedding4B => {
                "https://huggingface.co/dengcao/Qwen3-Embedding-4B-GGUF/resolve/main/qwen3-embedding-4b-Q5_K_M.gguf".to_string()
            }
            OllamaTextEmbeddingsInference::Qwen3Reranker8B => {
                "https://huggingface.co/dengcao/Qwen3-Reranker-8B-GGUF/resolve/main/qwen3-reranker-8b-F16.gguf".to_string()
            }
            OllamaTextEmbeddingsInference::Qwen3Reranker4B => {
                "https://huggingface.co/dengcao/Qwen3-Reranker-4B-GGUF/resolve/main/qwen3-reranker-4b-F16.gguf".to_string()
            }
            _ => {
                // Fallback for other models
                format!("https://huggingface.co/models/{model}")
            }
        }
    }

    /// Initialize storage and ensure ~/.hanzo structure exists
    pub fn init() -> Result<(), HanzoEmbeddingError> {
        Self::ensure_directories()?;

        // Create a README in the models directory
        let readme_path = Self::models_dir().join("README.md");
        if !readme_path.exists() {
            let readme_content = r#"# Hanzo Models Directory

This directory contains all models used by Hanzo components:
- `hanzoai` - Hanzo Engine CLI
- `hanzod` - Hanzo Network Node
- Hanzo App

## Directory Structure

- `embeddings/` - Embedding models
  - `qwen3-embedding-8b/` - Main Qwen3 8B embedding model (4096 dims)
  - `qwen3-embedding-4b/` - Qwen3 4B embedding model (2048 dims)
- `rerankers/` - Reranking models
  - `qwen3-reranker-8b/` - Qwen3 8B reranker
  - `qwen3-reranker-4b/` - Qwen3 4B reranker
- `llms/` - Large language models
- `cache/` - Model cache and temporary files

## Model Management

Models are automatically downloaded when first requested.
To manually download the recommended Qwen3-8B model:

```bash
hanzoai model pull qwen3-embedding-8b
```

Or with hanzod:
```bash
hanzod --download-model qwen3-embedding-8b
```
"#;
            fs::write(&readme_path, readme_content)
                .map_err(|e| HanzoEmbeddingError::FailedEmbeddingGeneration(
                    format!("Failed to create README: {e}")
                ))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_paths() {
        // Test that paths are correctly generated
        let qwen8b_path = ModelStorage::get_model_path(&OllamaTextEmbeddingsInference::Qwen3Embedding8B);
        assert!(qwen8b_path.ends_with("models/embeddings/qwen3-embedding-8b"));

        let reranker_path = ModelStorage::get_model_path(&OllamaTextEmbeddingsInference::Qwen3Reranker8B);
        assert!(reranker_path.ends_with("models/rerankers/qwen3-reranker-8b"));
    }

    #[test]
    fn test_hanzo_home() {
        let home = ModelStorage::hanzo_home();
        assert!(home.ends_with(".hanzo"));
    }
}