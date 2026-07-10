//! AgentDeployment reconciler — the autonomous-bot lifecycle.
//!
//! Watches `AgentDeployment` (short `bot`). For each CR it converges three
//! facts against two external control planes:
//!
//! 1. **Cloud Agent exists** with the desired `execution_mode` —
//!    `core::agents_client` (`GET/POST /v1/agents`).
//! 2. **Machine provisioned + bound** — `core::visor_client`
//!    (`POST /v1/machines/:id/bind-agent`, and, when provisioning is enabled,
//!    `POST /v1/launch-machine`).
//! 3. **desired == running** — the visor binding reconciles to `Bound`; the CR
//!    `status.phase` mirrors that honest state.
//!
//! ## Why this is a `kube::Controller` (unlike the apps DRIVE controller)
//!
//! Its reconcile SOURCE is a CRD Kind in this cluster (a watch), so it is a
//! standard `Controller` — same shape as `managed_database`. Its reconcile
//! ACTIONS reach two HTTP APIs (cloud + visor) — same shape as the apps
//! controller's `apps_client`. It composes both patterns rather than inventing
//! a third.
//!
//! ## Safety — provisioning is opt-in + fail-safe
//!
//! This controller can create cloud Agents and LAUNCH cloud machines (which
//! cost money). So mutation is gated exactly like the apps controller's drive:
//!
//! - **`AGENT_DEPLOY_MODE`** ∈ {`off` (default), `bind-only`, `on`}.
//!   - `off`: reconcile is inert — it only READS (agent + binding status) and
//!     reports; it never creates an Agent, launches, or binds.
//!   - `bind-only`: may create the cloud Agent and BIND an existing machine
//!     (`spec.machineId`), but NEVER launches a new machine (zero-cost path).
//!   - `on`: may additionally launch a machine when `spec.provider` is set and
//!     no `spec.machineId` is given.
//! - Without `AGENT_DEPLOY_CLOUD_URL` / `AGENT_DEPLOY_VISOR_URL` + a service
//!   token, the controller stays read-only regardless of mode (nothing to call).
//!
//! The mode decision is a pure function [`ProvisionPlan::for_spec`] so the gate
//! is unit-tested without a cluster, cloud, or visor.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, Resource, ResourceExt};
use tracing::{error, info, warn};

use crate::apply;
use crate::core::agents_client::{
    agent_in_mode, create_agent, get_agent, AgentsClientConfig, CreateAgentRequest,
};
use crate::core::visor_client::{
    bind_agent, get_binding, launch_machine, AgentBinding, CreateMachineSpec, VisorClientConfig,
};
use crate::core::{OperatorError, Result};
use crate::crd::{AgentDeployment, AgentDeploymentSpec, AgentDeploymentStatus, Phase};
use crate::crd_types::{build_condition, Condition};

/// The provisioning gate — `AGENT_DEPLOY_MODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployMode {
    /// Read-only: never create an Agent, launch, or bind. Only observe + report.
    Off,
    /// May create the Agent + bind an existing machine; never launches.
    BindOnly,
    /// May additionally launch a machine.
    On,
}

impl DeployMode {
    pub fn from_env_str(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "provision" => DeployMode::On,
            "bind-only" | "bind" | "bindonly" => DeployMode::BindOnly,
            _ => DeployMode::Off,
        }
    }

    fn may_mutate(self) -> bool {
        !matches!(self, DeployMode::Off)
    }
    fn may_launch(self) -> bool {
        matches!(self, DeployMode::On)
    }
}

/// What the controller will do about the machine for one spec — pure over
/// `(mode, spec)`. Computed by [`ProvisionPlan::for_spec`] so the gate is
/// unit-tested. Deciding the machine action is the load-bearing safety choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionPlan {
    /// Read-only — report status, mutate nothing.
    ReadOnly { reason: String },
    /// Bind this already-provisioned machine id.
    BindExisting { machine_id: String },
    /// Launch a new machine on `provider` (with the bot runtime tag) then bind.
    LaunchThenBind { provider: String },
    /// Nothing to act on — no machine id and no launchable provider.
    NoTarget { reason: String },
}

