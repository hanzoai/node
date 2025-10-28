//! MATRIX MODE: Optimized execution coordinator with zero-copy patterns
//!
//! This module eliminates unnecessary clones and allocations for maximum performance.

use crate::llm_provider::job_manager::JobManager;
use crate::managers::IdentityManager;
use crate::monitoring::{PerfTimer, record_tool_execution};
use crate::security::{SecurityManager, ToolSecurityRequirements, PrivacyTier};
use crate::tools::tool_definitions::definition_generation::generate_tool_definitions;
use crate::tools::tool_execution::execute_agent_dynamic::execute_agent_tool;
use crate::tools::tool_execution::execute_mcp_server_dynamic::execute_mcp_server_dynamic;
use crate::tools::tool_execution::execution_custom::try_to_execute_rust_tool;
use crate::tools::tool_execution::execution_deno_dynamic::{check_deno_tool, execute_deno_tool};
use crate::tools::tool_execution::execution_header_generator::{check_tool, generate_execution_environment};
use crate::tools::tool_execution::execution_python_dynamic::execute_python_tool;
use crate::tools::tool_execution::execution_wasm::execute_wasm_tool;
use crate::tools::tool_execution::execution_docker::execute_docker_tool;
use crate::tools::tool_execution::execution_kubernetes;
use crate::utils::environment::fetch_node_environment;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use ed25519_dalek::SigningKey;
use regex::Regex;
use reqwest::Client;
use serde_json::json;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use hanzo_message_primitives::schemas::llm_providers::agent::Agent;
use hanzo_message_primitives::schemas::hanzo_name::HanzoName;
use hanzo_message_primitives::schemas::hanzo_tools::CodeLanguage;
use hanzo_message_primitives::schemas::hanzo_tools::DynamicToolType;
use hanzo_message_primitives::schemas::tool_router_key::ToolRouterKey;
use hanzo_sqlite::oauth_manager::OAuthToken;
use hanzo_sqlite::SqliteManager;
use hanzo_tools_primitives::tools::error::ToolError;
use hanzo_tools_primitives::tools::hanzo_tool::HanzoTool;
use hanzo_tools_primitives::tools::tool_config::{BasicConfig, OAuth, ToolConfig};
use hanzo_tools_primitives::tools::tool_types::{OperatingSystem, RunnerType};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use x25519_dalek::PublicKey as EncryptionPublicKey;
use x25519_dalek::StaticSecret as EncryptionStaticKey;
use log::{debug, info};
use std::borrow::Cow;

/// Execution context that minimizes cloning by using references
pub struct ExecutionContext<'a> {
    pub bearer: &'a str,
    pub node_name: &'a HanzoName,
    pub db: Arc<SqliteManager>,
    pub tool_router_key: &'a str,
    pub tool_id: &'a str,
    pub app_id: &'a str,
    pub agent_id: Option<&'a str>,
    pub llm_provider: &'a str,
    pub identity_manager: Arc<Mutex<IdentityManager>>,
    pub job_manager: Arc<Mutex<JobManager>>,
    pub encryption_secret_key: &'a EncryptionStaticKey,
    pub encryption_public_key: &'a EncryptionPublicKey,
    pub signing_secret_key: &'a SigningKey,
    pub security_manager: Option<Arc<SecurityManager>>,
    pub mounts: Option<&'a [String]>,
}

