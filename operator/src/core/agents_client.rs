//! Client for the canonical cloud Agent registry (`/v1/agents`).
//!
//! The AgentDeployment controller's job #1 is to ensure the cloud Agent it
//! deploys exists with the desired execution mode. The single production agent
//! registry is cloud `/v1/agents` (`~/work/hanzo/cloud/clients/agents`): the
//! only place `Agent{id,org,name,model,instructions,tools,status}` lives. This
//! module is the operator's narrow window onto it — get one, create one — and
//! holds no reconcile policy (the controller owns that).
//!
//! ## Trust model
//!
//! Mirrors `core::apps_client` / `core::iam_admin`: a single service token from
//! the environment, presented as `Authorization: Bearer <token>`; optional
//! `X-Org-Id` for org scoping. The token never leaves the cluster.

use super::error::{OperatorError, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// HTTP timeout for cloud agent reads/writes. Short — a slow cloud must not
/// wedge the reconcile loop.
const HTTP_TIMEOUT_SECS: u64 = 15;

/// One agent as served by `GET /v1/agents/:name`. Only the fields the
/// reconciler acts on are decoded; unknown fields (model, instructions, tools,
/// runs) are ignored so the operator never couples to the full builder shape.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct AgentView {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub org: String,
    /// `execution_mode` on the registry row — the field a Bot requires to be
    /// `long-running`. Optional because older rows may not carry it.
    #[serde(default, rename = "executionMode", alias = "execution_mode")]
    pub execution_mode: Option<String>,
    #[serde(default)]
    pub status: String,
}

/// Body for `POST /v1/agents` — create an agent. Only the identity + execution
/// mode the Bot lifecycle needs; the cloud fills the rest with its defaults.
#[derive(Debug, Clone, Serialize)]
pub struct CreateAgentRequest<'a> {
    pub name: &'a str,
    pub org: &'a str,
    #[serde(rename = "executionMode")]
    pub execution_mode: &'a str,
    #[serde(rename = "schedule", skip_serializing_if = "str::is_empty")]
    pub schedule: &'a str,
}

/// Configuration for the cloud agents client. Explicit struct so it is
/// unit-testable and the per-deploy URL/token/org are not baked into call sites.
#[derive(Clone, Debug)]
pub struct AgentsClientConfig {
    /// Cloud base URL, e.g. `https://api.hanzo.ai` or the in-cluster
    /// `http://cloud.hanzo-system.svc.cluster.local:8000`.
    pub base_url: String,
    /// Service token for `Authorization: Bearer`. Empty is rejected before any
    /// request — the call is authenticated, never anonymous.
    pub token: String,
    /// Optional org scope (`X-Org-Id`).
    pub org_id: Option<String>,
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| OperatorError::Other(format!("build agents http client: {e}")))
}

fn require_token(config: &AgentsClientConfig) -> Result<()> {
    if config.token.is_empty() {
        return Err(OperatorError::Config(
            "agents call with empty service token".into(),
        ));
    }
    Ok(())
}

/// Fetch one agent by name, or `Ok(None)` if it does not exist. A 404 (or the
/// Casdoor-style `status=ok data=null`) maps to `None`; other non-2xx surface
/// as an error so an auth failure never masquerades as "does not exist" (the
/// same trap `iam_admin::application_exists` guards).
pub async fn get_agent(config: &AgentsClientConfig, name: &str) -> Result<Option<AgentView>> {
    require_token(config)?;
    let http = http_client()?;
    let url = format!(
        "{}/v1/agents/{}",
        config.base_url.trim_end_matches('/'),
        name
    );
    let mut req = http.get(&url).bearer_auth(&config.token);
    if let Some(org) = &config.org_id {
        req = req.header("X-Org-Id", org);
    }
    let resp = req.send().await.map_err(OperatorError::Http)?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = resp.status();
    let body = resp.text().await.map_err(OperatorError::Http)?;
    if !status.is_success() {
        return Err(OperatorError::Other(format!(
            "GET {url} failed (status={status}): {}",
            &body[..body.len().min(400)]
        )));
    }
    parse_agent_body(&body)
}

/// Parse an agent GET body. The cloud may return the row bare, wrapped in a
/// `{status,data}` envelope, or `{status:"ok",data:null}` for not-found. All
/// three collapse to `Option<AgentView>` here so the caller sees one shape.
fn parse_agent_body(body: &str) -> Result<Option<AgentView>> {
    // Parse once into a generic JSON value, then decide envelope vs bare
    // STRUCTURALLY. The prior code sniffed the raw text for the substring
    // `"data"` to disambiguate — but a bare agent whose own fields contain that
    // substring (an agent named `metadata`, instructions mentioning `"data"`,
    // etc.) was then falsely read as not-found, triggering a spurious re-create
    // and breaking get-then-create idempotency. Key presence is a property of
    // the parsed object, not of its serialized bytes.
    let value: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        OperatorError::Other(format!(
            "agent body parse: {e}; body={}",
            &body[..body.len().min(400)]
        ))
    })?;

    // Envelope: a JSON object carrying a top-level `data` key.
    // `{status,data:<agent|null>}` — null data means not-found.
    if let Some(obj) = value.as_object() {
        if let Some(data) = obj.get("data") {
            if data.is_null() {
                return Ok(None);
            }
            let agent: AgentView = serde_json::from_value(data.clone())
                .map_err(|e| OperatorError::Other(format!("agent envelope data parse: {e}")))?;
            return Ok(Some(agent));
        }
    }

    // Bare agent object (no top-level `data` key).
    let agent: AgentView = serde_json::from_value(value)
        .map_err(|e| OperatorError::Other(format!("agent body parse (bare): {e}")))?;
    if agent.name.is_empty() {
        return Ok(None);
    }
    Ok(Some(agent))
}

