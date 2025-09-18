use std::fmt;
use std::hash::Hash;

use crate::hanzo_embedding_errors::HanzoEmbeddingError;

pub type EmbeddingModelTypeString = String;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash)]
pub enum EmbeddingModelType {
    OllamaTextEmbeddingsInference(OllamaTextEmbeddingsInference),
}

impl EmbeddingModelType {
    pub fn from_string(s: &str) -> Result<Self, HanzoEmbeddingError> {
        OllamaTextEmbeddingsInference::from_string(s)
            .map(EmbeddingModelType::OllamaTextEmbeddingsInference)
            .map_err(|_| HanzoEmbeddingError::InvalidModelArchitecture)
    }

    /// Returns the default embedding model
    pub fn default() -> Self {
        std::env::var("DEFAULT_EMBEDDING_MODEL")
            .and_then(|s| Self::from_string(&s).map_err(|_| std::env::VarError::NotPresent))
            .unwrap_or({
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
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => model.vector_dimensions(),
        }
    }
}

impl fmt::Display for EmbeddingModelType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => write!(f, "{model}"),
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
    Qwen3Next,           // Qwen3-Next embedding model
    Qwen3Embedding8B,    // Qwen3 8B embedding model (the main one)
    Qwen3Embedding4B,    // Qwen3 4B embedding model
    Qwen3Reranker4B,     // Qwen3 4B reranker model
    Qwen3Reranker8B,     // Qwen3 8B reranker model
    Other(String),
}

impl OllamaTextEmbeddingsInference {
    const ALL_MINI_LML6V2: &'static str = "all-minilm:l6-v2";
    const SNOWFLAKE_ARCTIC_EMBED_M: &'static str = "snowflake-arctic-embed:xs";
    const JINA_EMBEDDINGS_V2_BASE_ES: &'static str = "jina/jina-embeddings-v2-base-es:latest";
    const EMBEDDING_GEMMA_300_M: &'static str = "embeddinggemma:300m";
    const QWEN3_NEXT: &'static str = "qwen3-next";
    const QWEN3_EMBEDDING_8B: &'static str = "qwen3-embedding-8b";
    const QWEN3_EMBEDDING_4B: &'static str = "qwen3-embedding-4b";
    const QWEN3_RERANKER_4B: &'static str = "qwen3-reranker-4b";
    const QWEN3_RERANKER_8B: &'static str = "qwen3-reranker-8b";

    pub fn from_string(s: &str) -> Result<Self, HanzoEmbeddingError> {
        match s {
            Self::ALL_MINI_LML6V2 => Ok(Self::AllMiniLML6v2),
            Self::SNOWFLAKE_ARCTIC_EMBED_M => Ok(Self::SnowflakeArcticEmbedM),
            Self::JINA_EMBEDDINGS_V2_BASE_ES => Ok(Self::JinaEmbeddingsV2BaseEs),
            Self::EMBEDDING_GEMMA_300_M => Ok(Self::EmbeddingGemma300M),
            Self::QWEN3_NEXT | "Qwen/Qwen2.5-7B-Instruct" => Ok(Self::Qwen3Next),
            Self::QWEN3_EMBEDDING_8B | "dengcao/Qwen3-Embedding-8B" | "Qwen/Qwen3-Embedding-8B" => Ok(Self::Qwen3Embedding8B),
            Self::QWEN3_EMBEDDING_4B | "dengcao/Qwen3-Embedding-4B" | "Qwen/Qwen3-Embedding-4B" => Ok(Self::Qwen3Embedding4B),
            Self::QWEN3_RERANKER_4B | "qwen3-reranker" | "dengcao/Qwen3-Reranker-4B" => Ok(Self::Qwen3Reranker4B),
            Self::QWEN3_RERANKER_8B | "dengcao/Qwen3-Reranker-8B" | "Qwen/Qwen3-Reranker-8B" => Ok(Self::Qwen3Reranker8B),
            other => Ok(Self::Other(other.to_string())),
        }
    }