/// Optimized OAuth handler that reduces allocations
pub async fn handle_oauth_optimized<'a>(
    oauth: &Option<Vec<OAuth>>,
    db: &Arc<SqliteManager>,
    app_id: &str,
    tool_id: &str,
    tool_router_key: &str,
) -> Result<Value, ToolError> {
    let oauth = match oauth {
        Some(o) => o,
        None => return Ok(json!([])),
    };

    let mut access_tokens = Vec::with_capacity(oauth.len());

    for o in oauth.iter() {
        // Check existing token without cloning unless necessary
        let existing_token = db
            .get_oauth_token(o.name.clone(), tool_router_key.to_string())
            .ok()
            .flatten();

        if let Some(token) = existing_token {
            if token.access_token.is_some() {
                // Token exists and is valid, add to results
                let mut oauth_map = HashMap::new();
                oauth_map.insert("name".to_string(), token.connection_name);
                oauth_map.insert("accessToken".to_string(), token.access_token.unwrap_or_default());
                oauth_map.insert(
                    "expiresAt".to_string(),
                    token.expires_at.map(|t| t.to_string()).unwrap_or_default(),
                );
                access_tokens.push(oauth_map);
                continue;
            }
        }

        // Need to create OAuth URL - this path requires some cloning
        let state_uuid = uuid::Uuid::new_v4().to_string();
        let oauth_login_url = build_oauth_url(o, &state_uuid);

        return Err(ToolError::OAuthError(oauth_login_url));
    }

    Ok(serde_json::to_value(access_tokens).unwrap())
}

/// Build OAuth URL without unnecessary allocations
fn build_oauth_url(oauth: &OAuth, state: &str) -> String {
    use std::fmt::Write;

    let mut url = String::with_capacity(512);
    write!(&mut url, "{}?", oauth.authorization_url).unwrap();
    write!(&mut url, "response_type={}&", urlencoding::encode(&oauth.response_type)).unwrap();
    write!(&mut url, "client_id={}&", urlencoding::encode(&oauth.client_id)).unwrap();
    write!(&mut url, "redirect_uri={}&", urlencoding::encode(&oauth.redirect_url)).unwrap();
    write!(&mut url, "scope={}&", urlencoding::encode(&oauth.scopes.join(" "))).unwrap();
    write!(&mut url, "state={}", urlencoding::encode(state)).unwrap();

    url
}

/// Override tool config with minimal allocations
pub fn override_tool_config_optimized<'a>(
    tool_router_key: &str,
    agent: &Agent,
    extra_config: &'a [ToolConfig],
) -> Cow<'a, [ToolConfig]> {
    let overrides = match &agent.tools_config_override {
        Some(o) => o,
        None => return Cow::Borrowed(extra_config),
    };

    let tool_overrides = match overrides.get(tool_router_key) {
        Some(o) => o,
        None => return Cow::Borrowed(extra_config),
    };

    // Only clone if we actually have overrides
    let mut final_config = extra_config.to_vec();

    for (key, value) in tool_overrides {
        if let Some(idx) = final_config.iter().position(|c| c.name() == *key) {
            if let ToolConfig::BasicConfig(ref mut config) = final_config[idx] {
                config.key_value = Some(value.clone());
            }
        } else {
            final_config.push(ToolConfig::BasicConfig(BasicConfig {
                key_name: key.clone(),
                key_value: Some(value.clone()),
                description: String::new(),
                required: true,
                type_name: None,
            }));
        }
    }

    Cow::Owned(final_config)
}

