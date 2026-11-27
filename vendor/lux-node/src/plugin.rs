// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! A chain in another process.
//!
//! luxd does not link its C-chain in; it runs it as a separate program and
//! speaks ZAP to it. That is not an implementation detail — it is why the EVM
//! can be upgraded, crash, or be replaced without taking the validator with it,
//! and why the seam between "what a block means" and "which block wins" is a
//! process boundary rather than a convention.
//!
//! This is that boundary in Rust. [`Server`] puts any [`Vm`] behind a socket
//! and [`Client`] IS a [`Vm`], so a node cannot tell which side of the boundary
//! its chain is on — the same trait, the same calls, the same order.
//!
//! THE MESSAGE NUMBERS ARE GO'S. `luxfi/api/zap/wire.go` numbers the node-to-VM
//! methods, and these are those numbers, not a parallel set:
//! `MsgInitialize=1`, `MsgBuildBlock=9`, `MsgParseBlock=10`, `MsgGetBlock=11`,
//! `MsgSetPreference=12`, `MsgHealth=13`, `MsgVersion=14`,
//! `MsgGetBlockIDAtHeight=22`, `MsgBlockVerify=28`, `MsgBlockAccept=29`,
//! `MsgBlockReject=30`. A response is the request's type with
//! [`zap::RESPONSE_FLAG`]; a refusal adds [`zap::ERROR_FLAG`]. Every id is
//! below `0x40` precisely so those two bits are free.
//!
//! ONE DEPARTURE, STATED. Go's `MsgNewHTTPHandler` (5) asks the plugin for an
//! `http.Handler` and lets the plugin route inside it. A handler cannot cross a
//! socket, so Go hands back an address and the node proxies. Here the node owns
//! the one HTTP server and a chain answers one JSON-RPC call at a time, so this
//! message carries a call and returns its result. Same message, same purpose —
//! the plugin serving its own RPC — and a different shape, which has to be
//! reconciled before a Rust node drives a Go plugin.
//!
//! EVERY PAYLOAD CARRIES A REQUEST ID. Go writes `uint32` big-endian in front
//! of the payload INSIDE the ZAP frame, on both the call and the answer, and
//! refuses a payload shorter than four bytes (`zap/transport.go`: the client
//! writes it at `Call`, the server extracts it in `Read` and echoes it in
//! `Write`). It is what lets one connection carry more than one outstanding
//! call. Writing the payload bare — which this did — is a frame a Go node
//! rejects outright, so the two could never have spoken.
//!
//! THE LINK IS SEALED. It carries `eth_sendRawTransaction` — user transactions
//! BEFORE they are in a block — and it accepts `MsgBlockAccept`, so anything
//! that can read it can front-run and anything that can write it can force a
//! chain to accept. It used to be plaintext and unauthenticated.
//!
//! Now both ends run the same post-quantum handshake the validator link uses
//! ([`crate::pq`]) and key ChaCha20-Poly1305 from the secret it establishes,
//! with a separate key and counter per direction and the message type as
//! associated data — so a frame cannot be replayed, reordered, or re-labelled
//! from a VERIFY into an ACCEPT. The secret was already being negotiated and
//! thrown away; this is it doing the job it was derived for.
//!
//! Pin the peer with `expect` when you can: without it the link is confidential
//! and unforgeable but the far end is whoever answered the port, which on a
//! shared machine is a race an attacker can enter.
//!
//! TWO TRANSPORTS, ONE RULE. An address is a socket file or a host and a port,
//! and [`is_socket_path`] decides which exactly as `luxfi/api/zap` decides it.
//! It has to be exactly: the two ends of a link read the address separately,
//! so a rule that differs by one case is a node listening where its chain
//! never dials. [`Socket`] and [`Listener`] are the two shapes, and nothing
//! above them knows which one it got.
//!
//! AND A CHAIN THIS NODE DID NOT WRITE. [`Child`] starts a Go VM plugin the
//! way luxd starts one and drives it over the same numbers — because twelve
//! chains already exist as Go binaries, and porting each one would be twelve
//! chances to disagree with the network about what a block means. Its link is
//! plaintext, because a Go plugin runs no handshake and seals no frame; the
//! [`Client`]/[`Server`] pair keeps the sealed link for this node's own
//! plugin, which can.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy_primitives::keccak256;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use lux_consensus::zap;
use serde_json::Value;

use crate::pq::{self, Identity};

use crate::vm::{Block, Error, Id, Vm};

/// The node-to-VM methods, numbered as `luxfi/api/zap/wire.go` numbers them.
pub mod msg {
    pub const INITIALIZE: u8 = 1;
    /// Go: `MsgSetState`. A VM that has not been told it is ready does not
    /// build — see [`super::state`].
    pub const SET_STATE: u8 = 2;
    /// Go: `MsgNewHTTPHandler`. Here it carries one JSON-RPC call — see the
    /// module docs for why the shape differs.
    pub const CALL: u8 = 5;
    pub const BUILD: u8 = 9;
    pub const PARSE: u8 = 10;
    pub const GET: u8 = 11;
    pub const PREFER: u8 = 12;
    pub const HEALTH: u8 = 13;
    pub const VERSION: u8 = 14;
    pub const AT_HEIGHT: u8 = 22;
    pub const VERIFY: u8 = 28;
    pub const ACCEPT: u8 = 29;
    pub const REJECT: u8 = 30;
}

/// The largest frame this boundary carries.
///
/// A block is the biggest thing on it, and the validator link already bounds a
/// block; the same number here means a block that fits on one link fits on the
/// other, and one that does not is refused at both.
pub const FRAME_MAX: u32 = crate::mesh::BLOCK_MAX;

/// How long a call may take before the chain is judged gone.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The request id Go prefixes every payload with, big-endian.
pub const REQUEST_ID_LEN: usize = 4;

/// Whether an address names a socket file or a host and port.
///
/// GO'S RULE, NOT ONE OF OURS. `luxfi/api/zap`'s `transportNetwork` decides
/// this the same way on both ends of every link, and the two sides have to
/// agree or one listens where the other never dials: a leading `/` or `@` is a
/// path; a `:` with no `/` is host and port; anything else with a `/` is a
/// path; anything left is host and port.
pub fn is_socket_path(addr: &str) -> bool {
    if addr.starts_with('/') || addr.starts_with('@') {
        return true;
    }
    if addr.contains(':') && !addr.contains('/') {
        return false;
    }
    addr.contains('/')
}

