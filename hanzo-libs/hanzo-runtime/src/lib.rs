use std::{path::PathBuf, time::Duration};

pub mod functions;

#[derive(Debug, Clone, Copy)]
pub enum NonRustRuntime {
    Deno,
    Python,
}

impl std::fmt::Display for NonRustRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NonRustRuntime::Deno => write!(f, "deno"),
            NonRustRuntime::Python => write!(f, "python"),
        }
    }
}

#[derive(Debug)]
pub enum RunError {
    NoRuntime { function: String, runtime: NonRustRuntime },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NoRuntime { function, runtime } => write!(
                f,
                "{} needs a {} runtime; this node has none. Non-Rust code runs in the sandbox.",
                function, runtime
            ),
        }
    }
}

pub struct NonRustCodeRunnerFactory {
    function_name: String,
    code: String,
    mount_files: Vec<PathBuf>,
    runtime: NonRustRuntime,
}

impl NonRustCodeRunnerFactory {
    pub fn new(function_name: impl Into<String>, code: impl Into<String>, mount_files: Vec<PathBuf>) -> Self {
        Self {
            function_name: function_name.into(),
            code: code.into(),
            mount_files,
            runtime: NonRustRuntime::Deno,
        }
    }

    pub fn with_runtime(mut self, runtime: NonRustRuntime) -> Self {
        self.runtime = runtime;
        self
    }

    pub fn create_runner<C>(&self, configurations: C) -> NonRustCodeRunner<C>
    where
        C: serde::Serialize,
    {
        NonRustCodeRunner {
            function_name: self.function_name.clone(),
            code: self.code.clone(),
            configurations,
            mount_files: self.mount_files.clone(),
            runtime: self.runtime,
        }
    }
}

/// One unit of non-Rust work: the source, its configuration, and the files it may read.
/// The fields describe the job; the sandbox is what executes it.
#[allow(dead_code)]
pub struct NonRustCodeRunner<C> {
    function_name: String,
    code: String,
    configurations: C,
    mount_files: Vec<PathBuf>,
    runtime: NonRustRuntime,
}

impl<C> NonRustCodeRunner<C>
where
    C: serde::Serialize,
{
    pub async fn run<P, T>(&self, _params: P, _timeout: Option<Duration>) -> Result<T, RunError>
    where
        P: serde::Serialize,
        T: serde::de::DeserializeOwned,
    {
        Err(RunError::NoRuntime {
            function: self.function_name.clone(),
            runtime: self.runtime,
        })
    }
}