/// Create an agent with the desired execution mode. Idempotency is the caller's
/// job (get-then-create); this is the raw POST.
pub async fn create_agent(config: &AgentsClientConfig, req: CreateAgentRequest<'_>) -> Result<()> {
    require_token(config)?;
    let http = http_client()?;
    let url = format!("{}/v1/agents", config.base_url.trim_end_matches('/'));
    let mut post = http.post(&url).bearer_auth(&config.token).json(&req);
    if let Some(org) = &config.org_id {
        post = post.header("X-Org-Id", org);
    }
    let resp = post.send().await.map_err(OperatorError::Http)?;
    let status = resp.status();
    let body = resp.text().await.map_err(OperatorError::Http)?;
    if !status.is_success() {
        return Err(OperatorError::Other(format!(
            "POST {url} failed (status={status}): {}",
            &body[..body.len().min(400)]
        )));
    }
    Ok(())
}

/// Convergence predicate: is this agent already in the desired execution mode?
/// A missing `execution_mode` on the row counts as NOT-yet-desired (the row
/// predates the Bot lifecycle and must be updated/recreated to carry it).
pub fn agent_in_mode(agent: &AgentView, desired_mode: &str) -> bool {
    agent.execution_mode.as_deref() == Some(desired_mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_agent_object() {
        let body = r#"{"name":"researcher","org":"hanzoai","executionMode":"long-running","status":"active"}"#;
        let a = parse_agent_body(body).unwrap().expect("some agent");
        assert_eq!(a.name, "researcher");
        assert_eq!(a.org, "hanzoai");
        assert_eq!(a.execution_mode.as_deref(), Some("long-running"));
    }

    #[test]
    fn parses_snake_case_execution_mode_alias() {
        let body = r#"{"name":"r","execution_mode":"long-running"}"#;
        let a = parse_agent_body(body).unwrap().expect("some agent");
        assert_eq!(a.execution_mode.as_deref(), Some("long-running"));
    }

    #[test]
    fn parses_envelope_data() {
        let body =
            r#"{"status":"ok","data":{"name":"r","org":"hanzoai","executionMode":"long-running"}}"#;
        let a = parse_agent_body(body).unwrap().expect("some agent");
        assert_eq!(a.name, "r");
        assert_eq!(a.execution_mode.as_deref(), Some("long-running"));
    }

    #[test]
    fn envelope_null_data_is_none() {
        let body = r#"{"status":"ok","data":null}"#;
        assert!(parse_agent_body(body).unwrap().is_none());
    }

    #[test]
    fn bare_object_without_name_is_none() {
        let body = r#"{"org":"hanzoai"}"#;
        assert!(parse_agent_body(body).unwrap().is_none());
    }

    #[test]
    fn bare_agent_containing_data_substring_still_parses() {
        // Regression: a bare agent whose NAME is `metadata` (contains the
        // substring "data") must NOT be misread as an envelope not-found. The
        // old raw-text `body.contains("\"data\"")` sniff broke this and would
        // spuriously re-create the agent.
        let body = r#"{"name":"metadata","org":"hanzoai","executionMode":"long-running"}"#;
        let a = parse_agent_body(body)
            .unwrap()
            .expect("agent must be found");
        assert_eq!(a.name, "metadata");
        assert_eq!(a.execution_mode.as_deref(), Some("long-running"));
    }

    #[test]
    fn bare_agent_with_data_valued_field_still_parses() {
        // A field VALUE containing the literal `"data"` substring must also not
        // trip the envelope path. (status is a decoded field; use it as carrier.)
        let body = r#"{"name":"r","org":"hanzoai","executionMode":"long-running","status":"has \"data\" inside"}"#;
        let a = parse_agent_body(body)
            .unwrap()
            .expect("agent must be found");
        assert_eq!(a.name, "r");
    }

    #[test]
    fn agent_in_mode_matches_and_rejects() {
        let a = AgentView {
            name: "r".into(),
            org: "hanzoai".into(),
            execution_mode: Some("long-running".into()),
            status: "active".into(),
        };
        assert!(agent_in_mode(&a, "long-running"));
        assert!(!agent_in_mode(&a, "ephemeral"));
        // Missing execution mode is never "in mode".
        let bare = AgentView {
            execution_mode: None,
            ..a.clone()
        };
        assert!(!agent_in_mode(&bare, "long-running"));
    }

    #[test]
    fn empty_token_rejected() {
        let cfg = AgentsClientConfig {
            base_url: "http://x".into(),
            token: String::new(),
            org_id: None,
        };
        assert!(require_token(&cfg).is_err());
    }
}
