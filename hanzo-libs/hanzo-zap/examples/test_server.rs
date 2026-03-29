//! Minimal ZAP test server for cross-language integration tests.
use hanzo_zap::{ZapServer, cloud_handler};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let server = ZapServer::new("rust-test-server", "127.0.0.1:13692");
    let handler = cloud_handler(|method, _auth, _body| async move {
        if method == "chat.completions" {
            let resp = serde_json::json!({
                "id": "cross-lang-001",
                "object": "chat.completion",
                "model": "test",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "hello from rust"}, "finish_reason": "stop"}]
            });
            Ok((200u32, serde_json::to_vec(&resp).unwrap(), String::new()))
        } else {
            Ok((404, Vec::new(), format!("unknown: {method}")))
        }
    });
    // Signal readiness
    println!("READY");
    server.serve(handler).await.unwrap();
}
