// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Background `hanzo_gen::target::beat_and_serve` integration for `hanzod`.
//!
//! Keeps this machine `online` + `serving: true` in `/v1/agent/targets` whenever
//! signed in (`me::active()`) and no companion `hanzo link` process is already
//! running, so Cloud, Desktop, and Mobile `targetId`-routed `gen` runs execute
//! automatically on this machine.

use crate::me;
use crate::paths;
use hanzo_gen::context::{hostname, machine_id};
use hanzo_gen::embedded::StaticHost;
use hanzo_gen::Root;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

const DEFAULT_CLOUD_API: &str = "https://api.hanzo.ai";

pub struct TargetRunnerGuard(JoinHandle<()>);

impl TargetRunnerGuard {
    pub fn shutdown(self) {
        self.0.abort();
    }
}

impl Drop for TargetRunnerGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn target_runner_enabled() -> bool {
    if let Ok(v) = std::env::var("HANZOD_TARGET_RUNNER") {
        if matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ) {
            return false;
        }
    }
    !crate::tasks_worker::companion_hanzo_link_running()
}

fn configured_cloud_api() -> String {
    let cfg: Value = std::fs::read(paths::data().join("hanzod.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    cfg.get("cloud_api")
        .and_then(Value::as_str)
        .or_else(|| cfg.pointer("/tasks_worker/cloud_api").and_then(Value::as_str))
        .map(str::to_owned)
        .or_else(|| std::env::var("HANZO_API_URL").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLOUD_API.to_string())
        .trim_end_matches('/')
        .to_string()
}

pub fn maybe_spawn_target_runner() -> Option<TargetRunnerGuard> {
    if !target_runner_enabled() {
        return None;
    }

    let handle = tokio::spawn(async move {
        let id = loop {
            if let Some(active) = me::active() {
                break active;
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        };

        let root = Root::user().unwrap_or_else(|| Root::at(paths::data()));
        let cloud_api = configured_cloud_api();
        let refresh_cmd = hanzo_gen::exe::find_for_root(&root, "hanzo")
            .map(|bin| vec![bin.display().to_string(), "auth".into(), "token".into()])
            .unwrap_or_default();

        let host = Arc::new(
            StaticHost::cloud(cloud_api.clone(), id.owner.clone(), id.name, Arc::new(me::fresh_token))
                .with_org(Some(id.owner))
                .with_refresh_cmd(refresh_cmd)
                .with_hanzo_key(std::env::var("HANZO_API_KEY").ok()),
        );

        let mid = machine_id(&root);
        let name = hostname(&root);
        let (_beat, _runner) = hanzo_gen::target::beat_and_serve(
            host,
            &root,
            hanzo_gen::host::BRAND,
            &cloud_api,
            &mid,
            &name,
        );

        // Hold `_beat` and `_runner` guards alive until `TargetRunnerGuard` drops.
        std::future::pending::<()>().await;
    });

    Some(TargetRunnerGuard(handle))
}
