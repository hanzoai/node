use hanzo_tools::tools::error::ToolError;

pub mod execute_agent_dynamic;
pub mod execute_mcp_server_dynamic;
pub mod execution_coordinator;
pub mod execution_custom;
pub mod execution_header_generator;

/// Deno and Python tools describe work this node cannot carry out: it holds the
/// definition, not a runtime for either language. Every path that used to reach
/// a local runtime reports the absence here, so the answer is the same wherever
/// the request came in.
pub fn no_local_runtime(language: &str) -> ToolError {
    ToolError::ExecutionError(format!(
        "{language} tools need a local runtime; this node has none. Non-Rust code runs in the sandbox."
    ))
}
