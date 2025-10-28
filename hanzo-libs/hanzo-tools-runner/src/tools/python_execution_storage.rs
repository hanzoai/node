use super::execution_storage::ExecutionStorage;

impl ExecutionStorage {
    pub fn python_run_host_venv_folder_path(&self) -> std::path::PathBuf {
        self.cache_folder_path.join("python-run-host-venv")
    }
    pub fn python_run_docker_venv_folder_path(&self) -> std::path::PathBuf {
        self.cache_folder_path.join("python-run-docker-venv")
    }
    pub fn python_run_docker_uv_cache_folder_path(&self) -> std::path::PathBuf {
        self.global_cache_folder_path.join("uv-cache-docker")
    }

    pub fn python_check_venv_folder_path(&self) -> std::path::PathBuf {
        self.cache_folder_path.join("python-check-venv")
    }
    pub fn init_for_python(&self, pristine_cache: Option<bool>) -> anyhow::Result<()> {
        self.init(pristine_cache)?;

        log::info!("creating python cache directories");
        std::fs::create_dir_all(self.python_check_venv_folder_path()).map_err(|e| {
            log::error!("failed to create python check venv directory: {}", e);
            e
        })?;
        std::fs::create_dir_all(self.python_run_host_venv_folder_path()).map_err(|e| {
            log::error!("failed to create python run host venv directory: {}", e);
            e
        })?;
        std::fs::create_dir_all(self.python_run_docker_venv_folder_path()).map_err(|e| {
            log::error!("failed to create python run docker venv directory: {}", e);
            e
        })?;
        std::fs::create_dir_all(self.python_run_docker_uv_cache_folder_path()).map_err(|e| {
            log::error!("failed to create uv cache directory: {}", e);
            e
        })?;
        Ok(())
    }
}
