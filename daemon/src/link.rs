//! The desktop's own outbound ZAP session (HIP-1350).
//!
//! The Dev worker tells hanzod the address its source reports to the cloud
//! (`https://zap.<apex>/v1/dev/zap/<source id>`) with a control record on mux
//! session 0. hanzod dials it over WebTransport, registers as that source's
//! server end with the machine's IAM bearer, and serves every phone session the
//! cloud joins to it exactly as it serves a local one: each becomes a mux session
//! to the worker, and the envelopes move unchanged in both directions.
//!
//! The cloud writes only response envelopes, so a phone's request arrives as one:
//! `[u32 LE channel][request envelope]` under the attach promise (an empty
//! envelope closes the channel). Replies go back as `METHOD_REPLY` requests whose
//! payload is `[u32 LE channel][response envelope]`.
//!
//! A refused or dropped session is retried with backoff for as long as the worker
//! lives; the address is the only thing the worker can change, and it can only name
//! a Hanzo ZAP host, because the machine's bearer goes to it.

use crate::me;
use crate::worker::Mux;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, watch};
use wtransport::{ClientConfig, Endpoint};
use zaphttp::{router_request, router_response_parts};

/// Registers the desktop as the server end of its source (cloud `apps/dev/relay.go`).
pub const METHOD_HOST: u32 = 0xFFFF_0001;
/// Carries one of the desktop's response envelopes to a phone.
pub const METHOD_REPLY: u32 = 0xFFFF_0002;
const ATTACH: u32 = 1;
const MAX_FRAME: usize = 33 << 20;
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(60);

/// The address a worker may ask hanzod to dial: https, a `zap.` host, the Dev path.
pub fn permitted(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let Some((host, path)) = rest.split_once('/') else { return false };
    host.starts_with("zap.")
        && !host.contains(['@', ':', ' '])
        && path.starts_with("v1/dev/zap/")
        && path.len() > "v1/dev/zap/".len()
        && !path["v1/dev/zap/".len()..].contains(['/', '?', '#'])
}

/// Keeps the session to whatever address the worker last announced, until the
/// worker's mux ends (the sender is dropped).
pub async fn serve(mux: Arc<Mux>, mut announced: watch::Receiver<Option<String>>) {
    loop {
        let url = announced.borrow_and_update().clone();
        match url {
            None => {
                if announced.changed().await.is_err() {
                    return;
                }
            }
            Some(url) => {
                tokio::select! {
                    changed = announced.changed() => if changed.is_err() { return },
                    _ = hold(&url, &mux) => {}
                }
            }
        }
    }
}

/// Dials, serves and re-dials with backoff; never returns.
async fn hold(url: &str, mux: &Arc<Mux>) {
    let mut wait = FIRST_RETRY;
    loop {
        match once(url, mux).await {
            Ok(()) => wait = FIRST_RETRY,
            Err(e) => eprintln!("hanzod: dev link {url}: {e}"),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(LAST_RETRY);
    }
}

fn refused(status: u32) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, format!("the cloud answered {status}"))
}

async fn frame<R: AsyncReadExt + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 4];
    match r.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_le_bytes(head) as usize;
    if n > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

