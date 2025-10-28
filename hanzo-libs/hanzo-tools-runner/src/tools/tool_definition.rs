use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub keywords: Vec<String>,
    pub configurations: Value,
    pub parameters: Value,
    pub result: Value,
    pub code: Option<String>,
    pub embedding_metadata: Option<EmbeddingMetadata>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingMetadata {
    pub model_name: String,
    pub embeddings: Vec<f32>,
}
