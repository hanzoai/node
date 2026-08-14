use super::parameters::Parameters;
use super::tool_config::{OAuth, ToolConfig};
use super::tool_output_arg::ToolOutputArg;
use super::tool_playground::ToolPlaygroundMetadata;
use super::tool_playground::{SqlQuery, SqlTable};
use super::tool_types::{OperatingSystem, RunnerType, ToolResult};
use crate::tools::error::ToolError;
use hanzo_messages::schemas::tool_router_key::ToolRouterKey;

#[derive(Debug, Clone, PartialEq)]
pub struct DenoTool {
    pub name: String,
    pub tool_router_key: Option<ToolRouterKey>,
    pub homepage: Option<String>,
    pub author: String,
    pub version: String,
    pub mcp_enabled: Option<bool>,
    pub js_code: String,
    pub tools: Vec<ToolRouterKey>,
    pub config: Vec<ToolConfig>,
    pub description: String,
    pub keywords: Vec<String>,
    pub input_args: Parameters,
    pub output_arg: ToolOutputArg,
    pub activated: bool,
    pub embedding: Option<Vec<f32>>,
    pub result: ToolResult,
    pub sql_tables: Option<Vec<SqlTable>>,
    pub sql_queries: Option<Vec<SqlQuery>>,
    pub file_inbox: Option<String>,
    pub oauth: Option<Vec<OAuth>>,
    pub assets: Option<Vec<String>>,
    pub runner: RunnerType,
    pub operating_system: Vec<OperatingSystem>,
    pub tool_set: Option<String>,
}

impl<'de> serde::Deserialize<'de> for DenoTool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Helper {
            name: String,
            #[serde(default)]
            tool_router_key: Option<String>,
            homepage: Option<String>,
            author: String,
            version: String,
            mcp_enabled: Option<bool>,
            js_code: String,
            #[serde(default)]
            #[serde(deserialize_with = "ToolRouterKey::deserialize_tool_router_keys")]
            #[serde(serialize_with = "ToolRouterKey::serialize_tool_router_keys")]
            tools: Vec<ToolRouterKey>,
            config: Vec<ToolConfig>,
            description: String,
            keywords: Vec<String>,
            input_args: Parameters,
            output_arg: ToolOutputArg,
            activated: bool,
            embedding: Option<Vec<f32>>,
            result: ToolResult,
            sql_tables: Option<Vec<SqlTable>>,
            sql_queries: Option<Vec<SqlQuery>>,
            file_inbox: Option<String>,
            oauth: Option<Vec<OAuth>>,
            assets: Option<Vec<String>>,
            runner: RunnerType,
            operating_system: Vec<OperatingSystem>,
            tool_set: Option<String>,
        }

        let helper = Helper::deserialize(deserializer)?;

        let tool_router_key = match helper.tool_router_key {
            Some(key_str) => Some(ToolRouterKey::from_string(&key_str).map_err(serde::de::Error::custom)?),
            None => Some(ToolRouterKey::new(
                "local".to_string(),
                helper.author.clone(),
                helper.name.clone(),
                None,
            )),
        };

        Ok(DenoTool {
            name: helper.name,
            tool_router_key,
            homepage: helper.homepage,
            author: helper.author,
            version: helper.version,
            mcp_enabled: helper.mcp_enabled,
            js_code: helper.js_code,
            tools: helper.tools,
            config: helper.config,
            description: helper.description,
            keywords: helper.keywords,
            input_args: helper.input_args,
            output_arg: helper.output_arg,
            activated: helper.activated,
            embedding: helper.embedding,
            result: helper.result,
            sql_tables: helper.sql_tables,
            sql_queries: helper.sql_queries,
            file_inbox: helper.file_inbox,
            oauth: helper.oauth,
            assets: helper.assets,
            runner: helper.runner,
            operating_system: helper.operating_system,
            tool_set: helper.tool_set,
        })
    }
}

