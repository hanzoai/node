use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::default::Default;

use hanzo_messages::schemas::hanzo_name::HanzoName;
use hanzo_messages::schemas::tool_router_key::ToolRouterKey;

use crate::tools::error::ToolError;
use crate::tools::parameters::Parameters;
use crate::tools::tool_config::{OAuth, ToolConfig};
use crate::tools::tool_output_arg::ToolOutputArg;
use crate::tools::tool_playground::{SqlQuery, SqlTable, ToolPlaygroundMetadata};
use crate::tools::tool_types::{OperatingSystem, RunnerType, ToolResult};

/// Docker tool implementation for containerized execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerTool {
    pub name: String,
    pub tool_router_key: Option<ToolRouterKey>,
    pub description: String,
    pub version: String,
    pub author: String,
    pub homepage: Option<String>,
    
    // Docker-specific configuration
    pub docker_image: String,
    pub docker_command: Option<String>,
    pub docker_entrypoint: Option<String>,
    pub docker_workdir: Option<String>,
    
    // Code to execute in the container
    pub code: String,
    pub language: String, // "python", "javascript", "rust", "bash", etc.
    
    // Resource limits
    pub cpu_limit: Option<f64>,
    pub memory_limit: Option<String>, // e.g., "512M", "1G"
    pub timeout_seconds: Option<u64>,
    
    // Network and security
    pub network_mode: Option<String>, // "none", "bridge", "host"
    pub privileged: Option<bool>,
    pub read_only: Option<bool>,
    pub cap_add: Vec<String>,
    pub cap_drop: Vec<String>,
    
    // Tool interface
    pub input_args: Parameters,
    pub output_arg: ToolOutputArg,
    pub result: ToolResult,
    pub config: Vec<ToolConfig>,
    
    // Integration
    pub tools: Vec<ToolRouterKey>,
    pub keywords: Vec<String>,
    pub activated: bool,
    pub embedding: Option<Vec<f32>>,
    pub mcp_enabled: Option<bool>,
    
    // Database and file operations
    pub sql_tables: Option<Vec<SqlTable>>,
    pub sql_queries: Option<Vec<SqlQuery>>,
    pub file_inbox: Option<String>,
    pub oauth: Option<Vec<OAuth>>,
    pub assets: Option<Vec<String>>,
    
    // Execution environment
    pub runner: RunnerType,
    pub operating_system: Vec<OperatingSystem>,
    pub tool_set: Option<String>,
    
    // Volume mounts (host_path:container_path)
    pub volumes: Vec<String>,
    pub environment: HashMap<String, String>,
}

impl DockerTool {
    /// Create a new Docker tool with default settings
    pub fn new(name: String, description: String, code: String, language: String) -> Self {
        Self {
            name,
            tool_router_key: None,
            description,
            version: "1.0.0".to_string(),
            author: "hanzo".to_string(),
            homepage: None,
            
            docker_image: Self::default_image_for_language(&language),
            docker_command: None,
            docker_entrypoint: None,
            docker_workdir: Some("/workspace".to_string()),
            
            code,
            language,
            
            cpu_limit: Some(2.0),
            memory_limit: Some("512M".to_string()),
            timeout_seconds: Some(300),
            
            network_mode: Some("none".to_string()),
            privileged: Some(false),
            read_only: Some(false),
            cap_add: vec![],
            cap_drop: vec!["ALL".to_string()],
            
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "{}".to_string() },
            result: ToolResult::new("object".to_string(), serde_json::json!({}), vec![]),
            config: vec![],
            
            tools: vec![],
            keywords: vec!["docker".to_string()],
            activated: false,
            embedding: None,
            mcp_enabled: Some(false),
            
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            
            runner: RunnerType::Docker,
            operating_system: vec![OperatingSystem::Linux],
            tool_set: None,
            
            volumes: vec![],
            environment: HashMap::new(),
        }
    }
    
    /// Get default Docker image for a programming language
    fn default_image_for_language(language: &str) -> String {
        match language.to_lowercase().as_str() {
            "python" => "python:3.11-slim".to_string(),
            "javascript" | "js" | "node" => "node:20-alpine".to_string(),
            "typescript" | "ts" => "node:20-alpine".to_string(),
            "rust" => "rust:1.75-slim".to_string(),
            "go" | "golang" => "golang:1.21-alpine".to_string(),
            "ruby" => "ruby:3.3-slim".to_string(),
            "java" => "openjdk:21-slim".to_string(),
            "bash" | "sh" => "alpine:latest".to_string(),
            _ => "ubuntu:22.04".to_string(),
        }
    }
    
    /// Check if all required configuration fields are present
    pub fn check_required_config_fields(&self) -> bool {
        for config in &self.config {
            if let ToolConfig::BasicConfig(basic_config) = config {
                if basic_config.required && basic_config.key_value.is_none() {
                    return false;
                }
            }
        }
        true
    }
    
    /// Get metadata for the tool playground
    pub fn get_metadata(&self) -> ToolPlaygroundMetadata {
        ToolPlaygroundMetadata {
            name: self.name.clone(),
            version: self.version.clone(),
            homepage: self.homepage.clone(),
            description: self.description.clone(),
            author: self.author.clone(),
            keywords: self.keywords.clone(),
            configurations: self.config.clone(),
            parameters: self.input_args.clone(),
            result: self.result.clone(),
            sql_tables: self.sql_tables.clone().unwrap_or_default(),
            sql_queries: self.sql_queries.clone().unwrap_or_default(),
            tools: Some(self.tools.clone()),
            oauth: self.oauth.clone(),
            runner: self.runner,
            operating_system: self.operating_system.clone(),
            tool_set: self.tool_set.clone(),
        }
    }
    
    /// Run the Docker tool
    pub async fn run(
        &self,
        env: HashMap<String, String>,
        node_ip: String,
        node_port: u16,
        support_files: HashMap<String, String>,
        parameters: Map<String, Value>,
        extra_config: Vec<ToolConfig>,
        node_storage_path: String,
        app_id: String,
        tool_id: String,
        node_name: HanzoName,
        use_code_from_tool: bool,
        tool_router_key: Option<String>,
        mounts: Option<Vec<String>>,
    ) -> Result<ToolResult, ToolError> {
        // This will be implemented to call the execution_docker module
        // For now, return a placeholder
        Err(ToolError::ExecutionError(
            "Docker tool execution not yet integrated".to_string()
        ))
    }
}

impl Default for DockerTool {
    fn default() -> Self {
        Self::new(
            "default_docker_tool".to_string(),
            "Default Docker tool".to_string(),
            "echo 'Hello from Docker'".to_string(),
            "bash".to_string(),
        )
    }
}