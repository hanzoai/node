//! The operator reconcile loop as a reachable library entrypoint.
//!
//! `main.rs` (the `operator` binary) and any embedding host — notably
//! `hanzod operator`, which supervises this binary in the merged node image —
//! call [`run`] to start the same loop: rustls provider, cluster client,
//! leader election, every controller, and the health server, until shutdown.
//!
//! Extracting the loop out of `main` is precisely what makes it callable
//! WITHOUT spawning the process: `operator::run::run(RunConfig::default())` is
//! the reconcile loop, reachable from a test or a larger binary.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::{routing::get, Router};
use kube::Client;
use tracing::{info, warn};

use crate::api_group::ApiGroup;
use crate::controllers;
use crate::core::{LeaderConfig, LeaderElection};

/// Reconcile-loop configuration. Mirrors the CLI/env surface 1:1, so the binary
/// is a thin `clap` → `RunConfig` adapter and embedders can build it directly.
#[derive(Clone, Debug)]
pub struct RunConfig {
    /// Log level: trace, debug, info, warn, error.
    pub log_level: String,
    /// Namespace to watch. Empty = all namespaces (requires ClusterRole).
    pub namespace: String,
    /// API-group override; `None` → `OPERATOR_API_GROUP` env → compile default.
    pub api_group: Option<String>,
    /// Health-check listener address.
    pub health_addr: String,
    /// Enable lease-based leader election.
    pub leader_election: bool,
    /// Operator namespace (where the Lease object lives).
    pub operator_namespace: String,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            namespace: String::new(),
            api_group: None,
            health_addr: "0.0.0.0:8081".to_string(),
            leader_election: true,
            operator_namespace: "hanzo-operator-system".to_string(),
        }
    }
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(
    axum::extract::State(state): axum::extract::State<Arc<AtomicBool>>,
) -> (axum::http::StatusCode, &'static str) {
    if state.load(Ordering::Relaxed) {
        (axum::http::StatusCode::OK, "ready")
    } else {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "not leader")
    }
}

