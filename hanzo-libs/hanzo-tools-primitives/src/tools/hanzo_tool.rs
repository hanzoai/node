use std::env;

use crate::tools::error::ToolError;
use crate::tools::rust_tools::RustTool;
use serde_json::{self, Value};

use hanzo_message_primitives::schemas::tool_router_key::ToolRouterKey;
use hanzo_message_primitives::schemas::{
    indexable_version::IndexableVersion,
    hanzo_tool_offering::{HanzoToolOffering, UsageType},
};

use super::agent_tool_wrapper::AgentToolWrapper;
use super::tool_config::OAuth;
use super::tool_playground::{SqlQuery, SqlTable, ToolPlaygroundMetadata};
use super::tool_types::{OperatingSystem, RunnerType};
use super::{
    deno_tools::DenoTool, mcp_server_tool::MCPServerTool, network_tool::NetworkTool, parameters::Parameters,
    python_tools::PythonTool, tool_config::ToolConfig, tool_output_arg::ToolOutputArg,
};

pub type IsEnabled = bool;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum HanzoTool {
    Rust(RustTool, IsEnabled),
    Network(NetworkTool, IsEnabled),
    Deno(DenoTool, IsEnabled),
    Python(PythonTool, IsEnabled),
    Agent(AgentToolWrapper, IsEnabled),
    MCPServer(MCPServerTool, IsEnabled),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Assets {
    pub file_name: String,
    pub data: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HanzoToolWithAssets {
    pub tool: HanzoTool,
    pub assets: Option<Vec<Assets>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HanzoToolHeader {
    pub name: String,
    pub description: String,
    pub tool_router_key: String,
    pub tool_type: String,
    pub formatted_tool_summary_for_ui: String,
    pub author: String,
    pub version: String,
    pub enabled: bool,
    pub mcp_enabled: Option<bool>,
    pub input_args: Parameters,
    pub output_arg: ToolOutputArg,
    pub config: Option<Vec<ToolConfig>>,
    pub usage_type: Option<UsageType>, // includes pricing
    // Note: do we need usage_type? it's already contained in the tool_offering
    pub tool_offering: Option<HanzoToolOffering>,
}

impl HanzoToolHeader {
    /// Sanitize the config by removing key-values from BasicConfig
    pub fn sanitize_config(&mut self) {
        if let Some(configs) = &self.config {
            self.config = Some(configs.iter().map(|config| config.sanitize()).collect());
        }
    }
}

impl HanzoTool {
    /// Generate a HanzoToolHeader from a HanzoTool
    pub fn to_header(&self) -> HanzoToolHeader {
        HanzoToolHeader {
            name: self.name(),
            description: self.description(),
            tool_router_key: self.tool_router_key().to_string_without_version(),
            tool_type: self.tool_type().to_string(),
            formatted_tool_summary_for_ui: self.formatted_tool_summary_for_ui(),
            author: self.author(),
            version: self.version(),
            enabled: self.is_enabled(),
            mcp_enabled: Some(self.is_mcp_enabled()),
            input_args: self.input_args(),
            output_arg: self.output_arg(),
            config: self.get_js_tool_config().cloned(),
            usage_type: self.get_usage_type(),
            tool_offering: None,
        }
    }

    /// The key that this tool will be stored under in the tool router
    pub fn tool_router_key(&self) -> ToolRouterKey {
        match self {
            HanzoTool::Rust(r, _) => ToolRouterKey::new("local".to_string(), r.author(), r.name.clone(), None),
            HanzoTool::Network(n, _) => ToolRouterKey::from_string(&n.tool_router_key).unwrap_or_else(|_| {
                ToolRouterKey::new(n.provider.to_string(), n.author.to_string(), n.name.clone(), None)
            }),
            HanzoTool::Deno(d, _) => {
                if let Some(key) = &d.tool_router_key {
                    key.clone()
                } else {
                    ToolRouterKey::new("local".to_string(), d.author.clone(), d.name.clone(), None)
                }
            }
            HanzoTool::Python(p, _) => {
                if let Some(key) = &p.tool_router_key {
                    key.clone()
                } else {
                    ToolRouterKey::new("local".to_string(), p.author.clone(), p.name.clone(), None)
                }
            }
            HanzoTool::Agent(a, _) => {
                ToolRouterKey::new("local".to_string(), a.author.clone(), a.agent_id.clone(), None)
            }
            HanzoTool::MCPServer(m, _) => {
                MCPServerTool::create_tool_router_key(m.mcp_server_command_hash.clone(), m.mcp_server_tool.clone())
            }
        }
    }

    /// Sanitize the config by removing key-values from BasicConfig
    pub fn sanitize_config(&mut self) {
        match self {
            HanzoTool::Deno(d, _) => {
                d.config = d.config.clone().iter().map(|config| config.sanitize()).collect();
            }
            HanzoTool::Python(p, _) => {
                p.config = p.config.clone().iter().map(|config| config.sanitize()).collect();
            }
            HanzoTool::MCPServer(m, _) => {
                m.config = m.config.clone().iter().map(|config| config.sanitize()).collect();
            }
            _ => (),
        }
    }

    /// Tool name
    pub fn name(&self) -> String {
        match self {
            HanzoTool::Rust(r, _) => r.name.clone(),
            HanzoTool::Network(n, _) => n.name.clone(),
            HanzoTool::Deno(d, _) => d.name.clone(),
            HanzoTool::Python(p, _) => p.name.clone(),
            HanzoTool::Agent(a, _) => a.name.clone(),
            HanzoTool::MCPServer(m, _) => m.name.clone(),
        }
    }
    /// Tool description
    pub fn description(&self) -> String {
        match self {
            HanzoTool::Rust(r, _) => r.description.clone(),
            HanzoTool::Network(n, _) => n.description.clone(),
            HanzoTool::Deno(d, _) => d.description.clone(),
            HanzoTool::Python(p, _) => p.description.clone(),
            HanzoTool::Agent(a, _) => a.description.clone(),
            HanzoTool::MCPServer(m, _) => m.description.clone(),
        }
    }

    /// Returns the input arguments of the tool
    pub fn input_args(&self) -> Parameters {
        match self {
            HanzoTool::Rust(r, _) => r.input_args.clone(),
            HanzoTool::Network(n, _) => n.input_args.clone(),
            HanzoTool::Deno(d, _) => d.input_args.clone(),
            HanzoTool::Python(p, _) => p.input_args.clone(),
            HanzoTool::Agent(a, _) => a.input_args.clone(),
            HanzoTool::MCPServer(m, _) => m.input_args.clone(),
        }
    }

    /// Returns the input arguments of the tool
    pub fn output_arg(&self) -> ToolOutputArg {
        match self {
            HanzoTool::Rust(r, _) => r.output_arg.clone(),
            HanzoTool::Network(n, _) => n.output_arg.clone(),
            HanzoTool::Deno(d, _) => d.output_arg.clone(),
            HanzoTool::Python(p, _) => p.output_arg.clone(),
            HanzoTool::Agent(a, _) => a.output_arg.clone(),
            HanzoTool::MCPServer(m, _) => m.output_arg.clone(),
        }
    }

    /// Returns the output arguments of the tool
    pub fn tool_type(&self) -> &'static str {
        match self {
            HanzoTool::Rust(_, _) => "Rust",
            HanzoTool::Network(_, _) => "Network",
            HanzoTool::Deno(_, _) => "Deno",
            HanzoTool::Python(_, _) => "Python",
            HanzoTool::Agent(_, _) => "Agent",
            HanzoTool::MCPServer(_, _) => "MCPServer",
        }
    }

    /// Returns the SQL queries of the tool
    pub fn sql_queries(&self) -> Vec<SqlQuery> {
        match self {
            HanzoTool::Deno(d, _) => d.sql_queries.clone().unwrap_or_default(),
            HanzoTool::Python(p, _) => p.sql_queries.clone().unwrap_or_default(),
            _ => vec![],
        }
    }

    /// Returns the SQL tables of the tool
    pub fn sql_tables(&self) -> Vec<SqlTable> {
        match self {
            HanzoTool::Deno(d, _) => d.sql_tables.clone().unwrap_or_default(),
            HanzoTool::Python(p, _) => p.sql_tables.clone().unwrap_or_default(),
            _ => vec![],
        }
    }

    pub fn get_oauth(&self) -> Option<Vec<OAuth>> {
        match self {
            HanzoTool::Deno(d, _) => d.oauth.clone(),
            HanzoTool::Python(p, _) => p.oauth.clone(),
            _ => None,
        }
    }

    pub fn get_tools(&self) -> Vec<ToolRouterKey> {
        match self {
            HanzoTool::Deno(d, _) => d.tools.clone(),
            HanzoTool::Python(p, _) => p.tools.clone(),
            _ => vec![],
        }
    }

    pub fn get_assets(&self) -> Option<Vec<String>> {
        match self {
            HanzoTool::Deno(d, _) => d.assets.clone(),
            HanzoTool::Python(p, _) => p.assets.clone(),
            _ => None,
        }
    }

    pub fn get_homepage(&self) -> Option<String> {
        match self {
            HanzoTool::Deno(d, _) => d.homepage.clone(),
            HanzoTool::Python(p, _) => p.homepage.clone(),
            _ => None,
        }
    }

    /// Returns a formatted summary of the tool
    pub fn formatted_tool_summary_for_ui(&self) -> String {
        format!(
            "Tool Name: {}\nAuthor: {}\nDescription: {}",
            self.name(),
            self.author(),
            self.description(),
        )
    }

    pub fn get_code(&self) -> String {
        match self {
            HanzoTool::Deno(d, _) => d.js_code.clone(),
            HanzoTool::Python(p, _) => p.py_code.clone(),
            _ => unreachable!(),
        }
    }

    pub fn update_name(&mut self, name: String) {
        match self {
            HanzoTool::Deno(d, _) => d.name = name,
            HanzoTool::Python(p, _) => p.name = name,
            HanzoTool::MCPServer(m, _) => m.name = name,
            _ => unreachable!(),
        }
    }

    pub fn update_author(&mut self, author: String) {
        match self {
            HanzoTool::Deno(d, _) => d.author = author,
            HanzoTool::Python(p, _) => p.author = author,
            HanzoTool::MCPServer(m, _) => m.author = author,
            _ => unreachable!(),
        }
    }

    pub fn get_runner(&self) -> RunnerType {
        match self {
            HanzoTool::Deno(d, _) => d.runner.clone(),
            HanzoTool::Python(p, _) => p.runner.clone(),
            _ => RunnerType::Any,
        }
    }

    pub fn get_operating_system(&self) -> Vec<OperatingSystem> {
        match self {
            HanzoTool::Deno(d, _) => d.operating_system.clone(),
            HanzoTool::Python(p, _) => p.operating_system.clone(),
            _ => vec![OperatingSystem::Linux, OperatingSystem::MacOS, OperatingSystem::Windows],
        }
    }

    pub fn get_tool_set(&self) -> Option<String> {
        match self {
            HanzoTool::Deno(d, _) => d.tool_set.clone(),
            HanzoTool::Python(p, _) => p.tool_set.clone(),
            HanzoTool::MCPServer(m, _) => m.tool_set.clone(),
            _ => None,
        }
    }

    /// Sets the embedding for the tool
    pub fn set_embedding(&mut self, embedding: Vec<f32>) {
        match self {
            HanzoTool::Rust(r, _) => r.tool_embedding = Some(embedding),
            HanzoTool::Network(n, _) => n.embedding = Some(embedding),
            HanzoTool::Deno(d, _) => d.embedding = Some(embedding),
            HanzoTool::Python(p, _) => p.embedding = Some(embedding),
            HanzoTool::Agent(a, _) => a.embedding = Some(embedding),
            HanzoTool::MCPServer(m, _) => m.embedding = Some(embedding),
        }
    }

    /// Returns the tool formatted as a JSON object for the function call format
    pub fn json_function_call_format(&self) -> Result<serde_json::Value, ToolError> {
        // Get the ToolRouterKey instance
        let tool_router_key = self.tool_router_key();

        // Extract the tool name directly from the ToolRouterKey
        let tool_name = ToolRouterKey::sanitize(&tool_router_key.name);

        let summary = serde_json::json!({
            "type": "function",
            "function": {
                "name": tool_name,
                "description": self.description(),
                "tool_router_key": tool_router_key.to_string_without_version(),
                "parameters": self.input_args()
            },
        });

        Ok(summary)
    }

    pub fn json_string_function_call_format(&self) -> Result<String, ToolError> {
        let summary_value = self.json_function_call_format()?;
        serde_json::to_string(&summary_value).map_err(|_| ToolError::FailedJSONParsing)
    }

    /// Formats the tool's info into a String to be used for generating the tool's embedding.
    pub fn format_embedding_string(&self) -> String {
        let formatted_name = self.name().replace("hanzo__", "").replace('_', " ");
        format!("{} {}", formatted_name, self.description())
    }

    /// Returns the embedding if it exists
    pub fn get_embedding(&self) -> Option<Vec<f32>> {
        match self {
            HanzoTool::Rust(r, _) => r.tool_embedding.clone(),
            HanzoTool::Network(n, _) => n.embedding.clone(),
            HanzoTool::Deno(d, _) => d.embedding.clone(),
            HanzoTool::Python(p, _) => p.embedding.clone(),
            HanzoTool::Agent(a, _) => a.embedding.clone(),
            HanzoTool::MCPServer(m, _) => m.embedding.clone(),
        }
    }

    /// Returns an Option<ToolConfig> based on an environment variable
    pub fn get_config_from_env(&self) -> Option<ToolConfig> {
        // Get the ToolRouterKey instance and convert it to a string
        let tool_key = self.tool_router_key().to_string_without_version().replace(":::", "___");
        let env_var_key = format!("TOOLKIT_{}", tool_key);

        if let Ok(env_value) = env::var(env_var_key) {
            // Attempt to parse the environment variable as JSON
            if let Ok(value) = serde_json::from_str::<Value>(&env_value) {
                // Attempt to deserialize the JSON value into a ToolConfig
                return ToolConfig::from_value(&value);
            }
        }

        None
    }

    /// Returns the author of the tool
    pub fn author(&self) -> String {
        match self {
            HanzoTool::Rust(r, _) => r.author(),
            HanzoTool::Network(n, _) => n.author.clone(),
            HanzoTool::Deno(d, _) => d.author.clone(),
            HanzoTool::Python(p, _) => p.author.clone(),
            HanzoTool::Agent(a, _) => a.author.clone(),
            HanzoTool::MCPServer(m, _) => m.author.clone(),
        }
    }

    /// Returns the version of the tool
    pub fn version(&self) -> String {
        match self {
            HanzoTool::Rust(r, _) => r.version.clone(),
            HanzoTool::Network(n, _) => n.version.clone(),
            HanzoTool::Deno(d, _) => d.version.clone(),
            HanzoTool::Python(p, _) => p.version.clone(),
            HanzoTool::Agent(_a, _) => "1.0.0".to_string(),
            HanzoTool::MCPServer(m, _) => m.version.clone(),
        }
    }

    /// Get the usage type, only valid for NetworkTool
    pub fn get_usage_type(&self) -> Option<UsageType> {
        if let HanzoTool::Network(n, _) = self {
            Some(n.usage_type.clone())
        } else {
            None
        }
    }

    /// Check if the tool is enabled
    pub fn is_enabled(&self) -> bool {
        match self {
            HanzoTool::Rust(_, enabled) => *enabled,
            HanzoTool::Network(_, enabled) => *enabled,
            HanzoTool::Deno(_, enabled) => *enabled,
            HanzoTool::Python(_, enabled) => *enabled,
            HanzoTool::Agent(_a, enabled) => *enabled,
            HanzoTool::MCPServer(_, enabled) => *enabled,
        }
    }

    /// Check if the tool is enabled for MCP
    pub fn is_mcp_enabled(&self) -> bool {
        match self {
            HanzoTool::Rust(tool, is_enabled) => *is_enabled && tool.mcp_enabled.unwrap_or(false),
            HanzoTool::Network(tool, is_enabled) => *is_enabled && tool.mcp_enabled.unwrap_or(false),
            HanzoTool::Deno(tool, is_enabled) => *is_enabled && tool.mcp_enabled.unwrap_or(false),
            HanzoTool::Python(tool, is_enabled) => *is_enabled && tool.mcp_enabled.unwrap_or(false),
            HanzoTool::Agent(a, is_enabled) => *is_enabled && a.mcp_enabled.unwrap_or(false),
            HanzoTool::MCPServer(a, is_enabled) => *is_enabled && a.mcp_enabled.unwrap_or(false),
        }
    }

    /// Enable the tool
    pub fn enable(&mut self) {
        match self {
            HanzoTool::Rust(_, enabled) => *enabled = true,
            HanzoTool::Network(_, enabled) => *enabled = true,
            HanzoTool::Deno(_, enabled) => *enabled = true,
            HanzoTool::Python(_, enabled) => *enabled = true,
            HanzoTool::Agent(_, enabled) => *enabled = true,
            HanzoTool::MCPServer(_, enabled) => *enabled = true,
        }
    }

    pub fn enable_mcp(&mut self) {
        match self {
            HanzoTool::Rust(tool, _) => tool.mcp_enabled = Some(true),
            HanzoTool::Network(tool, _) => tool.mcp_enabled = Some(true),
            HanzoTool::Deno(tool, _) => tool.mcp_enabled = Some(true),
            HanzoTool::Python(tool, _) => tool.mcp_enabled = Some(true),
            HanzoTool::Agent(tool, _) => tool.mcp_enabled = Some(true),
            HanzoTool::MCPServer(tool, _) => tool.mcp_enabled = Some(true),
        }
    }

    /// Disable the tool
    pub fn disable(&mut self) {
        match self {
            HanzoTool::Rust(_, enabled) => *enabled = false,
            HanzoTool::Network(_, enabled) => *enabled = false,
            HanzoTool::Deno(_, enabled) => *enabled = false,
            HanzoTool::Python(_, enabled) => *enabled = false,
            HanzoTool::Agent(_, enabled) => *enabled = false,
            HanzoTool::MCPServer(_, enabled) => *enabled = false,
        }
    }

    pub fn disable_mcp(&mut self) {
        match self {
            HanzoTool::Rust(tool, _) => tool.mcp_enabled = Some(false),
            HanzoTool::Network(tool, _) => tool.mcp_enabled = Some(false),
            HanzoTool::Deno(tool, _) => tool.mcp_enabled = Some(false),
            HanzoTool::Python(tool, _) => tool.mcp_enabled = Some(false),
            HanzoTool::Agent(tool, _) => tool.mcp_enabled = Some(false),
            HanzoTool::MCPServer(tool, _) => tool.mcp_enabled = Some(false),
        }
    }

    /// Get the config from a JSTool, return None if it's another type
    pub fn get_js_tool_config(&self) -> Option<&Vec<ToolConfig>> {
        if let HanzoTool::Deno(js_tool, _) = self {
            Some(&js_tool.config)
        } else {
            None
        }
    }

    pub fn get_config(&self) -> Vec<ToolConfig> {
        match self {
            HanzoTool::Rust(_, _) => vec![],
            HanzoTool::Network(_, _) => vec![],
            HanzoTool::Deno(js_tool, _) => js_tool.config.clone(),
            HanzoTool::Python(python_tool, _) => python_tool.config.clone(),
            HanzoTool::Agent(_a, _) => vec![],
            HanzoTool::MCPServer(mcp_tool, _) => mcp_tool.config.clone(),
        }
    }

    /// Check if the tool can be enabled
    pub fn can_be_enabled(&self) -> bool {
        match self {
            HanzoTool::Rust(_, _) => true,
            HanzoTool::Network(n_tool, _) => n_tool.check_required_config_fields(),
            HanzoTool::Deno(deno_tool, _) => deno_tool.check_required_config_fields(),
            HanzoTool::Python(_, _) => true,
            HanzoTool::Agent(_, _) => true,
            HanzoTool::MCPServer(mcp_tool, _) => mcp_tool.check_required_config_fields(),
        }
    }

    pub fn can_be_mcp_enabled(&self) -> bool {
        if !self.is_enabled() || self.is_mcp_enabled() {
            return false;
        }
        true
    }

    /// Convert to json
    pub fn to_json(&self) -> Result<String, ToolError> {
        serde_json::to_string(self).map_err(|_| ToolError::FailedJSONParsing)
    }

    /// Convert from json
    pub fn from_json(json: &str) -> Result<Self, ToolError> {
        let deserialized: Self = serde_json::from_str(json).map_err(|e| ToolError::ParseError(e.to_string()))?;
        Ok(deserialized)
    }

    /// Check if the tool is Rust-based
    pub fn is_rust_based(&self) -> bool {
        matches!(self, HanzoTool::Rust(_, _))
    }

    /// Check if the tool is JS-based
    pub fn is_js_based(&self) -> bool {
        matches!(self, HanzoTool::Deno(_, _))
    }

    /// Check if the tool is Workflow-based
    pub fn is_network_based(&self) -> bool {
        matches!(self, HanzoTool::Network(_, _))
    }

    pub fn version_indexable(&self) -> Result<IndexableVersion, String> {
        IndexableVersion::from_string(&self.version())
    }

    /// Returns the version number using IndexableVersion
    pub fn version_number(&self) -> Result<u64, String> {
        let indexable_version = self.version_indexable()?;
        Ok(indexable_version.get_version_number())
    }

    /// Returns a sanitized version of the tool name where all characters are lowercase
    /// and any non-alphanumeric characters (except '-' and '_') are replaced with underscores
    pub fn internal_sanitized_name(&self) -> String {
        let name_to_sanitize = match self {
            HanzoTool::Agent(agent, _) => agent.agent_id.clone(),
            _ => self.name(),
        };

        name_to_sanitize
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    }

    pub fn get_keywords(&self) -> Vec<String> {
        match self {
            HanzoTool::Rust(_, _) => vec![],
            HanzoTool::Network(_, _) => vec![],
            HanzoTool::Deno(d, _) => d.keywords.clone(),
            HanzoTool::Python(p, _) => p.keywords.clone(),
            HanzoTool::Agent(_a, _) => vec![],
            HanzoTool::MCPServer(m, _) => m.keywords.clone(),
        }
    }

    pub fn get_metadata(&self) -> Option<ToolPlaygroundMetadata> {
        match self {
            HanzoTool::Deno(d, _) => Some(d.get_metadata()),
            HanzoTool::Python(p, _) => Some(p.get_metadata()),
            HanzoTool::Rust(r, _) => Some(r.get_metadata()),
            HanzoTool::MCPServer(r, _) => Some(r.get_metadata()),
            _ => None,
        }
    }
}

impl From<RustTool> for HanzoTool {
    fn from(tool: RustTool) -> Self {
        HanzoTool::Rust(tool, true)
    }
}

impl From<DenoTool> for HanzoTool {
    fn from(tool: DenoTool) -> Self {
        HanzoTool::Deno(tool, true)
    }
}

impl From<NetworkTool> for HanzoTool {
    fn from(tool: NetworkTool) -> Self {
        HanzoTool::Network(tool, true)
    }
}

impl From<MCPServerTool> for HanzoTool {
    fn from(tool: MCPServerTool) -> Self {
        HanzoTool::MCPServer(tool, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::deno_tools::DenoTool;
    use crate::tools::parameters::Property;
    use crate::tools::tool_types::{OperatingSystem, RunnerType, ToolResult};
    use serde_json::json;
    use hanzo_message_primitives::schemas::tool_router_key::ToolRouterKey;
    use shinkai_tools_runner::tools::tool_definition::ToolDefinition;

    #[test]
    fn test_gen_router_key() {
        // Create a mock DenoTool with all required fields
        let tool_router_key = ToolRouterKey::new(
            "local".to_string(),
            "@@official.hanzo".to_string(),
            "Hanzo: Download Pages".to_string(),
            None,
        );

        let deno_tool = DenoTool {
            name: "Hanzo: Download Pages".to_string(),
            tool_router_key: Some(tool_router_key.clone()),
            homepage: Some("http://127.0.0.1/index.html".to_string()),
            description: "Downloads one or more URLs and converts their HTML content to Markdown".to_string(),
            mcp_enabled: Some(false),
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            config: vec![],
            author: "@@official.hanzo".to_string(),
            version: "1.0.0".to_string(),
            js_code: "".to_string(),
            tools: vec![],
            keywords: vec![],
            activated: false,
            embedding: None,
            result: ToolResult::new(
                "object".to_string(),
                json!({
                    "markdowns": { "type": "array", "items": { "type": "string" } }
                }),
                vec!["markdowns".to_string()],
            ),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::OnlyHost,
            operating_system: vec![OperatingSystem::Linux],
            tool_set: None,
        };

        // Create a HanzoTool instance
        let hanzo_tool = HanzoTool::Deno(deno_tool, false);

        // Generate the router key
        let router_key = hanzo_tool.tool_router_key();

        // Expected pattern: [^a-z0-9_]+ (plus the :::)
        let expected_key = "local:::__official_hanzo:::hanzo__download_pages";

        // Assert that the generated key matches the expected pattern
        assert_eq!(router_key.to_string_without_version(), expected_key);
    }

    #[test]
    fn test_set_playground_tool() {
        let tool_definition = ToolDefinition {
            id: "hanzo-tool-download-website".to_string(),
            name: "Download Website".to_string(),
            description: "Downloads a website and converts its content into Markdown.".to_string(),
            configurations: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
            }),
            result: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            author: "@@my_local_ai.sep-hanzo".to_string(),
            keywords: vec![
                "Deno".to_string(),
                "Markdown".to_string(),
                "HTML to Markdown".to_string(),
            ],
            code: Some("import { getHomePath } from './hanzo-local-support.ts';\n\n...".to_string()), /* Truncated for brevity */
            embedding_metadata: None,
        };

        let input_args = Parameters::with_single_property(
            "url",
            "string",
            "The URL to fetch",
            true,
            Some(serde_json::Value::String("https://example.com".to_string())),
        );

        let tool_router_key = ToolRouterKey::new(
            "local".to_string(),
            tool_definition.author.clone(),
            "hanzo__download_website".to_string(),
            None,
        );

        let deno_tool = DenoTool {
            name: "hanzo__download_website".to_string(),
            tool_router_key: Some(tool_router_key.clone()),
            homepage: Some("http://127.0.0.1/index.html".to_string()),
            version: "1.0.0".to_string(),
            mcp_enabled: Some(false),
            description: tool_definition.description.clone(),
            input_args: input_args.clone(),
            output_arg: ToolOutputArg {
                json: tool_definition.result.to_string(),
            },
            config: vec![],
            author: tool_definition.author.clone(),
            js_code: tool_definition.code.clone().unwrap_or_default(),
            tools: vec![],
            keywords: tool_definition.keywords.clone(),
            activated: false,
            embedding: None,
            result: ToolResult::new(
                "object".to_string(),
                tool_definition.result["properties"].clone(),
                vec![],
            ),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::OnlyHost,
            operating_system: vec![OperatingSystem::Windows],
            tool_set: None,
        };

        let hanzo_tool = HanzoTool::Deno(deno_tool, true);
        eprintln!("hanzo_tool: {:?}", hanzo_tool);

        eprintln!("hanzo params: {:?}", hanzo_tool.input_args());

        assert_eq!(hanzo_tool.name(), "hanzo__download_website");
        assert_eq!(
            hanzo_tool.description(),
            "Downloads a website and converts its content into Markdown."
        );
        assert_eq!(hanzo_tool.tool_type(), "Deno");
        assert!(hanzo_tool.is_enabled());
    }

    #[test]
    fn test_deserialize_hanzo_tool() {
        let json_payload = r#"
        {
            "type": "Deno",
            "content": [
                {
                    "description": "Tool for getting the default address of a Coinbase wallet",
                    "version": "1.0.0",
                    "activated": false,
                    "assets": null,
                    "author": "Hanzo",
                    "file_inbox": null,
                    "toolkit_name": "hanzo-tool-coinbase-get-my-address",
                    "sql_tables": [],
                    "sql_queries": [],
                    "embedding": [],
                    "oauth": null,
                    "config": [],
                    "keywords": [
                        "coinbase",
                        "address",
                        "hanzo"
                    ],
                    "tools": [],
                    "result": {
                        "type": "object",
                        "properties": {
                            "address": {
                                "type": "string",
                                "description": "hey"
                            }
                        },
                        "required": [
                            "address"
                        ]
                    },
                    "input_args": {
                        "type": "object",
                        "properties": {
                            "walletId": {
                                "type": "string",
                                "nullable": true,
                                "description": "The ID of the wallet to get the address for"
                            }
                        },
                        "required": []
                    },
                    "output_arg": {
                        "json": ""
                    },
                    "name": "Hanzo: Coinbase My Address Getter",
                    "js_code": "import { Coinbase, CoinbaseOptions } from 'npm:@coinbase/coinbase-sdk@0.0.16';\\n\\ntype Configurations = {\\n name: string;\\n privateKey: string;\\n walletId?: string;\\n useServerSigner?: string;\\n};\\ntype Parameters = {\\n walletId?: string;\\n};\\ntype Result = {\\n address: string;\\n};\\nexport type Run<C extends Record<string, any>, I extends Record<string, any>, R extends Record<string, any>> = (config: C, inputs: I) => Promise<R>;\\n\\nexport const run: Run<Configurations, Parameters, Result> = async (\\n configurations: Configurations,\\n params: Parameters,\\n): Promise<Result> => {\\n const coinbaseOptions: CoinbaseOptions = {\\n apiKeyName: configurations.name,\\n privateKey: configurations.privateKey,\\n useServerSigner: configurations.useServerSigner === 'true',\\n };\\n const coinbase = new Coinbase(coinbaseOptions);\\n const user = await coinbase.getDefaultUser();\\n\\n // Prioritize walletId from Params over Config\\n const walletId = params.walletId || configurations.walletId;\\n\\n // Throw an error if walletId is not defined\\n if (!walletId) {\\n throw new Error('walletId must be defined in either params or config');\\n }\\n\\n const wallet = await user.getWallet(walletId);\\n console.log(`Wallet retrieved: `, wallet.toString());\\n\\n // Retrieve the list of balances for the wallet\\n const address = await wallet.getDefaultAddress();\\n console.log(`Default Address: `, address);\\n\\n return {\\n address: address?.getId() || '',\\n };\\n};",
                    "homepage": null,
                    "runner": "any",
                    "operating_system": ["linux"],
                    "tool_set": null
                },
                false
            ]
        }
        "#;

        let deserialized_tool: Result<HanzoTool, _> = serde_json::from_str(json_payload);
        eprintln!("deserialized_tool: {:?}", deserialized_tool);

        assert!(deserialized_tool.is_ok(), "Failed to deserialize HanzoTool");

        if let Ok(HanzoTool::Deno(deno_tool, _)) = deserialized_tool {
            assert_eq!(deno_tool.name, "Hanzo: Coinbase My Address Getter");
            assert_eq!(deno_tool.author, "Hanzo");
            assert_eq!(deno_tool.version, "1.0.0");
            assert_eq!(deno_tool.runner, RunnerType::Any);
            assert_eq!(deno_tool.operating_system, vec![OperatingSystem::Linux]);
        } else {
            panic!("Expected Deno tool variant");
        }
    }

    #[test]
    fn test_serialize_deserialize_agent_tool() {
        // Create an AgentToolWrapper instance
        let agent_wrapper = AgentToolWrapper {
            name: "new pirate".to_string(),
            agent_id: "new_pirate".to_string(),
            author: "@@my_local_ai.sep-hanzo".to_string(),
            description: "".to_string(),
            input_args: Parameters {
                schema_type: "object".to_string(),
                properties: {
                    let mut props = std::collections::HashMap::new();
                    props.insert(
                        "prompt".to_string(),
                        Property::new("string".to_string(), "Message to the agent".to_string(), None),
                    );
                    props.insert(
                        "session_id".to_string(),
                        Property::new("string".to_string(), "Session identifier".to_string(), None),
                    );

                    let item_prop = Property::new("string".to_string(), "Image URL".to_string(), None);
                    props.insert(
                        "images".to_string(),
                        Property::with_array_items("Array of image URLs".to_string(), item_prop),
                    );
                    props
                },
                required: vec!["prompt".to_string()],
            },
            output_arg: ToolOutputArg {
                json: "{\"type\":\"string\",\"description\":\"Agent response\"}".to_string(),
            },
            mcp_enabled: Some(false),
            embedding: None,
        };

        // Create a HanzoTool::Agent instance
        let original_tool = HanzoTool::Agent(agent_wrapper, true);

        // Serialize to JSON
        let serialized = serde_json::to_string(&original_tool).expect("Failed to serialize Agent tool");

        // Deserialize from JSON
        let deserialized: HanzoTool = serde_json::from_str(&serialized).expect("Failed to deserialize Agent tool");

        // Verify the tool was properly deserialized
        match deserialized {
            HanzoTool::Agent(agent, enabled) => {
                assert_eq!(agent.name, "new pirate");
                assert_eq!(agent.agent_id, "new_pirate");
                assert_eq!(agent.author, "@@my_local_ai.sep-hanzo");
                assert_eq!(agent.description, "");
                assert!(enabled);

                // Verify the input_args structure
                assert_eq!(agent.input_args.schema_type, "object");
                assert!(agent.input_args.properties.contains_key("prompt"));
                assert!(agent.input_args.properties.contains_key("session_id"));
                assert!(agent.input_args.properties.contains_key("images"));

                // Verify required fields
                assert_eq!(agent.input_args.required, vec!["prompt".to_string()]);

                // Verify output_arg
                assert_eq!(
                    agent.output_arg.json,
                    "{\"type\":\"string\",\"description\":\"Agent response\"}"
                );

                // Verify mcp_enabled
                assert_eq!(agent.mcp_enabled, Some(false));
            }
            _ => panic!("Deserialized tool is not an Agent variant"),
        }
    }
}
