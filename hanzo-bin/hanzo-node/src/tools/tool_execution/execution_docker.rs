use bollard::container::{
    Config, CreateContainerOptions, LogOutput, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StatsOptions, WaitContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, Mount, MountTypeEnum};
use bollard::Docker;
use futures::stream::StreamExt;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::default::Default;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use hanzo_message_primitives::schemas::hanzo_name::HanzoName;
use hanzo_sqlite::SqliteManager;
use hanzo_tools_primitives::tools::error::ToolError;
use hanzo_tools_primitives::tools::parameters::Parameters;
use hanzo_tools_primitives::tools::tool_config::ToolConfig;

/// Docker container configuration for tool execution
#[derive(Debug, Clone)]
pub struct DockerToolConfig {
    /// Docker image to use (e.g., "python:3.11-slim", "node:20-alpine")
    pub image: String,
    /// CPU limit in cores (e.g., 1.5 for 1.5 CPUs)
    pub cpu_limit: Option<f64>,
    /// Memory limit in bytes
    pub memory_limit: Option<i64>,
    /// Memory + swap limit in bytes (-1 for unlimited swap)
    pub memory_swap_limit: Option<i64>,
    /// Network mode ("none", "bridge", "host", or custom network name)
    pub network_mode: Option<String>,
    /// Environment variables to set in the container
    pub env_vars: HashMap<String, String>,
    /// Volumes to mount (host_path -> container_path)
    pub volumes: Vec<(String, String)>,
    /// Working directory inside the container
    pub working_dir: Option<String>,
    /// Maximum execution time before killing the container
    pub timeout_seconds: u64,
    /// Whether to remove the container after execution
    pub auto_remove: bool,
    /// User to run as (e.g., "1000:1000")
    pub user: Option<String>,
    /// Security options (e.g., ["no-new-privileges"])
    pub security_opts: Vec<String>,
    /// Read-only root filesystem
    pub read_only: bool,
    /// Capabilities to add (e.g., ["NET_ADMIN"])
    pub cap_add: Vec<String>,
    /// Capabilities to drop (e.g., ["ALL"])
    pub cap_drop: Vec<String>,
}

impl Default for DockerToolConfig {
    fn default() -> Self {
        Self {
            image: "python:3.11-slim".to_string(),
            cpu_limit: Some(2.0),
            memory_limit: Some(512 * 1024 * 1024), // 512MB
            memory_swap_limit: Some(512 * 1024 * 1024), // No swap
            network_mode: Some("none".to_string()), // Isolated by default
            env_vars: HashMap::new(),
            volumes: vec![],
            working_dir: Some("/workspace".to_string()),
            timeout_seconds: 300, // 5 minutes
            auto_remove: true,
            user: None,
            security_opts: vec!["no-new-privileges".to_string()],
            read_only: false,
            cap_add: vec![],
            cap_drop: vec!["ALL".to_string()],
        }
    }
}

/// Extract Docker configuration from tool config
fn parse_docker_config(extra_config: &[ToolConfig]) -> DockerToolConfig {
    let mut config = DockerToolConfig::default();
    
    for tool_config in extra_config {
        if let ToolConfig::BasicConfig(basic) = tool_config {
            match basic.key_name.as_str() {
                "docker_image" => {
                    if let Some(Value::String(image)) = &basic.key_value {
                        config.image = image.clone();
                    }
                }
                "docker_cpu_limit" => {
                    if let Some(Value::Number(n)) = &basic.key_value {
                        if let Some(cpu) = n.as_f64() {
                            config.cpu_limit = Some(cpu);
                        }
                    }
                }
                "docker_memory_limit" => {
                    if let Some(Value::String(mem)) = &basic.key_value {
                        config.memory_limit = parse_memory_string(mem);
                    }
                }
                "docker_network" => {
                    if let Some(Value::String(net)) = &basic.key_value {
                        config.network_mode = Some(net.clone());
                    }
                }
                "docker_timeout" => {
                    if let Some(Value::Number(n)) = &basic.key_value {
                        if let Some(timeout) = n.as_u64() {
                            config.timeout_seconds = timeout;
                        }
                    }
                }
                "docker_user" => {
                    if let Some(Value::String(user)) = &basic.key_value {
                        config.user = Some(user.clone());
                    }
                }
                "docker_readonly" => {
                    if let Some(Value::Bool(ro)) = &basic.key_value {
                        config.read_only = *ro;
                    }
                }
                _ => {}
            }
        }
    }
    
    config
}

