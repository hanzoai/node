use super::{execution_storage::ExecutionStorage, runner_type::RunnerType};

impl ExecutionStorage {
    fn deno_cache_folder_path_host(&self) -> std::path::PathBuf {
        self.global_cache_folder_path.join("deno-cache-host")
    }
    fn deno_cache_folder_path_docker(&self) -> std::path::PathBuf {
        self.global_cache_folder_path.join("deno-cache-docker")
    }
    pub fn deno_cache_folder_path(&self, runner_type: RunnerType) -> std::path::PathBuf {
        match runner_type {
            RunnerType::Host => self.deno_cache_folder_path_host(),
            RunnerType::Docker => self.deno_cache_folder_path_docker(),
        }
    }
    pub fn init_for_deno(
        &self,
        pristine_cache: Option<bool>,
        runner_type: RunnerType,
    ) -> anyhow::Result<()> {
        self.init(pristine_cache)?;

        log::info!("creating deno cache directory");
        let deno_cache_dir = self.deno_cache_folder_path(runner_type);
        std::fs::create_dir_all(&deno_cache_dir).map_err(|e| {
            log::error!("failed to create deno cache directory: {}", e);
            e
        })?;

        // Workaround for puppeteer. Cosmiconfig try to read a forbidden folder
        log::info!("creating .config directory");
        let config_dir = self.root_folder_path.join(".config");
        std::fs::create_dir_all(&config_dir).map_err(|e| {
            log::error!("failed to create .config directory: {}", e);
            e
        })?;
        log::info!("creating config.json file");
        let config_json_path = config_dir.join("config.json");
        std::fs::write(
            &config_json_path,
            r#"
        {
            "puppeteer": {
                "option":  "value"
            }
        }
        "#,
        )
        .map_err(|e| {
            log::error!("failed to write config.json file: {}", e);
            e
        })?;

        log::info!("creating deno.json file");
        let deno_json_path = self.code_folder_path.join("deno.json");
        std::fs::write(&deno_json_path, "").map_err(|e| {
            log::error!("failed to write deno.json file: {}", e);
            e
        })?;

        Ok(())
    }
}
