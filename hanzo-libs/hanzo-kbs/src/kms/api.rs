//! HTTP API for KMS operations

use serde::{Deserialize, Serialize};
use warp::{Filter, Rejection, Reply};
use std::sync::Arc;

use crate::kms::KeyManagementService;
use crate::types::{KeyId, AgentDek, TenantKek};

/// API request/response types
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTenantKekRequest {
    pub tenant_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTenantKekResponse {
    pub kek: TenantKek,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateAgentDekRequest {
    pub agent_id: String,
    pub tenant_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateAgentDekResponse {
    pub dek: AgentDek,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WrapKeyRequest {
    pub key_data_base64: String,
    pub parent_key_id: KeyId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WrapKeyResponse {
    pub wrapped_key_base64: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RotateKeyRequest {
    pub key_id: KeyId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RotateKeyResponse {
    pub new_key_id: KeyId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DestroyKeyRequest {
    pub key_id: KeyId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetAuditLogsRequest {
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: chrono::DateTime<chrono::Utc>,
    pub filter: Option<crate::kms::AuditFilter>,
}

/// Create all KMS API routes
pub fn kms_routes<K: KeyManagementService + Clone + Send + Sync + 'static>(
    kms: Arc<K>,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    let create_tenant_kek = warp::path!("keys" / "tenant")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_kms(kms.clone()))
        .and_then(handle_create_tenant_kek);
    
    let create_agent_dek = warp::path!("keys" / "agent")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_kms(kms.clone()))
        .and_then(handle_create_agent_dek);
    
    let wrap_key = warp::path!("wrap")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_kms(kms.clone()))
        .and_then(handle_wrap_key);
    
    let rotate_key = warp::path!("rotate")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_kms(kms.clone()))
        .and_then(handle_rotate_key);
    
    let destroy_key = warp::path!("destroy")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_kms(kms.clone()))
        .and_then(handle_destroy_key);
    
    let get_audit_logs = warp::path!("audit")
        .and(warp::get())
        .and(warp::query())
        .and(with_kms(kms))
        .and_then(handle_get_audit_logs);
    
    create_tenant_kek
        .or(create_agent_dek)
        .or(wrap_key)
        .or(rotate_key)
        .or(destroy_key)
        .or(get_audit_logs)
}

fn with_kms<K: KeyManagementService + Clone + Send>(
    kms: Arc<K>,
) -> impl Filter<Extract = (Arc<K>,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || kms.clone())
}

async fn handle_create_tenant_kek<K: KeyManagementService>(
    req: CreateTenantKekRequest,
    kms: Arc<K>,
) -> Result<impl Reply, Rejection> {
    match kms.create_tenant_kek(&req.tenant_id).await {
        Ok(kek) => Ok(warp::reply::json(&CreateTenantKekResponse { kek })),
        Err(e) => {
            log::error!("Failed to create tenant KEK: {}", e);
            Err(warp::reject::reject())
        }
    }
}

async fn handle_create_agent_dek<K: KeyManagementService>(
    req: CreateAgentDekRequest,
    kms: Arc<K>,
) -> Result<impl Reply, Rejection> {
    match kms.create_agent_dek(&req.agent_id, &req.tenant_id).await {
        Ok(dek) => Ok(warp::reply::json(&CreateAgentDekResponse { dek })),
        Err(e) => {
            log::error!("Failed to create agent DEK: {}", e);
            Err(warp::reject::reject())
        }
    }
}

async fn handle_wrap_key<K: KeyManagementService>(
    req: WrapKeyRequest,
    kms: Arc<K>,
) -> Result<impl Reply, Rejection> {
    let key_data = base64::decode(&req.key_data_base64)
        .map_err(|_| warp::reject::reject())?;
    
    match kms.wrap_key(&key_data, &req.parent_key_id).await {
        Ok(wrapped) => Ok(warp::reply::json(&WrapKeyResponse {
            wrapped_key_base64: base64::encode(&wrapped),
        })),
        Err(e) => {
            log::error!("Failed to wrap key: {}", e);
            Err(warp::reject::reject())
        }
    }
}

async fn handle_rotate_key<K: KeyManagementService>(
    req: RotateKeyRequest,
    kms: Arc<K>,
) -> Result<impl Reply, Rejection> {
    match kms.rotate_key(&req.key_id).await {
        Ok(new_key_id) => Ok(warp::reply::json(&RotateKeyResponse { new_key_id })),
        Err(e) => {
            log::error!("Failed to rotate key: {}", e);
            Err(warp::reject::reject())
        }
    }
}

async fn handle_destroy_key<K: KeyManagementService>(
    req: DestroyKeyRequest,
    kms: Arc<K>,
) -> Result<impl Reply, Rejection> {
    match kms.destroy_key(&req.key_id).await {
        Ok(()) => Ok(warp::reply::with_status("", warp::http::StatusCode::NO_CONTENT)),
        Err(e) => {
            log::error!("Failed to destroy key: {}", e);
            Err(warp::reject::reject())
        }
    }
}

async fn handle_get_audit_logs<K: KeyManagementService>(
    req: GetAuditLogsRequest,
    kms: Arc<K>,
) -> Result<impl Reply, Rejection> {
    match kms.get_audit_logs(req.start_time, req.end_time, req.filter).await {
        Ok(logs) => Ok(warp::reply::json(&logs)),
        Err(e) => {
            log::error!("Failed to get audit logs: {}", e);
            Err(warp::reject::reject())
        }
    }
}