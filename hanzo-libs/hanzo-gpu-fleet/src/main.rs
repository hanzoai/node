// SPDX-License-Identifier: Apache-2.0
//! `hanzo-fleet-worker` — the standalone BYO-GPU claim loop.
//!
//! This is the light, open worker Hanzo Desktop spawns as a managed child when
//! the owner turns "Share GPU" on (replacing the deprecated-for-desktop Go
//! `hanzo gpu connect`). It is also the isolated end-to-end proof target:
//!
//!   HANZO_TOKEN=<session bearer> \
//!   HANZO_COMFYUI_URL=http://127.0.0.1:8188 \
//!   HANZO_STUDIO_URL=https://studio.hanzo.ai \
//!   cargo run -p hanzo-gpu-fleet
//!
//! The exact same [`hanzo_gpu_fleet::run`] powers the in-daemon (hanzod) task.
//! NO secrets are read from or written to disk: the session token arrives via
//! env (from the desktop) and lives only in memory.

use std::time::Duration;

use anyhow::{anyhow, Result};
use hanzo_gpu_fleet::{
    EngineConfig, FleetConfig, DEFAULT_CLOUD_URL, DEFAULT_COMFYUI_URL, DEFAULT_JOBS_NS,
    DEFAULT_STUDIO_URL,
};
use tokio_util::sync::CancellationToken;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let token = std::env::var("HANZO_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| anyhow!("HANZO_TOKEN is required (the owner's IAM session bearer)"))?;

    let engine = std::env::var("HANZO_ENGINE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .map(|probe_url| EngineConfig {
            probe_url,
            advertise_url: std::env::var("HANZO_ENGINE_ADVERTISE_URL").ok().filter(|u| !u.is_empty()),
        });

    let cfg = FleetConfig {
        cloud_url: env_or("HANZO_CLOUD_URL", DEFAULT_CLOUD_URL),
        token,
        jobs_ns: env_or("HANZO_JOBS_NS", DEFAULT_JOBS_NS),
        comfyui_url: env_or("HANZO_COMFYUI_URL", DEFAULT_COMFYUI_URL),
        studio_url: env_or("HANZO_STUDIO_URL", DEFAULT_STUDIO_URL),
        hostname: std::env::var("HANZO_FLEET_HOSTNAME").ok().filter(|h| !h.trim().is_empty()),
        engine,
    };

    let cancel = CancellationToken::new();
    // Stop the loop cleanly on Ctrl-C / SIGTERM so the machine goes offline.
    let sig = cancel.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {}
                _ = term.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ctrl_c.await;
        }
        sig.cancel();
    });

    let result = hanzo_gpu_fleet::run(cfg, cancel).await;
    // Give the offline disconnect a moment to flush before exit.
    tokio::time::sleep(Duration::from_millis(100)).await;
    result
}