    pub fn max_input_token_count(&self) -> usize {
        match self {
            Self::JinaEmbeddingsV2BaseEs => 1024,
            Self::EmbeddingGemma300M => 2048,
            Self::AllMiniLML6v2 => 512,
            Self::SnowflakeArcticEmbedM => 512,
            Self::Qwen3Next => 32768,        // 32K context
            Self::Qwen3Embedding8B => 32768, // 32K context for 8B embedding
            Self::Qwen3Embedding4B => 32768, // 32K context for 4B embedding
            Self::Qwen3Reranker4B => 8192,   // 8K context for 4B reranker
            Self::Qwen3Reranker8B => 8192,   // 8K context for 8B reranker
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
            Self::Qwen3Next => Ok(1536),        // 1536 dimensions
            Self::Qwen3Embedding8B => Ok(4096), // 4096 dimensions for 8B (best quality)
            Self::Qwen3Embedding4B => Ok(2048), // 2048 dimensions for 4B
            Self::Qwen3Reranker4B => Ok(768),   // Reranker score dimension
            Self::Qwen3Reranker8B => Ok(768),   // Reranker score dimension
            Self::Other(_) => Ok(768),          // Default for unknown models
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
            Self::Qwen3Next => write!(f, "{}", Self::QWEN3_NEXT),
            Self::Qwen3Embedding8B => write!(f, "{}", Self::QWEN3_EMBEDDING_8B),
            Self::Qwen3Embedding4B => write!(f, "{}", Self::QWEN3_EMBEDDING_4B),
            Self::Qwen3Reranker4B => write!(f, "{}", Self::QWEN3_RERANKER_4B),
            Self::Qwen3Reranker8B => write!(f, "{}", Self::QWEN3_RERANKER_8B),
            Self::Other(name) => write!(f, "{name}"),
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

    #[test]
    fn test_parse_qwen3_next() {
        // Test Qwen3-Next model name
        let parsed_model = OllamaTextEmbeddingsInference::from_string("qwen3-next");
        assert_eq!(parsed_model, Ok(OllamaTextEmbeddingsInference::Qwen3Next));

        // Test Qwen2.5 model name (parses as Qwen3Next)
        let parsed_model2 = OllamaTextEmbeddingsInference::from_string("Qwen/Qwen2.5-7B-Instruct");
        assert_eq!(parsed_model2, Ok(OllamaTextEmbeddingsInference::Qwen3Next));
    }

    #[test]
    fn test_parse_qwen3_embedding_models() {
        // Test Qwen3 embedding 8B model
        let parsed_model = OllamaTextEmbeddingsInference::from_string("qwen3-embedding-8b");
        assert_eq!(parsed_model, Ok(OllamaTextEmbeddingsInference::Qwen3Embedding8B));

        // Test other 8B variations
        let parsed_model2 = OllamaTextEmbeddingsInference::from_string("dengcao/Qwen3-Embedding-8B");
        assert_eq!(parsed_model2, Ok(OllamaTextEmbeddingsInference::Qwen3Embedding8B));

        // Test 4B model
        let parsed_model3 = OllamaTextEmbeddingsInference::from_string("qwen3-embedding-4b");
        assert_eq!(parsed_model3, Ok(OllamaTextEmbeddingsInference::Qwen3Embedding4B));
    }

    #[test]
    fn test_parse_qwen3_reranker() {
        // Test 4B reranker variations
        let parsed_model = OllamaTextEmbeddingsInference::from_string("qwen3-reranker-4b");
        assert_eq!(parsed_model, Ok(OllamaTextEmbeddingsInference::Qwen3Reranker4B));

        let parsed_model2 = OllamaTextEmbeddingsInference::from_string("qwen3-reranker");
        assert_eq!(parsed_model2, Ok(OllamaTextEmbeddingsInference::Qwen3Reranker4B));

        // Test 8B reranker
        let parsed_model3 = OllamaTextEmbeddingsInference::from_string("qwen3-reranker-8b");
        assert_eq!(parsed_model3, Ok(OllamaTextEmbeddingsInference::Qwen3Reranker8B));

        let parsed_model4 = OllamaTextEmbeddingsInference::from_string("dengcao/Qwen3-Reranker-8B");
        assert_eq!(parsed_model4, Ok(OllamaTextEmbeddingsInference::Qwen3Reranker8B));
    }

    #[test]
    fn test_qwen3_model_properties() {
        // Test Qwen3-Next properties
        let qwen3_next = OllamaTextEmbeddingsInference::Qwen3Next;
        assert_eq!(qwen3_next.max_input_token_count(), 32768);
        assert_eq!(qwen3_next.vector_dimensions(), Ok(1536));
        assert_eq!(qwen3_next.to_string(), "qwen3-next");

        // Test Qwen3 Embedding 8B properties
        let qwen3_embed_8b = OllamaTextEmbeddingsInference::Qwen3Embedding8B;
        assert_eq!(qwen3_embed_8b.max_input_token_count(), 32768);
        assert_eq!(qwen3_embed_8b.vector_dimensions(), Ok(4096));
        assert_eq!(qwen3_embed_8b.to_string(), "qwen3-embedding-8b");

        // Test Qwen3 Embedding 4B properties
        let qwen3_embed_4b = OllamaTextEmbeddingsInference::Qwen3Embedding4B;
        assert_eq!(qwen3_embed_4b.max_input_token_count(), 32768);
        assert_eq!(qwen3_embed_4b.vector_dimensions(), Ok(2048));
        assert_eq!(qwen3_embed_4b.to_string(), "qwen3-embedding-4b");

        // Test Qwen3 Reranker 4B properties
        let qwen3_reranker_4b = OllamaTextEmbeddingsInference::Qwen3Reranker4B;
        assert_eq!(qwen3_reranker_4b.max_input_token_count(), 8192);
        assert_eq!(qwen3_reranker_4b.vector_dimensions(), Ok(768));
        assert_eq!(qwen3_reranker_4b.to_string(), "qwen3-reranker-4b");

        // Test Qwen3 Reranker 8B properties
        let qwen3_reranker_8b = OllamaTextEmbeddingsInference::Qwen3Reranker8B;
        assert_eq!(qwen3_reranker_8b.max_input_token_count(), 8192);
        assert_eq!(qwen3_reranker_8b.vector_dimensions(), Ok(768));
        assert_eq!(qwen3_reranker_8b.to_string(), "qwen3-reranker-8b");
    }
}
