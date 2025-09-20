#[cfg(test)]
mod docker_integration_tests {
    use hanzo_tools_primitives::tools::docker_tools::DockerTool;
    use hanzo_tools_primitives::tools::hanzo_tool::HanzoTool;
    use hanzo_tools_primitives::tools::parameters::Parameters;
    use hanzo_tools_primitives::tools::tool_output_arg::ToolOutputArg;
    use hanzo_tools_primitives::tools::tool_types::{OperatingSystem, RunnerType, ToolResult};
    use serde_json::json;

    #[test]
    fn test_docker_tool_creation() {
        // Create a simple Docker tool for Python
        let docker_tool = DockerTool::new(
            "test_docker_python".to_string(),
            "Test Docker Python tool".to_string(),
            r#"
import json
import os

params = json.loads(os.environ.get('HANZO_PARAMS', '{}'))
name = params.get('name', 'World')
print(json.dumps({"message": f"Hello, {name}!"}))
"#.to_string(),
            "python".to_string(),
        );

        // Verify the tool properties
        assert_eq!(docker_tool.name, "test_docker_python");
        assert_eq!(docker_tool.description, "Test Docker Python tool");
        assert_eq!(docker_tool.language, "python");
        assert_eq!(docker_tool.docker_image, "python:3.11-slim");
        assert_eq!(docker_tool.runner, RunnerType::Docker);
    }

    #[test]
    fn test_docker_tool_as_hanzo_tool() {
        // Create a Docker tool
        let mut docker_tool = DockerTool::new(
            "test_js_docker".to_string(),
            "Test JavaScript Docker tool".to_string(),
            r#"
const params = JSON.parse(process.env.HANZO_PARAMS || '{}');
console.log(JSON.stringify({result: `Processed: ${params.input}`}));
"#.to_string(),
            "javascript".to_string(),
        );

        // Add input parameters
        docker_tool.input_args = Parameters::with_single_property(
            "input",
            "string",
            "Input to process",
            true,
            None,
        );

        // Convert to HanzoTool
        let hanzo_tool: HanzoTool = docker_tool.clone().into();

        // Verify it's a Docker variant
        assert_eq!(hanzo_tool.tool_type(), "Docker");
        assert_eq!(hanzo_tool.name(), "test_js_docker");
        assert!(hanzo_tool.is_enabled());
    }

    #[test]
    fn test_docker_tool_metadata() {
        let mut docker_tool = DockerTool::new(
            "metadata_test".to_string(),
            "Test metadata generation".to_string(),
            "echo 'test'".to_string(),
            "bash".to_string(),
        );

        // Set some additional properties
        docker_tool.cpu_limit = Some(1.5);
        docker_tool.memory_limit = Some("256M".to_string());
        docker_tool.network_mode = Some("bridge".to_string());
        docker_tool.keywords = vec!["test".to_string(), "docker".to_string()];

        // Get metadata
        let metadata = docker_tool.get_metadata();

        // Verify metadata
        assert_eq!(metadata.name, "metadata_test");
        assert_eq!(metadata.description, "Test metadata generation");
        assert_eq!(metadata.author, "hanzo");
        assert_eq!(metadata.keywords.len(), 2);
        assert_eq!(metadata.runner, RunnerType::Docker);
    }

    #[test]
    fn test_docker_config_parsing() {
        use hanzo_tools_primitives::tools::tool_config::{BasicConfig, ToolConfig};
        use serde_json::Value;

        let configs = vec![
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "docker_image".to_string(),
                key_value: Some(Value::String("alpine:latest".to_string())),
                description: "Docker image".to_string(),
                required: false,
                type_name: None,
            }),
            ToolConfig::BasicConfig(BasicConfig {
                key_name: "docker_cpu_limit".to_string(),
                key_value: Some(Value::Number(serde_json::Number::from_f64(0.5).unwrap())),
                description: "CPU limit".to_string(),
                required: false,
                type_name: None,
            }),
        ];

        // This would be used in execute_docker_tool
        // Just verify the configs are created correctly
        assert_eq!(configs.len(), 2);
        if let ToolConfig::BasicConfig(ref config) = configs[0] {
            assert_eq!(config.key_name, "docker_image");
        }
    }

    #[test]
    fn test_docker_language_to_image_mapping() {
        let test_cases = vec![
            ("python", "python:3.11-slim"),
            ("javascript", "node:20-alpine"),
            ("rust", "rust:1.75-slim"),
            ("go", "golang:1.21-alpine"),
            ("bash", "alpine:latest"),
        ];

        for (language, expected_image) in test_cases {
            let tool = DockerTool::new(
                format!("test_{}", language),
                format!("Test {} tool", language),
                "test code".to_string(),
                language.to_string(),
            );
            assert_eq!(
                tool.docker_image, expected_image,
                "Language {} should map to image {}",
                language, expected_image
            );
        }
    }
}