use std::fmt;
use std::hash::Hash;

use crate::hanzo_embedding_errors::HanzoEmbeddingError;

pub type EmbeddingModelTypeString = String;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash)]
pub enum EmbeddingModelType {
    OllamaTextEmbeddingsInference(OllamaTextEmbeddingsInference),
    NativeMistralEmbeddings(NativeMistralEmbeddings),
}

impl EmbeddingModelType {
    pub fn from_string(s: &str) -> Result<Self, HanzoEmbeddingError> {
        // Try native models first
        if let Ok(model) = NativeMistralEmbeddings::from_string(s) {
            return Ok(EmbeddingModelType::NativeMistralEmbeddings(model));
        }
        
        // Fall back to Ollama models
        OllamaTextEmbeddingsInference::from_string(s)
            .map(EmbeddingModelType::OllamaTextEmbeddingsInference)
            .map_err(|_| HanzoEmbeddingError::InvalidModelArchitecture)
    }

    pub fn max_input_token_count(&self) -> usize {
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => model.max_input_token_count(),
            EmbeddingModelType::NativeMistralEmbeddings(model) => model.max_input_token_count(),
        }
    }

    pub fn embedding_normalization_factor(&self) -> f32 {
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => model.embedding_normalization_factor(),
            EmbeddingModelType::NativeMistralEmbeddings(model) => model.embedding_normalization_factor(),
        }
    }

    pub fn vector_dimensions(&self) -> Result<usize, HanzoEmbeddingError> {
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => model.vector_dimensions(),
            EmbeddingModelType::NativeMistralEmbeddings(model) => model.vector_dimensions(),
        }
    }
}

impl fmt::Display for EmbeddingModelType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => write!(f, "{}", model),
            EmbeddingModelType::NativeMistralEmbeddings(model) => write!(f, "{}", model),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OllamaTextEmbeddingsInference {
    AllMiniLML6v2,
    #[serde(alias = "SnowflakeArcticEmbed_M")]
    SnowflakeArcticEmbedM,
    JinaEmbeddingsV2BaseEs,
    Other(String),
}

impl OllamaTextEmbeddingsInference {
    const ALL_MINI_LML6V2: &'static str = "all-minilm:l6-v2";
    const SNOWFLAKE_ARCTIC_EMBED_M: &'static str = "snowflake-arctic-embed:xs";
    const JINA_EMBEDDINGS_V2_BASE_ES: &'static str = "jina/jina-embeddings-v2-base-es:latest";

    pub fn from_string(s: &str) -> Result<Self, HanzoEmbeddingError> {
        match s {
            Self::ALL_MINI_LML6V2 => Ok(Self::AllMiniLML6v2),
            Self::SNOWFLAKE_ARCTIC_EMBED_M => Ok(Self::SnowflakeArcticEmbedM),
            Self::JINA_EMBEDDINGS_V2_BASE_ES => Ok(Self::JinaEmbeddingsV2BaseEs),
            _ => Err(HanzoEmbeddingError::InvalidModelArchitecture),
        }
    }

    pub fn max_input_token_count(&self) -> usize {
        match self {
            Self::JinaEmbeddingsV2BaseEs => 1024,
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
            Self::JinaEmbeddingsV2BaseEs => Ok(768),
            _ => Err(HanzoEmbeddingError::UnimplementedModelDimensions(format!(
                "{:?}",
                self
            ))),
        }
    }
}

