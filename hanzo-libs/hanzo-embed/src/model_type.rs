use std::fmt;
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::hanzo_embedding_errors::HanzoEmbeddingError;

pub type EmbeddingModelTypeString = String;

/// Process-wide override for the active embedding vector dimension.
/// `0` means "unset" → fall back to env / per-model defaults.
///
/// This is detected at runtime by probing the live embedding engine, so the
/// node adapts to ANY embedder / ANY dimension (zen 1024, gemma 768, ...)
/// instead of trusting a hardcoded name→dimension table.
static ACTIVE_VECTOR_DIMENSIONS: AtomicUsize = AtomicUsize::new(0);

/// Record the embedding dimension detected from the live engine. From then on,
/// every vector table, dimension validation and similarity search uses it.
pub fn set_active_vector_dimensions(dimensions: usize) {
    if dimensions > 0 {
        ACTIVE_VECTOR_DIMENSIONS.store(dimensions, Ordering::Relaxed);
    }
}

/// The runtime-detected embedding dimension, if known.
pub fn get_active_vector_dimensions() -> Option<usize> {
    match ACTIVE_VECTOR_DIMENSIONS.load(Ordering::Relaxed) {
        0 => None,
        d => Some(d),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash)]
pub enum EmbeddingModelType {
    OllamaTextEmbeddingsInference(OllamaTextEmbeddingsInference),
}

impl EmbeddingModelType {
    pub fn from_string(s: &str) -> Result<Self, HanzoEmbeddingError> {
        // from_string now returns Ok(Other(s.to_string())) for unknown models,
        // so this should never error, but we keep the Result type for API compatibility
        OllamaTextEmbeddingsInference::from_string(s)
            .map(EmbeddingModelType::OllamaTextEmbeddingsInference)
    }

    /// Returns the default embedding model
    pub fn default() -> Self {
        std::env::var("DEFAULT_EMBEDDING_MODEL")
            .and_then(|s| Self::from_string(&s).map_err(|_| std::env::VarError::NotPresent))
            .unwrap_or_else(|_| {
                EmbeddingModelType::OllamaTextEmbeddingsInference(OllamaTextEmbeddingsInference::EmbeddingGemma300M)
            })
    }

    pub fn max_input_token_count(&self) -> usize {
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => model.max_input_token_count(),
        }
    }

    pub fn embedding_normalization_factor(&self) -> f32 {
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => model.embedding_normalization_factor(),
        }
    }

    pub fn vector_dimensions(&self) -> Result<usize, HanzoEmbeddingError> {
        // 1) Runtime-detected dimension (probed from the live engine) wins — this
        //    is what makes "any embedder / any dimension" work with nothing hardcoded.
        if let Some(d) = get_active_vector_dimensions() {
            return Ok(d);
        }
        // 2) Explicit override via env (e.g. set by the desktop after probing).
        if let Ok(s) = std::env::var("EMBEDDING_VECTOR_DIMENSIONS") {
            if let Ok(d) = s.trim().parse::<usize>() {
                if d > 0 {
                    return Ok(d);
                }
            }
        }
        // 3) Fall back to the per-model default (last resort only).
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => model.vector_dimensions(),
        }
    }
}

impl fmt::Display for EmbeddingModelType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => write!(f, "{}", model),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OllamaTextEmbeddingsInference {
    AllMiniLML6v2,
    #[serde(alias = "SnowflakeArcticEmbed_M")]
    SnowflakeArcticEmbedM,
    JinaEmbeddingsV2BaseEs,
    EmbeddingGemma300M,
    Other(String),
}

impl OllamaTextEmbeddingsInference {
    const ALL_MINI_LML6V2: &'static str = "all-minilm:l6-v2";
    const SNOWFLAKE_ARCTIC_EMBED_M: &'static str = "snowflake-arctic-embed:xs";
    const JINA_EMBEDDINGS_V2_BASE_ES: &'static str = "jina/jina-embeddings-v2-base-es:latest";
    const EMBEDDING_GEMMA_300_M: &'static str = "embeddinggemma:300m";

