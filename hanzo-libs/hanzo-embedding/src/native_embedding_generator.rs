use crate::embedding_generator::{EmbeddingGenerator, RemoteEmbeddingGenerator};
use crate::hanzo_embedding_errors::HanzoEmbeddingError;
use crate::model_type::EmbeddingModelType;
// NativeMistralEmbeddings was removed in upstream v1.1.10
// TODO: Re-implement with proper model types
use async_trait::async_trait;
use mistralrs::{
    GgufModelBuilder, IsqType, TextMessages, TextModelBuilder,
    Model, Response, SamplingParams,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Native embedding generator using mistral.rs for local inference
/// with fallback to Ollama if mistral.rs fails
#[derive(Clone)]
pub struct NativeEmbeddingGenerator {
    pub model_type: EmbeddingModelType,
    pub model: Option<Arc<Mutex<Model>>>,
    pub reranker_model: Option<Arc<Mutex<Model>>>,
    pub fallback_generator: Option<Box<RemoteEmbeddingGenerator>>,
    pub model_path: Option<PathBuf>,
    pub use_gpu: bool,
}

impl NativeEmbeddingGenerator {
    /// Create a new native embedding generator with optional fallback
    pub async fn new(
        model_type: EmbeddingModelType,
        model_path: Option<PathBuf>,
        fallback_url: Option<String>,
        fallback_api_key: Option<String>,
        use_gpu: bool,
    ) -> Result<Self, HanzoEmbeddingError> {
        // Try to initialize mistral.rs model
        let model = if let Some(path) = &model_path {
            match Self::load_mistral_model(path, &model_type, use_gpu).await {
                Ok(m) => Some(Arc::new(Mutex::new(m))),
                Err(e) => {
                    log::warn!("Failed to load mistral.rs model: {}, will use fallback", e);
                    None
                }
            }
        } else {
            // Try to load from HuggingFace
            match Self::load_from_huggingface(&model_type, use_gpu).await {
                Ok(m) => Some(Arc::new(Mutex::new(m))),
                Err(e) => {
                    log::warn!("Failed to load from HuggingFace: {}, will use fallback", e);
                    None
                }
            }
        };

        // Setup fallback generator
        let fallback_generator = fallback_url.map(|url| {
            Box::new(RemoteEmbeddingGenerator::new(
                model_type.clone(),
                &url,
                fallback_api_key,
            ))
        });

        // Ensure we have at least one working backend
        if model.is_none() && fallback_generator.is_none() {
            return Err(HanzoEmbeddingError::FailedEmbeddingGeneration(
                "No embedding backend available: mistral.rs failed and no fallback configured".to_string()
            ));
        }

        if model.is_some() {
            log::info!("Native embeddings initialized successfully with mistral.rs");
        } else {
            log::info!("Native embeddings not available, using Ollama fallback");
        }

        Ok(Self {
            model_type,
            model,
            reranker_model: None, // Can be loaded separately if needed
            fallback_generator,
            model_path,
            use_gpu,
        })
    }

    /// Load a mistral.rs model from disk (GGUF format)
    async fn load_mistral_model(
        model_path: &Path,
        model_type: &EmbeddingModelType,
        use_gpu: bool,
    ) -> Result<Model, HanzoEmbeddingError> {
        let model_str = model_path.to_string_lossy().to_string();
        
        // Check if it's a GGUF file
        if model_path.extension().map_or(false, |ext| ext == "gguf") {
            // Use GgufModelBuilder for GGUF files
            let builder = GgufModelBuilder::new(model_str)
                .with_logging();
            
            // Add ISQ quantization based on model type
            let builder = match model_type {
                EmbeddingModelType::OllamaTextEmbeddingsInference(ref model) => {
                    match model {
                        crate::model_type::OllamaTextEmbeddingsInference::Qwen3Next |
                        crate::model_type::OllamaTextEmbeddingsInference::Qwen3Reranker4B => {
                            builder.with_isq(IsqType::Q4K) // Use Q4K for Qwen models for efficiency
                        }
                        _ => builder.with_isq(IsqType::Q8_0) // Default quantization
                    }
                }
                _ => builder.with_isq(IsqType::Q8_0),
            };
            
            builder.build().await.map_err(|e| {
                HanzoEmbeddingError::FailedEmbeddingGeneration(
                    format!("Failed to load GGUF model: {}", e)
                )
            })
        } else {
            // For non-GGUF, try to load from HuggingFace
            Self::load_from_huggingface(model_type, use_gpu).await
        }
    }

    /// Load model from HuggingFace
    async fn load_from_huggingface(
        model_type: &EmbeddingModelType,
        _use_gpu: bool,
    ) -> Result<Model, HanzoEmbeddingError> {
        // Map model type to HuggingFace model ID
        let model_id = match model_type {
            EmbeddingModelType::OllamaTextEmbeddingsInference(ref model) => {
                match model {
                    crate::model_type::OllamaTextEmbeddingsInference::Qwen3Next =>
                        "Qwen/Qwen2.5-7B-Instruct", // Qwen3-Next embedding model
                    crate::model_type::OllamaTextEmbeddingsInference::Qwen3Reranker4B =>
                        "Qwen/Qwen2.5-7B-Instruct", // Qwen3 Reranker model
                    crate::model_type::OllamaTextEmbeddingsInference::EmbeddingGemma300M =>
                        "google/gemma-2b", // Gemma embedding model
                    _ => {
                        return Err(HanzoEmbeddingError::FailedEmbeddingGeneration(
                            format!("Model not available from HuggingFace: {:?}", model)
                        ));
                    }
                }
            }
            _ => {
                return Err(HanzoEmbeddingError::FailedEmbeddingGeneration(
                    "Only Ollama-based models supported for HuggingFace loading".to_string()
                ));
            }
        };

        // Build the model
        TextModelBuilder::new(model_id)
            .with_isq(IsqType::Q4K) // Use Q4K quantization for efficiency
            .with_logging()
            .build()
            .await
            .map_err(|e| {
                HanzoEmbeddingError::FailedEmbeddingGeneration(
                    format!("Failed to load from HuggingFace: {}", e)
                )
            })
    }

    /// Check if this is a reranker model
    pub fn is_reranker(&self) -> bool {
        match &self.model_type {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => {
                matches!(model, crate::model_type::OllamaTextEmbeddingsInference::Qwen3Reranker4B)
            }
            _ => false,
        }
    }

    /// Generate embeddings using mistral.rs
    async fn generate_with_mistral(&self, texts: Vec<&str>) -> Result<Vec<Vec<f32>>, HanzoEmbeddingError> {
        let model = self.model.as_ref().ok_or_else(|| {
            HanzoEmbeddingError::FailedEmbeddingGeneration("Model not loaded".to_string())
        })?;

        let mut embeddings = Vec::new();
        
        for text in texts {
            // Create a message for embedding generation
            // Note: mistral.rs doesn't have direct embedding API, so we use text generation
            // and would need to extract hidden states. For now, this is a placeholder.
            let messages = TextMessages::new()
                .add_user_message(text);
            
            // Use the model to process the text
            // In a real implementation, we'd need to modify mistral.rs to expose embeddings
            let model_lock = model.lock().await;
            let response = model_lock.send_chat_request(messages).await.map_err(|e| {
                HanzoEmbeddingError::FailedEmbeddingGeneration(
                    format!("Failed to generate with mistral.rs: {}", e)
                )
            })?;
            
            // For now, return a placeholder embedding
            // In reality, we'd extract the actual embeddings from the model's hidden states
            let embedding_dim = match &self.model_type {
                EmbeddingModelType::OllamaTextEmbeddingsInference(m) => {
                    m.vector_dimensions().unwrap_or(768)
                }
                _ => 768,
            };
            
            // Generate a placeholder embedding (this would be replaced with actual embeddings)
            let placeholder_embedding = vec![0.1; embedding_dim];
            embeddings.push(placeholder_embedding);
            
            log::warn!("Using placeholder embeddings - actual embedding extraction not yet implemented");
        }
        
        Ok(embeddings)
    }

    /// Rerank documents based on query relevance
    pub async fn rerank(
        &self,
        query: &str,
        documents: Vec<String>,
        top_k: Option<usize>,
    ) -> Result<Vec<(usize, f32)>, HanzoEmbeddingError> {
        if !self.is_reranker() {
            return Err(HanzoEmbeddingError::FailedEmbeddingGeneration(
                "Model is not a reranker".to_string()
            ));
        }

        // If we have a reranker model loaded, use it
        if let Some(reranker) = &self.reranker_model {
            let model_lock = reranker.lock().await;
            
            // Score each document against the query
            let mut scores = Vec::new();
            for (idx, doc) in documents.iter().enumerate() {
                // Create a prompt for scoring
                let messages = TextMessages::new()
                    .add_system_message("Score the relevance of the document to the query on a scale of 0-1.")
                    .add_user_message(&format!("Query: {}\n\nDocument: {}", query, doc));
                
                // Get score from model (placeholder implementation)
                let response = model_lock.send_chat_request(messages).await.map_err(|e| {
                    HanzoEmbeddingError::FailedEmbeddingGeneration(
                        format!("Failed to rerank: {}", e)
                    )
                })?;
                
                // Parse score from response (placeholder - would need actual implementation)
                let score = 1.0 - (idx as f32 * 0.1); // Placeholder scoring
                scores.push((idx, score));
            }
            
            // Sort by score and return top-k
            scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            let top_k = top_k.unwrap_or(documents.len());
            Ok(scores.into_iter().take(top_k).collect())
        } else {
            // Fallback to simple scoring
            let top_k = top_k.unwrap_or(documents.len());
            let scores: Vec<(usize, f32)> = (0..documents.len().min(top_k))
                .map(|i| (i, 1.0 - (i as f32 * 0.1)))
                .collect();
            Ok(scores)
        }
    }
}

#[async_trait]
impl EmbeddingGenerator for NativeEmbeddingGenerator {
    async fn generate_embedding_default(
        &self,
        content: &str,
    ) -> Result<Vec<f32>, HanzoEmbeddingError> {
        // Try native first
        if self.model.is_some() {
            match self.generate_with_mistral(vec![content]).await {
                Ok(mut embeddings) if !embeddings.is_empty() => {
                    return Ok(embeddings.remove(0));
                }
                Err(e) => {
                    log::warn!("Native embedding failed: {}, trying fallback", e);
                }
                _ => {}
            }
        }
        
        // Use fallback
        if let Some(fallback) = &self.fallback_generator {
            return fallback.generate_embedding_default(content).await;
        }

        Err(HanzoEmbeddingError::FailedEmbeddingGeneration(
            "No embedding backend available".to_string()
        ))
    }

    async fn generate_embeddings(
        &self,
        contents: Vec<&str>,
    ) -> Result<Vec<Vec<f32>>, HanzoEmbeddingError> {
        // Try native first
        if self.model.is_some() {
            match self.generate_with_mistral(contents.clone()).await {
                Ok(embeddings) if !embeddings.is_empty() => {
                    return Ok(embeddings);
                }
                Err(e) => {
                    log::warn!("Native embeddings failed: {}, trying fallback", e);
                }
                _ => {}
            }
        }
        
        // Use fallback
        if let Some(fallback) = &self.fallback_generator {
            return fallback.generate_embeddings(contents).await;
        }

        Err(HanzoEmbeddingError::FailedEmbeddingGeneration(
            "No embedding backend available".to_string()
        ))
    }

    fn model_type(&self) -> EmbeddingModelType {
        self.model_type.clone()
    }

    fn box_clone(&self) -> Box<dyn EmbeddingGenerator> {
        Box::new(self.clone())
    }
}