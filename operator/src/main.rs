//! Hanzo Operator binary — a thin dispatcher over the `operator` library.
//!
//! One binary, N CRD Kinds at a configurable API group (default `hanzo.ai`),
//! serving Hanzo, Lux, Zoo, and Osage universes via `--api-group` /
//! `OPERATOR_API_GROUP`.
//!
//! Subcommands (the default is `run`, preserving the historical flags-only
//! entrypoint so the existing `ENTRYPOINT ["operator"]` keeps working):
//!
//! ```text
//! operator [run]     Run the reconcile loop (default when no subcommand).
//! operator install   Apply the CRDs + operator Deployment/RBAC into the cluster.
//! ```
//!
//! In the merged node image `hanzod operator …` supervises this binary and
//! `hanzod install …` calls `operator install`. The reconcile loop itself lives
//! in [`operator::run::run`] and the install objects in [`operator::install`],
//! so both are reachable as library calls — not just as a spawned process.

use clap::{Args, Parser, Subcommand};

use operator::api_group::ApiGroup;
use operator::install::{self, InstallConfig};
use operator::run::{self, RunConfig};

#[derive(Parser, Debug)]
#[command(name = "operator")]
#[command(about = "Kubernetes operator for Hanzo platform (canonical, all universes)", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Flags for the default (no-subcommand) run path.
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the reconcile loop (default when no subcommand is given).
    Run(RunArgs),
    /// Apply the CRDs + the operator's own Deployment/RBAC into the cluster.
    Install(InstallArgs),
}

/// Reconcile-loop flags. Mirror `RunConfig` 1:1.
#[derive(Args, Debug, Clone)]
struct RunArgs {
    /// Log level: trace, debug, info, warn, error.
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// Namespace to watch. Empty = all namespaces (requires ClusterRole).
    #[arg(long, env = "WATCH_NAMESPACE", default_value = "")]
    namespace: String,

    /// API group for CRDs. Overrides compile-time default `hanzo.ai`.
    /// Other universes: `lux.cloud`, `zoo.cloud`, `osage.cloud`.
    #[arg(long, env = "OPERATOR_API_GROUP")]
    api_group: Option<String>,

    /// Health-check listener address.
    #[arg(long, env = "HEALTH_ADDR", default_value = "0.0.0.0:8081")]
    health_addr: String,

    /// Enable lease-based leader election. Set false on local dev for
    /// single-replica runs.
    #[arg(long, env = "LEADER_ELECT", default_value = "true")]
    leader_election: bool,

    /// Operator namespace (where the Lease object lives).
    #[arg(long, env = "OPERATOR_NAMESPACE", default_value = "hanzo-operator-system")]
    operator_namespace: String,
}

impl From<RunArgs> for RunConfig {
    fn from(a: RunArgs) -> Self {
        RunConfig {
            log_level: a.log_level,
            namespace: a.namespace,
            api_group: a.api_group,
            health_addr: a.health_addr,
            leader_election: a.leader_election,
            operator_namespace: a.operator_namespace,
        }
    }
}

/// Install flags. `--image` is required (env `OPERATOR_IMAGE`) — install
/// refuses to render a placeholder/`:latest` image per the semver-pin policy.
#[derive(Args, Debug, Clone)]
struct InstallArgs {
    /// API group for the CRDs + the running operator. Default `hanzo.ai`.
    #[arg(long, env = "OPERATOR_API_GROUP")]
    api_group: Option<String>,

    /// Image the operator Deployment runs — must contain this operator binary
    /// (or `hanzod`, which supervises it). Required; semver-pinned, never `:latest`.
    #[arg(long, env = "OPERATOR_IMAGE")]
    image: Option<String>,

    /// Operator namespace (Deployment + ServiceAccount + Lease live here).
    #[arg(long, env = "OPERATOR_NAMESPACE", default_value = "hanzo-operator-system")]
    operator_namespace: String,

    /// Namespace the operator watches. Empty = all namespaces.
    #[arg(long, env = "WATCH_NAMESPACE", default_value = "")]
    watch_namespace: String,

    /// Container command for the Deployment (comma-separated). Default runs the
    /// operator via hanzod's supervised subcommand.
    #[arg(long, value_delimiter = ',', default_value = "hanzod,operator")]
    command: Vec<String>,

    /// Do not apply the CRD bundle (apply only Deployment/RBAC).
    #[arg(long, default_value = "false")]
    skip_crds: bool,

    /// Print the manifests instead of applying them (pipe to `kubectl apply -f -`).
    #[arg(long, default_value = "false")]
    dry_run: bool,
}

impl InstallArgs {
    fn into_config(self) -> anyhow::Result<InstallConfig> {
        let image = self.image.ok_or_else(|| {
            anyhow::anyhow!(
                "install requires --image (or OPERATOR_IMAGE): a semver-pinned image \
                 containing the operator/hanzod binary"
            )
        })?;
        Ok(InstallConfig {
            api_group: ApiGroup::resolve(self.api_group.as_deref()).group,
            image,
            operator_namespace: self.operator_namespace,
            watch_namespace: self.watch_namespace,
            command: self.command,
            apply_crds: !self.skip_crds,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        // No subcommand → run (historical flags-only entrypoint).
        None => run::run(cli.run.into()).await,
        Some(Command::Run(a)) => run::run(a.into()).await,
        Some(Command::Install(a)) => {
            let dry_run = a.dry_run;
            let cfg = a.into_config()?;
            if dry_run {
                print!("{}", install::render(&cfg)?);
                Ok(())
            } else {
                install::install(cfg).await
            }
        }
    }
}