impl serde::Serialize for DenoTool {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("DenoTool", 24)?;
        state.serialize_field("name", &self.name)?;
        if let Some(key) = &self.tool_router_key {
            state.serialize_field("tool_router_key", &key.to_string_with_version())?;
        } else {
            state.serialize_field("tool_router_key", &None::<String>)?;
        }
        state.serialize_field("homepage", &self.homepage)?;
        state.serialize_field("author", &self.author)?;
        state.serialize_field("version", &self.version)?;
        state.serialize_field("mcp_enabled", &self.mcp_enabled)?;
        state.serialize_field("js_code", &self.js_code)?;
        let tools_strings: Vec<String> = self.tools.iter().map(|k| k.to_string_with_version()).collect();
        state.serialize_field("tools", &tools_strings)?;
        state.serialize_field("config", &self.config)?;
        state.serialize_field("description", &self.description)?;
        state.serialize_field("keywords", &self.keywords)?;
        state.serialize_field("input_args", &self.input_args)?;
        state.serialize_field("output_arg", &self.output_arg)?;
        state.serialize_field("activated", &self.activated)?;
        state.serialize_field("embedding", &self.embedding)?;
        state.serialize_field("result", &self.result)?;
        state.serialize_field("sql_tables", &self.sql_tables)?;
        state.serialize_field("sql_queries", &self.sql_queries)?;
        state.serialize_field("file_inbox", &self.file_inbox)?;
        state.serialize_field("oauth", &self.oauth)?;
        state.serialize_field("assets", &self.assets)?;
        state.serialize_field("runner", &self.runner)?;
        state.serialize_field("operating_system", &self.operating_system)?;
        state.serialize_field("tool_set", &self.tool_set)?;
        state.end()
    }
}

impl DenoTool {
    pub fn new(
        name: String,
        homepage: Option<String>,
        author: String,
        version: String,
        mcp_enabled: Option<bool>,
        js_code: String,
        tools: Vec<ToolRouterKey>,
        config: Vec<ToolConfig>,
        description: String,
        keywords: Vec<String>,
        input_args: Parameters,
        output_arg: ToolOutputArg,
        activated: bool,
        embedding: Option<Vec<f32>>,
        result: ToolResult,
        sql_tables: Option<Vec<SqlTable>>,
        sql_queries: Option<Vec<SqlQuery>>,
        file_inbox: Option<String>,
        oauth: Option<Vec<OAuth>>,
        assets: Option<Vec<String>>,
        runner: RunnerType,
        operating_system: Vec<OperatingSystem>,
        tool_set: Option<String>,
    ) -> Self {
        let tool_router_key = ToolRouterKey::new("local".to_string(), author.clone(), name.clone(), None);

        DenoTool {
            name,
            tool_router_key: Some(tool_router_key),
            homepage,
            author,
            version,
            mcp_enabled,
            js_code,
            tools,
            config,
            description,
            keywords,
            input_args,
            output_arg,
            activated,
            embedding,
            result,
            sql_tables,
            sql_queries,
            file_inbox,
            oauth,
            assets,
            runner,
            operating_system,
            tool_set,
        }
    }

    /// Convert to json
    pub fn to_json(&self) -> Result<String, ToolError> {
        serde_json::to_string(self).map_err(|_| ToolError::FailedJSONParsing)
    }

    /// Convert from json
    pub fn from_json(json: &str) -> Result<Self, ToolError> {
        let deserialized: Self = serde_json::from_str(json)?;
        Ok(deserialized)
    }

    /// Check if all required config fields are set
    pub fn check_required_config_fields(&self) -> bool {
        for config in &self.config {
            let ToolConfig::BasicConfig(basic_config) = config;
            if basic_config.required && basic_config.key_value.is_none() {
                return false;
            }
        }
        true
    }

