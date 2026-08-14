use super::parameters::Parameters;
use super::tool_config::{OAuth, ToolConfig};
use super::tool_output_arg::ToolOutputArg;
use super::tool_playground::ToolPlaygroundMetadata;
use super::tool_playground::{SqlQuery, SqlTable};
use super::tool_types::{OperatingSystem, RunnerType, ToolResult};
use crate::tools::error::ToolError;
use hanzo_messages::schemas::tool_router_key::ToolRouterKey;

#[derive(Debug, Clone, PartialEq)]
pub struct PythonTool {
    pub version: String,
    pub name: String,
    pub tool_router_key: Option<ToolRouterKey>,
    pub homepage: Option<String>,
    pub author: String,
    pub mcp_enabled: Option<bool>,
    pub py_code: String,
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

impl PythonTool {
    /// Convert to json
    pub fn to_json(&self) -> Result<String, ToolError> {
        serde_json::to_string(self).map_err(|_| ToolError::FailedJSONParsing)
    }

    /// Convert from json
    pub fn from_json(json: &str) -> Result<Self, ToolError> {
        let deserialized: Self = serde_json::from_str(json)?;
        Ok(deserialized)
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

impl<'de> serde::Deserialize<'de> for PythonTool {
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
            py_code: String,
            #[serde(default)]
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

        Ok(PythonTool {
            name: helper.name,
            tool_router_key,
            homepage: helper.homepage,
            author: helper.author,
            version: helper.version,
            mcp_enabled: helper.mcp_enabled,
            py_code: helper.py_code,
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

impl serde::Serialize for PythonTool {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("PythonTool", 24)?;
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
        state.serialize_field("py_code", &self.py_code)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_tool_with_runner_type() {
        let tool = PythonTool {
            version: "1.0".to_string(),
            name: "test_tool".to_string(),
            tool_router_key: Some(ToolRouterKey::new(
                "local".to_string(),
                "test_author".to_string(),
                "test_tool".to_string(),
                None,
            )),
            homepage: None,
            author: "test_author".to_string(),
            mcp_enabled: Some(false),
            py_code: "print('hello')".to_string(),
            tools: vec![],
            config: vec![],
            description: "test description".to_string(),
            keywords: vec!["test".to_string()],
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            activated: true,
            embedding: None,
            result: ToolResult::new("object".to_string(), serde_json::Value::Null, vec![]),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::OnlyHost,
            operating_system: vec![OperatingSystem::Windows],
            tool_set: None,
        };

        assert_eq!(tool.runner, RunnerType::OnlyHost);
    }

    #[test]
    fn test_python_tool_with_operating_systems() {
        let tool = PythonTool {
            version: "1.0".to_string(),
            name: "test_tool".to_string(),
            tool_router_key: Some(ToolRouterKey::new(
                "local".to_string(),
                "test_author".to_string(),
                "test_tool".to_string(),
                None,
            )),
            homepage: None,
            author: "test_author".to_string(),
            mcp_enabled: Some(false),
            py_code: "print('hello')".to_string(),
            tools: vec![],
            config: vec![],
            description: "test description".to_string(),
            keywords: vec!["test".to_string()],
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            activated: true,
            embedding: None,
            result: ToolResult::new("object".to_string(), serde_json::Value::Null, vec![]),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::Any,
            operating_system: vec![OperatingSystem::Linux, OperatingSystem::Windows],
            tool_set: None,
        };

        assert_eq!(tool.operating_system.len(), 2);
        assert!(tool.operating_system.contains(&OperatingSystem::Linux));
        assert!(tool.operating_system.contains(&OperatingSystem::Windows));
    }

    #[test]
    fn test_python_tool_with_tool_set() {
        let tool = PythonTool {
            version: "1.0".to_string(),
            name: "test_tool".to_string(),
            tool_router_key: Some(ToolRouterKey::new(
                "local".to_string(),
                "test_author".to_string(),
                "test_tool".to_string(),
                None,
            )),
            homepage: None,
            author: "test_author".to_string(),
            mcp_enabled: Some(false),
            py_code: "print('hello')".to_string(),
            tools: vec![],
            config: vec![],
            description: "test description".to_string(),
            keywords: vec!["test".to_string()],
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            activated: true,
            embedding: None,
            result: ToolResult::new("object".to_string(), serde_json::Value::Null, vec![]),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::OnlyHost,
            operating_system: vec![OperatingSystem::Linux],
            tool_set: Some("test_set".to_string()),
        };

        assert_eq!(tool.tool_set, Some("test_set".to_string()));
    }

    #[test]
    fn test_python_tool_serialization() {
        let tool = PythonTool {
            version: "1.0".to_string(),
            name: "test_tool".to_string(),
            tool_router_key: Some(ToolRouterKey::new(
                "local".to_string(),
                "test_author".to_string(),
                "test_tool".to_string(),
                None,
            )),
            homepage: None,
            author: "test_author".to_string(),
            mcp_enabled: Some(false),
            py_code: "print('hello')".to_string(),
            tools: vec![],
            config: vec![],
            description: "test description".to_string(),
            keywords: vec!["test".to_string()],
            input_args: Parameters::new(),
            output_arg: ToolOutputArg { json: "".to_string() },
            activated: true,
            embedding: None,
            result: ToolResult::new("object".to_string(), serde_json::Value::Null, vec![]),
            sql_tables: None,
            sql_queries: None,
            file_inbox: None,
            oauth: None,
            assets: None,
            runner: RunnerType::OnlyHost,
            operating_system: vec![OperatingSystem::Linux],
            tool_set: Some("test_set".to_string()),
        };

        let json = tool.to_json().unwrap();
        let deserialized = PythonTool::from_json(&json).unwrap();

        assert_eq!(tool.runner, deserialized.runner);
        assert_eq!(tool.operating_system, deserialized.operating_system);
        assert_eq!(tool.tool_set, deserialized.tool_set);
        assert_eq!(tool.tool_router_key, deserialized.tool_router_key);
    }
}
