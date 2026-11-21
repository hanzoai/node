//! The `/v1` prefix table and the `Subsystem` trait.
//!
//! A subsystem owns a route prefix, starts on the first frame that reaches it
//! and stops once it has been idle for its own interval. Nothing runs until a
//! frame needs it; an idle daemon holds a table and a listener, no timers.

use crate::zap::{Request, Response};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

pub type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A duplex byte stream a subsystem may take over (a Dev session).
pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}
pub type Conn = Box<dyn Duplex>;

/// What answers a request: one response, or raw ZAP-HTTP frames relayed as the
/// subsystem produces them (a streamed response).
pub enum Reply {
    Whole(Response),
    Frames(mpsc::Receiver<Vec<u8>>),
}

pub trait Subsystem: Send + Sync {
    fn name(&self) -> &'static str;
    /// The route prefix it owns, e.g. `/v1/dev`. Matches whole path segments.
    fn prefix(&self) -> &'static str;
    /// How long it may sit unused before `stop`. Resident subsystems ignore it.
    fn idle(&self) -> Duration {
        Duration::from_secs(120)
    }
    /// Answered by hanzod itself: no process to start or stop.
    fn resident(&self) -> bool {
        false
    }
    /// Bring it up. Called under the table's start lock, only when not running.
    fn start(&self) -> Fut<'_, io::Result<()>>;
    /// Answer one request. `who` is the verified caller (`owner/name`) or, on the
    /// socket, the machine's own active identity.
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>>;
    /// Take over a whole stream (a Dev session). `first` is the envelope already read.
    fn stream<'a>(&'a self, _first: Vec<u8>, _conn: Conn) -> Fut<'a, io::Result<()>> {
        Box::pin(async { Err(io::Error::new(io::ErrorKind::Unsupported, "no streams here")) })
    }
    fn stop(&self) -> Fut<'_, ()>;
    /// The private loopback port of an HTTP upstream, while one runs. The
    /// loopback view proxies a request for this prefix to it byte for byte,
    /// which is how a streamed reply stays a stream.
    fn http_port(&self) -> Option<u16> {
        None
    }
}

struct Slot {
    sub: Arc<dyn Subsystem>,
    running: AtomicBool,
    active: AtomicUsize,
    last: Mutex<Instant>,
    starting: tokio::sync::Mutex<()>,
}

/// Holds a subsystem busy while a request or stream uses it.
pub struct Busy(Arc<Slot>);

