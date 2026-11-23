//! The one local ZAP socket, `<data>/hanzo/hanzod.zap.sock`, mode 0600.
//!
//! Two dialects arrive on it and are told apart by the frame type in the first
//! header: ZAP-HTTP requests (types 1-5, big-endian length prefix; the CLI, the
//! apps) and router envelopes (type 200, little-endian prefix; a Dev session).

use crate::daemon::{Daemon, Via};
use crate::paths;
use crate::table::Reply;
use crate::zap::{self, FRAME_ROUTER};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::{UnixListener as Listener, UnixStream as Stream};
#[cfg(not(unix))]
use tokio::net::{TcpListener as Listener, TcpStream as Stream};

/// Bind the socket, refusing if another hanzod already answers on it.
#[cfg(unix)]
pub async fn bind() -> io::Result<Listener> {
    let path = paths::socket();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).ok();
    }
    if Stream::connect(&path).await.is_ok() {
        return Err(io::Error::new(io::ErrorKind::AddrInUse, "another hanzod is already serving this socket"));
    }
    let _ = std::fs::remove_file(&path);
    // No window in which the socket exists with looser permissions.
    let old = unsafe { libc::umask(0o177) };
    let l = Listener::bind(&path);
    unsafe { libc::umask(old) };
    let l = l?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

#[cfg(not(unix))]
pub async fn bind() -> io::Result<Listener> {
    let path = paths::socket();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Listener::bind("127.0.0.1:0").await
}

#[cfg(unix)]
pub async fn serve(l: Listener, d: Arc<Daemon>) {
    let me = unsafe { libc::geteuid() };
    loop {
        let Ok((conn, _)) = l.accept().await else { continue };
        // Belt over the 0600 mode: only this user's processes are served.
        if conn.peer_cred().map(|c| c.uid() != me).unwrap_or(true) {
            continue;
        }
        let d = d.clone();
        tokio::spawn(async move {
            let _ = connection(conn, d).await;
        });
    }
}

#[cfg(not(unix))]
pub async fn serve(l: Listener, d: Arc<Daemon>) {
    loop {
        let Ok((conn, _)) = l.accept().await else { continue };
        let d = d.clone();
        tokio::spawn(async move {
            let _ = connection(conn, d).await;
        });
    }
}

async fn connection(mut conn: Stream, d: Arc<Daemon>) -> io::Result<()> {
    let mut head = [0u8; 4 + zap::HEADER];
    match conn.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
        Err(e) => return Err(e),
    }
    if zap::frame_type(&head[4..]) == Some(FRAME_ROUTER) {
        return session(conn, head, d).await;
    }
    let mut first = Some(head);
    loop {
        let frame = match first.take() {
            Some(h) => {
                let n = u32::from_be_bytes(h[0..4].try_into().unwrap());
                if n < zap::HEADER as u32 || n > zap::MAX_FRAME {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame size"));
                }
                let mut f = h[4..].to_vec();
                f.resize(n as usize, 0);
                conn.read_exact(&mut f[zap::HEADER..]).await?;
                f
            }
            None => match zap::read_frame(&mut conn).await? {
                Some(f) => f,
                None => return Ok(()),
            },
        };
        let req = match zap::decode_request(&frame) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        let (reply, _busy) = d.handle(Via::Socket, req).await;
        match reply {
            Reply::Whole(r) => zap::write_frame(&mut conn, &zap::encode_response(&r)).await?,
            Reply::Frames(mut rx) => {
                while let Some(f) = rx.recv().await {
                    zap::write_frame(&mut conn, &f).await?;
                }
            }
        }
    }
}

/// A Dev session: the first envelope's cap is the caller's bearer, checked
/// before a byte reaches the worker.
async fn session(mut conn: Stream, head: [u8; 4 + zap::HEADER], d: Arc<Daemon>) -> io::Result<()> {
    let n = u32::from_le_bytes(head[0..4].try_into().unwrap());
    if n < zap::HEADER as u32 || n > 32 << 20 {
        return Ok(());
    }
    let mut first = head.to_vec();
    first.resize(4 + n as usize, 0);
    conn.read_exact(&mut first[4 + zap::HEADER..]).await?;
    let bearer = match zap::router_cap(&first[4..]) {
        Ok(c) if !c.is_empty() => String::from_utf8_lossy(c).into_owned(),
        _ => {
            conn.write_all(&zap::router_refusal(401)).await?;
            return Ok(());
        }
    };
    if d.who(&bearer).await.is_err() {
        conn.write_all(&zap::router_refusal(401)).await?;
        return Ok(());
    }
    let (sub, _busy) = match d.table.route("/v1/dev").await {
        Ok(r) => r,
        Err(_) => {
            conn.write_all(&zap::router_refusal(500)).await?;
            return Ok(());
        }
    };
    if sub.stream(first, Box::new(conn)).await.is_err() {
        d.table.down("/v1/dev");
    }
    Ok(())
}
