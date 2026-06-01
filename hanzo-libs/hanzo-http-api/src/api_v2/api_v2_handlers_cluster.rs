use super::api_v2_router::with_sender;
use crate::node_commands::NodeCommand;
use async_channel::Sender;
use serde_json::Value;
use warp::Filter;

/// Hanzo cluster topology routes (opt-in cluster mode — see HANZO_CLUSTER_MODE).
///
/// - `GET  /v1/node/cluster/topology`          -> `{ cluster_mode, this_node, peer_count, peers }`
/// - `GET  /v1/node/cluster/peers`             -> `[ { peer_id, address, connected, card }, ... ]`
/// - `GET  /v1/node/cluster/models`            -> `{ cluster_mode, model_count, models }` (model_id -> serving nodes)
/// - `GET  /v1/node/cluster/route?model=X`     -> routing decision `{ model, decision, target, reason }`
/// - `POST /v1/node/cluster/chat`              -> route an OpenAI chat to a node serving the model (local or peer-forward)
/// - `POST /v1/node/cluster/chat_local`        -> serve a chat ONLY from the local engine (cluster-internal; peers call this)
/// - `GET  /v1/node/cluster/placement?model=X` -> scheduler plan: where to LOAD a model not yet served
/// - `POST /v1/node/cluster/search`            -> federated RAG: local + connected-peer fan-out, RRF-fused
/// - `POST /v1/node/cluster/search_local`      -> local RAG search (cluster-internal; peers call this)
///
/// `topology`/`peers` share the `V2ApiGetClusterTopology` command; `models`/`route`/`placement`
/// are backed by the cluster model index. The engine/RAG endpoints refuse to run unless
/// HANZO_CLUSTER_MODE is set; the `*_local` endpoints are peer-internal (LAN trust domain).
pub fn cluster_routes(
    node_commands_sender: Sender<NodeCommand>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let topology_route = warp::path("cluster")
        .and(warp::path("topology"))
        .and(warp::path::end())
        .and(warp::get())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(get_cluster_topology);

    let peers_route = warp::path("cluster")
        .and(warp::path("peers"))
        .and(warp::path::end())
        .and(warp::get())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(get_cluster_peers);

    let models_route = warp::path("cluster")
        .and(warp::path("models"))
        .and(warp::path::end())
        .and(warp::get())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(get_cluster_models);

    let route_route = warp::path("cluster")
        .and(warp::path("route"))
        .and(warp::path::end())
        .and(warp::get())
        .and(warp::query::<std::collections::HashMap<String, String>>())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(route_cluster_model);

    let chat_route = warp::path("cluster")
        .and(warp::path("chat"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(cluster_chat);

    let chat_local_route = warp::path("cluster")
        .and(warp::path("chat_local"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(cluster_chat_local);

    let placement_route = warp::path("cluster")
        .and(warp::path("placement"))
        .and(warp::path::end())
        .and(warp::get())
        .and(warp::query::<std::collections::HashMap<String, String>>())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(cluster_placement);

    let search_route = warp::path("cluster")
        .and(warp::path("search"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(cluster_search);

    let search_local_route = warp::path("cluster")
        .and(warp::path("search_local"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(cluster_search_local);

    // OpenAI-compatible cluster chat: lets a peer model be used as a normal LLM provider
    // (external_url = http://<node>/v1/node/cluster). Routes the model across the cluster.
    let openai_chat_route = warp::path("cluster")
        .and(warp::path("v1"))
        .and(warp::path("engine"))
        .and(warp::path("chat"))
        .and(warp::path("completions"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(with_sender(node_commands_sender.clone()))
        .and_then(cluster_openai_chat);

    topology_route
        .or(peers_route)
        .or(models_route)
        .or(route_route)
        .or(chat_route)
        .or(chat_local_route)
        .or(placement_route)
        .or(search_route)
        .or(search_local_route)
        .or(openai_chat_route)
}

pub async fn cluster_openai_chat(
    payload: Value,
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiClusterOpenaiChat { payload, res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;
    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn get_cluster_topology(
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiGetClusterTopology { res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn get_cluster_peers(
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiGetClusterTopology { res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(Value::Object(map)) => {
            let peers = map.get("peers").cloned().unwrap_or_else(|| Value::Array(vec![]));
            Ok(warp::reply::json(&peers))
        }
        Ok(other) => Ok(warp::reply::json(&other)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn get_cluster_models(
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiGetClusterModels { res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn route_cluster_model(
    query: std::collections::HashMap<String, String>,
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let model = query.get("model").cloned().unwrap_or_default();
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiRouteClusterModel { model, res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn cluster_chat(
    payload: Value,
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiClusterChat { payload, res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn cluster_chat_local(
    payload: Value,
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiClusterChatLocal { payload, res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn cluster_placement(
    query: std::collections::HashMap<String, String>,
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let model = query.get("model").cloned().unwrap_or_default();
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiClusterPlacement { model, res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn cluster_search(
    payload: Value,
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiClusterSearch { payload, res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}

pub async fn cluster_search_local(
    payload: Value,
    sender: Sender<NodeCommand>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let (res_sender, res_receiver) = async_channel::bounded(1);
    sender
        .send(NodeCommand::V2ApiClusterSearchLocal { payload, res: res_sender })
        .await
        .map_err(|_| warp::reject::reject())?;

    let result = res_receiver.recv().await.map_err(|_| warp::reject::reject())?;
    match result {
        Ok(response) => Ok(warp::reply::json(&response)),
        Err(error) => Err(warp::reject::custom(error)),
    }
}
