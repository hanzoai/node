use anyhow::{anyhow, Result};
use uuid::Uuid;
use async_trait::async_trait;
use k8s_openapi::api::batch::v1::{Job, JobSpec, JobStatus};
use k8s_openapi::api::core::v1::{
    ConfigMap, Container, EnvVar, PodSpec, PodTemplateSpec, Secret, Volume,
    VolumeMount, ConfigMapVolumeSource, SecretVolumeSource, EmptyDirVolumeSource,
    ResourceRequirements, PodSecurityContext, SecurityContext,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::{
    api::{Api, DeleteParams, ListParams, PostParams, WatchEvent, WatchParams},
    runtime::wait::{await_condition, conditions},
    Client, Config, Error as KubeError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use futures::{Stream, StreamExt, TryStreamExt};
use tokio::io::AsyncBufReadExt;
use base64::Engine;
use hanzo_tools::tools::kubernetes_tools::{KubernetesTool, K8sResourceRequirements as ToolK8sResourceRequirements};
use hanzo_tools::tools::tool_config::ToolConfig;
use hanzo_tools::tools::error::ToolError;
use hanzo_messages::schemas::hanzo_name::HanzoName;

/// Simple result structure for Kubernetes tool execution
#[derive(Debug)]
struct K8sToolResult {
    pub success: bool,
    pub data: Value,
    pub errors: Option<Vec<String>>,
}

/// Resource requirements for Kubernetes jobs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sResourceRequirements {
    pub cpu_request: Option<String>,
    pub cpu_limit: Option<String>,
    pub memory_request: Option<String>,
    pub memory_limit: Option<String>,
    pub gpu_count: Option<u32>,
    pub gpu_type: Option<String>, // e.g., "nvidia.com/gpu", "amd.com/gpu"
}

/// Kubernetes execution configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sExecutionConfig {
    pub namespace: String,
    pub service_account: Option<String>,
    pub image_pull_secret: Option<String>,
    pub node_selector: Option<BTreeMap<String, String>>,
    pub tolerations: Option<Vec<Value>>,
    pub resources: K8sResourceRequirements,
    pub timeout_seconds: u64,
    pub cleanup_on_completion: bool,
    pub enable_istio_sidecar: bool,
}

impl Default for K8sExecutionConfig {
    fn default() -> Self {
        Self {
            namespace: "hanzo-tools".to_string(),
            service_account: None,
            image_pull_secret: None,
            node_selector: None,
            tolerations: None,
            resources: K8sResourceRequirements {
                cpu_request: Some("100m".to_string()),
                cpu_limit: Some("1000m".to_string()),
                memory_request: Some("128Mi".to_string()),
                memory_limit: Some("1Gi".to_string()),
                gpu_count: None,
                gpu_type: None,
            },
            timeout_seconds: 3600, // 1 hour default
            cleanup_on_completion: true,
            enable_istio_sidecar: false,
        }
    }
}

/// Kubernetes tool executor
pub struct K8sToolExecutor {
    client: Client,
    config: K8sExecutionConfig,
}

impl K8sToolExecutor {
    /// Create new Kubernetes executor from configuration
    pub async fn new(config: K8sExecutionConfig) -> Result<Self> {
        // Try to load config from environment/kubeconfig
        let kube_config = Config::infer().await?;
        let client = Client::try_from(kube_config)?;

        Ok(Self { client, config })
    }

    /// Create from explicit configuration
    pub async fn from_config(kube_config: Config, exec_config: K8sExecutionConfig) -> Result<Self> {
        let client = Client::try_from(kube_config)?;
        Ok(Self {
            client,
            config: exec_config,
        })
    }

    /// Execute a tool as a Kubernetes Job
    pub async fn execute_tool(
        &self,
        tool: &KubernetesTool,
        parameters: Map<String, Value>,
        extra_config: Vec<ToolConfig>,
        node_name: HanzoName,
    ) -> Result<Value> {
        let job_name = format!("hanzo-tool-{}-{}",
            tool.name.to_lowercase().replace("_", "-"),
            &Uuid::new_v4().to_string()[0..8]
        );

        // Create ConfigMap for tool code and parameters
        let config_map = self.create_config_map(&job_name, tool, &parameters).await?;

        // Create Secret for sensitive data if needed
        let secret = if self.has_sensitive_data(&extra_config) {
            Some(self.create_secret(&job_name, &extra_config).await?)
        } else {
            None
        };

        // Create the Job
        let job = self.create_job(
            &job_name,
            tool,
            &config_map,
            secret.as_ref(),
            &parameters,
            &extra_config,
        ).await?;

        // Wait for job completion and stream logs
        let result = self.wait_and_stream_logs(&job_name, &job).await?;

        // Cleanup if configured
        if self.config.cleanup_on_completion {
            self.cleanup_resources(&job_name).await?;
        }

        Ok(result.data)
    }

