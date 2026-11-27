// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `/v1/computer` — local desktop control over the native backend in `hanzo-mcp`.
//!
//!   GET  /v1/computer                       the action table
//!   GET  /v1/computer/{action}?…            a read action, arguments from the query
//!   POST /v1/computer  {"action":…, …}      any action, arguments from JSON
//!   POST /v1/computer/{action}  {…}         any action, path wins over the body
//!
//! The screen and input implementation is the canonical `ComputerTool` from
//! `hanzo-mcp`; nothing here reimplements capture or events. What hanzod adds
//! is local policy: reads are open, acts need approval evidence on every call,
//! and every request is recorded in `audit.jsonl` beside the daemon state.

use crate::paths;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use hanzo_mcp::{tools::ComputerToolArgs, ComputerTool};
use serde_json::{json, Map, Value};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

/// Read actions change nothing on the machine.
pub const READ_ACTIONS: &[&str] = &[
    "info",
    "get_screens",
    "screen_size",
    "position",
    "list_windows",
    "get_active_window",
    "screenshot",
    "screenshot_region",
];

/// Act actions move the mouse, type, focus windows, or change tool settings.
pub const ACT_ACTIONS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "middle_click",
    "move",
    "move_relative",
    "drag",
    "drag_relative",
    "scroll",
    "type",
    "write",
    "press",
    "key_down",
    "key_up",
    "hotkey",
    "focus_window",
    "batch",
    "sleep",
    "set_pause",
    "set_failsafe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Read,
    Act,
}

/// The action name the backend's `FromStr` accepts, or `None` for anything this
/// route must refuse. Underscores, hyphens and camel case all name one action,
/// matching `ComputerToolArgs::action`'s parser.
pub fn classify(action: &str) -> Option<Kind> {
    let flat: String = action
        .trim()
        .chars()
        .filter(|c| *c != '_' && *c != '-' && !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let name = flat.as_str();
    if READ_ACTIONS.iter().any(|a| a.replace('_', "") == name) {
        Some(Kind::Read)
    } else if ACT_ACTIONS.iter().any(|a| a.replace('_', "") == name) {
        Some(Kind::Act)
    } else {
        None
    }
}

/// Approval evidence is per request, never a session flag: a header
/// `X-Hanzo-Approve: true` or a body field `{"approval": true}` (also
/// `{"approval": {"approved": true}}`). Anything else is absent.
pub fn approved(req: &Request, body: &Value) -> bool {
    let header = req
        .header("x-hanzo-approve")
        .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "approved"));
    let field = match body.get("approval") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Object(o)) => o.get("approved").and_then(Value::as_bool).unwrap_or(false),
        _ => false,
    };
    header || field
}

/// One action, or the actions nested in a `batch`, each with its policy kind.
/// A batch is gated as a whole: any act inside requires approval for the call.
pub fn ops(action: &str, args: &Value) -> Result<Vec<(String, Kind)>, String> {
    if action == "batch" {
        let list = args
            .get("actions")
            .and_then(Value::as_array)
            .ok_or_else(|| "batch requires an actions array".to_string())?;
        let mut out = Vec::with_capacity(list.len());
        for a in list {
            let name = a.get("action").and_then(Value::as_str).unwrap_or("info");
            let kind = classify(name).ok_or_else(|| format!("unknown nested action: {name}"))?;
            out.push((name.to_string(), kind));
        }
        if out.is_empty() {
            return Err("batch requires at least one action".to_string());
        }
        Ok(out)
    } else {
        let kind = classify(action).ok_or_else(|| format!("unknown action: {action}"))?;
        Ok(vec![(action.to_string(), kind)])
    }
}

/// Append-only audit trail. One JSON line per dispatched op (including denied
/// ones) with actor, action, time, approval evidence and outcome. Only fixed
/// strings are recorded: no coordinates, pixels, typed text, keys or tokens.
pub struct Audit {
    path: PathBuf,
    next: Mutex<u64>,
}

impl Audit {
    pub fn open(path: PathBuf) -> Audit {
        let next = std::fs::read_to_string(&path).map(|s| s.lines().count() as u64 + 1).unwrap_or(1);
        Audit { path, next: Mutex::new(next) }
    }