/// Start the operator reconcile loop and block until shutdown.
///
/// Safe to call from an embedding process: the rustls provider install and the
/// tracing subscriber init are best-effort (ignored if the host already set
/// them), so `hanzod operator` may call this in-process without a double-init
/// panic. The standalone binary is unaffected — it is the first to install both.
pub async fn run(cfg: RunConfig) -> anyhow::Result<()> {
    // rustls 0.23 compiles both aws-lc-rs (reqwest) and ring (kube) providers,
    // so the process-level CryptoProvider is ambiguous and the first TLS
    // handshake panics without an explicit default. Install aws-lc-rs; ignore
    // the Err that just means a provider is already installed by the host.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Logging — `try_init` so an embedding host that already set a global
    // subscriber does not panic us.
    let filter = tracing_subscriber::EnvFilter::try_new(&cfg.log_level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let api_group = ApiGroup::resolve(cfg.api_group.as_deref());
    info!(
        version = env!("CARGO_PKG_VERSION"),
        api_group = %api_group.group,
        namespace = %cfg.namespace,
        leader_election = cfg.leader_election,
        "Starting Hanzo Operator"
    );

    let client = Client::try_default().await?;
    info!("Connected to Kubernetes cluster");

    // Shutdown channel.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Leader election.
    let leader_election = LeaderElection::new(
        client.clone(),
        cfg.operator_namespace.clone(),
        LeaderConfig {
            lease_name: "hanzo-operator-leader".to_string(),
            identity_prefix: "hanzo-operator-".to_string(),
        },
    );
    let leader_flag = leader_election.leader_flag();
    if !cfg.leader_election {
        leader_flag.store(true, Ordering::Relaxed);
        info!("Leader election disabled, running as leader");
    }

    // Health server.
    let health_addr: SocketAddr = cfg.health_addr.parse()?;
    let health_app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(leader_flag.clone());

    let group = api_group.group.clone();
    let namespace = cfg.namespace.clone();
    let operator_namespace = cfg.operator_namespace.clone();
    let controllers_flag = leader_flag.clone();

    tokio::select! {
        // Leader election loop (only if enabled).
        _ = async {
            if cfg.leader_election {
                leader_election.run(shutdown_rx.clone()).await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            info!("Leader election exited");
        }

        // Controllers — wait for leadership then run all of them.
        _ = run_all_controllers(client.clone(), namespace.clone(), group.clone(), operator_namespace.clone(), controllers_flag.clone()) => {
            warn!("Controllers exited");
        }

        // Health server.
        res = axum::serve(
            tokio::net::TcpListener::bind(health_addr).await?,
            health_app.into_make_service(),
        ) => {
            if let Err(e) = res {
                tracing::error!(error = %e, "Health server exited");
            }
        }

        // Graceful shutdown on Ctrl-C.
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    let _ = shutdown_tx.send(true);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    info!("Operator stopped");
    Ok(())
}

/// Wait for leadership then run every controller in parallel.
async fn run_all_controllers(
    client: Client,
    namespace: String,
    api_group: String,
    operator_namespace: String,
    leader_flag: Arc<AtomicBool>,
) {
    // Block until we become the leader.
    loop {
        if leader_flag.load(Ordering::Relaxed) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    info!("Acquired leadership, starting all controllers");

    // Spawn every controller. `tokio::select!` won't help here because we
    // want them ALL to run concurrently for the operator's lifetime.
    tokio::join!(
        // Canonical Kinds.
        controllers::service::run_service_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::datastore::run_datastore_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::gateway::run_gateway_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::mpc::run_mpc_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::network::run_network_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::ingress::run_ingress_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::dns::run_dns_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::base::run_base_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::queue::run_queue_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::observability::run_observability_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::function::run_function_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::spa::run_spa_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::static_site::run_static_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        // v0.3.3: facade Kinds.
        controllers::sql::run_sql_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::kv::run_kv_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::docdb::run_docdb_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::s3::run_s3_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::iam::run_iam_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::kms::run_kms_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::llm::run_llm_controller(client.clone(), namespace.clone(), api_group.clone()),
        controllers::indexer::run_indexer_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::explorer::run_explorer_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        // v0.3.4: union with go/ — blockchain Kinds.
        controllers::luxruntime::run_luxruntime_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        controllers::nodefleet::run_nodefleet_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        // ManagedDatabase facade — per-tenant isolated Datastore workload.
        controllers::managed_database::run_managed_database_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        // AgentDeployment — autonomous-bot lifecycle. Watches the CRD; its
        // reconcile ACTIONS reach cloud /v1/agents + visor /v1/machines over
        // HTTP. Provisioning is opt-in + fail-safe: without AGENT_DEPLOY_CLOUD_URL
        // / AGENT_DEPLOY_VISOR_URL / token it runs READ-ONLY, and even configured
        // it never launches a machine unless AGENT_DEPLOY_MODE=on.
        controllers::agent_deployment::run_agent_deployment_controller(
            client.clone(),
            namespace.clone(),
            api_group.clone()
        ),
        // Tenant-RBAC — onboards platform-managed `tenant-<org>` namespaces
        // (watches Namespaces labelled hanzo.ai/managed-by=platform). Projects
        // cloud-api's per-tenant cloud-api-platform RoleBinding (the grant
        // cloud's waitForTenantRBAC gates on before the first deploy) + the
        // ghcr-pull image-pull Secret copied from the operator namespace. Not a
        // CRD Kind — its reconcile source is the tenant Namespace itself.
        controllers::tenant_rbac::run_tenant_rbac_controller(
            client.clone(),
            operator_namespace.clone()
        ),
        // Additive ZAP-native KMS secret projector — opt-in (off unless
        // KMS_ZAP_CONTROLLER=true). Watches the fixed kms.hanzo.ai KMSSecret
        // family; ignores non-zap-native CRs so the REST projector is untouched.
        controllers::kms_zap::run_kms_zap_controller(
            client.clone(),
            namespace.clone(),
            std::env::var("KMS_ZAP_CONTROLLER")
                .map(|v| v == "true")
                .unwrap_or(false),
        ),
        // Apps-lifecycle DRIVE controller (PR 5 of platform docs/APPS_LIFECYCLE.md)
        // — opt-in (off unless APPS_CONTROLLER=true) AND dry-run by default even
        // when on (APPS_DRIVE_MODE=off, per-app APPS_DRIVE_ALLOW). It reads the
        // platform `apps` table over GET /v1/apps and reconciles declared_tag →
        // cluster by patching Deployments. It can roll the whole fleet, so it
        // NEVER patches until the master enable, the drive mode, AND the per-app
        // allow-list all open. See controllers/apps.rs for the gate model.
        controllers::apps::run_apps_controller(
            client.clone(),
            namespace.clone(),
            std::env::var("APPS_CONTROLLER")
                .map(|v| v == "true")
                .unwrap_or(false),
        ),
    );
}