    /// Create ConfigMap with tool code and parameters
    async fn create_config_map(
        &self,
        name: &str,
        tool: &KubernetesTool,
        parameters: &Map<String, Value>,
    ) -> Result<ConfigMap> {
        let config_maps: Api<ConfigMap> = Api::namespaced(
            self.client.clone(),
            &self.config.namespace,
        );

        let mut data = BTreeMap::new();
        data.insert("tool_code".to_string(), tool.code.clone());
        data.insert("parameters".to_string(), serde_json::to_string(parameters)?);

        // Add any additional files from tool assets
        if let Some(assets) = &tool.assets {
            for asset in assets {
                data.insert(asset.file_name.clone(), asset.data.clone());
            }
        }

        let cm = ConfigMap {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(self.config.namespace.clone()),
                labels: Some(BTreeMap::from([
                    ("app".to_string(), "hanzo-node".to_string()),
                    ("tool".to_string(), tool.name.clone()),
                ])),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };

        config_maps
            .create(&PostParams::default(), &cm)
            .await
            .map_err(|e| anyhow!("Failed to create ConfigMap: {}", e))
    }

    /// Create Secret for sensitive configuration
    async fn create_secret(
        &self,
        name: &str,
        extra_config: &[ToolConfig],
    ) -> Result<Secret> {
        let secrets: Api<Secret> = Api::namespaced(
            self.client.clone(),
            &self.config.namespace,
        );

        let mut data = BTreeMap::new();

        // Extract sensitive fields from config
        for config in extra_config {
            if let ToolConfig::BasicConfig(basic) = config {
                // Check if this config item is sensitive
                if let Some(value) = &basic.key_value {
                    let key = &basic.key_name;
                    if key.contains("key") || key.contains("secret") || key.contains("token") {
                        let encoded = base64::engine::general_purpose::STANDARD
                            .encode(value.to_string().as_bytes());
                        data.insert(
                            key.clone(),
                            k8s_openapi::ByteString(encoded.into_bytes()),
                        );
                    }
                }
            }
        }

        let secret = Secret {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(self.config.namespace.clone()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };

        secrets
            .create(&PostParams::default(), &secret)
            .await
            .map_err(|e| anyhow!("Failed to create Secret: {}", e))
    }

    /// Create Kubernetes Job for tool execution
    async fn create_job(
        &self,
        name: &str,
        tool: &KubernetesTool,
        config_map: &ConfigMap,
        secret: Option<&Secret>,
        parameters: &Map<String, Value>,
        extra_config: &[ToolConfig],
    ) -> Result<Job> {
        let jobs: Api<Job> = Api::namespaced(
            self.client.clone(),
            &self.config.namespace,
        );

        // Build container spec
        let mut container = Container {
            name: "tool-executor".to_string(),
            image: Some(tool.image.clone()),
            image_pull_policy: Some("IfNotPresent".to_string()),
            ..Default::default()
        };

        // Add environment variables
        let mut env_vars = vec![
            EnvVar {
                name: "TOOL_NAME".to_string(),
                value: Some(tool.name.clone()),
                ..Default::default()
            },
            EnvVar {
                name: "HANZO_NODE_ENV".to_string(),
                value: Some("kubernetes".to_string()),
                ..Default::default()
            },
        ];

        // Add parameters as env vars
        for (key, value) in parameters {
            env_vars.push(EnvVar {
                name: format!("PARAM_{}", key.to_uppercase()),
                value: Some(value.to_string()),
                ..Default::default()
            });
        }

        container.env = Some(env_vars);

        // Add volume mounts
        let mut volume_mounts = vec![
            VolumeMount {
                name: "tool-config".to_string(),
                mount_path: "/config".to_string(),
                read_only: Some(true),
                ..Default::default()
            },
            VolumeMount {
                name: "workspace".to_string(),
                mount_path: "/workspace".to_string(),
                ..Default::default()
            },
        ];

        if secret.is_some() {
            volume_mounts.push(VolumeMount {
                name: "tool-secrets".to_string(),
                mount_path: "/secrets".to_string(),
                read_only: Some(true),
                ..Default::default()
            });
        }

        container.volume_mounts = Some(volume_mounts);

        // Set resource requirements
        container.resources = Some(self.build_resource_requirements(tool));

        // Set command if specified
        if let Some(entrypoint) = &tool.entrypoint {
            container.command = Some(entrypoint.clone());
        }

        if let Some(args) = &tool.args {
            container.args = Some(args.clone());
        }

        // Security context
        container.security_context = Some(SecurityContext {
            allow_privilege_escalation: Some(false),
            run_as_non_root: Some(true),
            run_as_user: Some(1000),
            ..Default::default()
        });

        // Build pod spec
        let mut pod_spec = PodSpec {
            containers: vec![container],
            restart_policy: Some("Never".to_string()),
            ..Default::default()
        };

        // Add volumes
        let mut volumes = vec![
            Volume {
                name: "tool-config".to_string(),
                config_map: Some(ConfigMapVolumeSource {
                    name: config_map.metadata.name.clone().unwrap_or_else(|| name.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Volume {
                name: "workspace".to_string(),
                empty_dir: Some(EmptyDirVolumeSource {
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];

        if let Some(secret) = secret {
            volumes.push(Volume {
                name: "tool-secrets".to_string(),
                secret: Some(SecretVolumeSource {
                    secret_name: Some(secret.metadata.name.clone().unwrap_or_else(|| name.to_string())),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }

        pod_spec.volumes = Some(volumes);

        // Set service account if configured
        if let Some(sa) = &self.config.service_account {
            pod_spec.service_account_name = Some(sa.clone());
        }

        // Set node selector
        if let Some(node_selector) = &self.config.node_selector {
            pod_spec.node_selector = Some(node_selector.clone());
        }

        // Set tolerations
        if let Some(tolerations) = &self.config.tolerations {
            pod_spec.tolerations = Some(
                tolerations
                    .iter()
                    .filter_map(|t| serde_json::from_value(t.clone()).ok())
                    .collect(),
            );
        }

        // Disable Istio sidecar if configured
        let mut annotations = BTreeMap::new();
        if !self.config.enable_istio_sidecar {
            annotations.insert(
                "sidecar.istio.io/inject".to_string(),
                "false".to_string(),
            );
        }

        // Create job spec
        let job = Job {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(self.config.namespace.clone()),
                labels: Some(BTreeMap::from([
                    ("app".to_string(), "hanzo-node".to_string()),
                    ("tool".to_string(), tool.name.clone()),
                    ("type".to_string(), "kubernetes-tool".to_string()),
                ])),
                ..Default::default()
            },
            spec: Some(JobSpec {
                template: PodTemplateSpec {
                    metadata: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        annotations: Some(annotations),
                        labels: Some(BTreeMap::from([
                            ("app".to_string(), "hanzo-node".to_string()),
                            ("tool".to_string(), tool.name.clone()),
                        ])),
                        ..Default::default()
                    }),
                    spec: Some(pod_spec),
                },
                active_deadline_seconds: Some(self.config.timeout_seconds as i64),
                backoff_limit: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };

        jobs
            .create(&PostParams::default(), &job)
            .await
            .map_err(|e| anyhow!("Failed to create Job: {}", e))
    }

    /// Build resource requirements for container
    fn build_resource_requirements(&self, tool: &KubernetesTool) -> ResourceRequirements {
        let mut requests = BTreeMap::new();
        let mut limits = BTreeMap::new();

        // Use tool-specific requirements or fall back to config
        let cpu_request = tool.resources.as_ref()
            .and_then(|r| r.cpu_request.clone())
            .or_else(|| self.config.resources.cpu_request.clone());

        let cpu_limit = tool.resources.as_ref()
            .and_then(|r| r.cpu_limit.clone())
            .or_else(|| self.config.resources.cpu_limit.clone());

        let memory_request = tool.resources.as_ref()
            .and_then(|r| r.memory_request.clone())
            .or_else(|| self.config.resources.memory_request.clone());

        let memory_limit = tool.resources.as_ref()
            .and_then(|r| r.memory_limit.clone())
            .or_else(|| self.config.resources.memory_limit.clone());

        if let Some(cpu) = cpu_request {
            requests.insert("cpu".to_string(), Quantity(cpu));
        }
        if let Some(cpu) = cpu_limit {
            limits.insert("cpu".to_string(), Quantity(cpu));
        }
        if let Some(mem) = memory_request {
            requests.insert("memory".to_string(), Quantity(mem));
        }
        if let Some(mem) = memory_limit {
            limits.insert("memory".to_string(), Quantity(mem));
        }

        // GPU resources
        let gpu_count = tool.resources.as_ref()
            .and_then(|r| r.gpu_count)
            .or(self.config.resources.gpu_count);

        let gpu_type = tool.resources.as_ref()
            .and_then(|r| r.gpu_type.clone())
            .or_else(|| self.config.resources.gpu_type.clone())
            .unwrap_or_else(|| "nvidia.com/gpu".to_string());

        if let Some(count) = gpu_count {
            limits.insert(gpu_type, Quantity(count.to_string()));
        }

        ResourceRequirements {
            requests: Some(requests),
            limits: Some(limits),
            ..Default::default()
        }
    }

    /// Wait for job completion and stream logs
    async fn wait_and_stream_logs(&self, name: &str, job: &Job) -> Result<K8sToolResult> {
        let jobs: Api<Job> = Api::namespaced(
            self.client.clone(),
            &self.config.namespace,
        );

        let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(
            self.client.clone(),
            &self.config.namespace,
        );

        // Start streaming logs from pod
        let mut output = String::new();
        let mut errors = Vec::new();

        // Wait for pod to be created
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Find the pod created by this job
        let pod_list = pods
            .list(&ListParams::default().labels(&format!("tool={}", name)))
            .await?;

        if let Some(pod) = pod_list.items.first() {
            if let Some(pod_name) = &pod.metadata.name {
                // Stream logs
                let log_params = kube::api::LogParams {
                    follow: true,
                    container: Some("tool-executor".to_string()),
                    ..Default::default()
                };

                // Get logs as a string instead of stream
                let logs = pods.logs(pod_name, &log_params).await?;

                // Process logs
                let log_future = async {
                    for line in logs.lines() {
                        output.push_str(line);
                        output.push('\n');
                        log::info!("K8s tool output: {}", line);
                    }
                    Ok::<_, anyhow::Error>(())
                };

                // Run log streaming with timeout
                let _ = timeout(
                    Duration::from_secs(self.config.timeout_seconds),
                    log_future,
                ).await;
            }
        }

        // Wait for job completion
        let wait_result = timeout(
            Duration::from_secs(self.config.timeout_seconds),
            await_condition(jobs.clone(), name, conditions::is_job_completed()),
        ).await;

        let final_job = jobs.get(name).await?;

        // Check job status
        let success = if let Some(status) = &final_job.status {
            status.succeeded.unwrap_or(0) > 0
        } else {
            false
        };

        if !success {
            if let Some(status) = &final_job.status {
                if let Some(conditions) = &status.conditions {
                    for condition in conditions {
                        if condition.type_ == "Failed" {
                            if let Some(message) = &condition.message {
                                errors.push(message.clone());
                            }
                        }
                    }
                }
            }
            if wait_result.is_err() {
                errors.push("Job execution timed out".to_string());
            }
        }

        // Parse output for structured result
        let data = if output.starts_with("{") {
            serde_json::from_str(&output).unwrap_or_else(|_| json!({"output": output}))
        } else {
            json!({"output": output})
        };

        Ok(K8sToolResult {
            success,
            data,
            errors: if errors.is_empty() { None } else { Some(errors) },
        })
    }

    /// Clean up Kubernetes resources
    async fn cleanup_resources(&self, name: &str) -> Result<()> {
        let jobs: Api<Job> = Api::namespaced(
            self.client.clone(),
            &self.config.namespace,
        );

        let config_maps: Api<ConfigMap> = Api::namespaced(
            self.client.clone(),
            &self.config.namespace,
        );

        let secrets: Api<Secret> = Api::namespaced(
            self.client.clone(),
            &self.config.namespace,
        );

        // Delete job (will cascade delete pods)
        let _ = jobs.delete(name, &DeleteParams::default()).await;

        // Delete ConfigMap
        let _ = config_maps.delete(name, &DeleteParams::default()).await;

        // Delete Secret if it exists
        let _ = secrets.delete(name, &DeleteParams::default()).await;

        log::info!("Cleaned up Kubernetes resources for job: {}", name);
        Ok(())
    }

    /// Check if configuration contains sensitive data
    fn has_sensitive_data(&self, extra_config: &[ToolConfig]) -> bool {
        for config in extra_config {
            if let ToolConfig::BasicConfig(basic) = config {
                let key = &basic.key_name;
                if key.contains("key") || key.contains("secret") || key.contains("token") {
                    return true;
                }
            }
        }
        false
    }
}

/// Execute a Kubernetes tool
pub async fn execute_kubernetes_tool(
    tool: KubernetesTool,
    parameters: Map<String, Value>,
    extra_config: Vec<ToolConfig>,
    node_name: HanzoName,
) -> Result<Value> {
    // Extract K8s configuration from extra_config
    let exec_config = extract_k8s_config(&extra_config).unwrap_or_default();

    // Create executor
    let executor = K8sToolExecutor::new(exec_config).await?;

    // Execute tool
    let result = executor.execute_tool(&tool, parameters, extra_config, node_name).await?;

    Ok(result)
}

/// Extract Kubernetes configuration from tool config
fn extract_k8s_config(configs: &[ToolConfig]) -> Option<K8sExecutionConfig> {
    for config in configs {
        if let ToolConfig::BasicConfig(basic) = config {
            // Check if this is a kubernetes config
            if basic.key_name == "kubernetes" {
                if let Some(value) = &basic.key_value {
                    if let Ok(exec_config) = serde_json::from_value::<K8sExecutionConfig>(value.clone()) {
                        return Some(exec_config);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_k8s_config_extraction() {
        use hanzo_tools::tools::tool_config::BasicConfig;

        let config = vec![
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "kubernetes".to_string(),
                description: "Kubernetes configuration".to_string(),
                required: true,
                type_name: Some("object".to_string()),
                key_value: Some(serde_json::json!({
                    "namespace": "test-namespace",
                    "timeout_seconds": 120
                })),
            }),
        ];

        let k8s_config = extract_k8s_config(&config);
        assert!(k8s_config.is_some());
        let k8s_config = k8s_config.unwrap();
        assert_eq!(k8s_config.namespace, "test-namespace");
        assert_eq!(k8s_config.timeout_seconds, 120);
    }

    #[tokio::test]
    async fn test_resource_requirements_builder() {
        let config = K8sExecutionConfig::default();
        let executor = K8sToolExecutor {
            client: Client::try_from(Config::default()).unwrap(),
            config: config.clone(),
        };

        let tool = KubernetesTool {
            name: "test-tool".to_string(),
            description: "Test tool".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            image: "hanzo/tool:latest".to_string(),
            entrypoint: None,
            args: None,
            code: "".to_string(),
            config: Default::default(),
            input_args: Default::default(),
            output: Default::default(),
            activated: true,
            embedding: None,
            result: None,
            assets: None,
            oauth: None,
            resources: Some(K8sResourceRequirements {
                cpu_request: Some("200m".to_string()),
                cpu_limit: Some("2000m".to_string()),
                memory_request: Some("256Mi".to_string()),
                memory_limit: Some("2Gi".to_string()),
                gpu_count: Some(1),
                gpu_type: Some("nvidia.com/gpu".to_string()),
            }),
        };

        let resources = executor.build_resource_requirements(&tool);

        assert!(resources.requests.is_some());
        assert!(resources.limits.is_some());

        let requests = resources.requests.unwrap();
        assert_eq!(requests.get("cpu").unwrap().0, "200m");
        assert_eq!(requests.get("memory").unwrap().0, "256Mi");

        let limits = resources.limits.unwrap();
        assert_eq!(limits.get("cpu").unwrap().0, "2000m");
        assert_eq!(limits.get("memory").unwrap().0, "2Gi");
        assert_eq!(limits.get("nvidia.com/gpu").unwrap().0, "1");
    }
}