/// Parse memory string like "512M", "1G" to bytes
fn parse_memory_string(mem_str: &str) -> Option<i64> {
    let mem_str = mem_str.to_uppercase();
    
    if let Some(gb_str) = mem_str.strip_suffix("G") {
        gb_str.parse::<i64>().ok().map(|gb| gb * 1024 * 1024 * 1024)
    } else if let Some(mb_str) = mem_str.strip_suffix("M") {
        mb_str.parse::<i64>().ok().map(|mb| mb * 1024 * 1024)
    } else if let Some(kb_str) = mem_str.strip_suffix("K") {
        kb_str.parse::<i64>().ok().map(|kb| kb * 1024)
    } else {
        mem_str.parse::<i64>().ok()
    }
}

/// Execute a tool inside a Docker container
pub async fn execute_docker_tool(
    bearer: String,
    db: Arc<SqliteManager>,
    node_name: HanzoName,
    parameters: Map<String, Value>,
    extra_config: Vec<ToolConfig>,
    tool_id: String,
    app_id: String,
    agent_id: Option<String>,
    code: String,
    language: String, // "python", "javascript", "rust", etc.
    mounts: Option<Vec<String>>,
) -> Result<Value, ToolError> {
    // Parse Docker configuration from tool config
    let docker_config = parse_docker_config(&extra_config);
    
    // Connect to Docker daemon
    let docker = Docker::connect_with_local_defaults()
        .map_err(|e| ToolError::ExecutionError(format!("Failed to connect to Docker: {}", e)))?;
    
    // Check if Docker is running
    docker.ping().await
        .map_err(|e| ToolError::ExecutionError(format!("Docker daemon not running: {}", e)))?;
    
    // Pull the image if needed
    ensure_image_exists(&docker, &docker_config.image).await?;
    
    // Generate a unique container name
    let container_name = format!("hanzo-tool-{}-{}", tool_id, uuid::Uuid::new_v4());
    
    // Create the container
    let container_id = create_tool_container(
        &docker,
        &container_name,
        &docker_config,
        &code,
        &language,
        parameters,
        mounts,
    ).await?;
    
    // Start the container
    docker.start_container(&container_id, None::<StartContainerOptions<String>>)
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Failed to start container: {}", e)))?;
    
    // Execute with timeout
    let result = timeout(
        Duration::from_secs(docker_config.timeout_seconds),
        execute_and_collect_output(&docker, &container_id)
    ).await;
    
    // Clean up the container
    if docker_config.auto_remove {
        let _ = docker.remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            })
        ).await;
    }
    
    // Process the result
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // Timeout occurred - kill the container
            let _ = docker.kill_container::<String>(&container_id, None).await;
            if docker_config.auto_remove {
                let _ = docker.remove_container(
                    &container_id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    })
                ).await;
            }
            Err(ToolError::ExecutionError(format!(
                "Container execution timed out after {} seconds",
                docker_config.timeout_seconds
            )))
        }
    }
}

/// Ensure the Docker image exists locally, pulling if necessary
async fn ensure_image_exists(docker: &Docker, image: &str) -> Result<(), ToolError> {
    // Check if image exists locally
    match docker.inspect_image(image).await {
        Ok(_) => {
            log::info!("Docker image {} already exists locally", image);
            return Ok(());
        }
        Err(_) => {
            log::info!("Docker image {} not found locally, pulling...", image);
        }
    }
    
    // Pull the image
    let options = Some(CreateImageOptions {
        from_image: image,
        ..Default::default()
    });
    
    let mut stream = docker.create_image(options, None, None);
    
    while let Some(result) = stream.next().await {
        match result {
            Ok(info) => {
                if let Some(status) = info.status {
                    log::debug!("Pull status: {}", status);
                }
            }
            Err(e) => {
                return Err(ToolError::ExecutionError(format!("Failed to pull image: {}", e)));
            }
        }
    }
    
    log::info!("Successfully pulled Docker image {}", image);
    Ok(())
}