fn framed(env: &[u8]) -> Vec<u8> {
    let mut out = (env.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(env);
    out
}

/// One session: attach, then serve until the cloud or the network ends it.
async fn once(url: &str, mux: &Arc<Mux>) -> io::Result<()> {
    let (_, tokens) = me::tokens().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "nobody is signed in on this machine"))?;
    if me::expiry(&tokens.access_token).is_some_and(|exp| exp <= now()) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the machine's token has expired; hanzo auth refreshes it"));
    }
    let config = ClientConfig::builder()
        .with_bind_default()
        .with_native_certs()
        .keep_alive_interval(Some(Duration::from_secs(15)))
        .build();
    run(url, mux, config, &tokens.access_token).await
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The session on a given client configuration, for `once` and for tests.
pub(crate) async fn run(url: &str, mux: &Arc<Mux>, config: ClientConfig, bearer: &str) -> io::Result<()> {
    let other = |e: &dyn std::fmt::Display| io::Error::other(e.to_string());
    let endpoint = Endpoint::client(config).map_err(|e| other(&e))?;
    let conn = endpoint.connect(url).await.map_err(|e| other(&e))?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| other(&e))?.await.map_err(|e| other(&e))?;
    send.write_all(&framed(&router_request(METHOD_HOST, ATTACH, bearer.as_bytes(), &[]))).await.map_err(|e| other(&e))?;
    let first = frame(&mut recv).await?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "the cloud closed before answering"))?;
    let (status, _, _) = router_response_parts(&first)?;
    if status != 200 {
        return Err(refused(status));
    }

    // Everything written to the cloud goes through one writer.
    let (out, mut queue) = mpsc::channel::<Vec<u8>>(64);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = queue.recv().await {
            if send.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    let mut channels: HashMap<u32, (u32, tokio::task::JoinHandle<()>)> = HashMap::new();
    let mut promise = ATTACH;
    let outcome = loop {
        let env = match frame(&mut recv).await {
            Ok(Some(env)) => env,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        let Ok((_, _, body)) = router_response_parts(&env) else { break Err(io::Error::new(io::ErrorKind::InvalidData, "not a router envelope")) };
        if body.len() < 4 {
            continue;
        }
        let chan = u32::from_le_bytes(body[0..4].try_into().unwrap());
        let request = &body[4..];
        if request.is_empty() {
            if let Some((sid, pump)) = channels.remove(&chan) {
                pump.abort();
                mux.close(sid).await;
            }
            continue;
        }
        if !channels.contains_key(&chan) {
            let (sid, mut from_worker) = mux.open().await?;
            let out = out.clone();
            let mut next = promise;
            let pump = tokio::spawn(async move {
                // The worker writes `[u32 LE length][envelope]` in chunks of any size.
                let mut held: Vec<u8> = Vec::new();
                while let Some(chunk) = from_worker.recv().await {
                    held.extend_from_slice(&chunk);
                    while held.len() >= 4 {
                        let n = u32::from_le_bytes(held[0..4].try_into().unwrap()) as usize;
                        if held.len() < 4 + n {
                            break;
                        }
                        let response: Vec<u8> = held[4..4 + n].to_vec();
                        held.drain(..4 + n);
                        next = next.wrapping_add(1);
                        let mut payload = chan.to_le_bytes().to_vec();
                        payload.extend_from_slice(&response);
                        if out.send(framed(&router_request(METHOD_REPLY, next, &[], &payload))).await.is_err() {
                            return;
                        }
                    }
                }
            });
            channels.insert(chan, (sid, pump));
            promise = promise.wrapping_add(1 << 20);
        }
        let sid = channels[&chan].0;
        mux.send(sid, &framed(request)).await?;
    };
    for (_, (sid, pump)) in channels {
        pump.abort();
        mux.close(sid).await;
    }
    writer.abort();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_hanzo_zap_host_may_be_dialed() {
        assert!(permitted("https://zap.hanzo.ai/v1/dev/zap/src_abc"));
        assert!(permitted("https://zap.lux.network/v1/dev/zap/src_abc"));
        for no in [
            "http://zap.hanzo.ai/v1/dev/zap/src_abc",
            "https://evil.example/v1/dev/zap/src_abc",
            "https://zap.hanzo.ai@evil.example/v1/dev/zap/x",
            "https://zap.hanzo.ai:8443/v1/dev/zap/src_abc",
            "https://zap.hanzo.ai/v1/other/src_abc",
            "https://zap.hanzo.ai/v1/dev/zap/",
            "https://zap.hanzo.ai/v1/dev/zap/a/b",
            "https://zap.hanzo.ai/v1/dev/zap/a?x=1",
        ] {
            assert!(!permitted(no), "{no}");
        }
    }

    use crate::worker::{spawn, Spec};
    use wtransport::{Identity, ServerConfig};
    use zaphttp::{router_request_parts, router_response};

    /// The cloud's end, as far as the desktop can tell: it takes the attach, pushes one
    /// phone envelope on channel 7, and hands back what the desktop replies.
    #[tokio::test]
    async fn the_desktop_serves_a_phones_envelope_through_the_worker_and_replies() {
        let server = Endpoint::server(
            ServerConfig::builder().with_bind_default(0).with_identity(Identity::self_signed(["localhost", "127.0.0.1"]).unwrap()).build(),
        )
        .unwrap();
        let port = server.local_addr().unwrap().port();
        let phone = router_request(3, 21, b"phone-bearer", b"list threads");
        let sent = phone.clone();
        let cloud = tokio::spawn(async move {
            let conn = server.accept().await.await.unwrap().accept().await.unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let attach = frame(&mut recv).await.unwrap().unwrap();
            let (method, promise, cap, _) = router_request_parts(&attach).unwrap();
            assert_eq!((method, promise, cap), (METHOD_HOST, ATTACH, &b"tok"[..]));
            send.write_all(&framed(&router_response(200, ATTACH, b""))).await.unwrap();
            let mut body = 7u32.to_le_bytes().to_vec();
            body.extend_from_slice(&sent);
            send.write_all(&framed(&router_response(200, ATTACH, &body))).await.unwrap();
            let reply = frame(&mut recv).await.unwrap().unwrap();
            // Hold the session open until the desktop has been heard.
            let parts = router_request_parts(&reply).map(|(m, _, _, p)| (m, p.to_vec())).unwrap();
            drop((send, recv, conn));
            parts
        });

        // A worker that answers every envelope with the envelope itself.
        let spec = Spec { program: "/bin/sh".into(), args: vec!["-c".into(), "exec cat <&3 >&3".into()], cwd: None, env: vec![("PATH".into(), "/usr/bin:/bin".into())], stdin: None };
        let worker = spawn(&spec).unwrap();
        let config = ClientConfig::builder().with_bind_default().with_no_cert_validation().build();
        let url = format!("https://127.0.0.1:{port}/v1/dev/zap/src_t");
        let mux = worker.mux.clone();
        let session = tokio::spawn(async move { run(&url, &mux, config, "tok").await });

        let (method, payload) = tokio::time::timeout(Duration::from_secs(20), cloud).await.unwrap().unwrap();
        assert_eq!(method, METHOD_REPLY);
        assert_eq!(u32::from_le_bytes(payload[0..4].try_into().unwrap()), 7);
        assert_eq!(&payload[4..], &phone[..], "the envelope came back from the worker unchanged, on its channel");
        session.abort();
    }
}
