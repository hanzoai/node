// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Crash backoff tracker and periodic desired-state recovery pass.

use super::reconciler::Reconciler;
use super::state::{default_service_endpoint, now_unix, BackoffPolicy, PLATFORM_SERVICES};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

#[derive(Debug, Clone)]
struct BackoffEntry {
    failures: u32,
    next_retry_at: Instant,
    armed_for_crash: bool,
    last_recovered_at: Option<Instant>,
}

impl Default for BackoffEntry {
    fn default() -> Self {
        Self {
            failures: 0,
            next_retry_at: Instant::now(),
            armed_for_crash: false,
            last_recovered_at: None,
        }
    }
}

pub(super) struct BackoffTracker {
    policy: BackoffPolicy,
    entries: StdMutex<BTreeMap<String, BackoffEntry>>,
}

impl BackoffTracker {
    pub(super) fn new(policy: BackoffPolicy) -> Self {
        Self {
            policy,
            entries: StdMutex::new(BTreeMap::new()),
        }
    }

    pub(super) fn clear_key(&self, key: &str) {
        self.entries.lock().unwrap().remove(key);
    }

    pub(super) fn clear_all(&self) {
        self.entries.lock().unwrap().clear();
    }

    pub(super) fn reset_if_healthy(&self, key: &str, actual_running: bool, now: Instant) {
        if !actual_running {
            return;
        }
        let mut guard = self.entries.lock().unwrap();
        if let Some(entry) = guard.get_mut(key) {
            entry.armed_for_crash = false;
            if let Some(recovered_at) = entry.last_recovered_at {
                if now.saturating_duration_since(recovered_at) >= self.policy.reset_after {
                    entry.failures = 0;
                    entry.last_recovered_at = None;
                }
            }
        }
    }

    pub(super) fn gate(&self, key: &str, now: Instant) -> Option<u64> {
        let mut guard = self.entries.lock().unwrap();
        let entry = guard.entry(key.to_string()).or_default();
        if !entry.armed_for_crash {
            if let Some(recovered_at) = entry.last_recovered_at {
                if now.saturating_duration_since(recovered_at) >= self.policy.reset_after {
                    entry.failures = 0;
                }
            }
            let delay = self.policy.delay_for(entry.failures);
            entry.failures = entry.failures.saturating_add(1);
            entry.next_retry_at = now + delay;
            entry.armed_for_crash = true;
            if !delay.is_zero() {
                return Some(delay.as_millis().max(1) as u64);
            }
        }
        if now < entry.next_retry_at {
            let rem = entry
                .next_retry_at
                .saturating_duration_since(now)
                .as_millis()
                .max(1) as u64;
            return Some(rem);
        }
        None
    }

    pub(super) fn record_success(&self, key: &str, now: Instant) {
        let mut guard = self.entries.lock().unwrap();
        let entry = guard.entry(key.to_string()).or_default();
        entry.armed_for_crash = false;
        entry.last_recovered_at = Some(now);
    }

    pub(super) fn record_failure(&self, key: &str, now: Instant) {
        let mut guard = self.entries.lock().unwrap();
        let entry = guard.entry(key.to_string()).or_default();
        let delay = self.policy.delay_for(entry.failures);
        entry.failures = entry.failures.saturating_add(1);
        entry.next_retry_at = now + delay;
        entry.armed_for_crash = true;
    }

    pub(super) fn failures_snapshot(&self) -> BTreeMap<String, u32> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.failures))
            .collect()
    }
}