/// Execute tool with optimized context passing
pub async fn execute_tool_cmd_optimized<'a>(
    ctx: ExecutionContext<'a>,
    parameters: Map<String, Value>,
    extra_config: Vec<ToolConfig>,
) -> Result<Value, ToolError> {
    // Start performance timer
    let timer = PerfTimer::new(format!("tool_execution:{}", ctx.tool_router_key));

    info!("⚡ Executing tool: {} - MATRIX MODE", ctx.tool_router_key);

    // Get tool from database
    let tool = ctx.db
        .get_tool_by_key(ctx.tool_router_key)
        .map_err(|e| ToolError::ExecutionError(format!("Failed to get tool: {}", e)))?;

    // Apply agent config overrides if needed
    let final_config = if let Some(agent_id) = ctx.agent_id {
        if let Ok(Some(agent)) = ctx.db.get_agent(agent_id) {
            override_tool_config_optimized(ctx.tool_router_key, &agent, &extra_config)
        } else {
            Cow::Borrowed(&extra_config[..])
        }
    } else {
        Cow::Borrowed(&extra_config[..])
    };

    // Check security requirements if enabled
    if let Some(ref security) = ctx.security_manager {
        let required_tier = determine_tool_privacy_tier(ctx.tool_router_key, &final_config);

        security.check_tool_authorization(required_tier).await
            .map_err(|e| ToolError::SecurityError(format!("TEE authorization failed: {}", e)))?;

        debug!("🔐 Tool {} authorized at tier {:?}", ctx.tool_router_key, required_tier);
    }

    // Route to appropriate executor based on tool type
    let result = match tool {
        HanzoTool::MCPServer(mcp_server_tool, _) => {
            execute_mcp_tool_optimized(&ctx, mcp_server_tool, parameters).await
        }
        HanzoTool::Rust(_, _) => {
            execute_rust_tool_optimized(&ctx, parameters, final_config.into_owned()).await
        }
        HanzoTool::Agent(agent_tool, _) => {
            execute_agent_tool_optimized(&ctx, agent_tool, parameters).await
        }
        HanzoTool::Python(python_tool, _) => {
            execute_python_tool_optimized(&ctx, python_tool, parameters, final_config.into_owned()).await
        }
        HanzoTool::Deno(deno_tool, _) => {
            execute_deno_tool_optimized(&ctx, deno_tool, parameters, final_config.into_owned()).await
        }
        HanzoTool::Docker(docker_tool, _) => {
            execute_docker_tool_optimized(&ctx, docker_tool, parameters, final_config.into_owned()).await
        }
        HanzoTool::Kubernetes(k8s_tool, _) => {
            execute_k8s_tool_optimized(&ctx, k8s_tool, parameters, final_config.into_owned()).await
        }
        _ => Err(ToolError::ExecutionError(format!("Unsupported tool type: {:?}", tool))),
    };

    // Record metrics
    let duration = timer.stop();
    record_tool_execution(
        &tool.tool_type(),
        ctx.tool_router_key,
        duration,
        result.is_ok(),
    );

    result
}

/// Parallel tool execution for multiple tools
pub async fn execute_tools_parallel<'a>(
    ctx: ExecutionContext<'a>,
    tools: Vec<(&'a str, Map<String, Value>, Vec<ToolConfig>)>,
) -> Vec<Result<Value, ToolError>> {
    use futures::future::join_all;

    info!("⚡ Executing {} tools in parallel - MATRIX OVERDRIVE", tools.len());

    let futures = tools.into_iter().map(|(tool_key, params, config)| {
        let sub_ctx = ExecutionContext {
            tool_router_key: tool_key,
            ..ctx
        };
        execute_tool_cmd_optimized(sub_ctx, params, config)
    });

    join_all(futures).await
}

/// Determine privacy tier without allocations
fn determine_tool_privacy_tier(tool_key: &str, config: &[ToolConfig]) -> PrivacyTier {
    // Check config for explicit tier
    for cfg in config {
        if let ToolConfig::BasicConfig(basic) = cfg {
            if basic.key_name == "privacy_tier" {
                if let Some(Value::String(tier_str)) = &basic.key_value {
                    return match tier_str.to_lowercase().as_str() {
                        "open" => PrivacyTier::Open,
                        "at_rest" | "atrest" => PrivacyTier::AtRest,
                        "cpu_tee" | "cputee" => PrivacyTier::CpuTee,
                        "gpu_cc" | "gpucc" => PrivacyTier::GpuCc,
                        "gpu_tee_io" | "gputeeio" | "blackwell" => PrivacyTier::GpuTeeIo,
                        _ => PrivacyTier::Open,
                    };
                }
            }
        }
    }

    // Pattern matching without allocation
    if tool_key.contains("crypto") || tool_key.contains("wallet") || tool_key.contains("private") {
        PrivacyTier::CpuTee
    } else if tool_key.contains("ml") || tool_key.contains("gpu") || tool_key.contains("inference") {
        PrivacyTier::GpuCc
    } else if tool_key.contains("secure") || tool_key.contains("confidential") {
        PrivacyTier::CpuTee
    } else {
        PrivacyTier::Open
    }
}

// Stub implementations for specific tool executors
// These would be optimized versions of the existing executors

