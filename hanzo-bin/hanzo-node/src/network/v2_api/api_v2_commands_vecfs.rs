use std::{collections::HashMap, sync::Arc};

use async_channel::Sender;
use base64::Engine;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde_json::Value;

use hanzo_embed::embedding_generator::EmbeddingGenerator;
use hanzo_fs::{
    hanzo_file_manager::{FileProcessingMode, HanzoFileManager},
    hanzo_fs_error::HanzoFsError,
};
use hanzo_http_api::node_api_router::APIError;
use hanzo_messages::{
    schemas::hanzo_fs::HanzoFileChunkCollection,
    hanzo_message::hanzo_message_schemas::{
        APIVecFsCopyFolder, APIVecFsCopyItem, APIVecFsCreateFolder, APIVecFsDeleteFolder, APIVecFsDeleteItem,
        APIVecFsMoveFolder, APIVecFsMoveItem, APIVecFsRetrievePathSimplifiedJson, APIVecFsRetrieveSourceFile,
        APIVecFsSearchItems,
    },
    hanzo_utils::hanzo_path::HanzoPath,
};
use hanzo_db_sqlite::SqliteManager;
use tokio::sync::Mutex;

use crate::{
    managers::IdentityManager,
    network::{node_error::NodeError, Node},
};

impl Node {
    pub async fn v2_api_vec_fs_retrieve_path_simplified_json(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsRetrievePathSimplifiedJson,
        bearer: String,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        let vr_path = HanzoPath::from_string(input_payload.path);

        let depth = input_payload.depth.unwrap_or(1);

        // Use list_directory_contents_with_depth to get directory contents with depth 1
        let directory_contents = HanzoFileManager::list_directory_contents_with_depth(vr_path, &db, depth);

        if let Err(e) = directory_contents {
            let api_error = APIError {
                code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                error: "Internal Server Error".to_string(),
                message: format!("Failed to retrieve directory contents: {}", e),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        // Convert directory contents to JSON
        let json_contents = serde_json::to_value(directory_contents.unwrap()).map_err(|e| NodeError::from(e))?;

        // Send the directory contents as a response
        let _ = res.send(Ok(json_contents)).await.map_err(|_| ());
        Ok(())
    }

    pub async fn v2_create_folder(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsCreateFolder,
        bearer: String,
        res: Sender<Result<String, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Check if the base path exists
        let base_path = HanzoPath::from_string(input_payload.path.clone());
        if !base_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Base path does not exist: {}", input_payload.path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        // Create the full path by appending folder_name to the path
        let full_path_str = if input_payload.path == "/" {
            format!("/{}", input_payload.folder_name)
        } else {
            format!("{}/{}", input_payload.path, input_payload.folder_name)
        };
        let full_path = HanzoPath::from_string(full_path_str);

        // Check if the full path already exists
        if full_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!(
                    "Path already exists: {}/{}",
                    input_payload.path, input_payload.folder_name
                ),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        // Create the folder using HanzoFileManager
        match HanzoFileManager::create_folder(full_path) {
            Ok(_) => {
                let _ = res.send(Ok("Folder created successfully".to_string())).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to create folder: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_move_item(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsMoveItem,
        bearer: String,
        res: Sender<Result<String, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Convert origin and destination paths
        let origin_path = HanzoPath::from_string(input_payload.origin_path.clone());
        if !origin_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Origin path does not exist: {}", input_payload.origin_path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        let destination_path = HanzoPath::from_string(input_payload.destination_path.clone());
        if destination_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Destination path already exists: {}", input_payload.destination_path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        // Move the file using HanzoFileManager
        match HanzoFileManager::move_file(origin_path, destination_path, &db) {
            Ok(_) => {
                let success_message = format!("Item moved successfully to {}", input_payload.destination_path);
                let _ = res.send(Ok(success_message)).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to move item: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_copy_item(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsCopyItem,
        bearer: String,
        res: Sender<Result<String, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Convert origin and destination paths
        let origin_path = HanzoPath::from_string(input_payload.origin_path.clone());
        if !origin_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Origin path does not exist: {}", input_payload.origin_path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        let destination_path = HanzoPath::from_string(input_payload.destination_path.clone());

        // Copy the file using HanzoFileManager
        match HanzoFileManager::copy_file(origin_path, destination_path) {
            Ok(_) => {
                let success_message = format!("Item copied successfully to {}", input_payload.destination_path);
                let _ = res.send(Ok(success_message)).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to copy item: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_move_folder(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsMoveFolder,
        bearer: String,
        res: Sender<Result<String, APIError>>,
    ) -> Result<(), NodeError> {
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Convert origin and destination paths
        let origin_path = HanzoPath::from_string(input_payload.origin_path.clone());
        if !origin_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Origin path does not exist: {}", input_payload.origin_path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        let destination_path = HanzoPath::from_string(input_payload.destination_path.clone());
        if destination_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Destination path already exists: {}", input_payload.destination_path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        // Move the folder using HanzoFileManager
        match HanzoFileManager::move_folder(origin_path, destination_path, &db) {
            Ok(_) => {
                let success_message = format!("Folder moved successfully to {}", input_payload.destination_path);
                let _ = res.send(Ok(success_message)).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to move folder: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_copy_folder(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        _input_payload: APIVecFsCopyFolder,
        bearer: String,
        res: Sender<Result<String, APIError>>,
    ) -> Result<(), NodeError> {
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        unimplemented!();

        // let requester_name = match identity_manager.lock().await.get_main_identity() {
        //     Some(Identity::Standard(std_identity)) => std_identity.clone().full_identity_name,
        //     _ => {
        //         let api_error = APIError {
        //             code: StatusCode::BAD_REQUEST.as_u16(),
        //             error: "Bad Request".to_string(),
        //             message: "Wrong identity type. Expected Standard identity.".to_string(),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         return Ok(());
        //     }
        // };

        // let origin_path = match HanzoPath::from_string(&input_payload.origin_path) {
        //     Ok(path) => path,
        //     Err(e) => {
        //         let api_error = APIError {
        //             code: StatusCode::BAD_REQUEST.as_u16(),
        //             error: "Bad Request".to_string(),
        //             message: format!("Failed to convert origin path to VRPath: {}", e),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         return Ok(());
        //     }
        // };

        // let destination_path = match HanzoPath::from_string(&input_payload.destination_path) {
        //     Ok(path) => path,
        //     Err(e) => {
        //         let api_error = APIError {
        //             code: StatusCode::BAD_REQUEST.as_u16(),
        //             error: "Bad Request".to_string(),
        //             message: format!("Failed to convert destination path to VRPath: {}", e),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         return Ok(());
        //     }
        // };

        // let writer = match vector_fs
        //     .new_writer(requester_name.clone(), origin_path, requester_name.clone())
        //     .await
        // {
        //     Ok(writer) => writer,
        //     Err(e) => {
        //         let api_error = APIError {
        //             code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        //             error: "Internal Server Error".to_string(),
        //             message: format!("Failed to create writer: {}", e),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         return Ok(());
        //     }
        // };

        // match vector_fs.copy_folder(&writer, destination_path).await {
        //     Ok(_) => {
        //         let success_message = format!("Folder copied successfully to {}", input_payload.destination_path);
        //         let _ = res.send(Ok(success_message)).await.map_err(|_| ());
        //         Ok(())
        //     }
        //     Err(e) => {
        //         let api_error = APIError {
        //             code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        //             error: "Internal Server Error".to_string(),
        //             message: format!("Failed to copy folder: {}", e),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         Ok(())
        //     }
        // }
    }

    pub async fn v2_delete_folder(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsDeleteFolder,
        bearer: String,
        res: Sender<Result<String, APIError>>,
    ) -> Result<(), NodeError> {
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Convert the path to HanzoPath
        let folder_path = HanzoPath::from_string(input_payload.path.clone());
        if !folder_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Folder path does not exist: {}", input_payload.path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        // Delete the folder using HanzoFileManager
        match HanzoFileManager::remove_folder(folder_path, &db) {
            Ok(_) => {
                let success_message = format!("Folder successfully deleted: {}", input_payload.path);
                let _ = res.send(Ok(success_message)).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to delete folder: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_delete_item(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsDeleteItem,
        bearer: String,
        res: Sender<Result<String, APIError>>,
    ) -> Result<(), NodeError> {
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Convert the path to HanzoPath
        let item_path = HanzoPath::from_string(input_payload.path.clone());
        if !item_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("File path does not exist: {}", input_payload.path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        // Ensure the path is a file
        if !item_path.is_file() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Path is not a file: {}", input_payload.path),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        // Delete the file using HanzoFileManager
        match HanzoFileManager::remove_file(item_path, &db) {
            Ok(_) => {
                let success_message = format!("File successfully deleted: {}", input_payload.path);
                let _ = res.send(Ok(success_message)).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to delete file: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_search_items(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsSearchItems,
        embedding_generator: Arc<dyn EmbeddingGenerator>,
        bearer: String,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Determine the search path
        let search_path_str = input_payload.path.as_deref().unwrap_or("/").to_string();
        let search_path = HanzoPath::from_string(search_path_str.clone());

        // Check if the search path exists
        if !search_path.exists() {
            let api_error = APIError {
                code: StatusCode::BAD_REQUEST.as_u16(),
                error: "Bad Request".to_string(),
                message: format!("Search path does not exist: {}", search_path_str),
            };
            let _ = res.send(Err(api_error)).await;
            return Ok(());
        }

        let mut parsed_file_ids = Vec::new();
        let mut paths_map = HashMap::new();

        let query_embedding = match embedding_generator
            .generate_embedding_default(&input_payload.search)
            .await
        {
            Ok(embedding) => embedding,
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to generate query embedding: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
                return Ok(());
            }
        };

        // Retrieve files in the specified path
        let search_prefix = search_path.relative_path();
        match db.get_parsed_files_by_prefix(&search_prefix) {
            Ok(parsed_files) => {
                for parsed_file in parsed_files {
                    parsed_file_ids.push(parsed_file.id.unwrap());
                    paths_map.insert(
                        parsed_file.id.unwrap(),
                        HanzoPath::from_string(parsed_file.relative_path.clone()),
                    );
                }
            }
            Err(e) => {
                // Handle the error, e.g., log it or send an error response
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to get parsed files: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
                return Ok(());
            }
        }

        // Perform a vector search on all parsed files
        let search_results = match db.search_chunks(
            &parsed_file_ids,
            query_embedding,
            input_payload.max_results.unwrap_or(100) as usize,
        ) {
            Ok(results) => results,
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to search chunks: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
                return Ok(());
            }
        };

        let results = HanzoFileChunkCollection {
            chunks: search_results.into_iter().map(|(chunk, _)| chunk).collect(),
            paths: Some(paths_map),
        };

        // Convert results to JSON
        let json_results = match serde_json::to_value(results) {
            Ok(json_results) => json_results,
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to convert results to JSON: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
                return Ok(());
            }
        };

        // Send the search results as a response
        let _ = res.send(Ok(json_results)).await.map_err(|_| ());
        Ok(())
    }

    /// Run a vector search against THIS node's store and return ranked chunks WITH score
    /// (lower = closer). Shared by the cluster-internal search endpoint and federated fan-out.
    /// No bearer: this is a cluster-internal primitive, gated by cluster mode at the callers.
    async fn cluster_local_search_results(
        db: &Arc<SqliteManager>,
        embedding_generator: &Arc<dyn EmbeddingGenerator>,
        node_name: &str,
        query: &str,
        max_results: usize,
        path: Option<&str>,
    ) -> Vec<Value> {
        if query.is_empty() {
            return Vec::new();
        }
        let query_embedding = match embedding_generator.generate_embedding_default(query).await {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let search_path = HanzoPath::from_string(path.unwrap_or("/").to_string());
        let search_prefix = search_path.relative_path();
        let mut file_ids: Vec<i64> = Vec::new();
        let mut paths_map: HashMap<i64, String> = HashMap::new();
        if let Ok(files) = db.get_parsed_files_by_prefix(&search_prefix) {
            for f in files {
                if let Some(id) = f.id {
                    file_ids.push(id);
                    paths_map.insert(id, f.relative_path.clone());
                }
            }
        }
        let results = db.search_chunks(&file_ids, query_embedding, max_results).unwrap_or_default();
        results
            .into_iter()
            .map(|(chunk, dist)| {
                serde_json::json!({
                    "content": chunk.content,
                    "score": dist,
                    "position": chunk.position,
                    "path": paths_map.get(&chunk.parsed_file_id),
                    "node_name": node_name,
                })
            })
            .collect()
    }

    /// `POST /v1/node/cluster/search_local` — cluster-internal local RAG search (no bearer);
    /// peers call this during federated fan-out. Returns this node's ranked chunks.
    pub async fn v2_api_cluster_search_local(
        db: Arc<SqliteManager>,
        embedding_generator: Arc<dyn EmbeddingGenerator>,
        node_name: String,
        payload: Value,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        if !Self::cluster_enabled() {
            let _ = res
                .send(Ok(serde_json::json!({ "error": "cluster mode is disabled (set HANZO_CLUSTER_MODE=1)" })))
                .await;
            return Ok(());
        }
        let query = payload
            .get("query")
            .or_else(|| payload.get("search"))
            .and_then(|q| q.as_str())
            .unwrap_or("");
        let path = payload.get("path").and_then(|p| p.as_str());
        let max_results = payload.get("max_results").and_then(|m| m.as_u64()).unwrap_or(10) as usize;
        let results =
            Self::cluster_local_search_results(&db, &embedding_generator, &node_name, query, max_results, path)
                .await;
        let _ = res
            .send(Ok(serde_json::json!({ "node_name": node_name, "result_count": results.len(), "results": results })))
            .await;
        Ok(())
    }

    /// `POST /v1/node/cluster/search` — federated RAG. Search THIS node + fan out to every
    /// connected peer's /cluster/search_local, then fuse the ranked lists with Reciprocal
    /// Rank Fusion. Returns fused results + which nodes were queried.
    pub async fn v2_api_cluster_search(
        node_name: String,
        cluster_peers: Option<crate::network::libp2p_manager::ClusterPeersHandle>,
        db: Arc<SqliteManager>,
        embedding_generator: Arc<dyn EmbeddingGenerator>,
        payload: Value,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        if !Self::cluster_enabled() {
            let _ = res
                .send(Ok(serde_json::json!({ "error": "cluster mode is disabled (set HANZO_CLUSTER_MODE=1)" })))
                .await;
            return Ok(());
        }
        let query = payload
            .get("query")
            .or_else(|| payload.get("search"))
            .and_then(|q| q.as_str())
            .unwrap_or("")
            .to_string();
        if query.is_empty() {
            let _ = res
                .send(Ok(serde_json::json!({ "error": "request body must include a 'query' field" })))
                .await;
            return Ok(());
        }
        let max_results = payload.get("max_results").and_then(|m| m.as_u64()).unwrap_or(10) as usize;
        let path = payload.get("path").and_then(|p| p.as_str());

        let mut queried: Vec<Value> = Vec::new();
        let mut ranked_lists: Vec<Vec<Value>> = Vec::new();

        // 1) Local results.
        let local =
            Self::cluster_local_search_results(&db, &embedding_generator, &node_name, &query, max_results, path)
                .await;
        queried.push(serde_json::json!({ "node_name": node_name, "location": "local", "result_count": local.len() }));
        ranked_lists.push(local);

        // 2) Fan out to connected peers' cluster.search_local over ZAP (binary, not HTTP).
        let mut peer_targets: Vec<(String, u64, String)> = Vec::new();
        if let Some(cp) = cluster_peers.as_ref() {
            for (_pid, entry) in cp.read().await.iter() {
                if !entry.connected {
                    continue;
                }
                let card = match &entry.card {
                    Some(c) => c,
                    None => continue,
                };
                let pname = card.get("node_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                let zap_port = card.get("zap_port").and_then(|p| p.as_u64());
                if let (Some(ip), Some(zp)) = (Self::extract_ip4(&entry.address), zap_port) {
                    peer_targets.push((ip, zp, pname));
                }
            }
        }
        let fan_payload = serde_json::json!({ "query": query, "max_results": max_results, "path": path });
        for (ip, zap_port, pname) in peer_targets {
            match Self::cluster_zap_call(&ip, zap_port, "cluster.search_local", &fan_payload).await {
                Ok(body) => {
                    let r = body.get("results").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                    queried.push(serde_json::json!({ "node_name": pname, "location": "peer", "result_count": r.len() }));
                    ranked_lists.push(r);
                }
                Err(e) => {
                    queried.push(serde_json::json!({ "node_name": pname, "location": "peer", "error": e }));
                }
            }
        }

        // 3) Reciprocal Rank Fusion across all lists.
        let fused = Self::rrf_fuse(ranked_lists, max_results);
        let _ = res
            .send(Ok(serde_json::json!({ "query": query, "result_count": fused.len(), "results": fused, "queried": queried })))
            .await;
        Ok(())
    }

    /// Reciprocal Rank Fusion of several already-ranked (best-first) result lists. Dedupes
    /// by content, sums 1/(k + rank) with k=60, returns the top `limit` by fused score.
    fn rrf_fuse(lists: Vec<Vec<Value>>, limit: usize) -> Vec<Value> {
        const K: f64 = 60.0;
        let mut scores: HashMap<String, f64> = HashMap::new();
        let mut repr: HashMap<String, Value> = HashMap::new();
        let mut sources: HashMap<String, Vec<Value>> = HashMap::new();
        for list in lists {
            for (rank, item) in list.into_iter().enumerate() {
                let key = item.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                if key.is_empty() {
                    continue;
                }
                *scores.entry(key.clone()).or_insert(0.0) += 1.0 / (K + rank as f64 + 1.0);
                let node = item.get("node_name").cloned().unwrap_or(Value::Null);
                sources.entry(key.clone()).or_default().push(node);
                repr.entry(key.clone()).or_insert(item);
            }
        }
        let mut fused: Vec<(String, f64)> = scores.into_iter().collect();
        fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        fused
            .into_iter()
            .take(limit)
            .filter_map(|(key, score)| {
                let mut item = repr.get(&key).cloned()?;
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("rrf_score".to_string(), serde_json::json!(score));
                    obj.insert("sources".to_string(), serde_json::json!(sources.get(&key).cloned().unwrap_or_default()));
                }
                Some(item)
            })
            .collect()
    }

    // (cluster_forward_search removed — peer federated search now rides ZAP via
    // Self::cluster_zap_call("cluster.search_local"), no HTTP between nodes.)

    pub async fn v2_retrieve_vector_resource(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        _path: String,
        bearer: String,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        unimplemented!();

        // let requester_name = match identity_manager.lock().await.get_main_identity() {
        //     Some(Identity::Standard(std_identity)) => std_identity.clone().full_identity_name,
        //     _ => {
        //         let api_error = APIError {
        //             code: StatusCode::BAD_REQUEST.as_u16(),
        //             error: "Bad Request".to_string(),
        //             message: "Wrong identity type. Expected Standard identity.".to_string(),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         return Ok(());
        //     }
        // };

        // let vr_path = match HanzoPath::from_string(&path) {
        //     Ok(path) => path,
        //     Err(e) => {
        //         let api_error = APIError {
        //             code: StatusCode::BAD_REQUEST.as_u16(),
        //             error: "Bad Request".to_string(),
        //             message: format!("Failed to convert path to VRPath: {}", e),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         return Ok(());
        //     }
        // };

        // let reader = match vector_fs
        //     .new_reader(requester_name.clone(), vr_path, requester_name.clone())
        //     .await
        // {
        //     Ok(reader) => reader,
        //     Err(e) => {
        //         let api_error = APIError {
        //             code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        //             error: "Internal Server Error".to_string(),
        //             message: format!("Failed to create reader: {}", e),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         return Ok(());
        //     }
        // };

        // let result = vector_fs.retrieve_vector_resource(&reader).await;

        // match result {
        //     Ok(result_value) => match result_value.to_json_value() {
        //         Ok(json_value) => {
        //             let _ = res.send(Ok(json_value)).await.map_err(|_| ());
        //             Ok(())
        //         }
        //         Err(e) => {
        //             let api_error = APIError {
        //                 code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        //                 error: "Internal Server Error".to_string(),
        //                 message: format!("Failed to convert result to JSON: {}", e),
        //             };
        //             let _ = res.send(Err(api_error)).await;
        //             Ok(())
        //         }
        //     },
        //     Err(e) => {
        //         let api_error = APIError {
        //             code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        //             error: "Internal Server Error".to_string(),
        //             message: format!("Failed to retrieve vector resource: {}", e),
        //         };
        //         let _ = res.send(Err(api_error)).await;
        //         Ok(())
        //     }
        // }
    }

    pub async fn v2_upload_file_to_folder(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        embedding_generator: Arc<dyn EmbeddingGenerator>,
        bearer: String,
        filename: String,
        file: Vec<u8>,
        path: String,
        _file_datetime: Option<DateTime<Utc>>,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Construct the full path for the file
        let full_path_str = if path == "/" {
            format!("/{}", filename)
        } else {
            format!("{}/{}", path, filename)
        };
        let full_path = HanzoPath::from_string(full_path_str.clone());

        // Save and process the file
        match HanzoFileManager::save_and_process_file(
            full_path.clone(),
            file,
            &db,
            FileProcessingMode::Auto,
            &*embedding_generator,
        )
        .await
        {
            Ok(_) => {
                let success_message = format!("File uploaded and processed successfully: {}", full_path_str);
                let _ = res.send(Ok(serde_json::json!({ "message": success_message }))).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to upload and process file: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_retrieve_file(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        input_payload: APIVecFsRetrieveSourceFile,
        bearer: String,
        res: Sender<Result<String, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Determine which file to return: processed or original
        let use_processed = input_payload.processed_file.unwrap_or(false);
        let vr_path = HanzoPath::from_string(input_payload.path.clone());

        if use_processed {
            // Retrieve the parsed file from the database
            let parsed_file =
                match db.get_parsed_file_by_hanzo_path(&HanzoPath::from_string(input_payload.path.clone())) {
                    Ok(Some(pf)) => pf,
                    Ok(None) => {
                        let api_error = APIError {
                            code: StatusCode::NOT_FOUND.as_u16(),
                            error: "Not Found".to_string(),
                            message: format!("Processed file not found in database: {}", input_payload.path),
                        };
                        let _ = res.send(Err(api_error)).await;
                        return Ok(());
                    }
                    Err(e) => {
                        let api_error = APIError {
                            code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                            error: "Internal Server Error".to_string(),
                            message: format!("Database error: {:?}", e),
                        };
                        let _ = res.send(Err(api_error)).await;
                        return Ok(());
                    }
                };

            // Retrieve all chunks for the parsed file, sort by position, and concatenate
            let mut chunks = match db.get_chunks_for_parsed_file(parsed_file.id.unwrap()) {
                Ok(chunks) => chunks,
                Err(e) => {
                    let api_error = APIError {
                        code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                        error: "Internal Server Error".to_string(),
                        message: format!("Failed to get file chunks: {:?}", e),
                    };
                    let _ = res.send(Err(api_error)).await;
                    return Ok(());
                }
            };
            chunks.sort_by_key(|c| c.position);
            let file_content: String = chunks.into_iter().map(|c| c.content).collect();
            let _ = res.send(Ok(file_content)).await.map_err(|_| ());
            return Ok(());
        } else {
            // Read the file content
            let file_content = match std::fs::read(vr_path.as_path()) {
                Ok(content) => content,
                Err(e) => {
                    let api_error = APIError {
                        code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                        error: "Internal Server Error".to_string(),
                        message: format!("Failed to read file content: {:?}", e),
                    };
                    let _ = res.send(Err(api_error)).await;
                    return Ok(());
                }
            };

            // Encode the file content in base64
            let encoded_file_content = base64::engine::general_purpose::STANDARD.encode(&file_content);

            // Send the encoded file content as a response
            let _ = res.send(Ok(encoded_file_content)).await.map_err(|_| ());
            Ok(())
        }
    }

    pub async fn v2_upload_file_to_job(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        embedding_generator: Arc<dyn EmbeddingGenerator>,
        bearer: String,
        job_id: String,
        filename: String,
        file: Vec<u8>,
        _file_datetime: Option<DateTime<Utc>>,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Save and process the file with the job ID
        match HanzoFileManager::save_and_process_file_with_jobid(
            &job_id,
            filename.clone(),
            file,
            &db,
            FileProcessingMode::Auto,
            &*embedding_generator,
        )
        .await
        {
            Ok(response) => {
                let success_message = format!(
                    "File uploaded and processed successfully for job {}: {}",
                    job_id, filename
                );
                let _ = res
                    .send(Ok(
                        serde_json::json!({ "message": success_message, "filename": response.filename() }),
                    ))
                    .await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to upload and process file for job {}: {:?}", job_id, e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_api_vec_fs_retrieve_files_for_job(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        job_id: String,
        bearer: String,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Retrieve files for the given job_id using HanzoFileManager
        let files_result = HanzoFileManager::get_all_files_and_folders_for_job(&job_id, &db);

        let files = match files_result {
            Ok(files) => files,
            Err(HanzoFsError::Io(io_error)) if io_error.kind() == std::io::ErrorKind::NotFound => {
                // Return an empty JSON array if the error is "No such file or directory"
                let _ = res.send(Ok(serde_json::json!([]))).await.map_err(|_| ());
                return Ok(());
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to retrieve files for job_id {}: {}", job_id, e),
                };
                let _ = res.send(Err(api_error)).await;
                return Ok(());
            }
        };

        // Convert the files information to JSON
        let json_files = serde_json::to_value(files).map_err(|e| NodeError::from(e))?;

        // Send the files information as a response
        let _ = res.send(Ok(json_files)).await.map_err(|_| ());
        Ok(())
    }

    pub async fn v2_api_vec_fs_get_folder_name_for_job(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        job_id: String,
        bearer: String,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Retrieve the folder name for the given job_id
        let folder_name_result = db.get_job_folder_name(&job_id);

        match folder_name_result {
            Ok(folder_name) => {
                let folder_name_json = serde_json::json!({
                    "folder_name": folder_name.relative_path().to_string()
                });
                let _ = res.send(Ok(folder_name_json)).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to retrieve folder name for job_id {}: {}", job_id, e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }

    pub async fn v2_api_search_files_by_name(
        db: Arc<SqliteManager>,
        _identity_manager: Arc<Mutex<IdentityManager>>,
        name: String,
        bearer: String,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        // Validate the bearer token
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        // Get the base path for searching
        let base_path = HanzoPath::from_base_path();

        // Search for files using HanzoFileManager::search_files_by_name_and_content
        match HanzoFileManager::search_files_by_name_and_content(base_path, &name, &db) {
            Ok(files) => {
                let json_files = serde_json::to_value(files).map_err(|e| NodeError::from(e))?;
                let _ = res.send(Ok(json_files)).await;
            }
            Err(e) => {
                let api_error = APIError {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    error: "Internal Server Error".to_string(),
                    message: format!("Failed to search files: {:?}", e),
                };
                let _ = res.send(Err(api_error)).await;
            }
        }

        Ok(())
    }
}
