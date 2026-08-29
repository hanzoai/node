//! The client half of a ZAP cloud call.
//!
//! One place performs the exchange — handshake, correlate, write, read,
//! check — because the correlation preamble and the response checks were
//! otherwise written out by hand at every call site, and a check written in
//! two places is a check that holds in one.

use crate::wire::*;
use tokio::io::{AsyncRead, AsyncWrite};

/// Exchange handshakes and make one correlated cloud call over `stream`.
///
/// Returns the peer's (status, body, error) triple. Every refusal is an
/// error string naming what was wrong with the frame, never a silently
/// skipped preamble.
pub async fn cloud_call<S>(
    stream: &mut S,
    node_id: &str,
    req_id: u32,
    method: &str,
    auth: &str,
    body: &[u8],
) -> Result<(u32, Vec<u8>, String), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_frame(stream, &build_handshake(node_id))
        .await
        .map_err(|e| format!("ZAP handshake write: {e}"))?;
    let hs = read_frame(stream)
        .await
        .map_err(|e| format!("ZAP handshake read: {e}"))?;
    let hs = Message::parse(hs).map_err(|e| format!("ZAP handshake parse: {e}"))?;
    // A peer that will not name itself is not a peer.
    parse_handshake(&hs).ok_or("ZAP handshake carries no usable node id")?;

    let request = wrap_correlated(req_id, REQ_FLAG_REQ, &build_cloud_request(method, auth, body));
    write_frame(stream, &request)
        .await
        .map_err(|e| format!("ZAP request write: {e}"))?;

    let data = read_frame(stream)
        .await
        .map_err(|e| format!("ZAP response read: {e}"))?;
    let (resp_id, flag, payload) =
        unwrap_correlated(&data).ok_or("ZAP response is not a correlated frame")?;
    if flag != REQ_FLAG_RESP {
        return Err(format!("ZAP response carries flag {flag}, not a response"));
    }
    // Answering a request nobody made is how a reply gets read as the reply
    // to something else.
    if resp_id != req_id {
        return Err(format!("ZAP response answers request {resp_id}, not {req_id}"));
    }
    let msg = Message::parse(payload.to_vec()).map_err(|e| format!("ZAP response parse: {e}"))?;
    if msg.msg_type() != MSG_TYPE_CLOUD {
        return Err(format!("ZAP response has msg_type {}", msg.msg_type()));
    }
    Ok(parse_cloud_response(&msg))
}