impl Reconciler {
    /// Observe actual runtime/platform/application health and restart crashed desired workloads
    /// subject to per-target bounded exponential backoff.
    pub async fn reconcile_once(&self, who: &str) -> Result<Value, (u16, String)> {
        let mut st = self.state.lock().await;
        let drifted = self.sync_locked(&mut st).await;
        let now = Instant::now();
        let mut mutated = drifted;
        let mut recovered: Vec<String> = Vec::new();
        let mut deferred: Vec<Value> = Vec::new();

        self.backoff.reset_if_healthy(
            "runtime",
            st.runtime.desired == "running" && st.runtime.actual == "running",
            now,
        );
        for name in PLATFORM_SERVICES {
            let healthy = st
                .platform
                .services
                .get(name)
                .is_some_and(|s| s.desired == "running" && s.actual == "running");
            self.backoff
                .reset_if_healthy(&format!("service:{name}"), healthy, now);
        }
        for (slug, app) in &st.applications {
            self.backoff.reset_if_healthy(
                &format!("app:{slug}"),
                app.desired == "running" && app.actual == "running",
                now,
            );
        }

        if st.runtime.desired == "running"
            && matches!(st.runtime.actual.as_str(), "stopped" | "crashed")
        {
            let key = "runtime".to_string();
            if let Some(retry_in_ms) = self.backoff.gate(&key, now) {
                deferred.push(json!({ "target": key, "retry_in_ms": retry_in_ms }));
            } else if self.ensure_runtime_running_locked(who, &mut st).await.is_ok() {
                self.backoff.record_success(&key, Instant::now());
                self.ledger.record(who, "lifecycle.reconcile.recover", &key);
                recovered.push(key);
                mutated = true;
            } else {
                self.backoff.record_failure(&key, Instant::now());
            }
        }

        for name in PLATFORM_SERVICES {
            let needs_recovery = st
                .platform
                .services
                .get(name)
                .is_some_and(|s| s.desired == "running" && s.actual != "running");
            if !needs_recovery {
                continue;
            }
            let key = format!("service:{name}");
            if let Some(retry_in_ms) = self.backoff.gate(&key, now) {
                deferred.push(json!({ "target": key, "retry_in_ms": retry_in_ms }));
                continue;
            }
            match self
                .executor
                .reconcile_service(name, "running", true, &st.runtime, &st.platform)
                .await
            {
                Ok((actual, endpoint)) => {
                    let ok = actual == "running";
                    if let Some(svc) = st.platform.services.get_mut(name) {
                        svc.actual = actual;
                        svc.endpoint = ok
                            .then(|| endpoint.or_else(|| default_service_endpoint(name)))
                            .flatten();
                    }
                    if ok {
                        self.backoff.record_success(&key, Instant::now());
                        self.ledger.record(who, "lifecycle.reconcile.recover", &key);
                        recovered.push(key);
                        mutated = true;
                    } else {
                        self.backoff.record_failure(&key, Instant::now());
                    }
                }
                Err(_) => {
                    self.backoff.record_failure(&key, Instant::now());
                }
            }
        }

        let app_slugs: Vec<String> = st.applications.keys().cloned().collect();
        for slug in app_slugs {
            let Some(app_clone) = st.applications.get(&slug).cloned() else {
                continue;
            };
            if app_clone.desired != "running" || app_clone.actual == "running" {
                continue;
            }
            let key = format!("app:{slug}");
            if let Some(retry_in_ms) = self.backoff.gate(&key, now) {
                deferred.push(json!({ "target": key, "retry_in_ms": retry_in_ms }));
                continue;
            }
            match self.executor.reconcile_app(&app_clone, "running").await {
                Ok(out) => {
                    let ok = out.actual == "running";
                    if let Some(app) = st.applications.get_mut(&slug) {
                        app.actual = out.actual;
                        app.pid = out.pid;
                        app.updated_at = now_unix();
                        if ok {
                            app.restart_count = app.restart_count.saturating_add(1);
                        }
                    }
                    if ok {
                        self.backoff.record_success(&key, Instant::now());
                        self.ledger.record(who, "lifecycle.reconcile.recover", &key);
                        recovered.push(key);
                        mutated = true;
                    } else {
                        self.backoff.record_failure(&key, Instant::now());
                    }
                }
                Err(_) => {
                    self.backoff.record_failure(&key, Instant::now());
                }
            }
        }

        if mutated {
            self.store
                .save(&mut st)
                .map_err(|e| (500, format!("persist reconcile state: {e}")))?;
        }

        let failures = self.backoff.failures_snapshot();
        Ok(json!({
            "ok": true,
            "recovered": recovered,
            "deferred": deferred,
            "failures": failures,
        }))
    }
}
