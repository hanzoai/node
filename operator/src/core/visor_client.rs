//! Client for visor's machine + agent-binding API (`/v1/machines`).
//!
//! The AgentDeployment controller's job #2 is to ensure a visor machine is
//! provisioned and bound to the `@hanzo/bot` runtime for the Agent. Visor
//! (`~/work/hanzo/visor`) owns the machine lifecycle across cloud providers and,
//! via the `feat/agent-machine-binding` API, the agent↔machine binding:
//!
//! - `POST /v1/machines/:id/bind-agent {org, agentName, botVersion}` → bind +
//!   reconcile; returns the `AgentBinding` with an honest status
//!   (`Pending`/`Bound`/`Error`).
//! - `GET  /v1/machines/:id/agent-binding` → the current reconciled binding.
//! - `POST /v1/launch-machine?owner=&provider=` (body: CreateMachineSpec) →
//!   launch a machine (used only when provisioning is enabled).
//!
//! ## Trust model
//!
//! Visor authenticates a caller as the trusted `app` subject via its IAM
//! application **clientId/clientSecret**, presented as HTTP Basic auth (visor's
//! `getUsernameByClientIdSecret` reads `Request.BasicAuth()`; it does NOT parse
//! `Authorization: Bearer`). So the client sends Basic(clientId, clientSecret)
//! when configured — that is what makes visor authorize the operator's
//! path-scoped `/v1/machines/:id/...` calls (subject `app/<app>`). The service
//! token is still sent as `Authorization: Bearer` for forward-compat / any
//! bearer-aware surface, but Basic is the load-bearing credential for visor.
//!
//! ## Envelope
//!
//! Visor wraps every response in `{status, msg, data, data2}` (`Response` in
//! `controllers/util.go`). We decode `status` + the typed `data`; a
//! `status="error"` surfaces `msg` as the error.

use super::error::{OperatorError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

const HTTP_TIMEOUT_SECS: u64 = 30;

/// Visor's `Response` envelope. `data` is decoded into the caller's expected
/// type `T`; `status="error"` maps `msg` into an `OperatorError`.
#[derive(Debug, Deserialize)]
struct VisorEnvelope<T> {
    #[serde(default)]
    status: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Option<T>,
}

/// The visor `AgentBinding` wire shape (subset the controller reads). Mirrors
/// `object.AgentBinding` — camelCase JSON tags.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentBinding {
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub public_ip: String,
}

impl AgentBinding {
    /// The visor binding's `Bound` terminal-desired state.
    pub const STATUS_BOUND: &'static str = "Bound";
    pub const STATUS_PENDING: &'static str = "Pending";
    pub const STATUS_ERROR: &'static str = "Error";

    pub fn is_bound(&self) -> bool {
        self.status == Self::STATUS_BOUND
    }
}

/// visor `CreateMachineSpec` (subset). `tags` carries the `hanzo-bot:<agent>`
/// runtime marker + `env:` cloud-init overrides.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CreateMachineSpec {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instance_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub region: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
}

/// The visor machine wire shape (subset — the launch response's id/name).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub state: String,
}

/// Configuration for the visor client. Explicit struct; per-deploy URL/token.
#[derive(Clone, Debug)]
pub struct VisorClientConfig {
    /// Visor base URL, e.g. `https://visor.hanzo.ai` or the in-cluster
    /// `http://visor.hanzo-system.svc.cluster.local:8000`.
    pub base_url: String,
    pub token: String,
    /// IAM application clientId/clientSecret — the credential visor authorizes as
    /// the `app` subject (HTTP Basic auth). When both are set, requests carry
    /// Basic auth; visor requires this to allow the path-scoped binding routes.
    pub client_id: String,
    pub client_secret: String,
}

impl VisorClientConfig {
    /// Apply visor auth to a request builder. Emits exactly ONE `Authorization`
    /// header: Basic(clientId,clientSecret) when configured (the credential visor
    /// authorizes as the `app` subject), otherwise Bearer(token) as a fallback.
    ///
    /// Both must never be set at once — reqwest APPENDS Authorization headers, so
    /// sending Bearer + Basic yields two headers and Go's `Request.BasicAuth()`
    /// (which reads the first `Authorization` value) would parse the Bearer one
    /// and fail, silently downgrading the operator to an anonymous, denied
    /// caller. Choosing one header keeps auth deterministic.
    fn authenticate(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if !self.client_id.is_empty() && !self.client_secret.is_empty() {
            req.basic_auth(&self.client_id, Some(&self.client_secret))
        } else {
            req.bearer_auth(&self.token)
        }
    }
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| OperatorError::Other(format!("build visor http client: {e}")))
}

