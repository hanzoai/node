use std::path::PathBuf;

#[derive(Clone)]
pub struct ExecutionContext {
    pub context_id: String,
    pub execution_id: String,
    pub code_id: String,
    pub storage: PathBuf,
    pub assets_files: Vec<PathBuf>,
    pub mount_files: Vec<PathBuf>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            context_id: nanoid::nanoid!(),
            execution_id: nanoid::nanoid!(),
            code_id: nanoid::nanoid!(),
            storage: PathBuf::from("./hanzo-tools-runner-execution-storage"),
            assets_files: Vec::new(),
            mount_files: Vec::new(),
        }
    }
}
