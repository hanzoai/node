use std::{
    io::Write,
    path::{self, PathBuf},
};

use super::{code_files::CodeFiles, path_buf_ext::PathBufExt};
use super::{execution_context::ExecutionContext, file_name_utils::sanitize_for_file_name};

#[derive(Default, Clone)]
pub struct ExecutionStorage {
    pub global_cache_folder_path: PathBuf,
    pub code_files: CodeFiles,
    pub context: ExecutionContext,
    pub code_id: String,
    pub root_folder_path: PathBuf,
    pub root_code_folder_path: PathBuf,
    pub code_folder_path: PathBuf,
    pub code_entrypoint_file_path: PathBuf,
    pub cache_folder_path: PathBuf,
    pub logs_folder_path: PathBuf,
    pub log_file_path: PathBuf,
    pub home_folder_path: PathBuf,
    pub assets_folder_path: PathBuf,
    pub mount_folder_path: PathBuf,
}

impl ExecutionStorage {
    pub fn new(code: CodeFiles, context: ExecutionContext) -> Self {
        let code_id = context.code_id.clone();
        let root_folder_path = path::absolute(
            context
                .storage
                .join(sanitize_for_file_name(context.context_id.clone()))
                .clone(),
        )
        .unwrap();
        let global_cache_folder_path =
            path::absolute(context.storage.join("global-cache")).unwrap();
        let root_code_folder_path = path::absolute(root_folder_path.join("code")).unwrap();
        let code_folder_path = path::absolute(root_code_folder_path.join(code_id.clone())).unwrap();
        let logs_folder_path = path::absolute(root_folder_path.join("logs")).unwrap();
        let log_file_path = path::absolute(logs_folder_path.join(format!(
            "log_{}_{}.log",
            sanitize_for_file_name(context.context_id.clone()),
            sanitize_for_file_name(context.execution_id.clone())
        )))
        .unwrap();
        let cache_folder_path = path::absolute(root_folder_path.join("cache")).unwrap();
        let code_entrypoint_file_path = code_folder_path.join(&code.entrypoint);
        Self {
            code_files: code,
            context,
            code_folder_path,
            code_id: code_id.clone(),
            root_folder_path: root_folder_path.clone(),
            root_code_folder_path,
            code_entrypoint_file_path,
            cache_folder_path,
            logs_folder_path: logs_folder_path.clone(),
            log_file_path,
            home_folder_path: root_folder_path.join("home"),
            assets_folder_path: root_folder_path.join("assets"),
            mount_folder_path: root_folder_path.join("mount"),
            global_cache_folder_path,
        }
    }

    pub fn init(&self, pristine_cache: Option<bool>) -> anyhow::Result<()> {
        for dir in [
            &self.root_folder_path,
            &self.root_code_folder_path,
            &self.code_folder_path,
            &self.cache_folder_path,
            &self.logs_folder_path,
            &self.home_folder_path,
        ] {
            log::info!("creating directory: {}", dir.display());
            std::fs::create_dir_all(dir).map_err(|e| {
                log::error!("failed to create directory {}: {}", dir.display(), e);
                e
            })?;
        }

        log::info!(
            "creating project files, entrypoint: {}",
            self.code_files.entrypoint
        );

        for (path, content) in self.code_files.files.iter() {
            let file_path = self.code_folder_path.join(path);
            log::info!("writing file: {}", file_path.display());
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    log::error!(
                        "failed to create parent directory {}: {}",
                        parent.display(),
                        e
                    );
                    e
                })?;
            }
            std::fs::write(&file_path, content).map_err(|e| {
                log::error!("failed to write file {}: {}", file_path.display(), e);
                e
            })?;
        }

        log::info!(
            "creating log file if not exists: {}",
            self.log_file_path.display()
        );
        if !self.log_file_path.exists() {
            std::fs::write(&self.log_file_path, "").map_err(|e| {
                log::error!("failed to create log file: {}", e);
                e
            })?;
        }

        if pristine_cache.unwrap_or(false) {
            std::fs::remove_dir_all(&self.cache_folder_path)?;
            std::fs::create_dir(&self.cache_folder_path)?;
            log::info!(
                "cleared cache directory: {}",
                self.cache_folder_path.display()
            );
        }

        Ok(())
    }

    pub fn append_log(&self, log: &str) -> anyhow::Result<()> {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let log_line = format!(
            "{},{},{},{},{}\n",
            timestamp, self.context.context_id, self.context.execution_id, self.code_id, log,
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true) // Create the file if it doesn't exist
            .open(self.log_file_path.clone())
            .map_err(|e| {
                log::error!("failed to open log file: {}", e);
                e
            })?;
        file.write_all(log_line.as_bytes())?;
        Ok(())
    }

    pub fn relative_to_root(&self, path: PathBuf) -> String {
        log::info!(
            "getting relative path from {} to {}",
            self.root_folder_path.display(),
            path.display()
        );
        let path = match path.strip_prefix(&self.root_folder_path) {
            Ok(p) => p,
            Err(e) => {
                log::error!("failed to strip prefix: {}", e);
                return String::new();
            }
        };
        log::debug!("relative path: {:?}", path);
        let result = path.to_path_buf().as_normalized_string();
        log::debug!("normalized path: {}", result);
        result
    }

    pub fn relative_to_global_cache(&self, path: PathBuf) -> String {
        let path = path.strip_prefix(&self.global_cache_folder_path).unwrap();
        path.to_path_buf().as_normalized_string()
    }
}

#[cfg(test)]
#[path = "execution_storage.test.rs"]
mod tests;