    pub fn from_string(s: &str) -> Result<Self, HanzoEmbeddingError> {
        match s {
            Self::ALL_MINI_LML6V2 => Ok(Self::AllMiniLML6v2),
            Self::SNOWFLAKE_ARCTIC_EMBED_M => Ok(Self::SnowflakeArcticEmbedM),
            Self::JINA_EMBEDDINGS_V2_BASE_ES => Ok(Self::JinaEmbeddingsV2BaseEs),
            Self::EMBEDDING_GEMMA_300_M => Ok(Self::EmbeddingGemma300M),
            _ => Ok(Self::Other(s.to_string())),
        }
    }

    pub fn max_input_token_count(&self) -> usize {
        match self {
            Self::JinaEmbeddingsV2BaseEs => 1024,
            Self::EmbeddingGemma300M => 2048,
            Self::AllMiniLML6v2 => 512,
            Self::SnowflakeArcticEmbedM => 512,
            _ => 512,
        }
    }

    pub fn embedding_normalization_factor(&self) -> f32 {
        match self {
            Self::JinaEmbeddingsV2BaseEs => 1.5,
            _ => 1.0,
        }
    }

    pub fn vector_dimensions(&self) -> Result<usize, HanzoEmbeddingError> {
        match self {
            Self::SnowflakeArcticEmbedM => Ok(384),
            Self::AllMiniLML6v2 => Ok(384),
            Self::JinaEmbeddingsV2BaseEs => Ok(768),
            Self::EmbeddingGemma300M => Ok(768),
            _ => Ok(1024), // zen/qwen3 1024-dim default so DB opens
        }
    }
}

impl fmt::Display for OllamaTextEmbeddingsInference {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::AllMiniLML6v2 => write!(f, "{}", Self::ALL_MINI_LML6V2),
            Self::SnowflakeArcticEmbedM => write!(f, "{}", Self::SNOWFLAKE_ARCTIC_EMBED_M),
            Self::JinaEmbeddingsV2BaseEs => write!(f, "{}", Self::JINA_EMBEDDINGS_V2_BASE_ES),
            Self::EmbeddingGemma300M => write!(f, "{}", Self::EMBEDDING_GEMMA_300_M),
            Self::Other(name) => write!(f, "{}", name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snowflake_arctic_embed_xs() {
        let model_str = "snowflake-arctic-embed:xs";
        let parsed_model = OllamaTextEmbeddingsInference::from_string(model_str);
        assert_eq!(parsed_model, Ok(OllamaTextEmbeddingsInference::SnowflakeArcticEmbedM));
    }

    #[test]
    fn test_parse_jina_embeddings_v2_base_es() {
        let model_str = "jina/jina-embeddings-v2-base-es:latest";
        let parsed_model = OllamaTextEmbeddingsInference::from_string(model_str);
        assert_eq!(parsed_model, Ok(OllamaTextEmbeddingsInference::JinaEmbeddingsV2BaseEs));
    }

    #[test]
    fn test_parse_embedding_gemma_300m() {
        let model_str = "embeddinggemma:300m";
        let parsed_model = OllamaTextEmbeddingsInference::from_string(model_str);
        assert_eq!(parsed_model, Ok(OllamaTextEmbeddingsInference::EmbeddingGemma300M));
    }

    #[test]
    fn test_parse_snowflake_arctic_embed_xs_as_embedding_model_type() {
        let model_str = "snowflake-arctic-embed:xs";
        let parsed_model = EmbeddingModelType::from_string(model_str);
        assert_eq!(
            parsed_model,
            Ok(EmbeddingModelType::OllamaTextEmbeddingsInference(
                OllamaTextEmbeddingsInference::SnowflakeArcticEmbedM
            ))
        );
    }

    #[test]
    fn test_parse_jina_embeddings_v2_base_es_as_embedding_model_type() {
        let model_str = "jina/jina-embeddings-v2-base-es:latest";
        let parsed_model = EmbeddingModelType::from_string(model_str);
        assert_eq!(
            parsed_model,
            Ok(EmbeddingModelType::OllamaTextEmbeddingsInference(
                OllamaTextEmbeddingsInference::JinaEmbeddingsV2BaseEs
            ))
        );
    }

    #[test]
    fn test_parse_embedding_gemma_300m_as_embedding_model_type() {
        let model_str = "embeddinggemma:300m";
        let parsed_model = EmbeddingModelType::from_string(model_str);
        assert_eq!(
            parsed_model,
            Ok(EmbeddingModelType::OllamaTextEmbeddingsInference(
                OllamaTextEmbeddingsInference::EmbeddingGemma300M
            ))
        );
    }
}
