//! The two halves against each other, over a real socket.
//!
//! The codec tests prove the bytes; this proves the exchange — handshake,
//! correlation, dispatch, response — end to end, with nothing stubbed.

use hanzo_zap::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

/// Serve on an ephemeral port and hand back its address.
async fn serve(handler: CloudHandler) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let bound = addr.clone();
    tokio::spawn(async move {
        let server = ZapServer::new("test-server", &bound);
        let _ = server.serve(handler).await;
    });
    // Yield until the listener is up, rather than sleeping a guess at how
    // long it takes.
    for _ in 0..10_000 {
        if TcpStream::connect(&addr).await.is_ok() {
            return addr;
        }
        tokio::task::yield_now().await;
    }
    panic!("server never bound {addr}");
}

#[tokio::test]
async fn a_call_reaches_the_handler_and_the_answer_comes_back() {
    let seen = Arc::new(AtomicU32::new(0));
    let seen_in_handler = seen.clone();
    let addr = serve(cloud_handler(move |method, auth, body| {
        let seen = seen_in_handler.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            assert_eq!(method, "chat.completions");
            assert_eq!(auth, "Bearer tok");
            assert_eq!(body, b"{\"model\":\"zen\"}");
            Ok((200, b"{\"ok\":true}".to_vec(), String::new()))
        }
    }))
    .await;

    let mut s = TcpStream::connect(&addr).await.unwrap();
    let (status, body, err) = cloud_call(
        &mut s,
        "test-client",
        7,
        "chat.completions",
        "Bearer tok",
        b"{\"model\":\"zen\"}",
    )
    .await
    .expect("the call must complete");

    assert_eq!(status, 200);
    assert_eq!(body, b"{\"ok\":true}");
    assert!(err.is_empty());
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_handler_error_arrives_as_a_status_and_a_message() {
    let addr = serve(cloud_handler(|_, _, _| async move {
        Err("upstream refused".to_string())
    }))
    .await;

    let mut s = TcpStream::connect(&addr).await.unwrap();
    let (status, body, err) = cloud_call(&mut s, "test-client", 1, "m", "", b"")
        .await
        .unwrap();
    assert_eq!(status, 500);
    assert!(body.is_empty());
    assert_eq!(err, "upstream refused");
}

#[tokio::test]
async fn many_calls_share_one_connection_and_keep_their_own_ids() {
    let addr = serve(cloud_handler(|method, _, _| async move {
        Ok((200, method.into_bytes(), String::new()))
    }))
    .await;

    let mut s = TcpStream::connect(&addr).await.unwrap();
    // The first call performs the handshake; the rest reuse it, which is what
    // the request loop on the far side is for.
    let (_, body, _) = cloud_call(&mut s, "test-client", 1, "first", "", b"").await.unwrap();
    assert_eq!(body, b"first");

    for id in 2u32..6 {
        let req = wrap_correlated(id, REQ_FLAG_REQ, &build_cloud_request("later", "", b""));
        write_frame(&mut s, &req).await.unwrap();
        let data = read_frame(&mut s).await.unwrap();
        let (resp_id, flag, payload) = unwrap_correlated(&data).expect("a correlated response");
        assert_eq!(resp_id, id, "the answer carries the caller's id");
        assert_eq!(flag, REQ_FLAG_RESP);
        let msg = Message::parse(payload.to_vec()).unwrap();
        assert_eq!(parse_cloud_response(&msg).1, b"later");
    }
}

#[tokio::test]
async fn a_peer_that_will_not_name_itself_is_dropped() {
    let addr = serve(cloud_handler(|_, _, _| async move {
        panic!("the handler must never run for a nameless peer");
    }))
    .await;

    // A well-formed frame whose declared id length is zero — the case Go's
    // decoder answers ok=false for.
    let mut b = Builder::new(128);
    let mut ob = b.start_object(64);
    ob.set_uint32(HANDSHAKE_ID_LEN_OFFSET, 0);
    ob.finish_as_root();

    let mut s = TcpStream::connect(&addr).await.unwrap();
    write_frame(&mut s, &b.finish()).await.unwrap();
    s.flush().await.unwrap();

    // The server closes rather than serving a request loop, so our next read
    // ends the stream instead of returning a handshake.
    let closed = read_frame(&mut s).await;
    assert!(closed.is_err(), "server answered a nameless peer");
}

#[tokio::test]
async fn an_oversized_frame_is_refused_before_it_is_allocated() {
    // The length prefix is attacker-controlled; a reader that trusts it
    // allocates whatever it is told to.
    let (mut a, mut b) = tokio::io::duplex(64);
    let huge = (MAX_MESSAGE_SIZE as u32 + 1).to_le_bytes();
    tokio::spawn(async move {
        let _ = a.write_all(&huge).await;
        let _ = a.flush().await;
    });
    let err = read_frame(&mut b).await.expect_err("must refuse");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}
