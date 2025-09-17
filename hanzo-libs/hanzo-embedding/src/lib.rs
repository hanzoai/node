pub mod embedding_generator;
pub mod model_type;
pub mod hanzo_embedding_errors;
pub mod mock_generator;
// pub mod native_embedding_generator; // Temporarily disabled - needs API fixes

// Re-export commonly used types
pub use model_type::OllamaTextEmbeddingsInference;
pub use model_type::EmbeddingModelType;

#[cfg(test)]
mod tests {
    use super::*;
    use model_type::EmbeddingModelType;
    use embedding_generator::RemoteEmbeddingGenerator;

    #[test]
    fn test_default_embedding_model() {
        // Test that default model works
        let model = EmbeddingModelType::default();
        assert!(model.max_input_token_count() > 0);
        assert!(model.vector_dimensions().is_ok());

        // Verify that our default generator works
        let generator = RemoteEmbeddingGenerator::new_default();
        assert!(generator.model_type.max_input_token_count() > 0);
    }

    // TODO: Re-enable these tests once native embedding model support is restored
    // The upstream removed NativeMistralEmbeddings in v1.1.10
    // We'll need to reintroduce it as part of our Hanzo enhancements
}