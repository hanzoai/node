use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use serde_json::{Map, Value};
use hanzo_messages::schemas::{
    hanzo_name::HanzoName,
    hanzo_tools::DynamicToolType,
};
use hanzo_db_sqlite::SqliteManager;
use hanzo_tools::tools::error::ToolError;
use hanzo_tools::tools::parameters::Parameters;
use hanzo_tools::tools::hanzo_tool::HanzoToolHeader;
use hanzo_tools::tools::tool_output_arg::ToolOutputArg;
use std::sync::Arc;
use tokio::sync::Mutex;
use x25519_dalek::{PublicKey as EncryptionPublicKey, StaticSecret as EncryptionStaticKey};

use crate::llm_provider::job_manager::JobManager;
use crate::managers::IdentityManager;
use crate::tools::tool_execution::execution_coordinator::execute_code;
use crate::tools::tool_implementation::tool_traits::ToolExecutor;

pub struct CodeExecutionProcessorTool {
    pub tool: HanzoToolHeader,
    pub _tool_embedding: Option<Vec<f32>>,
}

impl CodeExecutionProcessorTool {
    pub fn new() -> Self {
        let mut params = Parameters::new();
        params.add_property(
            "code".to_string(),
            "string".to_string(),
            "Python source code to execute.".to_string(),
            true,
            None,
        );

        Self {
            tool: HanzoToolHeader {
                name: "Hanzo Python Code Execution".to_string(),
                description: "Run arbitrary Python code using Hanzo's code execution runtime.".to_string(),
                tool_router_key: "local:::__official_hanzo:::hanzo_python_code_execution".to_string(),
                tool_type: "Rust".to_string(),
                formatted_tool_summary_for_ui: "Execute Python code".to_string(),
                author: "@@official.hanzo".to_string(),
                version: "1.0.0".to_string(),
                enabled: true,
                mcp_enabled: Some(false),
                input_args: params,
                output_arg: ToolOutputArg {
                    json: r#"{"type":"object","properties":{"stdout":{"type":"string"},"stderr":{"type":"string"},"result":{"type":"object"},"__created_files__":{"type":"array","items":{"type":"string"}}}}"#.to_string(),
                },
                config: None,
                usage_type: None,
                tool_offering: None,
            },
            _tool_embedding: None,
        }
    }
}

#[async_trait]
impl ToolExecutor for CodeExecutionProcessorTool {
    async fn execute(
        bearer: String,
        tool_id: String,
        app_id: String,
        db_clone: Arc<SqliteManager>,
        node_name_clone: HanzoName,
        identity_manager_clone: Arc<Mutex<IdentityManager>>,
        job_manager_clone: Arc<Mutex<JobManager>>,
        encryption_secret_key_clone: EncryptionStaticKey,
        encryption_public_key_clone: EncryptionPublicKey,
        signing_secret_key_clone: SigningKey,
        parameters: &Map<String, Value>,
        llm_provider: String,
    ) -> Result<Value, ToolError> {
        let raw_code = parameters
            .get("code")
            .and_then(|value| value.as_str())
            .ok_or_else(|| ToolError::ExecutionError("'code' parameter is required".to_string()))?;

        execute_code(
            DynamicToolType::PythonDynamic,
            raw_code.to_string(),
            Vec::new(),
            Map::new(),
            Vec::new(),
            None,
            db_clone,
            tool_id,
            app_id,
            None,
            llm_provider,
            bearer,
            node_name_clone,
            None,
            None,
            None,
            identity_manager_clone,
            job_manager_clone,
            encryption_secret_key_clone,
            encryption_public_key_clone,
            signing_secret_key_clone,
        )
        .await
    }
}
