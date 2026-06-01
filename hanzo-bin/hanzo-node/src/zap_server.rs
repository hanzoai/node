//! Native ZAP listener for Hanzo Node.
//!
//! Accepts ZAP binary protocol connections on the P2P port (default 3692)
//! and dispatches cloud service requests to the node's command channel.
//!
//! Forwarding modes (checked once at startup):
//!   1. ZAP:  HANZO_ENGINE_ZAP_URL is set  → native binary protocol to engine
//!   2. HTTP: fallback                      → http://127.0.0.1:{NODE_API_PORT}/v1/…

use async_channel::Sender;
use hanzo_http_api::node_api_router::APIError;
use hanzo_http_api::node_commands::NodeCommand;
use hanzo_zap::{
    ZapServer, cloud_handler,
    build_cloud_request, parse_cloud_response, build_handshake,
    Message, read_frame, write_frame, REQ_FLAG_REQ,
};
use log::{info, error};
use tokio::net::TcpStream;

/// Make a ZAP cloud request to any ZAP endpoint (engine OR a cluster peer) and return
/// `(status, body, error)`. This is the node's reusable ZAP *client* — cluster peer
/// forwarding rides this instead of HTTP.
pub async fn forward_via_zap(
    engine_addr: &str,
    method: &str,
    auth: &str,
    body: Vec<u8>,
) -> Result<(u32, Vec<u8>, String), String> {
    // Connect to engine ZAP endpoint
    let mut stream = TcpStream::connect(engine_addr)
        .await
        .map_err(|e| format!("ZAP connect to {engine_addr}: {e}"))?;
    stream.set_nodelay(true).ok();

    // Handshake
    let hs = build_handshake("hanzo-node");
    write_frame(&mut stream, &hs)
        .await
        .map_err(|e| format!("ZAP handshake write: {e}"))?;
    let hs_resp = read_frame(&mut stream)
        .await
        .map_err(|e| format!("ZAP handshake read: {e}"))?;
    let _ = Message::parse(hs_resp).map_err(|e| format!("ZAP handshake parse: {e}"))?;

    // Build cloud request and wrap with Call correlation header
    let msg = build_cloud_request(method, auth, &body);
    let req_id: u32 = 1;
    let mut wrapped = Vec::with_capacity(8 + msg.len());
    wrapped.extend_from_slice(&req_id.to_le_bytes());
    wrapped.extend_from_slice(&REQ_FLAG_REQ.to_le_bytes());
    wrapped.extend_from_slice(&msg);
    write_frame(&mut stream, &wrapped)
        .await
        .map_err(|e| format!("ZAP request write: {e}"))?;

    // Read response, skip 8-byte Call header
    let data = read_frame(&mut stream)
        .await
        .map_err(|e| format!("ZAP response read: {e}"))?;
    if data.len() < 8 {
        return Err("ZAP response too short".into());
    }
    let resp_msg = Message::parse(data[8..].to_vec())
        .map_err(|e| format!("ZAP response parse: {e}"))?;
    Ok(parse_cloud_response(&resp_msg))
}

/// Forward a cloud request to the local HTTP API.
async fn forward_via_http(
    api_port: u16,
    method: &str,
    auth: &str,
    body: Vec<u8>,
) -> Result<(u32, Vec<u8>, String), String> {
    let url = match method {
        "chat.completions" => {
            format!("http://127.0.0.1:{api_port}/v1/chat/completions")
        }
        other => {
            return Ok((404, Vec::new(), format!("unknown method: {other}")));
        }
    };

    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body);

    if !auth.is_empty() {
        req = req.header("Authorization", auth);
    }

    let resp = req.send().await.map_err(|e| format!("forward error: {e}"))?;
    let status = resp.status().as_u16() as u32;
    let resp_body = resp.bytes().await.map_err(|e| format!("body error: {e}"))?;

    Ok((status, resp_body.to_vec(), String::new()))
}

/// Dispatch a cluster-internal ZAP method into the node command channel. This is how a
/// peer's forwarded chat/search arrives over ZAP (binary, not HTTP) and gets served by
/// the local engine / RAG. The `*_local` commands never re-route, so forwarding can't loop.
async fn zap_dispatch_local(
    sender: &Sender<NodeCommand>,
    method: &str,
    body: Vec<u8>,
) -> Result<(u32, Vec<u8>, String), String> {
    let payload: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("ZAP {method}: bad json body: {e}"))?;
    let (tx, rx) = async_channel::bounded::<Result<serde_json::Value, APIError>>(1);
    let cmd = match method {
        "cluster.chat_local" => NodeCommand::V2ApiClusterChatLocal { payload, res: tx },
        "cluster.search_local" => NodeCommand::V2ApiClusterSearchLocal { payload, res: tx },
        other => return Ok((404, Vec::new(), format!("unknown cluster method: {other}"))),
    };
    sender
        .send(cmd)
        .await
        .map_err(|e| format!("ZAP {method}: command send failed: {e}"))?;
    match rx.recv().await {
        Ok(Ok(v)) => Ok((200, serde_json::to_vec(&v).unwrap_or_default(), String::new())),
        Ok(Err(api_err)) => Ok((api_err.code as u32, Vec::new(), api_err.message)),
        Err(e) => Err(format!("ZAP {method}: command recv failed: {e}")),
    }
}

/// Start the native ZAP listener alongside the HTTP API.
pub async fn start_zap_server(
    listen_addr: std::net::SocketAddr,
    node_commands_sender: Sender<NodeCommand>,
) {
    // In cluster mode, bind ZAP on all interfaces so LAN peers can reach it for cross-node
    // forwarding (default NODE_ZAP_IP is loopback-only, which would block peer ZAP calls).
    let cluster_mode = std::env::var("HANZO_CLUSTER_MODE")
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let listen_addr = if cluster_mode {
        std::net::SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), listen_addr.port())
    } else {
        listen_addr
    };
    info!("Starting ZAP server on {}", listen_addr);

    // Read forwarding config once at startup
    let engine_zap_url = std::env::var("HANZO_ENGINE_ZAP_URL").ok();
    let api_port: u16 = std::env::var("NODE_API_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3690);

    if let Some(ref addr) = engine_zap_url {
        info!("ZAP forwarding to engine at {} (native ZAP)", addr);
    } else {
        info!("ZAP forwarding to local HTTP API on port {}", api_port);
    }

    let server = ZapServer::new("hanzo-node", &listen_addr.to_string());

    let handler = cloud_handler(move |method, auth, body| {
        let engine_zap_url = engine_zap_url.clone();
        let node_commands_sender = node_commands_sender.clone();
        async move {
            match method.as_str() {
                // Cross-node cluster forwarding over ZAP (peers talk ZAP to us, not HTTP).
                "cluster.chat_local" | "cluster.search_local" => {
                    zap_dispatch_local(&node_commands_sender, &method, body).await
                }
                _ => {
                    if let Some(ref engine_addr) = engine_zap_url {
                        // Preferred: native ZAP binary protocol to engine
                        forward_via_zap(engine_addr, &method, &auth, body).await
                    } else {
                        // Fallback: HTTP to local API
                        forward_via_http(api_port, &method, &auth, body).await
                    }
                }
            }
        }
    });

    if let Err(e) = server.serve(handler).await {
        error!("ZAP server error: {}", e);
    }
}
