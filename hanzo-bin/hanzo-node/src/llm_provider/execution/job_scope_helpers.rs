use crate::llm_provider::job_manager::JobManager;
use hanzo_fs::hanzo_file_manager::HanzoFileManager;
use hanzo_messages::schemas::hanzo_fs::HanzoFileChunkCollection;
use hanzo_messages::hanzo_utils::job_scope::MinimalJobScope;
use hanzo_messages::hanzo_utils::hanzo_logging::{hanzo_log, HanzoLogLevel, HanzoLogOption};
use hanzo_messages::hanzo_utils::hanzo_path::HanzoPath;
use hanzo_db_sqlite::errors::SqliteManagerError;
use hanzo_db_sqlite::SqliteManager;
use std::collections::HashMap;
use std::result::Result::Ok;

impl JobManager {
    /// Retrieves all resources in the given job scope and returns them as a vector of HanzoFileChunkCollection.
    pub async fn retrieve_all_resources_in_job_scope(
        scope: &MinimalJobScope,
        sqlite_manager: &SqliteManager,
    ) -> Result<Vec<HanzoFileChunkCollection>, SqliteManagerError> {
        let mut collections = Vec::new();

        // Retrieve each file in the job scope
        for path in &scope.vector_fs_items {
            if let Some(collection) = JobManager::retrieve_file_chunks(path, sqlite_manager).await? {
                collections.push(collection);
            }
        }

        // Retrieve files inside vector_fs_folders
        for folder in &scope.vector_fs_folders {
            let files = match HanzoFileManager::list_directory_contents(folder.clone(), sqlite_manager) {
                Ok(files) => files,
                Err(e) => {
                    hanzo_log(
                        HanzoLogOption::JobExecution,
                        HanzoLogLevel::Error,
                        &format!("Error listing directory contents: {:?}", e),
                    );
                    return Err(SqliteManagerError::SomeError(format!("HanzoFsError: {:?}", e)));
                }
            };

            for file_info in files {
                if !file_info.is_directory && file_info.has_embeddings {
                    let file_path = HanzoPath::from_string(file_info.path);
                    if let Some(collection) = JobManager::retrieve_file_chunks(&file_path, sqlite_manager).await? {
                        collections.push(collection);
                    }
                }
            }
        }

        Ok(collections)
    }

    /// Static function to retrieve file chunks for a given path.
    pub async fn retrieve_file_chunks(
        path: &HanzoPath,
        sqlite_manager: &SqliteManager,
    ) -> Result<Option<HanzoFileChunkCollection>, SqliteManagerError> {
        match sqlite_manager.get_parsed_file_by_hanzo_path(path) {
            Ok(Some(parsed_file)) if parsed_file.embedding_model_used.is_some() => {
                let chunks = sqlite_manager.get_chunks_for_parsed_file(parsed_file.id.unwrap())?;
                let mut paths_map = HashMap::new();
                paths_map.insert(parsed_file.id.unwrap(), path.clone());
                Ok(Some(HanzoFileChunkCollection {
                    chunks,
                    paths: Some(paths_map),
                }))
            }
            Ok(Some(_)) => {
                hanzo_log(
                    HanzoLogOption::JobExecution,
                    HanzoLogLevel::Info,
                    &format!("File has no embeddings: {}", path),
                );
                Ok(None)
            }
            Ok(None) => {
                hanzo_log(
                    HanzoLogOption::JobExecution,
                    HanzoLogLevel::Error,
                    &format!("File not found in database: {}", path),
                );
                Ok(None)
            }
            Err(e) => {
                hanzo_log(
                    HanzoLogOption::JobExecution,
                    HanzoLogLevel::Error,
                    &format!("Error retrieving file from database: {} with error: {:?}", path, e),
                );
                Err(e)
            }
        }
    }
}

// TODO: implement tests under a cfg.