fn require_token(config: &VisorClientConfig) -> Result<()> {
    if config.token.is_empty() {
        return Err(OperatorError::Config(
            "visor call with empty service token".into(),
        ));
    }
    Ok(())
}

/// Decode a visor `Response` envelope from a raw body + HTTP status, returning
/// the typed `data`. Pure over `(status_success, body)` so the envelope
/// contract is unit-tested without a server.
fn decode_envelope<T: serde::de::DeserializeOwned + Default>(
    status_success: bool,
    body: &str,
) -> Result<T> {
    let env: VisorEnvelope<T> = serde_json::from_str(body).map_err(|e| {
        OperatorError::Other(format!(
            "visor response parse: {e}; body={}",
            &body[..body.len().min(400)]
        ))
    })?;
    if env.status == "error" {
        return Err(OperatorError::Other(format!("visor error: {}", env.msg)));
    }
    if !status_success && env.status != "ok" {
        return Err(OperatorError::Other(format!(
            "visor call failed: status_field={} msg={}",
            env.status, env.msg
        )));
    }
    env.data.ok_or_else(|| {
        OperatorError::Other(format!("visor response missing data (msg={})", env.msg))
    })
}

/// `POST /v1/machines/:id/bind-agent` — bind the machine to the Agent and get
/// back the reconciled binding.
pub async fn bind_agent(
    config: &VisorClientConfig,
    machine_id: &str,
    org: &str,
    agent_name: &str,
    bot_version: &str,
) -> Result<AgentBinding> {
    require_token(config)?;
    let http = http_client()?;
    let url = format!(
        "{}/v1/machines/{}/bind-agent",
        config.base_url.trim_end_matches('/'),
        machine_id
    );
    let payload = serde_json::json!({
        "org": org,
        "agentName": agent_name,
        "botVersion": bot_version,
    });
    let resp = config
        .authenticate(http.post(&url))
        .json(&payload)
        .send()
        .await
        .map_err(OperatorError::Http)?;
    let ok = resp.status().is_success();
    let body = resp.text().await.map_err(OperatorError::Http)?;
    decode_envelope::<AgentBinding>(ok, &body)
}

/// `GET /v1/machines/:id/agent-binding` — the current reconciled binding, or
/// `None` when the machine has no binding (`data:null`).
pub async fn get_binding(
    config: &VisorClientConfig,
    machine_id: &str,
) -> Result<Option<AgentBinding>> {
    require_token(config)?;
    let http = http_client()?;
    let url = format!(
        "{}/v1/machines/{}/agent-binding",
        config.base_url.trim_end_matches('/'),
        machine_id
    );
    let resp = config
        .authenticate(http.get(&url))
        .send()
        .await
        .map_err(OperatorError::Http)?;
    let ok = resp.status().is_success();
    let body = resp.text().await.map_err(OperatorError::Http)?;
    // A null `data` is a legitimate "no binding" — decode leniently.
    let env: VisorEnvelope<AgentBinding> = serde_json::from_str(&body).map_err(|e| {
        OperatorError::Other(format!(
            "visor get-binding parse: {e}; body={}",
            &body[..body.len().min(400)]
        ))
    })?;
    if env.status == "error" {
        return Err(OperatorError::Other(format!("visor error: {}", env.msg)));
    }
    if !ok && env.status != "ok" {
        return Err(OperatorError::Other(format!(
            "visor get-binding failed: status_field={} msg={}",
            env.status, env.msg
        )));
    }
    Ok(env.data)
}

