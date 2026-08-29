//! Native ZAP listener for Hanzo Node.
//!
//! Accepts ZAP binary protocol connections on the P2P port (default 3692)
//! and dispatches cloud service requests to the node's command channel.
//!
//! Forwarding modes (checked once at startup):
//!   1. ZAP:  HANZO_ENGINE_ZAP_URL is set  → native binary protocol to engine
//!   2. HTTP: fallback                      → http://127.0.0.1:{NODE_API_PORT}/v1/…

use async_channel::Sender;
use hanzo_http_api::node_commands::NodeCommand;
use hanzo_zap::{ZapServer, cloud_call, cloud_handler};
use log::{info, error};
use tokio::net::TcpStream;

/// Forward a cloud request to the engine via native ZAP binary protocol.
async fn forward_via_zap(
    engine_addr: &str,
    method: &str,
    auth: &str,
    body: Vec<u8>,
) -> Result<(u32, Vec<u8>, String), String> {
    let mut stream = TcpStream::connect(engine_addr)
        .await
        .map_err(|e| format!("ZAP connect to {engine_addr}: {e}"))?;
    stream.set_nodelay(true).ok();
    cloud_call(&mut stream, "hanzo-node", 1, method, auth, &body).await
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

/// Start the native ZAP listener alongside the HTTP API.
pub async fn start_zap_server(
    listen_addr: std::net::SocketAddr,
    _node_commands_sender: Sender<NodeCommand>,
) {
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
        async move {
            if let Some(ref engine_addr) = engine_zap_url {
                // Preferred: native ZAP binary protocol to engine
                forward_via_zap(engine_addr, &method, &auth, body).await
            } else {
                // Fallback: HTTP to local API
                forward_via_http(api_port, &method, &auth, body).await
            }
        }
    });

    if let Err(e) = server.serve(handler).await {
        error!("ZAP server error: {}", e);
    }
}