impl ProvisionPlan {
    /// Decide the machine plan. Order encodes the safety policy:
    /// 1. `Off` → always `ReadOnly` (never mutate).
    /// 2. explicit `machineId` → `BindExisting` (zero-cost, allowed in bind-only+on).
    /// 3. `provider` set + mode `on` → `LaunchThenBind`.
    /// 4. `provider` set but mode only `bind-only` → `ReadOnly` (launch refused).
    /// 5. neither → `NoTarget`.
    pub fn for_spec(mode: DeployMode, spec: &AgentDeploymentSpec) -> Self {
        if !mode.may_mutate() {
            return ProvisionPlan::ReadOnly {
                reason: "AGENT_DEPLOY_MODE=off — read-only".into(),
            };
        }
        if !spec.machine_id.is_empty() {
            return ProvisionPlan::BindExisting {
                machine_id: spec.machine_id.clone(),
            };
        }
        if !spec.provider.is_empty() {
            if mode.may_launch() {
                return ProvisionPlan::LaunchThenBind {
                    provider: spec.provider.clone(),
                };
            }
            return ProvisionPlan::ReadOnly {
                reason: "provider set but AGENT_DEPLOY_MODE=bind-only refuses to launch".into(),
            };
        }
        ProvisionPlan::NoTarget {
            reason: "no spec.machineId and no spec.provider — nothing to bind or launch".into(),
        }
    }
}

/// Runtime config, resolved once from the environment. `None` from `from_env`
/// means the controller runs read-only (URLs/token missing).
#[derive(Clone, Debug)]
pub struct AgentDeployConfig {
    pub cloud: AgentsClientConfig,
    pub visor: VisorClientConfig,
    pub mode: DeployMode,
    /// Machine owner used for launched machines (the org's visor owner). Defaults
    /// to the CR's `spec.org` when unset.
    pub machine_owner: Option<String>,
    /// Default instance type for launched machines.
    pub instance_type: String,
    /// Default region for launched machines.
    pub region: String,
    /// Requeue cadence.
    pub requeue: Duration,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| std::env::var(k).ok().filter(|s| !s.is_empty()))
}

impl AgentDeployConfig {
    /// Build from the environment. Returns `None` when the cloud URL, visor URL,
    /// or service token is missing — the controller then runs read-only.
    pub fn from_env() -> Option<Self> {
        let cloud_url = std::env::var("AGENT_DEPLOY_CLOUD_URL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let visor_url = std::env::var("AGENT_DEPLOY_VISOR_URL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let token = first_env(&[
            "AGENT_DEPLOY_SERVICE_TOKEN",
            "PLATFORM_SERVICE_TOKEN",
            "HANZO_API_KEY",
        ])
        .filter(|s| !s.is_empty())?;
        let org_id = std::env::var("AGENT_DEPLOY_ORG_ID")
            .ok()
            .filter(|s| !s.is_empty());
        // Visor authorizes the operator as the `app` subject via its IAM
        // application clientId/clientSecret (HTTP Basic). Without these, visor
        // sees an anonymous caller and denies the path-scoped binding routes, so
        // provisioning silently no-ops behind 403s — surface that loudly.
        let visor_client_id =
            first_env(&["AGENT_DEPLOY_VISOR_CLIENT_ID", "IAM_CLIENT_ID"]).unwrap_or_default();
        let visor_client_secret =
            first_env(&["AGENT_DEPLOY_VISOR_CLIENT_SECRET", "IAM_CLIENT_SECRET"])
                .unwrap_or_default();
        let mode =
            DeployMode::from_env_str(&std::env::var("AGENT_DEPLOY_MODE").unwrap_or_default());
        let machine_owner = std::env::var("AGENT_DEPLOY_MACHINE_OWNER")
            .ok()
            .filter(|s| !s.is_empty());
        let instance_type = std::env::var("AGENT_DEPLOY_INSTANCE_TYPE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let region = std::env::var("AGENT_DEPLOY_REGION")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let requeue = Duration::from_secs(
            std::env::var("AGENT_DEPLOY_REQUEUE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|&n| n >= 10)
                .unwrap_or(60),
        );

        Some(Self {
            cloud: AgentsClientConfig {
                base_url: cloud_url,
                token: token.clone(),
                org_id,
            },
            visor: VisorClientConfig {
                base_url: visor_url,
                token,
                client_id: visor_client_id,
                client_secret: visor_client_secret,
            },
            mode,
            machine_owner,
            instance_type,
            region,
            requeue,
        })
    }
}

/// Reconcile context — the k8s client, the resolved external config (None ⇒
/// read-only, no external calls), and the API group for owner refs.
#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub config: Option<AgentDeployConfig>,
    pub api_group: String,
    pub requeue: Duration,
}

pub async fn reconcile(cr: Arc<AgentDeployment>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("AgentDeployment has no namespace".into()))?;
    let generation = cr.meta().generation.unwrap_or(0);
    let spec = &cr.spec;

