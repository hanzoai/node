use std::path::PathBuf;

use super::{
    execution_context::ExecutionContext, runner_type::RunnerType,
    hanzo_node_location::HanzoNodeLocation,
};

#[derive(Clone)]
pub struct PythonRunnerOptions {
    pub context: ExecutionContext,
    pub uv_binary_path: PathBuf,
    pub code_runner_docker_image_name: String,
    pub force_runner_type: Option<RunnerType>,
    pub hanzo_node_location: HanzoNodeLocation,
}

impl Default for PythonRunnerOptions {
    fn default() -> Self {
        Self {
            context: ExecutionContext::default(),
            code_runner_docker_image_name: String::from("dcspark/hanzo-code-runner:0.9.4"),
            uv_binary_path: PathBuf::from(if cfg!(windows) {
                "./hanzo-tools-runner-resources/uv.exe"
            } else {
                "./hanzo-tools-runner-resources/uv"
            }),
            force_runner_type: None,
            hanzo_node_location: HanzoNodeLocation {
                protocol: String::from("http"),
                host: String::from("127.0.0.1"),
                port: 9550,
            },
        }
    }
}
