//! # Data Models and Schemas for Hanzo DB
//!
//! Defines the data models for all Hanzo Node tables.
//! Arrow schemas are only available with backend-lancedb feature.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "backend-lancedb")]
use arrow_array::{
    Array, BinaryArray, Float32Array, Int32Array, Int64Array, RecordBatch, StringArray,
    TimestampMillisecondArray, BooleanArray, FixedSizeListArray,
};

#[cfg(feature = "backend-lancedb")]
use arrow_schema::{DataType, Field, Schema as ArrowSchema, TimeUnit};

#[cfg(feature = "backend-lancedb")]
use std::sync::Arc;

/// Vector dimension for embeddings
#[cfg(feature = "backend-lancedb")]
const EMBEDDING_DIM: i32 = 1536; // OpenAI ada-002 dimension

/// Create Arrow schema for users table
#[cfg(feature = "backend-lancedb")]
pub fn user_schema() -> Arc<ArrowSchema> {
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("profile_name", DataType::Utf8, false),
        Field::new("identity_type", DataType::Utf8, false),
        Field::new("identity_public_key", DataType::Utf8, true),
        Field::new("encryption_public_key", DataType::Utf8, true),
        Field::new("signature_public_key", DataType::Utf8, true),
        Field::new("node_signature_public_key", DataType::Utf8, true),
        Field::new("node_encryption_public_key", DataType::Utf8, true),
        Field::new("permission_type", DataType::Utf8, true),
        Field::new("wallet_id", DataType::Utf8, true),
        Field::new("metadata", DataType::Utf8, true), // JSON string
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("is_active", DataType::Boolean, false),
    ]))
}

