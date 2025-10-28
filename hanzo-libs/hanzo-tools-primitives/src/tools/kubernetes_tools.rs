use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

use hanzo_message_primitives::schemas::hanzo_name::HanzoName;
use hanzo_message_primitives::schemas::tool_router_key::ToolRouterKey;

use crate::tools::error::ToolError;
use crate::tools::parameters::Parameters;
use crate::tools::tool_config::{OAuth, ToolConfig};
use crate::tools::tool_output_arg::ToolOutputArg;
use crate::tools::tool_playground::{SqlQuery, SqlTable, ToolPlaygroundMetadata};
use crate::tools::tool_types::{OperatingSystem, RunnerType, ToolResult};

/// Resource requirements for Kubernetes execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sResourceRequirements {
    pub cpu_request: Option<String>,
    pub cpu_limit: Option<String>,
    pub memory_request: Option<String>,
    pub memory_limit: Option<String>,
    pub gpu_count: Option<u32>,
    pub gpu_type: Option<String>, // e.g., "nvidia.com/gpu", "amd.com/gpu"
}

/// Kubernetes tool for distributed execution across clusters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubernetesTool {
    pub name: String,
    pub tool_router_key: Option<ToolRouterKey>,
    pub description: String,
    pub version: String,
    pub author: String,
    pub homepage: Option<String>,

    // Kubernetes-specific configuration
    pub image: String,                      // Container image to use
    pub namespace: Option<String>,          // Target namespace (default: "hanzo-tools")
    pub service_account: Option<String>,    // Service account for pod
    pub image_pull_secret: Option<String>,  // Secret for pulling private images

    // Container configuration
    pub entrypoint: Option<Vec<String>>,    // Container entrypoint override
    pub args: Option<Vec<String>>,          // Container arguments
    pub working_dir: Option<String>,        // Working directory in container

    // Code and execution
    pub code: String,                       // Code to execute
    pub language: String,                   // Programming language
    pub timeout_seconds: Option<u64>,       // Job timeout

    // Resource requirements
    pub resources: Option<K8sResourceRequirements>,

    // Node selection and scheduling
    pub node_selector: Option<HashMap<String, String>>, // Node labels for scheduling
    pub tolerations: Option<Vec<Value>>,                // Pod tolerations
    pub affinity: Option<Value>,                        // Pod affinity rules

    // Security
    pub run_as_user: Option<i64>,          // UID to run container as
    pub run_as_group: Option<i64>,         // GID to run container as
    pub run_as_non_root: Option<bool>,     // Require non-root user
    pub privileged: Option<bool>,          // Run privileged container
    pub capabilities_add: Vec<String>,     // Linux capabilities to add
    pub capabilities_drop: Vec<String>,    // Linux capabilities to drop

    // Tool interface
    pub input_args: Parameters,
    pub output_arg: ToolOutputArg,
    pub result: Option<ToolResult>,
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
    pub assets: Option<Vec<Assets>>,

    // Execution environment
    pub runner: RunnerType,
    pub operating_system: Vec<OperatingSystem>,
    pub tool_set: Option<String>,

    // Kubernetes volumes and config
    pub config_maps: Option<HashMap<String, String>>,   // ConfigMaps to mount
    pub secrets: Option<HashMap<String, String>>,        // Secrets to mount
    pub persistent_volumes: Option<Vec<String>>,         // PVC names to mount
    pub environment: HashMap<String, String>,            // Environment variables

    // Distributed execution options
    pub parallelism: Option<i32>,                       // Max parallel pod instances
    pub completions: Option<i32>,                       // Number of completions needed
    pub cleanup_on_completion: Option<bool>,            // Auto-cleanup resources
}

/// Asset files that can be mounted into the Kubernetes pod
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assets {
    pub file_name: String,
    pub data: String, // Base64 encoded content
}

