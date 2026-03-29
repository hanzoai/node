use crate::network::{node_error::NodeError, Node};
use async_channel::Sender;
use log::error;
use reqwest::StatusCode;
use serde_json::{json, Value};
use hanzo_http_api::node_api_router::APIError;
use hanzo_messages::schemas::llm_providers::serialized_llm_provider::LLMProviderInterface;
use hanzo_db_sqlite::SqliteManager;
use std::sync::Arc;

impl Node {
    /// POST /v1/chat/completions — proxy to the matching LLM provider's OpenAI-compatible endpoint.
    pub async fn v1_handle_chat_completion(
        db: Arc<SqliteManager>,
        bearer: String,
        body: Value,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        let requested_model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let providers = match db.get_all_llm_providers() {
            Ok(p) => p,
            Err(e) => {
                let _ = res
                    .send(Err(APIError {
                        code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                        error: "InternalError".to_string(),
                        message: format!("Failed to fetch LLM providers: {}", e),
                    }))
                    .await;
                return Ok(());
            }
        };

        // Find a provider whose model_type matches the requested model.
        // If no model is specified or no exact match, use the first provider that has an external_url.
        let provider = if requested_model.is_empty() {
            providers.iter().find(|p| p.external_url.is_some())
        } else {
            providers
                .iter()
                .find(|p| p.get_model_string() == requested_model && p.external_url.is_some())
                .or_else(|| providers.iter().find(|p| p.external_url.is_some()))
        };

        let provider = match provider {
            Some(p) => p,
            None => {
                let _ = res
                    .send(Err(APIError {
                        code: StatusCode::NOT_FOUND.as_u16(),
                        error: "NotFound".to_string(),
                        message: format!("No LLM provider found for model '{}'", requested_model),
                    }))
                    .await;
                return Ok(());
            }
        };

        let base_url = provider.external_url.clone().unwrap_or_default();
        let base_url = base_url.trim_end_matches('/');
        let url = format!("{}/v1/chat/completions", base_url);

        let client = reqwest::Client::new();
        let mut request = client.post(&url).json(&body);
        if let Some(ref api_key) = provider.api_key {
            if !api_key.is_empty() {
                request = request.bearer_auth(api_key);
            }
        }

        match request.send().await {
            Ok(response) => match response.json::<Value>().await {
                Ok(data) => {
                    let _ = res.send(Ok(data)).await;
                }
                Err(e) => {
                    error!("v1_handle_chat_completion: failed to parse upstream response: {}", e);
                    let _ = res
                        .send(Err(APIError {
                            code: StatusCode::BAD_GATEWAY.as_u16(),
                            error: "BadGateway".to_string(),
                            message: format!("Failed to parse upstream response: {}", e),
                        }))
                        .await;
                }
            },
            Err(e) => {
                error!("v1_handle_chat_completion: upstream request failed: {}", e);
                let _ = res
                    .send(Err(APIError {
                        code: StatusCode::BAD_GATEWAY.as_u16(),
                        error: "BadGateway".to_string(),
                        message: format!("Upstream request failed: {}", e),
                    }))
                    .await;
            }
        }

        Ok(())
    }

    /// POST /v1/messages — proxy to the matching LLM provider's Anthropic-compatible endpoint.
    pub async fn v1_handle_anthropic_messages(
        db: Arc<SqliteManager>,
        bearer: String,
        body: Value,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        let requested_model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let providers = match db.get_all_llm_providers() {
            Ok(p) => p,
            Err(e) => {
                let _ = res
                    .send(Err(APIError {
                        code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                        error: "InternalError".to_string(),
                        message: format!("Failed to fetch LLM providers: {}", e),
                    }))
                    .await;
                return Ok(());
            }
        };

        // Prefer a Claude-type provider, then fall back to any provider with an external_url.
        let provider = if requested_model.is_empty() {
            providers
                .iter()
                .find(|p| matches!(p.model, LLMProviderInterface::Claude(_)) && p.external_url.is_some())
                .or_else(|| providers.iter().find(|p| p.external_url.is_some()))
        } else {
            providers
                .iter()
                .find(|p| p.get_model_string() == requested_model && p.external_url.is_some())
                .or_else(|| {
                    providers
                        .iter()
                        .find(|p| matches!(p.model, LLMProviderInterface::Claude(_)) && p.external_url.is_some())
                })
                .or_else(|| providers.iter().find(|p| p.external_url.is_some()))
        };

        let provider = match provider {
            Some(p) => p,
            None => {
                let _ = res
                    .send(Err(APIError {
                        code: StatusCode::NOT_FOUND.as_u16(),
                        error: "NotFound".to_string(),
                        message: format!("No LLM provider found for model '{}'", requested_model),
                    }))
                    .await;
                return Ok(());
            }
        };

        let base_url = provider.external_url.clone().unwrap_or_default();
        let base_url = base_url.trim_end_matches('/');
        let url = format!("{}/v1/messages", base_url);

        let client = reqwest::Client::new();
        let mut request = client.post(&url).json(&body);
        if let Some(ref api_key) = provider.api_key {
            if !api_key.is_empty() {
                request = request.header("x-api-key", api_key);
                request = request.header("anthropic-version", "2023-06-01");
            }
        }

        match request.send().await {
            Ok(response) => match response.json::<Value>().await {
                Ok(data) => {
                    let _ = res.send(Ok(data)).await;
                }
                Err(e) => {
                    error!("v1_handle_anthropic_messages: failed to parse upstream response: {}", e);
                    let _ = res
                        .send(Err(APIError {
                            code: StatusCode::BAD_GATEWAY.as_u16(),
                            error: "BadGateway".to_string(),
                            message: format!("Failed to parse upstream response: {}", e),
                        }))
                        .await;
                }
            },
            Err(e) => {
                error!("v1_handle_anthropic_messages: upstream request failed: {}", e);
                let _ = res
                    .send(Err(APIError {
                        code: StatusCode::BAD_GATEWAY.as_u16(),
                        error: "BadGateway".to_string(),
                        message: format!("Upstream request failed: {}", e),
                    }))
                    .await;
            }
        }

        Ok(())
    }

    /// GET /v1/models — list all configured models in OpenAI-compatible format.
    pub async fn v1_handle_list_models(
        db: Arc<SqliteManager>,
        bearer: String,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        if Self::validate_bearer_token(&bearer, db.clone(), &res).await.is_err() {
            return Ok(());
        }

        let providers = match db.get_all_llm_providers() {
            Ok(p) => p,
            Err(e) => {
                let _ = res
                    .send(Err(APIError {
                        code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                        error: "InternalError".to_string(),
                        message: format!("Failed to fetch LLM providers: {}", e),
                    }))
                    .await;
                return Ok(());
            }
        };

        let models: Vec<Value> = providers
            .iter()
            .map(|p| {
                json!({
                    "id": p.get_model_string(),
                    "object": "model",
                    "created": 0,
                    "owned_by": format!("hanzo-node:{}", p.get_provider_string()),
                })
            })
            .collect();

        let _ = res
            .send(Ok(json!({
                "object": "list",
                "data": models,
            })))
            .await;

        Ok(())
    }
}