    if spec.agent_name.is_empty() || spec.org.is_empty() {
        let status = failed_status("agentName and org are required", generation);
        write_status(&ctx.client, &name, &namespace, status).await;
        // A spec error won't fix itself on requeue; back off longer.
        return Ok(Action::requeue(Duration::from_secs(300)));
    }

    // No external config ⇒ read-only stub: report Pending, requeue.
    let Some(config) = &ctx.config else {
        info!(agent = %spec.agent_name, "AgentDeployment read-only (no cloud/visor config); reporting Pending");
        let mut status = AgentDeploymentStatus {
            phase: Some(Phase::Pending),
            observed_generation: generation,
            message: "read-only: AGENT_DEPLOY_CLOUD_URL/VISOR_URL/token unset".into(),
            ..Default::default()
        };
        upsert_condition(
            &mut status.conditions,
            build_condition(
                "AgentReady",
                false,
                "ReadOnly",
                "no external config",
                generation,
            ),
        );
        write_status(&ctx.client, &name, &namespace, status).await;
        return Ok(Action::requeue(ctx.requeue));
    };

    let mut status = AgentDeploymentStatus {
        observed_generation: generation,
        ..Default::default()
    };

    // ---- Step 1: ensure the cloud Agent exists in the desired mode. ----
    let desired_mode = if spec.execution_mode.is_empty() {
        "long-running"
    } else {
        spec.execution_mode.as_str()
    };
    match ensure_agent(config, spec, desired_mode).await {
        Ok(ready) => {
            status.agent_ready = ready;
            upsert_condition(
                &mut status.conditions,
                build_condition(
                    "AgentReady",
                    ready,
                    if ready { "Ready" } else { "Pending" },
                    &format!(
                        "cloud Agent {}/{} mode={}",
                        spec.org, spec.agent_name, desired_mode
                    ),
                    generation,
                ),
            );
        }
        Err(e) => {
            warn!(agent = %spec.agent_name, error = %e, "ensure_agent failed");
            status.agent_ready = false;
            status.phase = Some(Phase::Degraded);
            status.message = format!("agent step failed: {e}");
            upsert_condition(
                &mut status.conditions,
                build_condition("AgentReady", false, "Error", &e.to_string(), generation),
            );
            write_status(&ctx.client, &name, &namespace, status).await;
            return Ok(Action::requeue(config.requeue));
        }
    }