    pub fn record(&self, who: &str, action: &str, approved: bool, required: bool, outcome: &str) -> u64 {
        let mut next = self.next.lock().unwrap();
        let id = *next;
        let time = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let line = json!({
            "id": id,
            "time": time,
            "who": who,
            "action": action,
            "approved": approved,
            "required": required,
            "outcome": outcome
        })
        .to_string();
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        if let Ok(mut f) = opts.open(&self.path) {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
            *next += 1;
        }
        id
    }
}

/// The subsystem. `tool` is a mutex because `ComputerTool::execute` takes
/// `&mut self`: only one desktop action may be in flight here at a time.
pub struct Computer {
    audit: Audit,
    tool: AsyncMutex<ComputerTool>,
}

impl Default for Computer {
    fn default() -> Self {
        Self::new()
    }
}

impl Computer {
    pub fn new() -> Self {
        Self::with_audit(paths::data().join("audit.jsonl"))
    }

    /// Test seam: the same policy with a private audit file.
    pub fn with_audit(path: PathBuf) -> Self {
        Computer { audit: Audit::open(path), tool: AsyncMutex::new(ComputerTool::new()) }
    }

    fn denied(&self, who: &str, action: &str, approved: bool, required: bool, status: u16, why: &str) -> Response {
        self.audit.record(who, action, approved, required, "denied");
        Response::error(status, why)
    }
}

impl Subsystem for Computer {
    fn name(&self) -> &'static str {
        "computer"
    }
    fn prefix(&self) -> &'static str {
        "/v1/computer"
    }
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            let path = req.path().to_string();
            let mut body = match req.method.as_str() {
                "POST" => match serde_json::from_slice::<Value>(&req.body) {
                    Ok(Value::Object(m)) => m,
                    Ok(_) => {
                        return Ok(Reply::Whole(Response::error(400, "body must be a JSON object")))
                    }
                    Err(e) => return Ok(Reply::Whole(Response::error(400, &format!("invalid JSON: {e}")))),
                },
                "GET" => Map::new(),
                _ => return Ok(Reply::Whole(Response::error(405, "GET or POST"))),
            };

            // /v1/computer/{action} wins over the body's action field.
            let tail = path.strip_prefix("/v1/computer").unwrap_or("").trim_start_matches('/');
            if !tail.is_empty() {
                let action = unescape(tail);
                body.insert("action".into(), Value::String(action));
            }
            if !req.method.eq_ignore_ascii_case("POST") {
                for (k, v) in query(&req.target) {
                    body.insert(k, v);
                }
            }

            if path == "/v1/computer" && !req.method.eq_ignore_ascii_case("POST") {
                return Ok(Reply::Whole(Response::json(
                    200,
                    &json!({
                        "name": "computer",
                        "read": READ_ACTIONS,
                        "act": ACT_ACTIONS,
                        "approval": "X-Hanzo-Approve: true, or {\"approval\": true} in the body",
                        "audit": "audit.jsonl beside the daemon state dir",
                        "backend": "hanzo-mcp ComputerTool"
                    }),
                )));
            }

            let action = body.get("action").and_then(Value::as_str).unwrap_or("info").to_string();
            let args = Value::Object(body);
            let evidence = approved(&req, &args);
            let dispatched = match ops(&action, &args) {
                Ok(o) => o,
                Err(why) => {
                    return Ok(Reply::Whole(self.denied(who, &action, evidence, false, 400, &why)))
                }
            };
            let required = dispatched.iter().any(|(_, k)| *k == Kind::Act);
            if required && !evidence {
                return Ok(Reply::Whole(self.denied(
                    who,
                    &action,
                    evidence,
                    required,
                    403,
                    &format!("approval required for act action {action}: send X-Hanzo-Approve: true or {{\"approval\": true}}"),
                )));
            }

            // Approval evidence is policy metadata, not a tool argument.
            let mut tool_args = args.as_object().cloned().unwrap_or_default();
            tool_args.remove("approval");
            tool_args.insert("action".into(), Value::String(action.clone()));
            let tool_args = match serde_json::from_value::<ComputerToolArgs>(Value::Object(tool_args)) {
                Ok(a) => a,
                Err(e) => {
                    return Ok(Reply::Whole(self.denied(who, &action, evidence, required, 400, &e.to_string())))
                }
            };

            let result = {
                let mut tool = self.tool.lock().await;
                tool.execute(tool_args).await
            };
            let response = match result {
                Ok(text) => {
                    let value = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
                    // A batch reports each nested op's own result; a failed nested
                    // op must be audited as an error, not hidden by the envelope.
                    let nested = if action == "batch" {
                        value.get("results").and_then(Value::as_array).cloned()
                    } else {
                        None
                    };
                    for (i, (name, kind)) in dispatched.iter().enumerate() {
                        let outcome = match nested.as_ref().and_then(|rs| rs.get(i)) {
                            Some(r) if r.get("success").and_then(Value::as_bool) == Some(true) => "ok",
                            Some(_) => "error",
                            None => "ok",
                        };
                        self.audit.record(who, name, evidence, *kind == Kind::Act, outcome);
                    }
                    Response::json(200, &value)
                }
                Err(e) => {
                    for (name, kind) in &dispatched {
                        self.audit.record(who, name, evidence, *kind == Kind::Act, "error");
                    }
                    Response::error(400, &e.to_string())
                }
            };
            Ok(Reply::Whole(response))
        })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async {})
    }
}