/// Create a Docker container for tool execution
async fn create_tool_container(
    docker: &Docker,
    name: &str,
    config: &DockerToolConfig,
    code: &str,
    language: &str,
    parameters: Map<String, Value>,
    mounts: Option<Vec<String>>,
) -> Result<String, ToolError> {
    // Prepare the command based on language
    let (cmd, code_file) = match language {
        "python" => {
            (vec!["python", "-c"], Some(code.to_string()))
        }
        "javascript" | "typescript" => {
            (vec!["node", "-e"], Some(code.to_string()))
        }
        "rust" => {
            // For Rust, we'd need to compile first
            return Err(ToolError::ExecutionError(
                "Rust execution in Docker not yet implemented".to_string()
            ));
        }
        _ => {
            (vec!["sh", "-c"], Some(code.to_string()))
        }
    };
    
    // Build environment variables
    let mut env = vec![];
    for (key, value) in &config.env_vars {
        env.push(format!("{}={}", key, value));
    }
    
    // Add parameters as environment variables
    env.push(format!("HANZO_PARAMS={}", serde_json::to_string(&parameters)
        .map_err(|e| ToolError::ExecutionError(format!("Failed to serialize parameters: {}", e)))?));
    
    // Build mounts
    let mut docker_mounts = vec![];
    if let Some(mount_paths) = mounts {
        for mount_path in mount_paths {
            let parts: Vec<&str> = mount_path.split(':').collect();
            if parts.len() == 2 {
                docker_mounts.push(Mount {
                    target: Some(parts[1].to_string()),
                    source: Some(parts[0].to_string()),
                    typ: Some(MountTypeEnum::BIND),
                    read_only: Some(config.read_only),
                    ..Default::default()
                });
            }
        }
    }
    
    // Build the container configuration
    let mut container_config = Config {
        image: Some(config.image.clone()),
        cmd: Some(cmd.into_iter().map(String::from).collect()),
        env: Some(env),
        working_dir: config.working_dir.clone(),
        user: config.user.clone(),
        ..Default::default()
    };
    
    // Add the code as the last command argument
    if let Some(code_content) = code_file {
        if let Some(ref mut cmd) = container_config.cmd {
            cmd.push(code_content);
        }
    }
    
    // Build host configuration with resource limits
    let mut host_config = HostConfig {
        mounts: if docker_mounts.is_empty() { None } else { Some(docker_mounts) },
        network_mode: config.network_mode.clone(),
        auto_remove: Some(config.auto_remove),
        security_opt: if config.security_opts.is_empty() { 
            None 
        } else { 
            Some(config.security_opts.clone()) 
        },
        readonly_rootfs: Some(config.read_only),
        cap_add: if config.cap_add.is_empty() { None } else { Some(config.cap_add.clone()) },
        cap_drop: if config.cap_drop.is_empty() { None } else { Some(config.cap_drop.clone()) },
        ..Default::default()
    };
    
    // Set CPU limit
    if let Some(cpu_limit) = config.cpu_limit {
        host_config.cpu_quota = Some((cpu_limit * 100000.0) as i64);
        host_config.cpu_period = Some(100000);
    }
    
    // Set memory limits
    if let Some(mem_limit) = config.memory_limit {
        host_config.memory = Some(mem_limit);
    }
    if let Some(swap_limit) = config.memory_swap_limit {
        host_config.memory_swap = Some(swap_limit);
    }
    
    container_config.host_config = Some(host_config);
    
    // Create the container
    let options = CreateContainerOptions {
        name,
        platform: None,
    };
    
    let container_info = docker.create_container(Some(options), container_config)
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Failed to create container: {}", e)))?;
    
    Ok(container_info.id)
}