/// `POST /v1/launch-machine?owner=&provider=` — launch a machine carrying the
/// `hanzo-bot:<agent>` runtime tag so the @hanzo/bot cloud-init installs and
/// reconcile can observe `Bound`. Only called when provisioning is enabled.
pub async fn launch_machine(
    config: &VisorClientConfig,
    owner: &str,
    provider: &str,
    spec: &CreateMachineSpec,
) -> Result<Machine> {
    require_token(config)?;
    let http = http_client()?;
    let url = format!(
        "{}/v1/launch-machine?owner={}&provider={}",
        config.base_url.trim_end_matches('/'),
        owner,
        provider
    );
    let resp = config
        .authenticate(http.post(&url))
        .json(spec)
        .send()
        .await
        .map_err(OperatorError::Http)?;
    let ok = resp.status().is_success();
    let body = resp.text().await.map_err(OperatorError::Http)?;
    decode_envelope::<Machine>(ok, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_ok_envelope_returns_data() {
        let body = r#"{"status":"ok","msg":"","data":{"machineId":"hanzoai/m1","org":"hanzoai","agentName":"researcher","status":"Bound","message":"running"}}"#;
        let b = decode_envelope::<AgentBinding>(true, body).unwrap();
        assert_eq!(b.machine_id, "hanzoai/m1");
        assert_eq!(b.agent_name, "researcher");
        assert!(b.is_bound());
    }

    #[test]
    fn decode_error_envelope_is_err() {
        let body = r#"{"status":"error","msg":"machine not found: hanzoai/x","data":null}"#;
        let err = decode_envelope::<AgentBinding>(false, body).unwrap_err();
        assert!(err.to_string().contains("machine not found"));
    }

    #[test]
    fn decode_pending_binding() {
        let body = r#"{"status":"ok","data":{"machineId":"hanzoai/m1","status":"Pending","message":"provisioning"}}"#;
        let b = decode_envelope::<AgentBinding>(true, body).unwrap();
        assert_eq!(b.status, AgentBinding::STATUS_PENDING);
        assert!(!b.is_bound());
    }

    #[test]
    fn binding_status_helpers() {
        let bound = AgentBinding {
            status: "Bound".into(),
            ..Default::default()
        };
        assert!(bound.is_bound());
        let errored = AgentBinding {
            status: "Error".into(),
            ..Default::default()
        };
        assert!(!errored.is_bound());
    }

    #[test]
    fn create_machine_spec_serializes_tags() {
        let mut spec = CreateMachineSpec {
            name: "bot-researcher".into(),
            ..Default::default()
        };
        spec.tags.insert("hanzo-bot".into(), "researcher".into());
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"hanzo-bot\":\"researcher\""));
        assert!(json.contains("\"name\":\"bot-researcher\""));
        // Empty optional fields are skipped, not emitted as "".
        assert!(!json.contains("\"region\""));
    }

    #[test]
    fn empty_token_rejected() {
        let cfg = VisorClientConfig {
            base_url: "http://x".into(),
            token: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
        };
        assert!(require_token(&cfg).is_err());
    }

    #[test]
    fn authenticate_adds_basic_when_client_creds_present() {
        // With client creds, the request must carry HTTP Basic (what visor reads).
        let cfg = VisorClientConfig {
            base_url: "http://x".into(),
            token: "tok".into(),
            client_id: "app-hanzo-visor".into(),
            client_secret: "s3cr3t".into(),
        };
        let http = reqwest::Client::new();
        let req = cfg.authenticate(http.get("http://x/v1/machines/a%2Fb/agent-binding"));
        let built = req.build().unwrap();
        // EXACTLY one Authorization header (reqwest appends — two would break
        // Go's Request.BasicAuth() which reads only the first value).
        let auths: Vec<&str> = built
            .headers()
            .get_all(reqwest::header::AUTHORIZATION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        assert_eq!(auths.len(), 1, "must send exactly one Authorization header");
        assert!(
            auths[0].starts_with("Basic "),
            "expected Basic auth for visor, got {:?}",
            auths[0]
        );
    }

    #[test]
    fn authenticate_falls_back_to_bearer_without_client_creds() {
        let cfg = VisorClientConfig {
            base_url: "http://x".into(),
            token: "tok".into(),
            client_id: String::new(),
            client_secret: String::new(),
        };
        let http = reqwest::Client::new();
        let req = cfg.authenticate(http.get("http://x/v1/machines/a%2Fb/agent-binding"));
        let built = req.build().unwrap();
        let auth = built
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            auth.starts_with("Bearer "),
            "expected Bearer auth fallback, got {auth:?}"
        );
    }
}