    // ---- Step 2 + 3: ensure a machine is provisioned + bound. ----
    let plan = ProvisionPlan::for_spec(config.mode, spec);
    match ensure_binding(config, spec, &plan).await {
        Ok(Some(binding)) => {
            status.machine_id = binding.machine_id.clone();
            status.binding_status = binding.status.clone();
            let bound = binding.is_bound();
            let ready = status.agent_ready && bound;
            status.phase = Some(if ready {
                Phase::Running
            } else if binding.status == AgentBinding::STATUS_ERROR {
                Phase::Degraded
            } else {
                Phase::Creating
            });
            status.message = binding.message.clone();
            upsert_condition(
                &mut status.conditions,
                build_condition(
                    "MachineBound",
                    bound,
                    if bound { "Bound" } else { &binding.status },
                    &binding.message,
                    generation,
                ),
            );
            upsert_condition(
                &mut status.conditions,
                build_condition(
                    "Ready",
                    ready,
                    if ready { "Available" } else { "NotReady" },
                    &format!("agentReady={} bound={}", status.agent_ready, bound),
                    generation,
                ),
            );
        }
        Ok(None) => {
            // Read-only / no-target plan: agent may be ready but no machine acted on.
            status.phase = Some(if status.agent_ready {
                Phase::Pending
            } else {
                Phase::Creating
            });
            status.message = plan_reason(&plan);
            upsert_condition(
                &mut status.conditions,
                build_condition(
                    "MachineBound",
                    false,
                    "NoAction",
                    &plan_reason(&plan),
                    generation,
                ),
            );
        }
        Err(e) => {
            warn!(agent = %spec.agent_name, error = %e, "ensure_binding failed");
            status.phase = Some(Phase::Degraded);
            status.message = format!("binding step failed: {e}");
            upsert_condition(
                &mut status.conditions,
                build_condition("MachineBound", false, "Error", &e.to_string(), generation),
            );
        }
    }

    write_status(&ctx.client, &name, &namespace, status).await;
    Ok(Action::requeue(config.requeue))
}

/// Ensure the cloud Agent exists with `desired_mode`. Returns whether it is
/// ready (exists AND in the desired mode). In `Off` mode it only reads: an agent
/// that exists-but-wrong-mode or is missing is reported not-ready but never
/// created.
async fn ensure_agent(
    config: &AgentDeployConfig,
    spec: &AgentDeploymentSpec,
    desired_mode: &str,
) -> Result<bool> {
    match get_agent(&config.cloud, &spec.agent_name).await? {
        Some(agent) => Ok(agent_in_mode(&agent, desired_mode)),
        None => {
            if !config.mode.may_mutate() {
                return Ok(false); // read-only: report missing, don't create.
            }
            create_agent(
                &config.cloud,
                CreateAgentRequest {
                    name: &spec.agent_name,
                    org: &spec.org,
                    execution_mode: desired_mode,
                    schedule: &spec.schedule,
                },
            )
            .await?;
            info!(agent = %spec.agent_name, mode = %desired_mode, "created cloud Agent");
            Ok(true)
        }
    }
}

/// Ensure a machine is provisioned + bound per `plan`. Returns:
/// - `Some(binding)` when a bind happened (or an existing binding was read),
/// - `None` when the plan is read-only / no-target (nothing to act on).
async fn ensure_binding(
    config: &AgentDeployConfig,
    spec: &AgentDeploymentSpec,
    plan: &ProvisionPlan,
) -> Result<Option<AgentBinding>> {
    match plan {
        ProvisionPlan::ReadOnly { .. } | ProvisionPlan::NoTarget { .. } => {
            // Best-effort read of any existing binding for whichever machine the
            // spec names, so read-only mode still surfaces real convergence.
            if !spec.machine_id.is_empty() {
                return get_binding(&config.visor, &spec.machine_id).await;
            }
            Ok(None)
        }
        ProvisionPlan::BindExisting { machine_id } => {
            let b = bind_agent(
                &config.visor,
                machine_id,
                &spec.org,
                &spec.agent_name,
                &spec.bot_version,
            )
            .await?;
            Ok(Some(b))
        }
        ProvisionPlan::LaunchThenBind { provider } => {
            let owner = config
                .machine_owner
                .clone()
                .unwrap_or_else(|| spec.org.clone());
            let machine_name = format!("bot-{}", spec.agent_name);
            let mut mspec = CreateMachineSpec {
                name: machine_name.clone(),
                display_name: format!("bot: {}", spec.agent_name),
                instance_type: config.instance_type.clone(),
                region: config.region.clone(),
                ..Default::default()
            };
            // Stamp the runtime tag so the @hanzo/bot cloud-init installs and the
            // visor binding can reconcile to Bound; pass the bot version via env.
            mspec
                .tags
                .insert("hanzo-bot".into(), spec.agent_name.clone());
            mspec
                .tags
                .insert("env:AGENT_NODE_ID".into(), spec.agent_name.clone());
            let machine = launch_machine(&config.visor, &owner, provider, &mspec).await?;
            let machine_id = if machine.id.contains('/') {
                machine.id.clone()
            } else {
                format!("{}/{}", owner, machine.name)
            };
            info!(machine = %machine_id, provider = %provider, "launched bot machine");
            let b = bind_agent(
                &config.visor,
                &machine_id,
                &spec.org,
                &spec.agent_name,
                &spec.bot_version,
            )
            .await?;
            Ok(Some(b))
        }
    }
}