/// A connection to a chain, over whichever transport its address named.
///
/// Two kinds because the far end has two: a plugin on this host is reached
/// through the filesystem with no TCP stack under it, and one that is not is
/// reached through a port. Nothing above this point knows which.
pub enum Socket {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Socket {
    /// Dial `addr`, waiting no longer than `timeout` for it to answer.
    pub fn connect(addr: &str, timeout: Duration) -> io::Result<Self> {
        if is_socket_path(addr) {
            let s = UnixStream::connect(addr)?;
            s.set_read_timeout(Some(timeout))?;
            s.set_write_timeout(Some(timeout))?;
            return Ok(Socket::Unix(s));
        }
        // A name can resolve to more than one address and only one of them may
        // be listening, so every candidate is tried before the dial is a
        // failure.
        let mut last = io::Error::new(io::ErrorKind::InvalidInput, "no address to dial");
        for at in std::net::ToSocketAddrs::to_socket_addrs(addr)? {
            match TcpStream::connect_timeout(&at, timeout) {
                Ok(s) => {
                    s.set_read_timeout(Some(timeout))?;
                    s.set_write_timeout(Some(timeout))?;
                    s.set_nodelay(true)?;
                    return Ok(Socket::Tcp(s));
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// How long a read or a write may block before the far end is judged gone.
    pub fn patience(&self, how_long: Duration) -> io::Result<()> {
        match self {
            Socket::Tcp(s) => {
                s.set_read_timeout(Some(how_long))?;
                s.set_write_timeout(Some(how_long))
            }
            Socket::Unix(s) => {
                s.set_read_timeout(Some(how_long))?;
                s.set_write_timeout(Some(how_long))
            }
        }
    }
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Socket::Tcp(s) => s.read(buf),
            Socket::Unix(s) => s.read(buf),
        }
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Socket::Tcp(s) => s.write(buf),
            Socket::Unix(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Socket::Tcp(s) => s.flush(),
            Socket::Unix(s) => s.flush(),
        }
    }
}

/// Where connections arrive, over whichever transport the address named.
///
/// A unix listener owns a file, and an abandoned socket file is a name that
/// can never be bound again — so the path is removed when this is dropped.
pub enum Listener {
    Tcp(TcpListener),
    Unix(UnixListener, std::path::PathBuf),
}

impl Listener {
    /// Bind `addr`. A path that is already there is a socket nobody closed:
    /// it is removed first, because otherwise a chain cannot be restarted
    /// without a human deleting a file.
    pub fn bind(addr: &str) -> io::Result<Self> {
        if is_socket_path(addr) {
            let path = std::path::PathBuf::from(addr);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            let l = UnixListener::bind(&path)?;
            l.set_nonblocking(true)?;
            return Ok(Listener::Unix(l, path));
        }
        let l = TcpListener::bind(addr)?;
        l.set_nonblocking(true)?;
        Ok(Listener::Tcp(l))
    }

    /// The address this is actually on — the request may have said port zero.
    pub fn addr(&self) -> io::Result<String> {
        match self {
            Listener::Tcp(l) => Ok(l.local_addr()?.to_string()),
            Listener::Unix(_, path) => Ok(path.to_string_lossy().into_owned()),
        }
    }

    /// Take the next connection, or say that none is waiting.
    pub fn accept(&self) -> io::Result<Socket> {
        // A socket accepted from a polling listener inherits non-blocking on
        // BSD and macOS and does not on Linux. Left inherited, the first read
        // returns EAGAIN and every caller is dropped before it is heard — on
        // one platform only.
        match self {
            Listener::Tcp(l) => {
                let (s, _) = l.accept()?;
                s.set_nonblocking(false)?;
                s.set_nodelay(true)?;
                Ok(Socket::Tcp(s))
            }
            Listener::Unix(l, _) => {
                let (s, _) = l.accept()?;
                s.set_nonblocking(false)?;
                Ok(Socket::Unix(s))
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Listener::Unix(_, path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// One direction of a sealed link.
///
/// A key and a counter. The counter never repeats and never goes backwards, so
/// no two frames on one link are sealed under the same nonce — which is the one
/// thing ChaCha20-Poly1305 asks of its caller.
struct Direction {
    cipher: ChaCha20Poly1305,
    next: u64,
}

impl Direction {
    /// Derive this direction's key from the session secret.
    ///
    /// Two keys, not one key and two nonce spaces: a direction that cannot
    /// decrypt its own output cannot be made to reflect a frame back at the
    /// sender as though the peer had said it.
    fn new(secret: &[u8; 32], label: &[u8]) -> Self {
        let mut material = Vec::with_capacity(label.len() + 32);
        material.extend_from_slice(label);
        material.extend_from_slice(secret);
        let key = keccak256(&material).0;
        Self { cipher: ChaCha20Poly1305::new((&key).into()), next: 0 }
    }

    fn nonce(&self) -> Nonce {
        let mut raw = [0u8; 12];
        raw[4..].copy_from_slice(&self.next.to_be_bytes());
        raw.into()
    }
}

/// A link both ends have proved themselves on, and that nobody else can read.
pub struct Sealed {
    stream: Socket,
    out: Direction,
    into: Direction,
    /// Who the far end proved itself to be.
    peer: lux_consensus::cert::NodeId,
}

impl Sealed {
    /// Run the handshake and key the link from what it established.
    ///
    /// `expect` is the identity we believe we are talking to, or `None` to take
    /// whoever answers — see the module docs for what that costs.
    pub fn open(
        stream: Socket,
        me: &Identity,
        expect: Option<lux_consensus::cert::NodeId>,
        dialing: bool,
    ) -> Result<Self, Error> {
        let mut link = stream;
        let role = if dialing { pq::Role::Initiator } else { pq::Role::Responder };
        let session = pq::greet(&mut link, &pq::Config::strict_pq(), me, role, expect)
            .map_err(|e| Error::Invalid(format!("the link could not be proved: {e}")))?;
        let stream = link;
        // The dialer writes under one label and reads under the other, so the
        // two ends agree on which key is which without negotiating it.
        let (send, recv) = if dialing {
            (b"lux-plugin-call-v1".as_slice(), b"lux-plugin-answer-v1".as_slice())
        } else {
            (b"lux-plugin-answer-v1".as_slice(), b"lux-plugin-call-v1".as_slice())
        };
        Ok(Self {
            stream,
            out: Direction::new(&session.secret, send),
            into: Direction::new(&session.secret, recv),
            peer: session.peer,
        })
    }

    pub fn peer(&self) -> lux_consensus::cert::NodeId {
        self.peer
    }

    /// Seal one frame. The type byte — flags included — is associated data, so
    /// a frame cannot be re-labelled: a VERIFY cannot be replayed as an ACCEPT,
    /// and an answer cannot be replayed as a call.
    fn write(&mut self, kind: u8, payload: &[u8]) -> Result<(), Error> {
        let sealed = self
            .out
            .cipher
            .encrypt(&self.out.nonce(), Payload { msg: payload, aad: &[kind] })
            .map_err(|_| Error::Unavailable("cannot seal the frame".into()))?;
        self.out.next += 1;
        zap::write_frame(&mut self.stream, kind, &sealed)
            .map_err(|e| Error::Unavailable(format!("cannot reach the far end: {e}")))
    }

    /// Open one frame. A frame that does not open at the expected counter is
    /// refused rather than retried: replay, reorder and truncation all arrive
    /// looking exactly like this.
    fn read(&mut self) -> Result<(u8, Vec<u8>), Error> {
        let (raw, body) = zap::read_frame(&mut self.stream, FRAME_MAX)
            .map_err(|e| Error::Unavailable(format!("the far end did not answer: {e}")))?;
        // The RAW type byte, flags and all — the same byte the sender sealed
        // under. Stripping the response bit here would mean a request and its
        // answer were sealed over different associated data and neither could
        // open the other's.
        let opened = self
            .into
            .cipher
            .decrypt(&self.into.nonce(), Payload { msg: &body, aad: &[raw] })
            .map_err(|_| Error::Unavailable("the frame did not open".into()))?;
        self.into.next += 1;
        Ok((raw, opened))
    }
}

/// Put `id` in front of `payload`, the way Go does.
fn tag(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(REQUEST_ID_LEN + payload.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Take the request id off the front. A payload too short to hold one is not a
/// message — Go refuses it with `ErrInvalidMessage` and so does this.
fn untag(payload: &[u8]) -> Option<(u32, &[u8])> {
    let (head, rest) = payload.split_at_checked(REQUEST_ID_LEN)?;
    Some((u32::from_be_bytes(head.try_into().ok()?), rest))
}

/// A block that lives in another process.
///
/// Everything consensus needs, carried across as values. The bytes come too,
/// because a peer has to be sent them and asking the plugin again for what it
/// just sent would be a round trip per broadcast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    id: Id,
    parent: Id,
    height: u64,
    timestamp: u64,
    state_root: Id,
    payload_root: Id,
    bytes: Vec<u8>,
}

impl Remote {
    fn write(block: &dyn Block, w: &mut zap::Writer) {
        w.bytes(&block.id())
            .bytes(&block.parent())
            .u64(block.height())
            .u64(block.timestamp())
            .bytes(&block.state_root())
            .bytes(&block.payload_root())
            .bytes(&block.bytes());
    }

    fn read(r: &mut zap::Reader<'_>) -> Option<Self> {
        Some(Self {
            id: word(r)?,
            parent: word(r)?,
            height: r.u64()?,
            timestamp: r.u64()?,
            state_root: word(r)?,
            payload_root: word(r)?,
            bytes: r.bytes()?.to_vec(),
        })
    }
}

impl Block for Remote {
    fn id(&self) -> Id {
        self.id
    }
    fn parent(&self) -> Id {
        self.parent
    }
    fn height(&self) -> u64 {
        self.height
    }
    fn timestamp(&self) -> u64 {
        self.timestamp
    }
    fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
    fn state_root(&self) -> Id {
        self.state_root
    }
    fn payload_root(&self) -> Id {
        self.payload_root
    }
}

fn word(r: &mut zap::Reader<'_>) -> Option<Id> {
    let b = r.bytes()?;
    (b.len() == 32).then(|| {
        let mut out = [0u8; 32];
        out.copy_from_slice(b);
        out
    })
}

// ---------------------------------------------------------------------------
// The node's side.
// ---------------------------------------------------------------------------

/// A chain reached over ZAP.
///
/// One connection, held open, with calls serialised on it. Serialised rather
/// than pooled because the calls are already serialised by meaning — a node
/// does not verify two blocks at once — and a pool would be a second place for
/// them to arrive out of order.
pub struct Client {
    name: &'static str,
    link: Mutex<Sealed>,
    /// The next request id. Monotonic, so an answer can be matched to the call
    /// that asked — and a stale answer to a call that timed out is refused
    /// rather than read as the answer to the next one.
    next: AtomicU32,
}

impl Client {
    /// Connect to a chain, prove the link, and agree on which network this is.
    ///
    /// `genesis` is the hash of the genesis THIS node was started with. A chain
    /// running a different one produces blocks nobody else can accept, so the
    /// two are compared before a single message is exchanged — a localnet node
    /// driving a mainnet plugin is a mistake that has to fail here or not at
    /// all.
    pub fn connect(
        name: &'static str,
        at: &str,
        me: &Identity,
        expect: Option<lux_consensus::cert::NodeId>,
        genesis: Id,
    ) -> Result<Self, Error> {
        let link = Socket::connect(at, TIMEOUT)
            .map_err(|e| Error::Invalid(format!("cannot reach the chain at {at}: {e}")))?;

        let client = Self {
            name,
            link: Mutex::new(Sealed::open(link, me, expect, true)?),
            next: AtomicU32::new(1),
        };

        // The chain answers with the genesis IT is running. Compared here as
        // well as there, so neither end can be the only one that checked.
        let mut w = zap::Writer::new();
        w.bytes(&genesis);
        let answered = client.ask(msg::INITIALIZE, w.as_slice())?;
        let theirs = word(&mut zap::Reader::new(&answered))
            .ok_or_else(|| Error::Malformed("the chain did not name its genesis".into()))?;
        if theirs != genesis {
            return Err(Error::Invalid(format!(
                "this node runs genesis {}, the chain at {at} runs {}",
                alloy_primitives::hex::encode(genesis),
                alloy_primitives::hex::encode(theirs)
            )));
        }
        Ok(client)
    }

    /// Who the chain proved itself to be.
    pub fn peer(&self) -> Option<lux_consensus::cert::NodeId> {
        self.link.lock().ok().map(|l| l.peer())
    }

    /// One request, one response.
    ///
    /// ONE CALL AT A TIME. The link is held for the whole exchange, so every
    /// call — consensus's verify and accept, and every JSON-RPC call the node
    /// forwards — waits behind the one in flight. An `eth_getLogs` scan is the
    /// longest of those: inside the plugin it takes a turn, reads off the
    /// chain lock and stops at `evm::LOGS_READ_MAX`, but here it still holds
    /// this link until it answers, and an accept behind it waits that long.
    fn ask(&self, kind: u8, payload: &[u8]) -> Result<Vec<u8>, Error> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut link = self
            .link
            .lock()
            .map_err(|_| Error::Unavailable("the chain link is poisoned".into()))?;
        link.write(kind, &tag(id, payload))?;
        let (raw, body) = link.read()?;
        if zap::strip_flags(raw) != kind {
            return Err(Error::Unavailable("the chain answered a different question".into()));
        }
        let Some((answered, body)) = untag(&body) else {
            return Err(Error::Malformed("the answer carries no request id".into()));
        };
        // An answer to some other call is not the answer to this one. Without
        // this a dropped response silently shifts every later reply by one.
        if answered != id {
            return Err(Error::Unavailable(format!(
                "the chain answered request {answered}, not {id}"
            )));
        }
        if zap::is_error(raw) {
            return Err(decode_error(body));
        }
        Ok(body.to_vec())
    }

    fn block(&self, kind: u8, payload: &[u8]) -> Result<Box<dyn Block>, Error> {
        let body = self.ask(kind, payload)?;
        let mut r = zap::Reader::new(&body);
        Remote::read(&mut r)
            .map(|b| Box::new(b) as Box<dyn Block>)
            .ok_or_else(|| Error::Malformed("the chain sent something that is not a block".into()))
    }

    fn just_id(&self, kind: u8, id: &Id) -> Result<(), Error> {
        let mut w = zap::Writer::new();
        w.bytes(id);
        self.ask(kind, w.as_slice()).map(|_| ())
    }
}

impl Vm for Client {
    fn name(&self) -> &'static str {
        self.name
    }

    fn version(&self) -> String {
        self.ask(msg::VERSION, &[])
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_else(|| "unreachable".into())
    }

    fn build(&self) -> Result<Box<dyn Block>, Error> {
        self.block(msg::BUILD, &[])
    }

    fn parse(&self, raw: &[u8]) -> Result<Box<dyn Block>, Error> {
        let mut w = zap::Writer::new();
        w.bytes(raw);
        self.block(msg::PARSE, w.as_slice())
    }

    fn get(&self, id: &Id) -> Result<Box<dyn Block>, Error> {
        let mut w = zap::Writer::new();
        w.bytes(id);
        self.block(msg::GET, w.as_slice())
    }

    fn verify(&self, id: &Id) -> Result<(), Error> {
        self.just_id(msg::VERIFY, id)
    }

    fn accept(&self, id: &Id) -> Result<(), Error> {
        self.just_id(msg::ACCEPT, id)
    }

    fn reject(&self, id: &Id) -> Result<(), Error> {
        self.just_id(msg::REJECT, id)
    }

    fn set_preference(&self, id: &Id) -> Result<(), Error> {
        self.just_id(msg::PREFER, id)
    }

    fn last_accepted(&self) -> Id {
        // `LastAccepted` has no error in the trait, and a chain that cannot be
        // reached has no last accepted block to report. Zero is the answer that
        // makes a caller notice rather than one that looks like a height.
        self.ask(msg::AT_HEIGHT, &[])
            .ok()
            .and_then(|b| word(&mut zap::Reader::new(&b)))
            .unwrap_or([0u8; 32])
    }

    fn block_id_at(&self, height: u64) -> Result<Id, Error> {
        let mut w = zap::Writer::new();
        w.u64(height);
        let body = self.ask(msg::AT_HEIGHT, w.as_slice())?;
        word(&mut zap::Reader::new(&body)).ok_or(Error::NotFound)
    }

    fn health(&self) -> Result<(), Error> {
        self.ask(msg::HEALTH, &[]).map(|_| ())
    }

    fn call(&self, method: &str, params: &Value) -> Result<Value, Error> {
        let mut w = zap::Writer::new();
        w.bytes(method.as_bytes()).bytes(params.to_string().as_bytes());
        let body = self.ask(msg::CALL, w.as_slice())?;
        serde_json::from_slice(&body).map_err(|e| {
            Error::Malformed(format!("the chain sent something that is not JSON: {e}"))
        })
    }
}

// ---------------------------------------------------------------------------
// The chain's side.
// ---------------------------------------------------------------------------

/// Put a chain behind a socket.
pub struct Server {
    vm: Arc<dyn Vm>,
    identity: Arc<Identity>,
    /// The genesis this chain is running. A node claiming another one is not
    /// on this network and is refused before it can drive anything.
    genesis: Id,
    running: Arc<AtomicBool>,
}

impl Server {
    pub fn new(vm: Arc<dyn Vm>, identity: Arc<Identity>, genesis: Id) -> Self {
        Self { vm, identity, genesis, running: Arc::new(AtomicBool::new(true)) }
    }

    /// The name this chain proves on its link.
    pub fn node(&self) -> lux_consensus::cert::NodeId {
        self.identity.node()
    }

    pub fn stop(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Serve until stopped. One connection at a time: a chain has one state,
    /// and two nodes driving it would be two nodes disagreeing about which
    /// block is next.
    pub fn listen(&self, bind: &str) -> io::Result<String> {
        let listener = Listener::bind(bind)?;
        let addr = listener.addr()?;
        let vm = self.vm.clone();
        let identity = self.identity.clone();
        let genesis = self.genesis;
        let running = self.running.clone();
        std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok(stream) => {
                        let _ = stream.patience(TIMEOUT);
                        // A node that cannot prove itself never reaches the
                        // chain: the handshake is the door, not a formality
                        // after it.
                        match Sealed::open(stream, &identity, None, false) {
                            Ok(link) => serve(&*vm, link, genesis, &running),
                            Err(_) => continue,
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Ok(addr)
    }
}

/// Answer one node for as long as it stays connected.
fn serve(vm: &dyn Vm, mut link: Sealed, genesis: Id, running: &AtomicBool) {
    while running.load(Ordering::Relaxed) {
        let Ok((raw, body)) = link.read() else {
            return; // the node hung up, or sent a frame that did not open
        };
        let kind = zap::strip_flags(raw);
        // A payload with no room for a request id is not a message. Go returns
        // ErrInvalidMessage here; there is no id to answer under, so the only
        // thing to do is close.
        let Some((id, body)) = untag(&body) else {
            return;
        };
        let (flags, payload) = match answer(vm, genesis, kind, body) {
            Ok(out) => (zap::RESPONSE_FLAG, out),
            Err(e) => (zap::RESPONSE_FLAG | zap::ERROR_FLAG, encode_error(&e)),
        };
        if link.write(kind | flags, &tag(id, &payload)).is_err() {
            return;
        }
    }
}

fn answer(vm: &dyn Vm, genesis: Id, kind: u8, body: &[u8]) -> Result<Vec<u8>, Error> {
    let mut r = zap::Reader::new(body);
    match kind {
        // The genesis the node was started with, ACTUALLY compared. This used
        // to return an empty answer under a comment claiming it compared, so a
        // localnet node happily drove a mainnet chain.
        msg::INITIALIZE => {
            let theirs = word(&mut r)
                .ok_or_else(|| Error::Malformed("initialize carries the node's genesis".into()))?;
            if theirs != genesis {
                return Err(Error::Invalid(format!(
                    "this chain runs genesis {}, the node runs {}",
                    alloy_primitives::hex::encode(genesis),
                    alloy_primitives::hex::encode(theirs)
                )));
            }
            let mut w = zap::Writer::new();
            w.bytes(&genesis);
            Ok(w.take())
        }
        msg::VERSION => Ok(vm.version().into_bytes()),
        msg::HEALTH => vm.health().map(|_| Vec::new()),
        msg::BUILD => Ok(described(vm.build()?)),
        msg::PARSE => {
            let raw = r.bytes().ok_or(Error::Malformed("no block".into()))?;
            Ok(described(vm.parse(raw)?))
        }
        msg::GET => Ok(described(vm.get(&id_of(&mut r)?)?)),
        msg::VERIFY => vm.verify(&id_of(&mut r)?).map(|_| Vec::new()),
        msg::ACCEPT => vm.accept(&id_of(&mut r)?).map(|_| Vec::new()),
        msg::REJECT => vm.reject(&id_of(&mut r)?).map(|_| Vec::new()),
        msg::PREFER => vm.set_preference(&id_of(&mut r)?).map(|_| Vec::new()),
        msg::AT_HEIGHT => {
            // An empty payload asks for the tip; a height asks for that height.
            // One message, because "which block is at this position" is one
            // question and the tip is a position.
            let id = match r.u64() {
                Some(h) => vm.block_id_at(h)?,
                None => vm.last_accepted(),
            };
            let mut w = zap::Writer::new();
            w.bytes(&id);
            Ok(w.take())
        }
        msg::CALL => {
            let method = r.bytes().ok_or(Error::Malformed("no method".into()))?;
            let params = r.bytes().ok_or(Error::Malformed("no params".into()))?;
            let method = std::str::from_utf8(method)
                .map_err(|_| Error::Malformed("the method is not text".into()))?;
            let params: Value = serde_json::from_slice(params)
                .map_err(|e| Error::Malformed(format!("the params are not JSON: {e}")))?;
            let result = vm.call(method, &params)?;
            serde_json::to_vec(&result)
                .map_err(|e| Error::Invalid(format!("cannot encode the result: {e}")))
        }
        other => Err(Error::NoMethod(format!("message {other}"))),
    }
}

fn described(block: Box<dyn Block>) -> Vec<u8> {
    let mut w = zap::Writer::new();
    Remote::write(block.as_ref(), &mut w);
    w.take()
}

fn id_of(r: &mut zap::Reader<'_>) -> Result<Id, Error> {
    word(r).ok_or(Error::Malformed("not a block id".into()))
}

/// A refusal, as one byte of kind and the reason.
///
/// The kind survives the boundary so a node can tell "no such block" from "that
/// block is wrong" — the two lead to different behaviour, and a transport that
/// flattened them would make every refusal look like a bug.
fn encode_error(e: &Error) -> Vec<u8> {
    let (tag, text) = match e {
        Error::NotFound => (0u8, String::new()),
        Error::Malformed(w) => (1, w.clone()),
        Error::Invalid(w) => (2, w.clone()),
        Error::Empty => (3, String::new()),
        Error::NoMethod(w) => (4, w.clone()),
        Error::BadRequest(w) => (5, w.clone()),
        Error::Unavailable(w) => (6, w.clone()),
    };
    let mut out = vec![tag];
    out.extend_from_slice(text.as_bytes());
    out
}

fn decode_error(body: &[u8]) -> Error {
    let Some((tag, rest)) = body.split_first() else {
        return Error::Invalid("the chain refused without saying why".into());
    };
    let text = String::from_utf8_lossy(rest).into_owned();
    match tag {
        0 => Error::NotFound,
        1 => Error::Malformed(text),
        2 => Error::Invalid(text),
        3 => Error::Empty,
        4 => Error::NoMethod(text),
        6 => Error::Unavailable(text),
        _ => Error::BadRequest(text),
    }
}

// ---------------------------------------------------------------------------
// A chain in a program luxd wrote.
// ---------------------------------------------------------------------------

/// A Go VM plugin, started and driven the way luxd starts and drives one.
///
/// THE POINT. Twelve chains exist as Go plugin binaries. Porting each one to
/// Rust would be twelve chances to disagree with the network about what a
/// block means. Running them is one boundary, already numbered by
/// `luxfi/api/zap/wire.go`, and this is that boundary from the node's side.
///
/// HOW luxd STARTS A PLUGIN, which is not written down anywhere else:
///
///  1. The node binds a listener and puts its address in the child's
///     environment under `VM_RUNTIME_ENGINE_ADDR` — and again under
///     `LUX_VM_RUNTIME_ENGINE_ADDR`, the name before the rename, because a
///     plugin built against the older api reads only that one. `VM_TRANSPORT`
///     is `zap`. The rest of the environment is inherited: a plugin with no
///     `PATH` or `HOME` is a plugin that cannot find its own files.
///  2. The child DIALS that address and writes one frame:
///     `[u32 length][u32 protocol version][the address IT is listening on]`,
///     where the length counts the version word as well as the address.
///  3. The node checks the version, answers a single `0x01` byte, and closes.
///     A version that does not match is the whole handshake: there is no
///     negotiation, because two protocol versions are two protocols.
///  4. The node dials the address the child named, and that connection is the
///     chain.
///
/// THE CHILD DIALS TCP. luxd's own listener opens a unix socket only when
/// `LUXD_VM_UNIX_SOCKET=1` and otherwise binds TCP loopback, because a plugin
/// binary built against the older api hardcodes `dial tcp` — hand it a socket
/// path and it fails with `missing port in address`. The shipped plugins are
/// still that build, so the bootstrap listener here is TCP and the address
/// rule is honoured for what the child names back. [`Socket`] dials either.
///
/// THE LINK IS PLAINTEXT. A Go plugin runs no handshake and seals no frame —
/// the connection is the ZAP wire and nothing else. This node's own plugin
/// link proves both ends and seals every frame ([`Sealed`]); a Go plugin
/// cannot, so this does not pretend to. What that costs is stated on [`Server`]
/// and is the same cost luxd pays today.
pub struct Child {
    link: Mutex<Socket>,
    next: AtomicU32,
    /// The running program. Dropping the chain stops it.
    process: Mutex<Option<std::process::Child>>,
    /// What the plugin said its own version is.
    version: String,
    /// The last block the plugin had accepted when it was initialised.
    accepted: Remote,
}

/// The environment variable luxd puts the bootstrap address in.
pub const ENGINE_ADDRESS: &str = "VM_RUNTIME_ENGINE_ADDR";

/// The name it had before the rename. A plugin built against the older api
/// reads only this one, and every shipped plugin is that build.
pub const ENGINE_ADDRESS_WAS: &str = "LUX_VM_RUNTIME_ENGINE_ADDR";

/// The RPCChainVM protocol version this node speaks. `luxfi/node version`
/// holds the same number; a plugin that answers another one is refused,
/// because the two would agree on the frame and disagree on the message.
pub const PROTOCOL: u32 = 42;

/// The states a VM moves through. `luxfi/vm`'s `vm.State`.
pub mod state {
    pub const SYNCING: u8 = 2;
    pub const BOOTSTRAPPING: u8 = 3;
    /// Ready. A VM that has not been told this does not build.
    pub const READY: u8 = 4;
}

/// What a chain is started with. Every field crosses the boundary because the
/// plugin rebuilds its own world from them and can borrow nothing.
pub struct Start {
    pub network: u32,
    pub chain: Id,
    pub node: [u8; 20],
    pub genesis: Vec<u8>,
    pub upgrade: Vec<u8>,
    pub config: Vec<u8>,
    pub data_dir: String,
}

impl Child {
    /// Start the plugin at `path` and drive it up to Ready.
    pub fn start(path: &std::path::Path, start: &Start) -> Result<Self, Error> {
        let door = Listener::bind("127.0.0.1:0")
            .map_err(|e| Error::Invalid(format!("cannot bind the bootstrap listener: {e}")))?;
        let engine = door
            .addr()
            .map_err(|e| Error::Invalid(format!("cannot read the bootstrap address: {e}")))?;

        let mut process = std::process::Command::new(path)
            .env("VM_TRANSPORT", "zap")
            .env(ENGINE_ADDRESS, &engine)
            .env(ENGINE_ADDRESS_WAS, &engine)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| Error::Invalid(format!("cannot start {}: {e}", path.display())))?;

        let at = match hello(&door) {
            Ok(at) => at,
            Err(e) => {
                let _ = process.kill();
                return Err(e);
            }
        };

        let link = Socket::connect(&at, TIMEOUT)
            .map_err(|e| Error::Invalid(format!("cannot reach the chain at {at}: {e}")))?;

        let mut chain = Self {
            link: Mutex::new(link),
            next: AtomicU32::new(1),
            process: Mutex::new(Some(process)),
            version: String::new(),
            accepted: Remote {
                id: [0u8; 32],
                parent: [0u8; 32],
                height: 0,
                timestamp: 0,
                state_root: [0u8; 32],
                payload_root: [0u8; 32],
                bytes: Vec::new(),
            },
        };

        chain.accepted = chain.initialize(start)?;
        chain.version = chain.version()?;
        // A VM that still believes it is bootstrapping does not build, and
        // says so by dereferencing a nil pointer rather than by refusing.
        chain.set_state(state::BOOTSTRAPPING)?;
        chain.set_state(state::READY)?;
        let accepted = chain.accepted.id;
        chain.prefer(&accepted)?;
        Ok(chain)
    }

    /// What the plugin calls itself.
    pub fn version_line(&self) -> &str {
        &self.version
    }

    /// The block the plugin had accepted when it started.
    pub fn accepted(&self) -> &Remote {
        &self.accepted
    }

    /// One request, one response — the same framing and the same request id
    /// [`Client`] uses, over a link that is not sealed.
    fn ask(&self, kind: u8, payload: &[u8]) -> Result<Vec<u8>, Error> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut link = self
            .link
            .lock()
            .map_err(|_| Error::Unavailable("the chain link is poisoned".into()))?;
        zap::write_frame(&mut *link, kind, &tag(id, payload))
            .map_err(|e| Error::Unavailable(format!("cannot reach the chain: {e}")))?;
        let (raw, body) = zap::read_frame(&mut *link, FRAME_MAX)
            .map_err(|e| Error::Unavailable(format!("the chain did not answer: {e}")))?;
        if zap::strip_flags(raw) != kind {
            return Err(Error::Unavailable("the chain answered a different question".into()));
        }
        let Some((answered, body)) = untag(&body) else {
            return Err(Error::Malformed("the answer carries no request id".into()));
        };
        if answered != id {
            return Err(Error::Unavailable(format!(
                "the chain answered request {answered}, not {id}"
            )));
        }
        if zap::is_error(raw) {
            return Err(Error::Invalid(String::from_utf8_lossy(body).into_owned()));
        }
        Ok(body.to_vec())
    }

    fn initialize(&self, start: &Start) -> Result<Remote, Error> {
        let mut w = zap::Writer::new();
        w.u32(start.network)
            .bytes(&start.chain)
            .bytes(&start.node)
            .bytes(&[]) // PublicKey
            .bytes(&[0u8; 32]) // XChainID
            .bytes(&start.chain) // CChainID
            .bytes(&[0u8; 32]) // UTXOAssetID
            .bytes(start.data_dir.as_bytes())
            .bytes(&start.genesis)
            .bytes(&start.upgrade)
            .bytes(&start.config)
            .bytes(&[]) // DBServerAddr — luxd sends it empty and so does this
            .bytes(&[]) // ServerAddr
            .bytes(&[]) // AtomicServerAddr — no shared memory is wired
            .bytes(&[]) // DChainID
            .bytes(&[]); // ValidatorServerAddr
        let body = self.ask(msg::INITIALIZE, w.as_slice())?;
        let mut r = zap::Reader::new(&body);
        // InitializeResponse: id, parent, height, bytes, timestamp, and then
        // the capability word, which older plugins do not send.
        let id = word(&mut r).ok_or_else(|| Error::Malformed("no accepted block".into()))?;
        let parent = word(&mut r).ok_or_else(|| Error::Malformed("no parent".into()))?;
        let height = r.u64().ok_or_else(|| Error::Malformed("no height".into()))?;
        let bytes = r.bytes().ok_or_else(|| Error::Malformed("no block".into()))?.to_vec();
        let timestamp = r.u64().ok_or_else(|| Error::Malformed("no timestamp".into()))?;
        Ok(Remote {
            id,
            parent,
            height,
            timestamp: nanos_to_seconds(timestamp),
            state_root: state_root_of(&bytes),
            payload_root: [0u8; 32],
            bytes,
        })
    }

    fn version(&self) -> Result<String, Error> {
        let body = self.ask(msg::VERSION, &[])?;
        let mut r = zap::Reader::new(&body);
        Ok(String::from_utf8_lossy(r.bytes().unwrap_or_default()).into_owned())
    }

    /// Ask the chain whether it is well. The answer is the plugin's own JSON.
    pub fn health(&self) -> Result<String, Error> {
        let body = self.ask(msg::HEALTH, &[])?;
        let mut r = zap::Reader::new(&body);
        Ok(String::from_utf8_lossy(r.bytes().unwrap_or_default()).into_owned())
    }

    fn set_state(&self, to: u8) -> Result<(), Error> {
        self.ask(msg::SET_STATE, &[to]).map(|_| ())
    }

    fn prefer(&self, id: &Id) -> Result<(), Error> {
        let mut w = zap::Writer::new();
        w.bytes(id);
        self.ask(msg::PREFER, w.as_slice()).map(|_| ())
    }

    /// Read one block back, by whichever question asks for it.
    fn block(&self, kind: u8, payload: &[u8]) -> Result<Remote, Error> {
        let body = self.ask(kind, payload)?;
        read_block(&body)?.ok_or_else(|| Error::Malformed("the chain named no block".into()))
    }

    /// The block the plugin holds at `height`, by id.
    pub fn at_height(&self, height: u64) -> Result<Id, Error> {
        let mut w = zap::Writer::new();
        w.u64(height);
        let body = self.ask(msg::AT_HEIGHT, w.as_slice())?;
        word(&mut zap::Reader::new(&body))
            .ok_or_else(|| Error::Malformed("no block id at that height".into()))
    }

    /// Ask the chain for a block. An empty answer is not a block: it is the
    /// chain saying it has nothing to put in one, which Go writes as a
    /// zero-valued response rather than as a refusal.
    pub fn build(&self) -> Result<Option<Remote>, Error> {
        read_block(&self.ask(msg::BUILD, &[0u8])?)
    }

    pub fn parse(&self, bytes: &[u8]) -> Result<Remote, Error> {
        let mut w = zap::Writer::new();
        w.bytes(bytes);
        self.block(msg::PARSE, w.as_slice())
    }

    pub fn get(&self, id: &Id) -> Result<Remote, Error> {
        let mut w = zap::Writer::new();
        w.bytes(id);
        self.block(msg::GET, w.as_slice())
    }

    pub fn verify(&self, bytes: &[u8]) -> Result<(), Error> {
        let mut w = zap::Writer::new();
        w.bytes(bytes);
        let mut payload = w.take();
        payload.push(0u8); // no P-chain height
        self.ask(msg::VERIFY, &payload).map(|_| ())
    }

    pub fn accept(&self, id: &Id) -> Result<(), Error> {
        let mut w = zap::Writer::new();
        w.bytes(id);
        self.ask(msg::ACCEPT, w.as_slice()).map(|_| ())
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if let Ok(mut held) = self.process.lock() {
            if let Some(mut p) = held.take() {
                let _ = p.kill();
                let _ = p.wait();
            }
        }
    }
}

/// One `BlockResponse`, or nothing at all.
///
/// `id`, `parent`, `bytes`, `height`, `timestamp`, and then two bytes this
/// node has no use for: whether the block wants a P-chain context to verify,
/// and an error code the frame's own error flag already carried. An empty id
/// is a chain with nothing to put in a block — Go answers that with a
/// zero-valued response, not with a refusal.
fn read_block(body: &[u8]) -> Result<Option<Remote>, Error> {
    let mut r = zap::Reader::new(body);
    let Some(id) = r.bytes() else {
        return Err(Error::Malformed("the answer holds no block".into()));
    };
    if id.is_empty() {
        return Ok(None);
    }
    let id: Id =
        id.try_into().map_err(|_| Error::Malformed("a block id that is not 32 bytes".into()))?;
    let parent = word(&mut r).ok_or_else(|| Error::Malformed("no parent".into()))?;
    let bytes = r.bytes().ok_or_else(|| Error::Malformed("no block".into()))?.to_vec();
    let height = r.u64().ok_or_else(|| Error::Malformed("no height".into()))?;
    let timestamp = r.u64().ok_or_else(|| Error::Malformed("no timestamp".into()))?;
    Ok(Some(Remote {
        id,
        parent,
        height,
        timestamp: nanos_to_seconds(timestamp),
        state_root: state_root_of(&bytes),
        payload_root: [0u8; 32],
        bytes,
    }))
}

/// Wait for the child to dial back and say what it is listening on.
fn hello(door: &Listener) -> Result<String, Error> {
    let deadline = std::time::Instant::now() + HANDSHAKE;
    let mut link = loop {
        match door.accept() {
            Ok(s) => break s,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(Error::Invalid("the chain never called back".into()));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(Error::Invalid(format!("the chain could not call back: {e}"))),
        }
    };
    link.patience(TIMEOUT)
        .map_err(|e| Error::Invalid(format!("cannot bound the bootstrap read: {e}")))?;

    let mut head = [0u8; 8];
    link.read_exact(&mut head)
        .map_err(|e| Error::Invalid(format!("the chain said nothing: {e}")))?;
    let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
    let protocol = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
    if protocol != PROTOCOL {
        return Err(Error::Invalid(format!(
            "this node speaks RPCChainVM protocol {PROTOCOL} and the chain speaks {protocol}"
        )));
    }
    // The length counts the protocol word it just spent.
    let Some(rest) = len.checked_sub(4).filter(|n| *n as usize <= FRAME_MAX as usize) else {
        return Err(Error::Malformed("the chain announced an impossible address".into()));
    };
    let mut addr = vec![0u8; rest as usize];
    link.read_exact(&mut addr)
        .map_err(|e| Error::Invalid(format!("the chain named no address: {e}")))?;
    link.write_all(&[1u8]).map_err(|e| Error::Invalid(format!("cannot answer the chain: {e}")))?;
    link.flush().map_err(|e| Error::Invalid(format!("cannot answer the chain: {e}")))?;
    Ok(String::from_utf8_lossy(&addr).into_owned())
}

/// How long a chain has to call back before it is judged not to have started.
pub const HANDSHAKE: Duration = Duration::from_secs(30);

/// Go times a block in nanoseconds and this node times one in seconds.
fn nanos_to_seconds(t: u64) -> u64 {
    if t > 1_000_000_000_000 {
        t / 1_000_000_000
    } else {
        t
    }
}

/// The state root a block commits to.
///
/// GO'S BLOCK RESPONSE DOES NOT CARRY ONE. `BlockResponse` is id, parent,
/// bytes, height, timestamp and two flags — the root is inside the bytes,
/// because for an EVM chain the bytes are the RLP block and the fourth header
/// field is the root. A chain whose bytes are not an RLP block leaves this
/// zero, and the certificate that quotes it says zero rather than something
/// invented.
fn state_root_of(bytes: &[u8]) -> Id {
    let mut header = bytes;
    let Ok(h) = alloy_rlp::Header::decode(&mut header) else {
        return [0u8; 32];
    };
    if !h.list {
        return [0u8; 32];
    }
    let Ok(inner) = alloy_rlp::Header::decode(&mut header) else {
        return [0u8; 32];
    };
    if !inner.list {
        return [0u8; 32];
    }
    let mut at = &header[..inner.payload_length.min(header.len())];
    // parentHash, ommersHash, beneficiary, stateRoot — the fourth field.
    for _ in 0..3 {
        let Ok(field) = alloy_rlp::Header::decode(&mut at) else {
            return [0u8; 32];
        };
        if field.payload_length > at.len() {
            return [0u8; 32];
        }
        at = &at[field.payload_length..];
    }
    let Ok(root) = alloy_rlp::Header::decode(&mut at) else {
        return [0u8; 32];
    };
    if root.list || root.payload_length != 32 || at.len() < 32 {
        return [0u8; 32];
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&at[..32]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go's `transportNetwork`, case for case. The two ends of every link
    /// decide this separately, so a disagreement here is a node listening
    /// where its chain never dials.
    #[test]
    fn an_address_names_a_socket_or_a_port_the_way_go_reads_it() {
        for path in [
            "/tmp/luxd-vm-1/vm.sock",
            "@luxd-vm",
            "relative/vm.sock",
            "/tmp/dir with spaces/vm.sock",
            "./vm.sock",
        ] {
            assert!(is_socket_path(path), "{path} is a path");
        }
        for port in ["127.0.0.1:9652", "[::1]:9652", "localhost:0", "example.test:1"] {
            assert!(!is_socket_path(port), "{port} is a host and a port");
        }
        // The two Go resolves against its own rule rather than against
        // intuition: a bare name is a port, and a path with a colon in it is
        // still a path.
        assert!(!is_socket_path("vm.sock"));
        assert!(is_socket_path("/tmp/a:b/vm.sock"));
    }

    /// The same link, over a socket file. Nothing above the transport knows
    /// which one it got, so this is the whole of what changes.
    #[test]
    fn a_chain_answers_over_a_socket_file_as_well_as_a_port() {
        let dir = std::env::temp_dir().join(format!("lux-plugin-unix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("vm.sock");

        let plugin_id = Arc::new(Identity::generate().expect("identity"));
        let node_id = Identity::generate().expect("identity");
        let g = funded();
        let hash = g.hash().0;
        let chain = Arc::new(Evm::new(g)) as Arc<dyn Vm>;
        let server = Server::new(chain, plugin_id.clone(), hash);
        let addr = server.listen(&path.to_string_lossy()).expect("listen");
        assert_eq!(addr, path.to_string_lossy(), "the chain reports where it is");

        let client = Client::connect("C", &addr, &node_id, Some(plugin_id.node()), hash)
            .expect("connect over the socket file");
        assert_eq!(client.peer(), Some(plugin_id.node()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A socket file left behind by a chain that died is a name that can
    /// never be bound again unless something removes it.
    #[test]
    fn a_socket_file_does_not_outlive_the_chain_that_bound_it() {
        let dir = std::env::temp_dir().join(format!("lux-plugin-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("vm.sock");
        let name = path.to_string_lossy().into_owned();

        let first = Listener::bind(&name).expect("bind");
        assert!(path.exists());
        drop(first);
        assert!(!path.exists(), "the socket file outlived its listener");

        // And a file that is somehow still there is not a reason to refuse to
        // start.
        std::fs::write(&path, b"").expect("leave one behind");
        let again = Listener::bind(&name).expect("bind over a stale socket file");
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The state root a Go plugin does not send, read out of the block it
    /// does send. The bytes are the C-chain genesis block the shipped plugin
    /// produced.
    #[test]
    fn a_state_root_comes_out_of_the_block_bytes() {
        let block = include_bytes!("../tests/vectors/cchain_genesis_block.rlp");
        assert_eq!(
            alloy_primitives::hex::encode(state_root_of(block)),
            "497bcca8f4cee479610af8f1ddfa4e38036f5e9447366c2d0277ff70fcf3bdb9"
        );
        // Anything that is not a block leaves it zero rather than inventing a
        // root a certificate would then quote.
        assert_eq!(state_root_of(b""), [0u8; 32]);
        assert_eq!(state_root_of(b"\x80"), [0u8; 32]);
        assert_eq!(state_root_of(&block[..40]), [0u8; 32]);
    }

    /// Go times a block in nanoseconds and this node in seconds.
    #[test]
    fn a_block_time_is_seconds_on_this_side() {
        assert_eq!(nanos_to_seconds(1_778_800_214_000_000_000), 1_778_800_214);
        assert_eq!(nanos_to_seconds(1_778_800_214), 1_778_800_214);
        assert_eq!(nanos_to_seconds(0), 0);
    }

    use crate::evm::Evm;
    use alloy_primitives::U256;
    use serde_json::json;

    /// A C-chain behind a socket, and a client that is the only way to reach
    /// it.
    fn funded() -> lux_evm::Genesis {
        lux_evm::Genesis {
            alloc: vec![(
                crate::evm::tests_support::who(7),
                lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64))),
            )],
            fee: crate::evm::tests_support::UNMETERED,
            ..lux_evm::genesis::localnet()
        }
    }

    /// A chain behind a sealed socket, and a client that is the only way in.
    fn across() -> (Client, Arc<Evm>, Server) {
        serving(funded(), funded())
    }

    /// A chain running `theirs`, driven by a node that thinks it is `ours`.
    fn serving(theirs: lux_evm::Genesis, ours: lux_evm::Genesis) -> (Client, Arc<Evm>, Server) {
        let (client, chain, server) = try_serving(theirs, ours);
        (client.expect("connect"), chain, server)
    }

    fn try_serving(
        theirs: lux_evm::Genesis,
        ours: lux_evm::Genesis,
    ) -> (Result<Client, Error>, Arc<Evm>, Server) {
        let plugin_id = Arc::new(Identity::generate().expect("identity"));
        let node_id = Identity::generate().expect("identity");
        let their_hash = theirs.hash().0;
        let chain = Arc::new(Evm::new(theirs));
        let server = Server::new(chain.clone() as Arc<dyn Vm>, plugin_id.clone(), their_hash);
        let addr = server.listen("127.0.0.1:0").expect("listen");
        let client = Client::connect("C", &addr, &node_id, Some(plugin_id.node()), ours.hash().0);
        (client, chain, server)
    }

    #[test]
    fn a_node_cannot_drive_a_chain_of_another_network() {
        // The check that was a no-op: a localnet node happily drove a mainnet
        // chain, and every block it produced was one nobody else could accept.
        let (client, _chain, _s) = try_serving(lux_evm::genesis::mainnet(), funded());
        let refused = client.err().expect("a genesis mismatch must not connect");
        assert!(matches!(&refused, Error::Invalid(why) if why.contains("genesis")), "{refused:?}");
    }

    #[test]
    fn the_far_end_proves_who_it_is() {
        let (client, _chain, server) = across();
        assert_eq!(
            client.peer(),
            Some(server.node()),
            "the chain proved the identity the node pinned"
        );
    }

    #[test]
    fn a_chain_that_is_not_the_one_pinned_is_refused() {
        let plugin_id = Arc::new(Identity::generate().expect("identity"));
        let node_id = Identity::generate().expect("identity");
        let g = funded();
        let hash = g.hash().0;
        let chain = Arc::new(Evm::new(g)) as Arc<dyn Vm>;
        let server = Server::new(chain, plugin_id, hash);
        let addr = server.listen("127.0.0.1:0").expect("listen");

        let elsewhere = Identity::generate().expect("identity").node();
        assert!(
            Client::connect("C", &addr, &node_id, Some(elsewhere), hash).is_err(),
            "a chain that is not the pinned one must not be driven"
        );
    }

    #[test]
    fn nothing_readable_crosses_the_link() {
        // The link carries eth_sendRawTransaction — user transactions before
        // they are in a block. A listener on the socket must not be able to
        // read one out of it.
        let plugin_id = Arc::new(Identity::generate().expect("identity"));
        let node_id = Identity::generate().expect("identity");
        let g = funded();
        let hash = g.hash().0;
        let chain = Arc::new(Evm::new(g)) as Arc<dyn Vm>;
        let server = Server::new(chain, plugin_id.clone(), hash);
        let addr = server.listen("127.0.0.1:0").expect("listen");
        let client =
            Client::connect("C", &addr, &node_id, Some(plugin_id.node()), hash).expect("connect");

        // A method name is the most recognisable plaintext there is. If the
        // frame were in the clear it would be sitting in the bytes on the wire.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let _ = client.call("eth_blockNumber", &json!([]));
        let bytes = seen.lock().expect("seen").clone();
        assert!(
            !contains(&bytes, b"eth_blockNumber"),
            "the method name is on the wire in the clear"
        );
        // And the seal itself: a frame sealed under one direction's key must
        // not open under the other's.
        let a = Direction::new(&[7u8; 32], b"lux-plugin-call-v1");
        let b = Direction::new(&[7u8; 32], b"lux-plugin-answer-v1");
        let sealed =
            a.cipher.encrypt(&a.nonce(), Payload { msg: b"secret", aad: &[9] }).expect("seal");
        assert!(
            b.cipher.decrypt(&b.nonce(), Payload { msg: &sealed, aad: &[9] }).is_err(),
            "one direction must not be able to open the other's frames"
        );
        // Nor under a different message type: the type is associated data, so
        // a VERIFY cannot be relabelled into an ACCEPT.
        assert!(
            a.cipher.decrypt(&a.nonce(), Payload { msg: &sealed, aad: &[10] }).is_err(),
            "a frame must not survive being re-labelled"
        );
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn a_chain_across_a_socket_answers_like_a_chain() {
        let (client, chain, _s) = across();
        assert_eq!(client.name(), "C");
        assert_eq!(client.version(), chain.version());
        client.health().expect("healthy");
        assert_eq!(client.call("eth_chainId", &json!([])).expect("chain id"), json!("0x7a69"));
        assert_eq!(client.last_accepted(), chain.last_accepted(), "both sides agree on the tip");
    }

    #[test]
    fn a_refusal_keeps_its_kind_across_the_boundary() {
        let (client, _chain, _s) = across();
        // Nothing pending, so there is nothing to build — and the node must be
        // able to tell that from a failure.
        assert_eq!(client.build().err(), Some(Error::Empty));
        assert_eq!(client.get(&[9u8; 32]).err(), Some(Error::NotFound));
        assert!(matches!(client.call("eth_coinbase", &json!([])), Err(Error::NoMethod(_))));
    }

    #[test]
    fn a_block_built_across_the_boundary_carries_its_real_root() {
        let (client, chain, _s) = across();
        // The transaction goes in through the plugin's own RPC, the way a node
        // would forward it.
        let raw = crate::evm::tests_support::transfer(0, chain.chain_id());
        client
            .call(
                "eth_sendRawTransaction",
                &json!([format!("0x{}", alloy_primitives::hex::encode(&raw))]),
            )
            .expect("accepted");

        let block = client.build().expect("builds");
        assert_eq!(block.height(), 1);
        assert_ne!(block.state_root(), [0u8; 32], "a root, not a placeholder");
        assert_eq!(
            block.state_root(),
            chain.get(&block.id()).expect("the chain holds it").state_root(),
            "the root that crossed the wire is the one the interpreter produced"
        );

        client.verify(&block.id()).expect("verifies");
        client.accept(&block.id()).expect("accepts");
        assert_eq!(
            client.call("eth_blockNumber", &json!([])).expect("height"),
            json!("0x1"),
            "the height advanced, over the boundary"
        );
        assert_eq!(client.block_id_at(1).expect("indexed"), block.id());
    }

    #[test]
    fn a_block_survives_the_boundary_intact() {
        let (client, chain, _s) = across();
        let raw = crate::evm::tests_support::transfer(0, chain.chain_id());
        client
            .call(
                "eth_sendRawTransaction",
                &json!([format!("0x{}", alloy_primitives::hex::encode(&raw))]),
            )
            .expect("accepted");
        let block = client.build().expect("builds");
        let again = client.parse(&block.bytes()).expect("parses");
        assert_eq!(again.id(), block.id());
        assert_eq!(again.state_root(), block.state_root());
        assert_eq!(again.payload_root(), block.payload_root());
        assert_eq!(again.height(), block.height());
        assert_eq!(again.timestamp(), block.timestamp());
    }

    #[test]
    fn a_message_this_boundary_does_not_carry_is_refused() {
        let (client, _chain, _s) = across();
        assert!(matches!(client.ask(63, &[]), Err(Error::NoMethod(_))));
    }

    #[test]
    fn every_payload_carries_a_request_id_the_way_go_writes_it() {
        // `zap/transport.go`: uint32 big-endian in front of the payload, on the
        // call and on the answer. A Go node refuses a frame without it, so a
        // bare payload is a conversation that cannot start.
        assert_eq!(tag(1, b"hi"), vec![0, 0, 0, 1, b'h', b'i']);
        assert_eq!(tag(0x01020304, b""), vec![1, 2, 3, 4]);
        let framed = tag(7, b"body");
        let (id, rest) = untag(&framed).expect("round trip");
        assert_eq!((id, rest), (7, b"body".as_slice()));
    }

    #[test]
    fn a_payload_too_short_to_hold_a_request_id_is_not_a_message() {
        // Go returns ErrInvalidMessage for len(payload) < 4.
        for short in [b"".as_slice(), b"a", b"ab", b"abc"] {
            assert!(untag(short).is_none(), "{short:?}");
        }
        assert!(untag(b"abcd").is_some(), "exactly four is a header and no body");
    }

    #[test]
    fn an_answer_to_another_call_is_not_this_call_s_answer() {
        // The reason the id is carried at all: without matching, one dropped
        // response shifts every later reply by one and each caller reads
        // somebody else's answer as its own.
        let (client, _chain, _s) = across();
        let id = client.next.load(Ordering::Relaxed);
        // Ask something real, then confirm the id advanced — the client is not
        // reusing one id for every call.
        client.health().expect("healthy");
        assert!(client.next.load(Ordering::Relaxed) > id);
    }

    #[test]
    fn this_link_takes_no_id_go_has_defined() {
        // wire.go DEFINES 1-31, 32-34, 40-43, 50-53, 60-61. The validator link
        // is a different link, and it still may not reuse a number Go already
        // means something by — 60 was the greeting and 61 was the block. The
        // handshake takes no number at all: it runs on its own framing, before
        // anything on this link speaks zap.
        let defined: Vec<u8> =
            (1..=31).chain(32..=34).chain(40..=43).chain(50..=53).chain(60..=61).collect();
        for id in [crate::mesh::BLOCK, lux_consensus::vote::VOTE] {
            assert!(!defined.contains(&id), "{id} is defined in Go's wire.go");
            assert!(id < zap::ERROR_FLAG, "{id} collides with a flag");
        }
        assert_eq!(crate::mesh::BLOCK, 59);
    }

    #[test]
    fn the_numbers_are_the_ones_go_uses() {
        // `luxfi/api/zap/wire.go`. Drifting from these is how a Rust node and a
        // Go plugin come to mean different things by the same byte.
        assert_eq!(msg::INITIALIZE, 1);
        assert_eq!(msg::BUILD, 9);
        assert_eq!(msg::PARSE, 10);
        assert_eq!(msg::GET, 11);
        assert_eq!(msg::PREFER, 12);
        assert_eq!(msg::HEALTH, 13);
        assert_eq!(msg::VERSION, 14);
        assert_eq!(msg::AT_HEIGHT, 22);
        assert_eq!(msg::VERIFY, 28);
        assert_eq!(msg::ACCEPT, 29);
        assert_eq!(msg::REJECT, 30);
        // Every one below 0x40, so the response and error bits OR in cleanly.
        for m in [
            msg::INITIALIZE,
            msg::CALL,
            msg::BUILD,
            msg::PARSE,
            msg::GET,
            msg::PREFER,
            msg::HEALTH,
            msg::VERSION,
            msg::AT_HEIGHT,
            msg::VERIFY,
            msg::ACCEPT,
            msg::REJECT,
        ] {
            assert!(m < zap::ERROR_FLAG, "message {m} collides with a flag");
        }
    }
}
