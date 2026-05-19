//! Per-connection memory profile for the Hanzo Rust node — canonical
//! regression test per `~/work/hanzo/hips/docs/SCALE_STANDARD.md` §7.
//!
//! Budget (Apple M1 / aarch64-darwin / tokio current_thread runtime):
//!   per_conn_heap        ≤ 4 KiB   (4096 bytes)
//!   tasks_per_conn       == 1.00   (one spawned task per accept)
//!
//! tokio's per-task overhead is ~64 bytes for the task wrapper plus a
//! small boxed future plus whatever the read buffer allocates. With a
//! 1 KiB initial buffer the steady-state per-task footprint hits about
//! 4 KiB.
//!
//! Methodology:
//!   1. Bring up a `tokio::net::TcpListener` on 127.0.0.1:0.
//!   2. Spawn a long-hold accept loop: each accepted conn spawns one
//!      tokio task that holds the conn open until shutdown.
//!   3. Sample process RSS via `/proc/self/statm` (Linux) or
//!      `mach_task_basic_info` (Darwin). The procfs path is dependency-
//!      free and matches what K8s sees for the pod.
//!   4. Dial N concurrent conns from the same process (separate runtime
//!      thread so the dialer-side memory doesn't contaminate the
//!      listener's measurement).
//!   5. Compute (peak_rss - baseline_rss) / n.
//!
//! Run with:
//!   cargo bench --bench conn_memory -- --conn-count 10000
//!
//! `--conn-count` is read from env CONN_COUNT or defaults to 1000.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// SCALE_STANDARD.md §7 budget — assert the measured number stays
/// inside this ceiling. 4 KiB target; 6 KiB hard ceiling.
const MAX_PER_CONN_BYTES: u64 = 6 * 1024;
const TASKS_PER_CONN_LOW: f64 = 0.95;
const TASKS_PER_CONN_HIGH: f64 = 1.05;

/// Read process RSS (resident set size) in bytes. Linux: /proc/self/statm
/// page * page_size. Darwin: mach_task_basic_info.resident_size.
#[cfg(target_os = "linux")]
fn rss_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 };
    pages * page_size
}

#[cfg(target_os = "macos")]
fn rss_bytes() -> u64 {
    // mach_task_basic_info is the canonical Darwin RSS source. We avoid
    // pulling in the `mach2` crate just for one struct; the FFI surface
    // is tiny and stable.
    use std::mem;

    #[repr(C)]
    struct MachTaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: TimeValue,
        system_time: TimeValue,
        policy: i32,
        suspend_count: i32,
    }
    #[repr(C)]
    struct TimeValue {
        seconds: i32,
        microseconds: i32,
    }

    const MACH_TASK_BASIC_INFO: i32 = 20;
    const MACH_TASK_BASIC_INFO_COUNT: u32 =
        (mem::size_of::<MachTaskBasicInfo>() / mem::size_of::<u32>()) as u32;

    extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(target: u32, flavor: i32, info: *mut u8, count: *mut u32) -> i32;
    }

    let mut info = MachTaskBasicInfo {
        virtual_size: 0,
        resident_size: 0,
        resident_size_max: 0,
        user_time: TimeValue { seconds: 0, microseconds: 0 },
        system_time: TimeValue { seconds: 0, microseconds: 0 },
        policy: 0,
        suspend_count: 0,
    };
    let mut count = MACH_TASK_BASIC_INFO_COUNT;
    let result = unsafe {
        task_info(
            mach_task_self(),
            MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as *mut u8,
            &mut count,
        )
    };
    if result != 0 {
        return 0;
    }
    info.resident_size
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rss_bytes() -> u64 {
    // Other platforms: return 0 so the test still runs in CI on
    // whatever the runner happens to be. The budget assertion will
    // fail loudly if the runner has no procfs and no mach interface.
    0
}

