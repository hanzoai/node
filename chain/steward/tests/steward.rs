// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use steward::{target, Pins, Steward};

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// A `hanzo-chain` that listens where it is told to and answers `--version`.
const FAKE: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && { echo "hanzod 0.2.5"; exit 0; }
while [ $# -gt 0 ]; do [ "$1" = "--rpc" ] && rpc="$2"; shift; done
exec python3 -c "import socket,sys,time;h,p=sys.argv[1].rsplit(':',1);s=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);s.bind((h,int(p)));s.listen(5);time.sleep(600)" "$rpc"
"#;

fn tarball(binary: &[u8]) -> Vec<u8> {
    let mut t = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_size(binary.len() as u64);
    h.set_mode(0o755);
    h.set_cksum();
    t.append_data(&mut h, "hanzo-chain", binary).unwrap();
    let raw = t.into_inner().unwrap();
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(&raw).unwrap();
    e.finish().unwrap()
}

/// Serves `body` to any request, once per connection, until dropped.
fn serve(body: Vec<u8>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut s: TcpStream = s;
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    format!("http://{addr}")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn pins(tar_sha: &str, bin_sha: &str, node_tag: &str) -> Pins {
    Pins::parse(&format!(
        r#"{{"version":"0.2.5","lux_node":"v0.2.2",
            "targets":{{"{t}":{{"tarball":"{tar_sha}","binary":"{bin_sha}"}}}},
            "networks":{{"devnet":{{"lux_node":"{node_tag}","pchain":"127.0.0.1:1"}}}}}}"#,
        t = target()
    ))
    .unwrap()
}

fn steward(d: &tempfile::TempDir, base: String, p: Pins) -> Steward {
    let mut s = Steward::new(d.path().join("hanzod"), d.path().join("bin"), base, p);
    s.rpc = ([127, 0, 0, 1], free_port()).into();
    s.mesh = ([127, 0, 0, 1], free_port()).into();
    s
}

fn put(s: &Steward, n: &str) -> steward::Reply {
    s.handle("PUT", format!(r#"{{"network":"{n}"}}"#).as_bytes())
}

#[test]
fn off_leaves_nothing() {
    let d = tempfile::tempdir().unwrap();
    let s = steward(&d, "http://127.0.0.1:1".into(), pins("a", "b", "v0.2.2"));
    let r = s.handle("GET", b"");
    assert_eq!(r.body["enabled"], false);
    assert_eq!(r.body["running"], false);
    assert!(!d.path().join("hanzod").exists());
    assert!(!d.path().join("bin").exists());
    assert!(TcpStream::connect(s.rpc).is_err() && TcpStream::connect(s.mesh).is_err());
}

#[test]
fn no_pin_no_start() {
    let d = tempfile::tempdir().unwrap();
    let p = Pins::parse(r#"{"version":"0.2.5","lux_node":"v0.2.2","targets":{},"networks":{}}"#).unwrap();
    let r = put(&steward(&d, "http://127.0.0.1:1".into(), p), "devnet");
    assert_eq!((r.status, r.body["error"].as_str().unwrap()), (422, format!("no pinned digest for {}", target()).as_str()));
    assert!(!d.path().join("hanzod").exists());
}

#[test]
fn a_tarball_that_is_not_the_pin_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let t = tarball(FAKE.as_bytes());
    let s = steward(&d, serve(t), pins(&sha(b"other"), &sha(FAKE.as_bytes()), "v0.2.2"));
    let r = put(&s, "devnet");
    assert_eq!(r.status, 422);
    assert!(r.body["error"].as_str().unwrap().contains("pinned digest"));
    assert!(!d.path().join("bin/hanzo-chain").exists());
}

#[test]
fn a_binary_that_is_not_the_pin_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let t = tarball(FAKE.as_bytes());
    let s = steward(&d, serve(t.clone()), pins(&sha(&t), &sha(b"other"), "v0.2.2"));
    let r = put(&s, "devnet");
    assert_eq!(r.status, 422);
    assert!(!d.path().join("bin/hanzo-chain").exists());
}

#[test]
fn a_lux_node_mismatch_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let t = tarball(FAKE.as_bytes());
    let s = steward(&d, serve(t.clone()), pins(&sha(&t), &sha(FAKE.as_bytes()), "v0.3.0"));
    let r = put(&s, "devnet");
    assert_eq!(r.status, 422);
    assert!(r.body["error"].as_str().unwrap().contains("lux-node mismatch"));
    assert!(!d.path().join("bin").exists());
}

#[cfg(unix)]
#[test]
fn on_then_off_holds_the_ports_only_while_on() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let t = tarball(FAKE.as_bytes());
    let s = steward(&d, serve(t.clone()), pins(&sha(&t), &sha(FAKE.as_bytes()), "v0.2.2"));

    let r = put(&s, "devnet");
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["running"], true);
    assert!(TcpStream::connect(s.rpc).is_ok());
    assert_eq!(std::fs::metadata(d.path().join("hanzod")).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(put(&s, "devnet").status, 409);

    let r = s.handle("DELETE", b"");
    assert_eq!(r.body["running"], false);
    assert_eq!(r.body["enabled"], false);
    assert!(TcpStream::connect(s.rpc).is_err());
}

#[cfg(unix)]
#[test]
fn a_key_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let t = tarball(FAKE.as_bytes());
    let s = steward(&d, serve(t.clone()), pins(&sha(&t), &sha(FAKE.as_bytes()), "v0.2.2"));
    std::fs::create_dir_all(d.path().join("hanzod")).unwrap();
    let k = d.path().join("hanzod/identity.key");
    std::fs::write(&k, [0u8; 32]).unwrap();
    std::fs::set_permissions(&k, std::fs::Permissions::from_mode(0o644)).unwrap();
    let r = put(&s, "devnet");
    assert_eq!(r.status, 500);
    assert!(r.body["error"].as_str().unwrap().contains("readable by others"));
}