fn plan_reason(plan: &ProvisionPlan) -> String {
    match plan {
        ProvisionPlan::ReadOnly { reason } | ProvisionPlan::NoTarget { reason } => reason.clone(),
        ProvisionPlan::BindExisting { machine_id } => format!("binding {machine_id}"),
        ProvisionPlan::LaunchThenBind { provider } => format!("launching on {provider}"),
    }
}

fn failed_status(msg: &str, generation: i64) -> AgentDeploymentStatus {
    let mut status = AgentDeploymentStatus {
        phase: Some(Phase::Degraded),
        observed_generation: generation,
        message: msg.to_string(),
        ..Default::default()
    };
    upsert_condition(
        &mut status.conditions,
        build_condition("Ready", false, "InvalidSpec", msg, generation),
    );
    status
}

/// Upsert a condition in-place by `type_`.
fn upsert_condition(conditions: &mut Vec<Condition>, new_cond: Condition) {
    if let Some(slot) = conditions.iter_mut().find(|c| c.type_ == new_cond.type_) {
        *slot = new_cond;
    } else {
        conditions.push(new_cond);
    }
}

async fn write_status(client: &Client, name: &str, namespace: &str, status: AgentDeploymentStatus) {
    let api: Api<AgentDeployment> = Api::namespaced(client.clone(), namespace);
    let patch = serde_json::json!({ "status": status });
    let pp = PatchParams::apply(apply::FIELD_MANAGER);
    if let Err(e) = api.patch_status(name, &pp, &Patch::Merge(&patch)).await {
        warn!(error = %e, "failed to update AgentDeployment status");
    }
}