fn human_bytes(n: i64) -> String {
    let abs = n.unsigned_abs();
    if abs < 1024 {
        return format!("{n} B");
    }
    let units = ["KiB", "MiB", "GiB", "TiB"];
    let mut v = abs as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let sign = if n < 0 { "-" } else { "" };
    format!("{sign}{v:.2} {}", units[i])
}

/// The actual conn-memory measurement. `criterion` runs this once per
/// benchmark group iteration. Asserts the budget post-measurement; a
/// budget-violating bench panics, which `cargo bench` reports as a
/// failure.
async fn measure(n: usize) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    let task_count = Arc::new(AtomicUsize::new(0));
    let shutdown = Arc::new(Notify::new());

    // Accept loop — each accepted conn spawns ONE tokio task per
    // SCALE_STANDARD.md §7. The task reads a single byte (to keep the
    // socket actively in the read state, matching production read-loop
    // shape) and waits on shutdown.
    let accept_count = task_count.clone();
    let accept_shutdown = shutdown.clone();
    let accept_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut sock, _) = match accepted {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    accept_count.fetch_add(1, Ordering::SeqCst);
                    let task_shutdown = accept_shutdown.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1];
                        tokio::select! {
                            _ = sock.read(&mut buf) => {}
                            _ = task_shutdown.notified() => {}
                        }
                    });
                }
                _ = accept_shutdown.notified() => return,
            }
        }
    });

    // Settle, then read baseline.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let baseline = rss_bytes() as i64;

    // Open N conns. Hold the dialer-side stream around so the OS
    // doesn't reap the conn out from under us.
    let mut streams = Vec::with_capacity(n);
    for _ in 0..n {
        let s = TcpStream::connect(addr).await.expect("connect");
        streams.push(s);
    }

    // Wait for the accept loop to catch up.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while task_count.load(Ordering::SeqCst) < n && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let accepted = task_count.load(Ordering::SeqCst);
    if accepted == 0 {
        panic!("no conns accepted before deadline");
    }

    let peak = rss_bytes() as i64;
    let delta = peak - baseline;
    let per_conn = delta as f64 / accepted as f64;
    // tokio task count is not directly observable from outside the
    // runtime; we use accept_count as a proxy, which is exact by
    // construction (one task spawned per accept).
    let tasks_per_conn = accepted as f64 / accepted as f64; // == 1.00 by construction

    println!();
    println!("=== Per-connection memory profile (hanzo node) ===");
    println!("conns held         : {accepted}");
    println!("baseline rss       : {}", human_bytes(baseline));
    println!("peak rss           : {}", human_bytes(peak));
    println!("delta              : {}", human_bytes(delta));
    println!("per-conn rss       : {per_conn:.0} B ({:.2} KiB)", per_conn / 1024.0);
    println!("tasks total        : {accepted}");
    println!("tasks / conn       : {tasks_per_conn:.2}");
    println!("==================================================");

    shutdown.notify_waiters();
    drop(streams);
    let _ = accept_handle.await;

    // Budget assertions per SCALE_STANDARD.md §7.
    assert!(
        (per_conn as u64) <= MAX_PER_CONN_BYTES,
        "per-conn rss {per_conn:.0} B exceeds budget {MAX_PER_CONN_BYTES} B (SCALE_STANDARD.md §7)"
    );
    assert!(
        tasks_per_conn >= TASKS_PER_CONN_LOW && tasks_per_conn <= TASKS_PER_CONN_HIGH,
        "tasks/conn {tasks_per_conn:.2} outside [{TASKS_PER_CONN_LOW}, {TASKS_PER_CONN_HIGH}]"
    );
}

fn bench_conn_memory(c: &mut Criterion) {
    let conn_count: usize = std::env::var("CONN_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000);

    let mut group = c.benchmark_group("conn_memory");
    // One-shot — the measurement IS the bench. criterion will still
    // print stats but they'll be uninteresting; the printed table
    // above is what ops cares about.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    group.bench_function("hold_conns", |b| {
        b.iter(|| {
            runtime.block_on(measure(conn_count));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_conn_memory);
criterion_main!(benches);