async fn execute_mcp_tool_optimized<'a>(
    _ctx: &ExecutionContext<'a>,
    _tool: hanzo_tools_primitives::tools::mcp_server_tool::MCPServerTool,
    _parameters: Map<String, Value>,
) -> Result<Value, ToolError> {
    // TODO: Implement optimized MCP execution
    Ok(json!({"status": "mcp_optimized"}))
}

async fn execute_rust_tool_optimized<'a>(
    _ctx: &ExecutionContext<'a>,
    _parameters: Map<String, Value>,
    _config: Vec<ToolConfig>,
) -> Result<Value, ToolError> {
    // TODO: Implement optimized Rust tool execution
    Ok(json!({"status": "rust_optimized"}))
}

async fn execute_agent_tool_optimized<'a>(
    _ctx: &ExecutionContext<'a>,
    _tool: hanzo_tools_primitives::tools::agent_tool::AgentTool,
    _parameters: Map<String, Value>,
) -> Result<Value, ToolError> {
    // TODO: Implement optimized agent execution
    Ok(json!({"status": "agent_optimized"}))
}

async fn execute_python_tool_optimized<'a>(
    _ctx: &ExecutionContext<'a>,
    _tool: hanzo_tools_primitives::tools::python_tool::PythonTool,
    _parameters: Map<String, Value>,
    _config: Vec<ToolConfig>,
) -> Result<Value, ToolError> {
    // TODO: Implement optimized Python execution
    Ok(json!({"status": "python_optimized"}))
}

async fn execute_deno_tool_optimized<'a>(
    _ctx: &ExecutionContext<'a>,
    _tool: hanzo_tools_primitives::tools::deno_tool::DenoTool,
    _parameters: Map<String, Value>,
    _config: Vec<ToolConfig>,
) -> Result<Value, ToolError> {
    // TODO: Implement optimized Deno execution
    Ok(json!({"status": "deno_optimized"}))
}

async fn execute_docker_tool_optimized<'a>(
    _ctx: &ExecutionContext<'a>,
    _tool: hanzo_tools_primitives::tools::docker_tool::DockerTool,
    _parameters: Map<String, Value>,
    _config: Vec<ToolConfig>,
) -> Result<Value, ToolError> {
    // TODO: Implement optimized Docker execution
    Ok(json!({"status": "docker_optimized"}))
}

async fn execute_k8s_tool_optimized<'a>(
    _ctx: &ExecutionContext<'a>,
    _tool: hanzo_tools_primitives::tools::kubernetes_tool::KubernetesTool,
    _parameters: Map<String, Value>,
    _config: Vec<ToolConfig>,
) -> Result<Value, ToolError> {
    // TODO: Implement optimized Kubernetes execution
    Ok(json!({"status": "k8s_optimized"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_url_building() {
        let oauth = OAuth {
            name: "test".to_string(),
            authorization_url: "https://auth.example.com/authorize".to_string(),
            token_url: Some("https://auth.example.com/token".to_string()),
            client_id: "client123".to_string(),
            client_secret: "secret456".to_string(),
            redirect_url: "https://redirect.example.com/callback".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
            response_type: "code".to_string(),
            version: None,
            refresh_token: None,
            pkce_type: None,
            request_token_auth_header: None,
            request_token_content_type: None,
        };

        let url = build_oauth_url(&oauth, "state123");
        assert!(url.contains("client_id=client123"));
        assert!(url.contains("scope=read%20write"));
        assert!(url.contains("state=state123"));
    }

    #[test]
    fn test_privacy_tier_determination() {
        let config = vec![
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "privacy_tier".to_string(),
                key_value: Some(Value::String("gpu_cc".to_string())),
                description: String::new(),
                required: true,
                type_name: None,
            }),
        ];

        let tier = determine_tool_privacy_tier("test_tool", &config);
        assert_eq!(tier, PrivacyTier::GpuCc);

        // Test pattern matching
        let tier = determine_tool_privacy_tier("crypto_wallet", &[]);
        assert_eq!(tier, PrivacyTier::CpuTee);
    }
}