pub fn on_error(_obj: Arc<AgentDeployment>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "AgentDeployment reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_agent_deployment_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<AgentDeployment> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    let config = AgentDeployConfig::from_env();
    let requeue = config
        .as_ref()
        .map(|c| c.requeue)
        .unwrap_or(Duration::from_secs(60));
    match &config {
        Some(c) => {
            if c.mode.may_mutate()
                && (c.visor.client_id.is_empty() || c.visor.client_secret.is_empty())
            {
                warn!(
                    "AGENT_DEPLOY_MODE={:?} but no visor clientId/clientSecret \
                     (AGENT_DEPLOY_VISOR_CLIENT_ID/SECRET or IAM_CLIENT_ID/SECRET) — \
                     visor authorizes the operator via Basic auth; binding/launch calls \
                     will be denied (403) without it",
                    c.mode
                );
            }
            info!(
                group = %api_group,
                mode = ?c.mode,
                "Starting AgentDeployment controller (cloud + visor configured)"
            )
        }
        None => info!(
            group = %api_group,
            "Starting AgentDeployment controller (READ-ONLY — set AGENT_DEPLOY_CLOUD_URL, \
             AGENT_DEPLOY_VISOR_URL, service token to enable provisioning)"
        ),
    }
    let ctx = Arc::new(Ctx {
        client,
        config,
        api_group,
        requeue,
    });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "AgentDeployment reconcile error");
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(machine_id: &str, provider: &str) -> AgentDeploymentSpec {
        AgentDeploymentSpec {
            agent_name: "researcher".into(),
            org: "hanzoai".into(),
            execution_mode: "long-running".into(),
            machine_id: machine_id.into(),
            provider: provider.into(),
            ..Default::default()
        }
    }

    // ---- DeployMode parsing ----

    #[test]
    fn deploy_mode_defaults_off() {
        assert_eq!(DeployMode::from_env_str(""), DeployMode::Off);
        assert_eq!(DeployMode::from_env_str("garbage"), DeployMode::Off);
        assert_eq!(DeployMode::from_env_str("off"), DeployMode::Off);
    }

    #[test]
    fn deploy_mode_variants() {
        assert_eq!(DeployMode::from_env_str("on"), DeployMode::On);
        assert_eq!(DeployMode::from_env_str("provision"), DeployMode::On);
        assert_eq!(DeployMode::from_env_str("bind-only"), DeployMode::BindOnly);
        assert_eq!(DeployMode::from_env_str("bind"), DeployMode::BindOnly);
    }

    // ---- ProvisionPlan gate — the load-bearing safety choice ----

    #[test]
    fn off_mode_never_mutates_even_with_machine_or_provider() {
        for s in [
            spec("hanzoai/m1", ""),
            spec("", "DigitalOcean"),
            spec("hanzoai/m1", "DigitalOcean"),
        ] {
            assert!(
                matches!(
                    ProvisionPlan::for_spec(DeployMode::Off, &s),
                    ProvisionPlan::ReadOnly { .. }
                ),
                "Off must be ReadOnly for spec {s:?}"
            );
        }
    }

    #[test]
    fn explicit_machine_binds_in_bind_only_and_on() {
        for mode in [DeployMode::BindOnly, DeployMode::On] {
            match ProvisionPlan::for_spec(mode, &spec("hanzoai/m1", "")) {
                ProvisionPlan::BindExisting { machine_id } => assert_eq!(machine_id, "hanzoai/m1"),
                other => panic!("{mode:?} with machineId must BindExisting, got {other:?}"),
            }
        }
    }

    #[test]
    fn provider_launches_only_in_on_mode() {
        // on: launches.
        match ProvisionPlan::for_spec(DeployMode::On, &spec("", "DigitalOcean")) {
            ProvisionPlan::LaunchThenBind { provider } => assert_eq!(provider, "DigitalOcean"),
            other => panic!("on+provider must LaunchThenBind, got {other:?}"),
        }
        // bind-only: refuses to launch (the money-guard).
        assert!(matches!(
            ProvisionPlan::for_spec(DeployMode::BindOnly, &spec("", "DigitalOcean")),
            ProvisionPlan::ReadOnly { .. }
        ));
    }

    #[test]
    fn machine_id_wins_over_provider() {
        // Both set → bind the explicit machine, never launch (cheapest safe path).
        match ProvisionPlan::for_spec(DeployMode::On, &spec("hanzoai/m1", "DigitalOcean")) {
            ProvisionPlan::BindExisting { machine_id } => assert_eq!(machine_id, "hanzoai/m1"),
            other => panic!("machineId must win, got {other:?}"),
        }
    }

    #[test]
    fn no_target_when_nothing_set() {
        assert!(matches!(
            ProvisionPlan::for_spec(DeployMode::On, &spec("", "")),
            ProvisionPlan::NoTarget { .. }
        ));
        assert!(matches!(
            ProvisionPlan::for_spec(DeployMode::BindOnly, &spec("", "")),
            ProvisionPlan::NoTarget { .. }
        ));
    }

    #[test]
    fn upsert_condition_replaces_by_type() {
        let mut conds = vec![build_condition("Ready", false, "A", "m1", 1)];
        upsert_condition(&mut conds, build_condition("Ready", true, "B", "m2", 2));
        assert_eq!(conds.len(), 1);
        assert_eq!(conds[0].status, "True");
        assert_eq!(conds[0].reason, "B");
        upsert_condition(
            &mut conds,
            build_condition("AgentReady", true, "C", "m3", 3),
        );
        assert_eq!(conds.len(), 2);
    }

    #[test]
    fn default_execution_mode_is_long_running() {
        // A bare spec (serde default) carries the long-running mode.
        let s: AgentDeploymentSpec =
            serde_json::from_str(r#"{"agentName":"r","org":"hanzoai"}"#).unwrap();
        assert_eq!(s.execution_mode, "long-running");
    }
}