impl fmt::Display for OllamaTextEmbeddingsInference {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::AllMiniLML6v2 => write!(f, "{}", Self::ALL_MINI_LML6V2),
            Self::SnowflakeArcticEmbedM => write!(f, "{}", Self::SNOWFLAKE_ARCTIC_EMBED_M),
            Self::JinaEmbeddingsV2BaseEs => write!(f, "{}", Self::JINA_EMBEDDINGS_V2_BASE_ES),
            Self::Other(name) => write!(f, "{}", name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum NativeMistralEmbeddings {
    MistralEmbed,
    E5MistralEmbed,
    BgeM3,
    // Qwen models for embeddings
    Qwen3Next,
    Qwen25Embed,
    // Reranker models for better retrieval
    Qwen3Reranker4B,
    BgeRerankerV2,
    JinaRerankerV2,
    Custom(String),
}

impl NativeMistralEmbeddings {
    const MISTRAL_EMBED: &'static str = "mistral-embed";
    const E5_MISTRAL_EMBED: &'static str = "e5-mistral-embed";
    const BGE_M3: &'static str = "bge-m3";
    const QWEN3_NEXT: &'static str = "qwen3-next";
    const QWEN25_EMBED: &'static str = "qwen2.5-embed";
    const QWEN3_RERANKER_4B: &'static str = "qwen3-reranker-4b";
    const BGE_RERANKER_V2: &'static str = "bge-reranker-v2";
    const JINA_RERANKER_V2: &'static str = "jina-reranker-v2";

    pub fn from_string(s: &str) -> Result<Self, HanzoEmbeddingError> {
        match s {
            Self::MISTRAL_EMBED => Ok(Self::MistralEmbed),
            Self::E5_MISTRAL_EMBED => Ok(Self::E5MistralEmbed),
            Self::BGE_M3 => Ok(Self::BgeM3),
            Self::QWEN3_NEXT => Ok(Self::Qwen3Next),
            Self::QWEN25_EMBED => Ok(Self::Qwen25Embed),
            Self::QWEN3_RERANKER_4B => Ok(Self::Qwen3Reranker4B),
            Self::BGE_RERANKER_V2 => Ok(Self::BgeRerankerV2),
            Self::JINA_RERANKER_V2 => Ok(Self::JinaRerankerV2),
            _ if s.starts_with("native:") => Ok(Self::Custom(s.strip_prefix("native:").unwrap().to_string())),
            _ => Err(HanzoEmbeddingError::InvalidModelArchitecture),
        }
    }
    
    pub fn is_reranker(&self) -> bool {
        match self {
            Self::Qwen3Reranker4B | 
            Self::BgeRerankerV2 | 
            Self::JinaRerankerV2 => true,
            Self::Custom(s) => s.contains("reranker"),
            _ => false,
        }
    }

    pub fn max_input_token_count(&self) -> usize {
        match self {
            Self::MistralEmbed => 2048,
            Self::E5MistralEmbed => 4096,
            Self::BgeM3 => 8192,
            Self::Qwen3Next => 32768, // Qwen supports long context
            Self::Qwen25Embed => 32768,
            Self::Qwen3Reranker4B => 8192,
            Self::BgeRerankerV2 => 512,
            Self::JinaRerankerV2 => 1024,
            Self::Custom(_) => 512, // Conservative default
        }
    }

    pub fn embedding_normalization_factor(&self) -> f32 {
        1.0 // Native models typically don't need normalization
    }

    pub fn vector_dimensions(&self) -> Result<usize, HanzoEmbeddingError> {
        match self {
            Self::MistralEmbed => Ok(1024),
            Self::E5MistralEmbed => Ok(1024),
            Self::BgeM3 => Ok(1024),
            Self::Qwen3Next => Ok(1536), // Qwen uses 1536 dims
            Self::Qwen25Embed => Ok(1536),
            // Rerankers don't produce embeddings, they score pairs
            Self::Qwen3Reranker4B => Err(HanzoEmbeddingError::InvalidModelArchitecture),
            Self::BgeRerankerV2 => Err(HanzoEmbeddingError::InvalidModelArchitecture),
            Self::JinaRerankerV2 => Err(HanzoEmbeddingError::InvalidModelArchitecture),
            Self::Custom(_) => Ok(768), // Default dimension
        }
    }
}

impl fmt::Display for NativeMistralEmbeddings {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::MistralEmbed => write!(f, "{}", Self::MISTRAL_EMBED),
            Self::E5MistralEmbed => write!(f, "{}", Self::E5_MISTRAL_EMBED),
            Self::BgeM3 => write!(f, "{}", Self::BGE_M3),
            Self::Qwen3Next => write!(f, "{}", Self::QWEN3_NEXT),
            Self::Qwen25Embed => write!(f, "{}", Self::QWEN25_EMBED),
            Self::Qwen3Reranker4B => write!(f, "{}", Self::QWEN3_RERANKER_4B),
            Self::BgeRerankerV2 => write!(f, "{}", Self::BGE_RERANKER_V2),
            Self::JinaRerankerV2 => write!(f, "{}", Self::JINA_RERANKER_V2),
            Self::Custom(name) => write!(f, "native:{}", name),
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
    fn test_qwen3_next_embedding_model() {
        let model_str = "qwen3-next";
        let parsed_model = EmbeddingModelType::from_string(model_str);
        assert_eq!(
            parsed_model,
            Ok(EmbeddingModelType::NativeMistralEmbeddings(
                NativeMistralEmbeddings::Qwen3Next
            ))
        );
        
        // Verify properties
        if let Ok(EmbeddingModelType::NativeMistralEmbeddings(model)) = parsed_model {
            assert_eq!(model.max_input_token_count(), 32768);
            assert_eq!(model.vector_dimensions(), Ok(1536));
            assert!(!model.is_reranker());
        }
    }

    #[test]
    fn test_qwen3_reranker_4b_model() {
        let model_str = "qwen3-reranker-4b";
        let parsed_model = EmbeddingModelType::from_string(model_str);
        assert_eq!(
            parsed_model,
            Ok(EmbeddingModelType::NativeMistralEmbeddings(
                NativeMistralEmbeddings::Qwen3Reranker4B
            ))
        );
        
        // Verify properties
        if let Ok(EmbeddingModelType::NativeMistralEmbeddings(model)) = parsed_model {
            assert_eq!(model.max_input_token_count(), 8192);
            assert!(model.is_reranker());
            // Rerankers don't produce embeddings
            assert!(model.vector_dimensions().is_err());
        }
    }

    #[test]
    fn test_model_display() {
        let qwen_embed = NativeMistralEmbeddings::Qwen3Next;
        assert_eq!(format!("{}", qwen_embed), "qwen3-next");
        
        let qwen_reranker = NativeMistralEmbeddings::Qwen3Reranker4B;
        assert_eq!(format!("{}", qwen_reranker), "qwen3-reranker-4b");
    }
}