impl Drop for Busy {
    fn drop(&mut self) {
        *self.0.last.lock().unwrap() = Instant::now();
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Table {
    slots: Vec<Arc<Slot>>,
}

impl Table {
    pub fn new(subs: Vec<Arc<dyn Subsystem>>) -> Table {
        let slots = subs
            .into_iter()
            .map(|sub| {
                Arc::new(Slot {
                    running: AtomicBool::new(sub.resident()),
                    sub,
                    active: AtomicUsize::new(0),
                    last: Mutex::new(Instant::now()),
                    starting: tokio::sync::Mutex::new(()),
                })
            })
            .collect();
        Table { slots }
    }

    /// The longest-prefix match on whole path segments.
    fn find(&self, path: &str) -> Option<&Arc<Slot>> {
        self.slots
            .iter()
            .filter(|s| {
                let p = s.sub.prefix();
                path == p || path.strip_prefix(p).is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|s| s.sub.prefix().len())
    }

    /// The prefixes, for `GET /v1/hanzod`.
    pub fn prefixes(&self) -> Vec<(&'static str, &'static str, bool)> {
        self.slots.iter().map(|s| (s.sub.prefix(), s.sub.name(), s.running.load(Ordering::SeqCst))).collect()
    }

    /// Route `path`: start its subsystem if it is down, and hold it busy.
    pub async fn route(&self, path: &str) -> io::Result<(Arc<dyn Subsystem>, Busy)> {
        let slot = self.find(path).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such route"))?.clone();
        slot.active.fetch_add(1, Ordering::SeqCst);
        let busy = Busy(slot.clone());
        if !slot.running.load(Ordering::SeqCst) {
            let _g = slot.starting.lock().await;
            if !slot.running.load(Ordering::SeqCst) {
                // A missing program is the subsystem failing to start, not the route
                // failing to exist: keep the two apart for the caller.
                slot.sub.start().await.map_err(|e| match e.kind() {
                    io::ErrorKind::NotFound => io::Error::other(e.to_string()),
                    _ => e,
                })?;
                slot.running.store(true, Ordering::SeqCst);
                reap(slot.clone());
            }
        }
        Ok((slot.sub.clone(), busy))
    }

    /// Mark a subsystem down because its process died on its own; the next
    /// frame to its prefix starts it again.
    pub fn down(&self, prefix: &str) {
        if let Some(s) = self.slots.iter().find(|s| s.sub.prefix() == prefix) {
            if !s.sub.resident() {
                s.running.store(false, Ordering::SeqCst);
            }
        }
    }

    pub async fn stop_all(&self) {
        for s in &self.slots {
            if !s.sub.resident() && s.running.swap(false, Ordering::SeqCst) {
                s.sub.stop().await;
            }
        }
    }
}

/// While a subsystem runs, one task sleeps until it could be idle; when it is,
/// the subsystem is stopped and the task ends. Nothing sleeps while it is down.
fn reap(slot: Arc<Slot>) {
    if slot.sub.resident() {
        return;
    }
    tokio::spawn(async move {
        let idle = slot.sub.idle();
        loop {
            let since = slot.last.lock().unwrap().elapsed();
            if slot.active.load(Ordering::SeqCst) == 0 && since >= idle {
                if slot.running.swap(false, Ordering::SeqCst) {
                    slot.sub.stop().await;
                }
                return;
            }
            if !slot.running.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(idle.saturating_sub(since).max(Duration::from_millis(250))).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    struct Fake {
        prefix: &'static str,
        starts: AtomicU32,
        stops: AtomicU32,
    }

    impl Subsystem for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn prefix(&self) -> &'static str {
            self.prefix
        }
        fn idle(&self) -> Duration {
            Duration::from_millis(50)
        }
        fn start(&self) -> Fut<'_, io::Result<()>> {
            Box::pin(async {
                self.starts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
        fn frame<'a>(&'a self, _: &'a str, _: Request) -> Fut<'a, io::Result<Reply>> {
            Box::pin(async { Ok(Reply::Whole(Response::error(200, "ok"))) })
        }
        fn stop(&self) -> Fut<'_, ()> {
            Box::pin(async {
                self.stops.fetch_add(1, Ordering::SeqCst);
            })
        }
    }

    fn fake(prefix: &'static str) -> Arc<Fake> {
        Arc::new(Fake { prefix, starts: AtomicU32::new(0), stops: AtomicU32::new(0) })
    }

    #[tokio::test]
    async fn prefixes_match_whole_segments_and_the_longest_wins() {
        let (a, b) = (fake("/v1"), fake("/v1/dev"));
        let t = Table::new(vec![a.clone(), b.clone()]);
        assert_eq!(t.route("/v1/dev/session").await.unwrap().0.prefix(), "/v1/dev");
        assert_eq!(t.route("/v1/devices").await.unwrap().0.prefix(), "/v1");
        assert!(t.route("/healthz").await.is_err());
    }

    #[tokio::test]
    async fn a_subsystem_starts_once_on_demand_and_stops_when_idle() {
        let f = fake("/v1/x");
        let t = Table::new(vec![f.clone()]);
        assert_eq!(f.starts.load(Ordering::SeqCst), 0, "nothing starts before a frame");
        {
            let _a = t.route("/v1/x").await.unwrap();
            let _b = t.route("/v1/x/y").await.unwrap();
            assert_eq!(f.starts.load(Ordering::SeqCst), 1);
            tokio::time::sleep(Duration::from_millis(400)).await;
            assert_eq!(f.stops.load(Ordering::SeqCst), 0, "busy is not idle");
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(f.stops.load(Ordering::SeqCst), 1);
        let _c = t.route("/v1/x").await.unwrap();
        assert_eq!(f.starts.load(Ordering::SeqCst), 2, "the next frame starts it again");
    }
}
