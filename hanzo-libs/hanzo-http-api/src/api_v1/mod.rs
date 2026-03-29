use async_channel::Sender;
use reqwest::StatusCode;
use serde_json::{json, Value};
use warp::Filter;

use crate::node_commands::NodeCommand;

/// Warp filter that injects the NodeCommand sender into handler arguments.
fn with_sender(
    sender: Sender<NodeCommand>,
) -> impl Filter<Extract = (Sender<NodeCommand>,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || sender.clone())
}

/// Top-level v1 routes. Mounted under `/v1` by the router.
pub fn v1_routes(
    node_commands_sender: Sender<NodeCommand>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let chat_completions = warp::path!("chat" / "completions")
        .and(warp::post())
        .and(with_sender(node_commands_sender.clone()))
        .and(warp::header::<String>("authorization"))
        .and(warp::body::json())
        .and_then(chat_completions_handler);

    let anthropic_messages = warp::path("messages")
        .and(warp::post())
        .and(with_sender(node_commands_sender.clone()))
        .and(warp::header::<String>("authorization"))
        .and(warp::body::json())
        .and_then(anthropic_messages_handler);

    let list_models = warp::path("models")
        .and(warp::get())
        .and(with_sender(node_commands_sender.clone()))
        .and(warp::header::<String>("authorization"))
        .and_then(list_models_handler);

    chat_completions.or(anthropic_messages).or(list_models)
}

/// POST /v1/chat/completions  --  OpenAI Chat Completions API
async fn chat_completions_handler(
    node_commands_sender: Sender<NodeCommand>,
    authorization: String,
    body: Value,
) -> Result<impl warp::Reply, warp::Rejection> {
    let bearer = authorization
        .strip_prefix("Bearer ")
        .unwrap_or("")
        .to_string();
    let (res_sender, res_receiver) = async_channel::bounded(1);
    node_commands_sender
        .send(NodeCommand::V1ChatCompletion {
            bearer,
            body,
            res: res_sender,
        })
        .await
        .map_err(|_| warp::reject::reject())?;
    let result = res_receiver
        .recv()
        .await
        .map_err(|_| warp::reject::reject())?;

    match result {
        Ok(data) => Ok(warp::reply::with_status(
            warp::reply::json(&data),
            StatusCode::OK,
        )),
        Err(error) => Ok(warp::reply::with_status(
            warp::reply::json(&json!({
                "error": {
                    "message": error.message,
                    "type": "invalid_request_error",
                    "code": error.error,
                }
            })),
            StatusCode::from_u16(error.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        )),
    }
}

/// POST /v1/messages  --  Anthropic Messages API
async fn anthropic_messages_handler(
    node_commands_sender: Sender<NodeCommand>,
    authorization: String,
    body: Value,
) -> Result<impl warp::Reply, warp::Rejection> {
    let bearer = authorization
        .strip_prefix("Bearer ")
        .unwrap_or("")
        .to_string();
    let (res_sender, res_receiver) = async_channel::bounded(1);
    node_commands_sender
        .send(NodeCommand::V1AnthropicMessages {
            bearer,
            body,
            res: res_sender,
        })
        .await
        .map_err(|_| warp::reject::reject())?;
    let result = res_receiver
        .recv()
        .await
        .map_err(|_| warp::reject::reject())?;

    match result {
        Ok(data) => Ok(warp::reply::with_status(
            warp::reply::json(&data),
            StatusCode::OK,
        )),
        Err(error) => Ok(warp::reply::with_status(
            warp::reply::json(&json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": error.message,
                }
            })),
            StatusCode::from_u16(error.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        )),
    }
}

/// GET /v1/models  --  List available models
async fn list_models_handler(
    node_commands_sender: Sender<NodeCommand>,
    authorization: String,
) -> Result<impl warp::Reply, warp::Rejection> {
    let bearer = authorization
        .strip_prefix("Bearer ")
        .unwrap_or("")
        .to_string();
    let (res_sender, res_receiver) = async_channel::bounded(1);
    node_commands_sender
        .send(NodeCommand::V1ListModels {
            bearer,
            res: res_sender,
        })
        .await
        .map_err(|_| warp::reject::reject())?;
    let result = res_receiver
        .recv()
        .await
        .map_err(|_| warp::reject::reject())?;

    match result {
        Ok(data) => Ok(warp::reply::with_status(
            warp::reply::json(&data),
            StatusCode::OK,
        )),
        Err(error) => Ok(warp::reply::with_status(
            warp::reply::json(&json!({
                "error": {
                    "message": error.message,
                    "type": "invalid_request_error",
                    "code": error.error,
                }
            })),
            StatusCode::from_u16(error.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        )),
    }
}
