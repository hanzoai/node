#[cfg(test)]
mod kubernetes_integration_tests {
    use hanzo_tools_primitives::tools::kubernetes_tools::{KubernetesTool, K8sResourceRequirements};
    use hanzo_tools_primitives::tools::hanzo_tool::HanzoTool;
    use serde_json::json;

    #[test]
    fn test_kubernetes_tool_creation() {
        let k8s_tool = KubernetesTool::new(
            "test-tool".to_string(),
            "Test Kubernetes tool".to_string(),
            "print('Hello from Kubernetes')".to_string(),
            "python".to_string(),
        );

        assert_eq!(k8s_tool.name, "test-tool");
        assert_eq!(k8s_tool.language, "python");
        assert_eq!(k8s_tool.image, "python:3.11-slim");
        assert!(k8s_tool.resources.is_some());
    }

    #[test]
    fn test_kubernetes_tool_with_gpu() {
        let mut k8s_tool = KubernetesTool::new(
            "gpu-tool".to_string(),
            "GPU-enabled tool".to_string(),
            "import torch; print(torch.cuda.is_available())".to_string(),
            "python".to_string(),
        );

        k8s_tool = k8s_tool.with_gpu(2, Some("nvidia.com/gpu".to_string()));

        assert!(k8s_tool.resources.is_some());
        let resources = k8s_tool.resources.unwrap();
        assert_eq!(resources.gpu_count, Some(2));
        assert_eq!(resources.gpu_type, Some("nvidia.com/gpu".to_string()));
    }

    #[test]
    fn test_hanzo_tool_kubernetes_variant() {
        let k8s_tool = KubernetesTool::new(
            "k8s-test".to_string(),
            "Test tool".to_string(),
            "echo 'test'".to_string(),
            "bash".to_string(),
        );

        let hanzo_tool = HanzoTool::Kubernetes(k8s_tool.clone(), true);

        match hanzo_tool {
            HanzoTool::Kubernetes(tool, enabled) => {
                assert_eq!(tool.name, "k8s-test");
                assert!(enabled);
            }
            _ => panic!("Expected Kubernetes variant"),
        }
    }

    #[test]
    fn test_kubernetes_tool_validation() {
        let mut k8s_tool = KubernetesTool::new(
            "".to_string(),
            "Invalid tool".to_string(),
            "code".to_string(),
            "python".to_string(),
        );

        // Empty name should fail validation
        assert!(k8s_tool.validate().is_err());

        // Fix the name
        k8s_tool.name = "valid-tool".to_string();
        assert!(k8s_tool.validate().is_ok());

        // Test excessive GPU count
        k8s_tool.resources = Some(K8sResourceRequirements {
            cpu_request: None,
            cpu_limit: None,
            memory_request: None,
            memory_limit: None,
            gpu_count: Some(10), // Too many
            gpu_type: None,
        });
        assert!(k8s_tool.validate().is_err());
    }

    #[test]
    fn test_kubernetes_tool_serialization() {
        let k8s_tool = KubernetesTool::new(
            "serialization-test".to_string(),
            "Test serialization".to_string(),
            "console.log('test')".to_string(),
            "javascript".to_string(),
        );

        let hanzo_tool = HanzoTool::Kubernetes(k8s_tool, true);

        // Serialize to JSON
        let json = serde_json::to_value(&hanzo_tool).unwrap();

        // Check structure
        assert_eq!(json["type"], "Kubernetes");
        assert_eq!(json["content"][0]["name"], "serialization-test");
        assert_eq!(json["content"][1], true);

        // Deserialize back
        let deserialized: HanzoTool = serde_json::from_value(json).unwrap();

        match deserialized {
            HanzoTool::Kubernetes(tool, enabled) => {
                assert_eq!(tool.name, "serialization-test");
                assert!(enabled);
            }
            _ => panic!("Deserialization failed"),
        }
    }
}