    pub fn get_metadata(&self) -> ToolPlaygroundMetadata {
        ToolPlaygroundMetadata {
            name: self.name.clone(),
            description: self.description.clone(),
            keywords: self.keywords.clone(),
            homepage: self.homepage.clone(),
            author: self.author.clone(),
            version: self.version.clone(),
            configurations: self.config.clone(),
            parameters: self.input_args.clone(),
            result: self.result.clone(),
            sql_tables: self.sql_tables.clone().unwrap_or_default(),
            sql_queries: self.sql_queries.clone().unwrap_or_default(),
            tools: Some(self.tools.clone()),
            oauth: self.oauth.clone(),
            runner: self.runner.clone(),
            operating_system: self.operating_system.clone(),
            tool_set: self.tool_set.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tools::tool_config::BasicConfig;

    use super::*;
    use serde_json::json;

    #[test]
    fn test_deserialize_jstool_result_with_hashmap_properties() {
        let json_data = r#"
    {
        "type": "object",
        "properties": {
            "walletId": {"type": "string", "nullable": true},
            "seed": {"type": "string", "nullable": true},
            "address": {"type": "string", "nullable": true}
        },
        "required": []
    }
    "#;

        let deserialized: ToolResult = serde_json::from_str(json_data).expect("Failed to deserialize JSToolResult");

        assert_eq!(deserialized.r#type, "object");
        assert!(deserialized.properties.is_object());
        assert_eq!(deserialized.required, Vec::<String>::new());

        if let Some(wallet_id) = deserialized.properties.get("walletId") {
            assert_eq!(wallet_id.get("type").and_then(|v| v.as_str()), Some("string"));
            assert_eq!(wallet_id.get("nullable").and_then(|v| v.as_bool()), Some(true));
        } else {
            panic!("walletId property missing");
        }

        if let Some(seed) = deserialized.properties.get("seed") {
            assert_eq!(seed.get("type").and_then(|v| v.as_str()), Some("string"));
            assert_eq!(seed.get("nullable").and_then(|v| v.as_bool()), Some(true));
        } else {
            panic!("seed property missing");
        }

        if let Some(address) = deserialized.properties.get("address") {
            assert_eq!(address.get("type").and_then(|v| v.as_str()), Some("string"));
            assert_eq!(address.get("nullable").and_then(|v| v.as_bool()), Some(true));
        } else {
            panic!("address property missing");
        }
    }

    #[test]
    fn test_deserialize_deno_tool() {
        let json_data = r#"{
            "author": "Hanzo",
            "homepage": "http://example.com",
            "config": [
                {
                    "BasicConfig": {
                        "description": "",
                        "key_name": "name",
                        "key_value": null,
                        "required": true,
                        "type_name": null
                    }
                },
                {
                    "BasicConfig": {
                        "description": "",
                        "key_name": "privateKey",
                        "key_value": null,
                        "required": true,
                        "type_name": null
                    }
                },
                {
                    "BasicConfig": {
                        "description": "",
                        "key_name": "useServerSigner",
                        "key_value": null,
                        "required": false,
                        "type_name": null
                    }
                }
            ],
            "description": "Tool for creating a Coinbase wallet",
            "input_args": {
                "properties": {},
                "required": [],
                "type": "object"
            },
            "name": "Coinbase Wallet Creator",
            "output_arg": {
                "json": ""
            },
            "version": "1.0.0",
            "js_code": "",
            "keywords": [],
            "activated": false,
            "tools": [],
            "runner": "any",
            "tool_set": null,
            "operating_system": [],
            "result": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }"#;

        let deserialized: DenoTool = serde_json::from_str(json_data).expect("Failed to deserialize DenoTool");

        assert_eq!(deserialized.author, "Hanzo");
        assert_eq!(deserialized.name, "Coinbase Wallet Creator");
        assert_eq!(deserialized.version, "1.0.0");
        assert_eq!(deserialized.description, "Tool for creating a Coinbase wallet");
        assert_eq!(deserialized.homepage, Some("http://example.com".to_string()));
        assert_eq!(deserialized.runner, RunnerType::Any);
        assert_eq!(deserialized.tool_set, None);
        assert_eq!(
            deserialized.tool_router_key,
            Some(ToolRouterKey::new(
                "local".to_string(),
                "Hanzo".to_string(),
                "Coinbase Wallet Creator".to_string(),
                None
            ))
        );

        // Verify config entries
        assert_eq!(deserialized.config.len(), 3);
        let ToolConfig::BasicConfig(config) = &deserialized.config[0];
        assert_eq!(config.key_name, "name");
        assert!(config.required);

        let ToolConfig::BasicConfig(config) = &deserialized.config[1];
        assert_eq!(config.key_name, "privateKey");
        assert!(config.required);

        let ToolConfig::BasicConfig(config) = &deserialized.config[2];
        assert_eq!(config.key_name, "useServerSigner");
        assert!(!config.required);
    }

    #[test]
    fn test_email_fetcher_tool_config() {
        let tool = DenoTool {
            tool_router_key: Some(ToolRouterKey::new(
                "local".to_string(),
                "Hanzo".to_string(),
                "Email Fetcher".to_string(),
                None,
            )),
            name: "Email Fetcher".to_string(),
            homepage: Some("http://127.0.0.1/index.html".to_string()),
            author: "Hanzo".to_string(),
            version: "1.0.0".to_string(),
            description: "Fetches emails from an IMAP server".to_string(),
            mcp_enabled: Some(false),
            keywords: vec!["email".to_string(), "imap".to_string()],
            js_code: "".to_string(),
            tools: vec![],
            config: vec![
                ToolConfig::BasicConfig(BasicConfig {
                    key_name: "imap_server".to_string(),
                    description: "The IMAP server address".to_string(),
                    required: true,
                    type_name: Some("string".to_string()),
                    key_value: None,
                }),
                ToolConfig::BasicConfig(BasicConfig {
                    key_name: "username".to_string(),
                    description: "The username for the IMAP account".to_string(),
                    required: true,
                    type_name: Some("string".to_string()),
                    key_value: None,
                }),
                ToolConfig::BasicConfig(BasicConfig {
                    key_name: "password".to_string(),
                    description: "The password for the IMAP account".to_string(),
                    required: true,
                    type_name: Some("string".to_string()),
                    key_value: None,
                }),
                ToolConfig::BasicConfig(BasicConfig {
                    key_name: "port".to_string(),
                    description: "The port number for the IMAP server (defaults to 993 for IMAPS)".to_string(),
                    required: false,
                    type_name: Some("integer".to_string()),
                    key_value: None,
                }),
                ToolConfig::BasicConfig(BasicConfig {
                    key_name: "ssl".to_string(),
                    description: "Whether to use SSL for the IMAP connection (defaults to true)".to_string(),
                    required: false,
                    type_name: Some("boolean".to_string()),
                    key_value: None,
                }),
            ],
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            activated: false,
            embedding: None,
            result: ToolResult::new("object".to_string(), json!({}), vec![]),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::Any,
            operating_system: vec![OperatingSystem::Linux],
            tool_set: None,
        };

        let serialized = serde_json::to_string_pretty(&tool).expect("Failed to serialize DenoTool");

        let deserialized: DenoTool = serde_json::from_str(&serialized).expect("Failed to deserialize DenoTool");

        // Test check_required_config_fields with no values set
        assert!(
            !deserialized.check_required_config_fields(),
            "Should fail when required fields have no values"
        );

        // Create a tool with values set for required fields
        let mut tool_with_values = deserialized.clone();
        tool_with_values.config = vec![
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "imap_server".to_string(),
                description: "The IMAP server address".to_string(),
                required: true,
                type_name: Some("string".to_string()),
                key_value: Some(serde_json::Value::String("imap.example.com".to_string())),
            }),
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "username".to_string(),
                description: "The username for the IMAP account".to_string(),
                required: true,
                type_name: Some("string".to_string()),
                key_value: Some(serde_json::Value::String("user@example.com".to_string())),
            }),
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "password".to_string(),
                description: "The password for the IMAP account".to_string(),
                required: true,
                type_name: Some("string".to_string()),
                key_value: Some(serde_json::Value::String("password123".to_string())),
            }),
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "port".to_string(),
                description: "The port number for the IMAP server (defaults to 993 for IMAPS)".to_string(),
                required: false,
                type_name: Some("integer".to_string()),
                key_value: None,
            }),
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "ssl".to_string(),
                description: "Whether to use SSL for the IMAP connection (defaults to true)".to_string(),
                required: false,
                type_name: Some("boolean".to_string()),
                key_value: None,
            }),
        ];

        assert!(
            tool_with_values.check_required_config_fields(),
            "Should pass when required fields have values"
        );

        // Test serialization/deserialization
        let serialized = serde_json::to_string(&tool).expect("Failed to serialize DenoTool");
        let deserialized: DenoTool = serde_json::from_str(&serialized).expect("Failed to deserialize DenoTool");

        // Check specific configs
        let imap_server_config = deserialized
            .config
            .iter()
            .find(|c| match c {
                ToolConfig::BasicConfig(bc) => bc.key_name == "imap_server",
                _ => false,
            })
            .unwrap();
        let ToolConfig::BasicConfig(config) = imap_server_config;
        assert_eq!(config.description, "The IMAP server address");
        assert_eq!(config.type_name, Some("string".to_string()));
        assert!(config.required);
        assert_eq!(config.key_value, None);

        let port_config = deserialized
            .config
            .iter()
            .find(|c| match c {
                ToolConfig::BasicConfig(bc) => bc.key_name == "port",
                _ => false,
            })
            .unwrap();
        let ToolConfig::BasicConfig(config) = port_config;
        assert_eq!(
            config.description,
            "The port number for the IMAP server (defaults to 993 for IMAPS)"
        );
        assert_eq!(config.type_name, Some("integer".to_string()));
        assert!(!config.required);
        assert_eq!(config.key_value, None);
    }

    #[test]
    fn test_deno_tool_runner_types() {
        let tool = DenoTool {
            tool_router_key: Some(ToolRouterKey::new(
                "local".to_string(),
                "Test Author".to_string(),
                "Test Tool".to_string(),
                None,
            )),
            name: "Test Tool".to_string(),
            homepage: None,
            author: "Test Author".to_string(),
            version: "1.0.0".to_string(),
            js_code: "".to_string(),
            tools: vec![],
            config: vec![],
            description: "Test description".to_string(),
            keywords: vec![],
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            activated: false,
            mcp_enabled: Some(false),
            embedding: None,
            result: ToolResult::new("object".to_string(), json!({}), vec![]),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::OnlyDocker,
            operating_system: vec![],
            tool_set: None,
        };

        // Test serialization/deserialization with RunnerType
        let serialized = serde_json::to_string(&tool).expect("Failed to serialize DenoTool");
        let deserialized: DenoTool = serde_json::from_str(&serialized).expect("Failed to deserialize DenoTool");

        assert_eq!(deserialized.runner, RunnerType::OnlyDocker);

        // Test different runner types
        let mut tool_any = tool.clone();
        tool_any.runner = RunnerType::Any;
        let serialized = serde_json::to_string(&tool_any).expect("Failed to serialize DenoTool");
        let deserialized: DenoTool = serde_json::from_str(&serialized).expect("Failed to deserialize DenoTool");
        assert_eq!(deserialized.runner, RunnerType::Any);
    }

    #[test]
    fn test_deno_tool_operating_systems() {
        let tool = DenoTool {
            name: "Test Tool".to_string(),
            tool_router_key: Some(ToolRouterKey::new(
                "local".to_string(),
                "Test Author".to_string(),
                "Test Tool".to_string(),
                None,
            )),
            homepage: None,
            author: "Test Author".to_string(),
            version: "1.0.0".to_string(),
            js_code: "".to_string(),
            tools: vec![],
            config: vec![],
            description: "Test description".to_string(),
            keywords: vec![],
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            activated: false,
            mcp_enabled: Some(false),
            embedding: None,
            result: ToolResult::new("object".to_string(), json!({}), vec![]),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::Any,
            operating_system: vec![OperatingSystem::Linux, OperatingSystem::Windows],
            tool_set: None,
        };

        // Test serialization/deserialization with operating systems
        let serialized = serde_json::to_string(&tool).expect("Failed to serialize DenoTool");
        let deserialized: DenoTool = serde_json::from_str(&serialized).expect("Failed to deserialize DenoTool");

        assert_eq!(deserialized.operating_system.len(), 2);
        assert!(deserialized.operating_system.contains(&OperatingSystem::Linux));
        assert!(deserialized.operating_system.contains(&OperatingSystem::Windows));
    }

    #[test]
    fn test_deno_tool_tool_set() {
        let tool = DenoTool {
            tool_router_key: Some(ToolRouterKey::new(
                "local".to_string(),
                "Test Author".to_string(),
                "Test Tool".to_string(),
                None,
            )),
            name: "Test Tool".to_string(),
            homepage: None,
            author: "Test Author".to_string(),
            version: "1.0.0".to_string(),
            js_code: "".to_string(),
            tools: vec![],
            config: vec![],
            description: "Test description".to_string(),
            keywords: vec![],
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            activated: false,
            mcp_enabled: Some(false),
            embedding: None,
            result: ToolResult::new("object".to_string(), json!({}), vec![]),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::Any,
            operating_system: vec![],
            tool_set: Some("test-tool-set".to_string()),
        };

        // Test serialization/deserialization with tool_set
        let serialized = serde_json::to_string(&tool).expect("Failed to serialize DenoTool");
        let deserialized: DenoTool = serde_json::from_str(&serialized).expect("Failed to deserialize DenoTool");

        assert_eq!(deserialized.tool_set, Some("test-tool-set".to_string()));

        // Test with None tool_set
        let mut tool_no_set = tool.clone();
        tool_no_set.tool_set = None;
        let serialized = serde_json::to_string(&tool_no_set).expect("Failed to serialize DenoTool");
        let deserialized: DenoTool = serde_json::from_str(&serialized).expect("Failed to deserialize DenoTool");
        assert_eq!(deserialized.tool_set, None);
    }
}