impl KubernetesTool {
    /// Create a new Kubernetes tool with default settings
    pub fn new(name: String, description: String, code: String, language: String) -> Self {
        Self {
            name,
            tool_router_key: None,
            description,
            version: "1.0.0".to_string(),
            author: "hanzo".to_string(),
            homepage: None,

            image: Self::default_image_for_language(&language),
            namespace: Some("hanzo-tools".to_string()),
            service_account: None,
            image_pull_secret: None,

            entrypoint: None,
            args: None,
            working_dir: Some("/workspace".to_string()),

            code,
            language,
            timeout_seconds: Some(3600), // 1 hour default

            resources: Some(K8sResourceRequirements {
                cpu_request: Some("100m".to_string()),
                cpu_limit: Some("1000m".to_string()),
                memory_request: Some("128Mi".to_string()),
                memory_limit: Some("1Gi".to_string()),
                gpu_count: None,
                gpu_type: None,
            }),

            node_selector: None,
            tolerations: None,
            affinity: None,

            run_as_user: Some(1000),
            run_as_group: Some(1000),
            run_as_non_root: Some(true),
            privileged: Some(false),
            capabilities_add: vec![],
            capabilities_drop: vec!["ALL".to_string()],

            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "{}".to_string() },
            result: None,
            config: vec![],

            tools: vec![],
            keywords: vec!["kubernetes".to_string(), "k8s".to_string(), "distributed".to_string()],
            activated: false,
            embedding: None,
            mcp_enabled: Some(false),

            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,

            runner: RunnerType::Kubernetes,
            operating_system: vec![OperatingSystem::Linux],
            tool_set: None,

            config_maps: None,
            secrets: None,
            persistent_volumes: None,
            environment: HashMap::new(),

            parallelism: Some(1),
            completions: Some(1),
            cleanup_on_completion: Some(true),
        }
    }

    /// Get default container image for a programming language
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
            "cuda" | "gpu" => "nvidia/cuda:12.2.0-runtime-ubuntu22.04".to_string(),
            _ => "ubuntu:22.04".to_string(),
        }
    }

    /// Set resource requirements for the tool
    pub fn with_resources(mut self, resources: K8sResourceRequirements) -> Self {
        self.resources = Some(resources);
        self
    }

    /// Set GPU requirements
    pub fn with_gpu(mut self, count: u32, gpu_type: Option<String>) -> Self {
        if let Some(ref mut resources) = self.resources {
            resources.gpu_count = Some(count);
            resources.gpu_type = gpu_type.or(Some("nvidia.com/gpu".to_string()));
        } else {
            self.resources = Some(K8sResourceRequirements {
                cpu_request: Some("100m".to_string()),
                cpu_limit: Some("1000m".to_string()),
                memory_request: Some("128Mi".to_string()),
                memory_limit: Some("1Gi".to_string()),
                gpu_count: Some(count),
                gpu_type: gpu_type.or(Some("nvidia.com/gpu".to_string())),
            });
        }
        self
    }

    /// Set node selector for specific hardware requirements
    pub fn with_node_selector(mut self, selector: HashMap<String, String>) -> Self {
        self.node_selector = Some(selector);
        self
    }

    /// Add tolerations for node taints
    pub fn with_tolerations(mut self, tolerations: Vec<Value>) -> Self {
        self.tolerations = Some(tolerations);
        self
    }

    /// Set parallel execution options
    pub fn with_parallelism(mut self, parallelism: i32, completions: i32) -> Self {
        self.parallelism = Some(parallelism);
        self.completions = Some(completions);
        self
    }

    /// Check if all required configuration fields are present
    pub fn check_required_config_fields(&self) -> bool {
        for config in &self.config {
            if let ToolConfig::BasicConfig(basic) = config {
                // Check if required fields have values
                if basic.required && basic.key_value.is_none() {
                    return false;
                }
            }
        }
        true
    }

    /// Validate the tool configuration
    pub fn validate(&self) -> Result<(), ToolError> {
        if self.name.is_empty() {
            return Err(ToolError::ExecutionError("Tool name cannot be empty".to_string()));
        }

        if self.image.is_empty() {
            return Err(ToolError::ExecutionError("Container image cannot be empty".to_string()));
        }

        if self.code.is_empty() {
            return Err(ToolError::ExecutionError("Tool code cannot be empty".to_string()));
        }

        // Validate resource requirements if present
        if let Some(ref resources) = self.resources {
            if let Some(ref gpu_count) = resources.gpu_count {
                if *gpu_count > 8 {
                    return Err(ToolError::ExecutionError(
                        "GPU count exceeds maximum allowed (8)".to_string()
                    ));
                }
            }
        }

        Ok(())
    }

    /// Get metadata for the tool
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
            result: self.result.clone().unwrap_or_else(|| {
                ToolResult::new("object".to_string(), serde_json::json!({}), vec![])
            }),
            sql_tables: self.sql_tables.clone().unwrap_or_default(),
            sql_queries: self.sql_queries.clone().unwrap_or_default(),
            tools: Some(self.tools.clone()),
            oauth: self.oauth.clone(),
            runner: self.runner,
            operating_system: self.operating_system.clone(),
            tool_set: self.tool_set.clone(),
        }
    }
}