/// Execute the container and collect output
async fn execute_and_collect_output(
    docker: &Docker,
    container_id: &str,
) -> Result<Value, ToolError> {
    // Wait for container to finish
    let mut wait_stream = docker.wait_container(
        container_id,
        Some(WaitContainerOptions {
            condition: "not-running",
        })
    );
    
    // Collect logs while waiting
    let logs_options = LogsOptions {
        stdout: true,
        stderr: true,
        follow: false,
        tail: "all",
        ..Default::default()
    };
    
    let mut log_stream = docker.logs(container_id, Some(logs_options));
    let mut stdout = String::new();
    let mut stderr = String::new();
    
    // Collect logs
    while let Some(log_result) = log_stream.next().await {
        match log_result {
            Ok(LogOutput::StdOut { message }) => {
                stdout.push_str(&String::from_utf8_lossy(&message));
            }
            Ok(LogOutput::StdErr { message }) => {
                stderr.push_str(&String::from_utf8_lossy(&message));
            }
            Ok(_) => {}
            Err(e) => {
                log::warn!("Error reading container logs: {}", e);
            }
        }
    }
    
    // Wait for container to exit
    let exit_status = if let Some(wait_result) = wait_stream.next().await {
        match wait_result {
            Ok(status) => status.status_code,
            Err(e) => {
                return Err(ToolError::ExecutionError(format!("Failed to wait for container: {}", e)));
            }
        }
    } else {
        return Err(ToolError::ExecutionError("Container wait stream ended unexpectedly".to_string()));
    };
    
    // Check exit status
    if exit_status != 0 {
        return Err(ToolError::ExecutionError(format!(
            "Container exited with status {}: {}",
            exit_status,
            stderr
        )));
    }
    
    // Try to parse stdout as JSON, fallback to text
    let output = if let Ok(json_value) = serde_json::from_str::<Value>(&stdout) {
        json_value
    } else {
        json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_status,
        })
    };
    
    Ok(output)
}

/// Get container resource usage statistics
pub async fn get_container_stats(
    docker: &Docker,
    container_id: &str,
) -> Result<Value, ToolError> {
    let options = StatsOptions {
        stream: false,
        one_shot: true,
    };
    
    let mut stats_stream = docker.stats(container_id, Some(options));
    
    if let Some(stats_result) = stats_stream.next().await {
        match stats_result {
            Ok(stats) => {
                // Extract relevant statistics
                // Extract CPU usage - bollard's Stats has direct fields
                let cpu_usage = stats.cpu_stats.cpu_usage.total_usage;
                    
                // Extract memory stats
                let memory_usage = stats.memory_stats.usage
                    .unwrap_or(0);
                    
                let memory_limit = stats.memory_stats.limit
                    .unwrap_or(0);
                
                Ok(json!({
                    "cpu_usage": cpu_usage,
                    "memory_usage": memory_usage,
                    "memory_limit": memory_limit,
                    "memory_percent": if memory_limit > 0 {
                        (memory_usage as f64 / memory_limit as f64) * 100.0
                    } else {
                        0.0
                    }
                }))
            }
            Err(e) => {
                Err(ToolError::ExecutionError(format!("Failed to get container stats: {}", e)))
            }
        }
    } else {
        Err(ToolError::ExecutionError("Stats stream ended unexpectedly".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_memory_string() {
        assert_eq!(parse_memory_string("1024"), Some(1024));
        assert_eq!(parse_memory_string("512M"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory_string("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory_string("2048K"), Some(2048 * 1024));
        assert_eq!(parse_memory_string("invalid"), None);
    }
    
    #[test]
    fn test_docker_config_default() {
        let config = DockerToolConfig::default();
        assert_eq!(config.image, "python:3.11-slim");
        assert_eq!(config.cpu_limit, Some(2.0));
        assert_eq!(config.memory_limit, Some(512 * 1024 * 1024));
        assert_eq!(config.network_mode, Some("none".to_string()));
        assert_eq!(config.timeout_seconds, 300);
        assert!(config.auto_remove);
    }
    
    #[test]
    fn test_parse_docker_config() {
        let configs = vec![
            ToolConfig::BasicConfig(hanzo_tools_primitives::tools::tool_config::BasicConfig {
                key_name: "docker_image".to_string(),
                key_value: Some(Value::String("node:20-alpine".to_string())),
                description: "Docker image".to_string(),
                required: false,
                type_name: None,
            }),
            ToolConfig::BasicConfig(hanzo_tools_primitives::tools::tool_config::BasicConfig {
                key_name: "docker_cpu_limit".to_string(),
                key_value: Some(Value::Number(serde_json::Number::from_f64(1.5).unwrap())),
                description: "CPU limit".to_string(),
                required: false,
                type_name: None,
            }),
            ToolConfig::BasicConfig(hanzo_tools_primitives::tools::tool_config::BasicConfig {
                key_name: "docker_memory_limit".to_string(),
                key_value: Some(Value::String("1G".to_string())),
                description: "Memory limit".to_string(),
                required: false,
                type_name: None,
            }),
        ];
        
        let config = parse_docker_config(&configs);
        assert_eq!(config.image, "node:20-alpine");
        assert_eq!(config.cpu_limit, Some(1.5));
        assert_eq!(config.memory_limit, Some(1024 * 1024 * 1024));
    }
}