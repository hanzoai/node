//! ZAP server listener — embeds directly into any Hanzo product binary.
//!
//! Accepts ZAP connections, performs handshake, dispatches MsgType 100
//! cloud service requests to a user-provided handler function.
//!
//! Usage:
//! ```rust,ignore
//! use hanzo_zap::ZapServer;
//!
//! let server = ZapServer::new("hanzo-node", "0.0.0.0:3692");
//! server.serve(|method, auth, body| async move {
//!     // method = "chat.completions"
//!     // auth = "Bearer ..."
//!     // body = JSON bytes
//!     Ok((200, response_bytes, String::new()))
//! }).await?;
//! ```

use crate::wire::*;
use std::future::Future;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

/// Handler function signature for cloud service requests.
/// Receives (method, auth, body_bytes) and returns (status, response_body, error_string).
pub type CloudHandler = Arc<
    dyn Fn(String, String, Vec<u8>) -> std::pin::Pin<
        Box<dyn Future<Output = Result<(u32, Vec<u8>, String), String>> + Send>,
    > + Send
        + Sync,
>;

/// A ZAP protocol server that embeds into any Hanzo product.
pub struct ZapServer {
    node_id: String,
    listen_addr: String,
}

impl ZapServer {
    pub fn new(node_id: &str, listen_addr: &str) -> Self {
        Self {
            node_id: node_id.to_string(),
            listen_addr: listen_addr.to_string(),
        }
    }

    /// Start accepting ZAP connections and dispatching to the handler.
    pub async fn serve(&self, handler: CloudHandler) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(&self.listen_addr).await?;
        info!("ZAP listening on {}", self.listen_addr);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    debug!("ZAP connection from {}", addr);
                    let node_id = self.node_id.clone();
                    let handler = handler.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, &node_id, handler).await {
                            error!("ZAP connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("ZAP accept error: {}", e);
                }
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    node_id: &str,
    handler: CloudHandler,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    stream.set_nodelay(true).ok();

    // Read handshake
    let hs_data = read_frame(&mut stream).await?;
    let hs_msg = Message::parse(hs_data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let peer_id = parse_handshake(&hs_msg);
    debug!("ZAP handshake from peer={}", peer_id);

    // Send our handshake
    let our_hs = build_handshake(node_id);
    write_frame(&mut stream, &our_hs).await?;

    // Request loop
    loop {
        let data = match read_frame(&mut stream).await {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                debug!("ZAP peer disconnected");
                break;
            }
            Err(e) => return Err(e.into()),
        };

        // Expect 8-byte Call correlation header + ZAP message
        if data.len() < 8 {
            warn!("ZAP frame too short: {} bytes", data.len());
            continue;
        }

        let req_id_bytes = &data[0..4];
        let flag = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);

        if flag != REQ_FLAG_REQ {
            continue;
        }

        // Parse the ZAP message (skip 8-byte Call header)
        let msg = match Message::parse(data[8..].to_vec()) {
            Ok(m) => m,
            Err(e) => {
                warn!("ZAP message parse error: {}", e);
                continue;
            }
        };

        if msg.msg_type() != MSG_TYPE_CLOUD {
            warn!("ZAP unexpected msg_type={}", msg.msg_type());
            continue;
        }

        let (method, auth, body) = parse_cloud_request(&msg);
        let method = method.to_string();
        let auth = auth.to_string();
        let body = body.to_vec();

        // Dispatch to handler
        let (status, resp_body, resp_error) = match handler(method, auth, body).await {
            Ok(r) => r,
            Err(e) => (500, Vec::new(), e),
        };

        // Build ZAP response
        let resp_msg = build_cloud_response(status, &resp_body, &resp_error);

        // Wrap with 8-byte Call correlation header (same req_id, RESP flag)
        let mut wrapped = Vec::with_capacity(8 + resp_msg.len());
        wrapped.extend_from_slice(req_id_bytes);
        wrapped.extend_from_slice(&REQ_FLAG_RESP.to_le_bytes());
        wrapped.extend_from_slice(&resp_msg);

        write_frame(&mut stream, &wrapped).await?;
    }

    Ok(())
}

/// Convenience: create a CloudHandler from an async function.
pub fn cloud_handler<F, Fut>(f: F) -> CloudHandler
where
    F: Fn(String, String, Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(u32, Vec<u8>, String), String>> + Send + 'static,
{
    Arc::new(move |method, auth, body| {
        let fut = f(method, auth, body);
        Box::pin(fut)
    })
}