/// Create Arrow schema for tools table
#[cfg(feature = "backend-lancedb")]
pub fn tool_schema() -> Arc<ArrowSchema> {
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, true),
        Field::new("version", DataType::Utf8, false),
        Field::new("tool_type", DataType::Utf8, false), // rust, js, python, mcp
        Field::new("parameters_schema", DataType::Utf8, true), // JSON schema
        Field::new("enabled", DataType::Boolean, false),
        Field::new("config", DataType::Utf8, true), // JSON config
        Field::new("usage_count", DataType::Int64, false),
        Field::new("avg_execution_time_ms", DataType::Float32, true),
        Field::new("last_used_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Create Arrow schema for agents table
#[cfg(feature = "backend-lancedb")]
pub fn agent_schema() -> Arc<ArrowSchema> {
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, true),
        Field::new("model", DataType::Utf8, false),
        Field::new("system_prompt", DataType::Utf8, true),
        Field::new("temperature", DataType::Float32, true),
        Field::new("max_tokens", DataType::Int32, true),
        Field::new("tools", DataType::Utf8, true), // JSON array of tool IDs
        Field::new("created_by", DataType::Utf8, false),
        Field::new("is_public", DataType::Boolean, false),
        Field::new("usage_count", DataType::Int64, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Create Arrow schema for embeddings table
#[cfg(feature = "backend-lancedb")]
pub fn embedding_schema() -> Arc<ArrowSchema> {
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("content_hash", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                EMBEDDING_DIM,
            ),
            false,
        ),
        Field::new("model", DataType::Utf8, false),
        Field::new("source_type", DataType::Utf8, false), // document, chat, tool_output
        Field::new("source_id", DataType::Utf8, true),
        Field::new("metadata", DataType::Utf8, true), // JSON metadata
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Create Arrow schema for jobs table
#[cfg(feature = "backend-lancedb")]
pub fn job_schema() -> Arc<ArrowSchema> {
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("job_type", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false), // pending, running, completed, failed
        Field::new("priority", DataType::Int32, false),
        Field::new("payload", DataType::Utf8, false), // JSON payload
        Field::new("result", DataType::Utf8, true), // JSON result
        Field::new("error", DataType::Utf8, true),
        Field::new("created_by", DataType::Utf8, false),
        Field::new("agent_id", DataType::Utf8, true),
        Field::new("started_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
        Field::new("completed_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Create Arrow schema for sessions table
#[cfg(feature = "backend-lancedb")]
pub fn session_schema() -> Arc<ArrowSchema> {
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("user_id", DataType::Utf8, false),
        Field::new("agent_id", DataType::Utf8, true),
        Field::new("messages", DataType::Utf8, false), // JSON array of messages
        Field::new("context", DataType::Utf8, true), // JSON context
        Field::new("token_count", DataType::Int64, false),
        Field::new("cost", DataType::Float32, true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Create Arrow schema for multimodal table
#[cfg(feature = "multimodal")]
pub fn multimodal_schema() -> Arc<ArrowSchema> {
    Arc::new(ArrowSchema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("media_type", DataType::Utf8, false), // image, audio, video
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("data", DataType::Binary, false), // Raw binary data
        Field::new("thumbnail", DataType::Binary, true), // Optional thumbnail
        Field::new("width", DataType::Int32, true),
        Field::new("height", DataType::Int32, true),
        Field::new("duration_ms", DataType::Int64, true), // For audio/video
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                EMBEDDING_DIM,
            ),
            true,
        ),
        Field::new("text_content", DataType::Utf8, true), // OCR or transcription
        Field::new("metadata", DataType::Utf8, true), // JSON metadata
        Field::new("created_by", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// User model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub profile_name: String,
    pub identity_type: String,
    pub identity_public_key: Option<String>,
    pub encryption_public_key: Option<String>,
    pub signature_public_key: Option<String>,
    pub node_signature_public_key: Option<String>,
    pub node_encryption_public_key: Option<String>,
    pub permission_type: Option<String>,
    pub wallet_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub is_active: bool,
}

/// Tool model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub tool_type: ToolType,
    pub parameters_schema: Option<serde_json::Value>,
    pub enabled: bool,
    pub config: Option<serde_json::Value>,
    pub usage_count: i64,
    pub avg_execution_time_ms: Option<f32>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Tool type enum
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Rust,
    JavaScript,
    Python,
    MCP,
}

/// Agent model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub model: String,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<i32>,
    pub tools: Vec<String>,
    pub created_by: String,
    pub is_public: bool,
    pub usage_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Embedding model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub id: Uuid,
    pub content: String,
    pub content_hash: String,
    pub vector: Vec<f32>,
    pub model: String,
    pub source_type: String,
    pub source_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Job model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub job_type: String,
    pub status: JobStatus,
    pub priority: i32,
    pub payload: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub created_by: String,
    pub agent_id: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Job status enum
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Session model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub messages: Vec<Message>,
    pub context: Option<serde_json::Value>,
    pub token_count: i64,
    pub cost: Option<f32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Message model for sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Tool call model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: Option<serde_json::Value>,
}

/// Multimodal content model
#[cfg(feature = "multimodal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalContent {
    pub id: Uuid,
    pub media_type: MediaType,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub thumbnail: Option<Vec<u8>>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub duration_ms: Option<i64>,
    pub embedding: Option<Vec<f32>>,
    pub text_content: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

/// Media type enum
#[cfg(feature = "multimodal")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Image,
    Audio,
    Video,
    Document,
}

/// Helper to convert models to RecordBatch
#[cfg(feature = "backend-lancedb")]
pub trait ToRecordBatch {
    fn to_record_batch(&self) -> Result<RecordBatch>;
}

/// Helper to convert from RecordBatch to models
#[cfg(feature = "backend-lancedb")]
pub trait FromRecordBatch: Sized {
    fn from_record_batch(batch: &RecordBatch, row: usize) -> Result<Self>;
}

#[cfg(all(test, feature = "backend-lancedb"))]
mod tests {
    use super::*;

    #[test]
    fn test_user_schema() {
        let schema = user_schema();
        assert_eq!(schema.fields().len(), 14);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(schema.field(0).data_type(), &DataType::Utf8);
    }

    #[test]
    fn test_embedding_schema() {
        let schema = embedding_schema();
        let vector_field = schema.field(3);
        assert_eq!(vector_field.name(), "vector");
        
        if let DataType::FixedSizeList(_, size) = vector_field.data_type() {
            assert_eq!(*size, EMBEDDING_DIM);
        } else {
            panic!("Expected FixedSizeList for vector field");
        }
    }

    #[test]
    fn test_tool_type_serialization() {
        let tool_type = ToolType::JavaScript;
        let json = serde_json::to_string(&tool_type).unwrap();
        assert_eq!(json, r#""javascript""#);
        
        let deserialized: ToolType = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, ToolType::JavaScript));
    }

    #[test]
    fn test_job_status_serialization() {
        let status = JobStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, r#""running""#);
        
        let deserialized: JobStatus = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, JobStatus::Running));
    }
}