/// Minimal `%XX` decoding for an action in the path.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Query arguments for the read-on-GET form. Numbers and the handful of array
/// arguments are coerced so the same `ComputerToolArgs` serves both methods.
fn query(target: &str) -> Vec<(String, Value)> {
    let Some((_, q)) = target.split_once('?') else { return Vec::new() };
    q.split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let k = unescape(k);
            let v = unescape(v);
            let value = match k.as_str() {
                "region" => Value::Array(v.split(',').filter_map(|n| n.trim().parse::<i32>().ok()).map(Value::from).collect()),
                "keys" => Value::Array(v.split(',').map(|s| Value::String(s.to_string())).collect()),
                "x" | "y" | "dx" | "dy" | "end_x" | "end_y" | "amount" | "width" | "height" => {
                    v.parse::<i32>().map(Value::from).unwrap_or(Value::String(v))
                }
                "duration" | "interval" | "value" => v.parse::<f64>().map(Value::from).unwrap_or(Value::String(v)),
                "clear" => Value::Bool(matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes")),
                _ => Value::String(v),
            };
            Some((k, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, target: &str, body: Value, headers: &[(&str, &str)]) -> Request {
        Request {
            method: method.into(),
            target: target.into(),
            headers: headers.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    #[test]
    fn every_documented_action_is_classified() {
        for a in READ_ACTIONS {
            assert_eq!(classify(a), Some(Kind::Read), "{a}");
        }
        for a in ACT_ACTIONS {
            assert_eq!(classify(a), Some(Kind::Act), "{a}");
        }
        for a in ["doubleclick", "GET-ACTIVE-WINDOW", "key up", "screenshotRegion"] {
            assert!(classify(a).is_some(), "{a}");
        }
        assert_eq!(classify(""), None);
        assert_eq!(classify("rm_rf"), None);
    }

    #[test]
    fn approval_is_per_request_evidence_only() {
        let none = req("POST", "/v1/computer", json!({"action": "click", "x": 1, "y": 2}), &[]);
        let no_body: Value = serde_json::from_slice(&none.body).unwrap();
        assert!(!approved(&none, &no_body));
        let header = req("POST", "/v1/computer", json!({}), &[("X-HANZO-APPROVE", " true ")]);
        assert!(approved(&header, &json!({})));
        let bad_header = req("POST", "/v1/computer", json!({}), &[("X-Hanzo-Approve", "no")]);
        assert!(!approved(&bad_header, &json!({})));
        assert!(approved(&none, &json!({"approval": true})));
        assert!(approved(&none, &json!({"approval": {"approved": true}})));
        assert!(!approved(&none, &json!({"approval": {"approved": false}})));
        assert!(!approved(&none, &json!({"approval": "true"})));
    }

    #[test]
    fn batch_is_gated_as_a_whole() {
        let batch = json!({"actions": [{"action": "screenshot"}, {"action": "click", "x": 2, "y": 3}]});
        let listed = ops("batch", &batch).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].1, Kind::Read);
        assert_eq!(listed[1].1, Kind::Act);
        assert!(listed.iter().any(|(_, k)| *k == Kind::Act));
        assert!(ops("batch", &json!({"actions": [{"action": "screen_size"}]}))
            .unwrap()
            .iter()
            .all(|(_, k)| *k == Kind::Read));
        assert!(ops("batch", &json!({"actions": [{"action": "rm_rf"}]})).is_err());
        assert!(ops("batch", &json!({})).is_err());
        assert!(ops("nope", &json!({})).is_err());
    }

    #[test]
    fn audit_rows_have_the_shape_and_never_carry_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let audit = Audit::open(path.clone());
        // The only arguments `record` takes are policy metadata, never tool args.
        audit.record("hanzo/z", "type", true, true, "ok");
        let line = std::fs::read_to_string(&path).unwrap();
        let row: Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        for key in ["id", "time", "who", "action", "approved", "required", "outcome"] {
            assert!(row.get(key).is_some(), "missing {key}: {row}");
        }
        assert_eq!(row["who"], "hanzo/z");
        assert_eq!(row["action"], "type");
        assert_eq!(row["approved"], true);
        assert_eq!(row["required"], true);
        assert_eq!(row["outcome"], "ok");
        assert!(!line.contains("text"), "audit leaked typed text");
        assert!(!line.contains("base64"));
    }

    #[tokio::test]
    async fn act_without_approval_and_unknown_actions_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let c = Computer::with_audit(path.clone());

        let act = req("POST", "/v1/computer/click", json!({"x": 10, "y": 20}), &[]);
        let Reply::Whole(r) = c.frame("hanzo/z", act).await.unwrap() else { panic!() };
        assert_eq!(r.status, 403, "{}", String::from_utf8_lossy(&r.body));
        assert!(String::from_utf8_lossy(&r.body).contains("approval required"));

        let bad = req("POST", "/v1/computer", json!({"action": "rm_rf"}), &[]);
        let Reply::Whole(r) = c.frame("hanzo/z", bad).await.unwrap() else { panic!() };
        assert_eq!(r.status, 400);

        let malformed = Request {
            method: "POST".into(),
            target: "/v1/computer".into(),
            headers: vec![],
            body: b"{".to_vec(),
        };
        let Reply::Whole(r) = c.frame("hanzo/z", malformed).await.unwrap() else { panic!() };
        assert_eq!(r.status, 400);

        let batch = req(
            "POST",
            "/v1/computer",
            json!({"action": "batch", "actions": [{"action": "click", "x": 1, "y": 2}]}),
            &[],
        );
        let Reply::Whole(r) = c.frame("hanzo/z", batch).await.unwrap() else { panic!() };
        assert_eq!(r.status, 403, "batch of acts must be gated as a whole");

        let rows: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(rows.iter().any(|r| r["action"] == "click" && r["outcome"] == "denied" && r["approved"] == false));
        assert!(rows.iter().any(|r| r["action"] == "rm_rf" && r["outcome"] == "denied"));
        assert!(rows.iter().any(|r| r["action"] == "batch" && r["outcome"] == "denied"));
        assert!(rows.iter().all(|r| r["outcome"] != "ok"), "no act should have run");
    }

    #[tokio::test]
    async fn a_read_batch_logs_each_nested_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let c = Computer::with_audit(path.clone());
        let r = req(
            "POST",
            "/v1/computer",
            json!({"action": "batch", "actions": [{"action": "screen_size"}, {"action": "position"}]}),
            &[],
        );
        let Reply::Whole(resp) = c.frame("hanzo/z", r).await.unwrap() else { panic!() };
        assert_eq!(resp.status, 200, "{}", String::from_utf8_lossy(&resp.body));
        let rows: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0]["action"], "screen_size");
        assert_eq!(rows[1]["action"], "position");
        assert!(rows.iter().all(|r| r["outcome"] == "ok" && r["required"] == false));
    }

    #[tokio::test]
    async fn read_ops_need_no_approval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let c = Computer::with_audit(path.clone());
        let r = req("POST", "/v1/computer/screen_size", json!({}), &[]);
        let Reply::Whole(resp) = c.frame("hanzo/z", r).await.unwrap() else { panic!() };
        assert_eq!(resp.status, 200, "{}", String::from_utf8_lossy(&resp.body));
        let row: Value = serde_json::from_str(std::fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
        assert_eq!(row["action"], "screen_size");
        assert_eq!(row["required"], false);
        assert_eq!(row["outcome"], "ok");
    }
}
