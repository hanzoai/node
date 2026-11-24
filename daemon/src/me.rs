//! The machine's identity as hanzod reads it, through the crate the CLI shares:
//! the non-secret index (`config.toml [auth] active`) names who is active and
//! the 0600 credential vault holds their tokens. hanzod never mints, refreshes
//! or rewrites a token; it reads what the CLI's sign-in filed.

use hanzo_identity::identity::Identity;
use hanzo_identity::token::{self, TokenSet, Vault};

pub const BRAND: &str = "hanzo";

pub fn active() -> Option<Identity> {
    hanzo_identity::index::active(BRAND)
}

pub fn label(id: &Identity) -> String {
    format!("{}/{}", id.owner, id.name)
}

/// The active identity and its stored tokens, if signed in.
pub fn tokens() -> Option<(Identity, TokenSet)> {
    let id = active()?;
    let v = token::vault().ok()?;
    let t = token::load(&*v as &dyn Vault, BRAND, &id).ok()??;
    Some((id, t))
}

/// `exp` of a JWT, unverified: a label for "is it spent", never a decision.
pub fn expiry(access_token: &str) -> Option<i64> {
    use base64::Engine;
    let p = access_token.split('.').nth(1)?;
    let b = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<serde_json::Value>(&b).ok()?.get("exp")?.as_i64()
}

/// Extract `(owner, name)` claims from a JWT payload (used only to identify this
/// machine's configured `HANZO_TOKEN_COMMAND` credential; HTTP callers are still
/// cryptographically verified by `auth::Verifier`).
pub fn jwt_identity(access_token: &str) -> Option<Identity> {
    use base64::Engine;
    let p = access_token.split('.').nth(1)?;
    let b = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(p.trim_end_matches('='))
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&b).ok()?;
    let owner = v.get("owner")?.as_str()?.trim();
    let name = v.get("name")?.as_str()?.trim();
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(Identity {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// Effective machine identity: interactive `hanzo auth` vault identity first,
/// falling back to `HANZO_TOKEN_COMMAND`'s IAM identity on headless fleet nodes.
pub fn effective_identity() -> Option<Identity> {
    active().or_else(|| fresh_token().and_then(|t| jwt_identity(&t)))
}

/// Check whether a JWT is a fleet worker `client_credentials` token (`*/hanzo-grid`),
/// which is scoped to `/v1/workflow` + `/v1/mcp` and rejected by `/v1/responses` (`401 audience`).
pub fn is_worker_only_jwt(token: &str) -> bool {
    use base64::Engine;
    let Some(p) = token.split('.').nth(1) else {
        return false;
    };
    let Ok(b) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p.trim_end_matches('='))
    else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) else {
        return false;
    };
    let name = v.get("name").and_then(|s| s.as_str()).unwrap_or("");
    let sub = v.get("sub").and_then(|s| s.as_str()).unwrap_or("");
    name == "hanzo-grid" || sub.ends_with("/hanzo-grid")
}

/// Active user inference credential for `hanzo-gen` / `dev` model routing:
/// reads the user's `hanzo auth` vault token (refreshing via `hanzo auth token`
/// when needed) or `HANZO_API_KEY`, excluding fleet-only `hanzo-grid` worker tokens.
pub fn fresh_inference_token() -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Some((_, t)) = tokens() {
        let tok = t.access_token.trim();
        if !tok.is_empty()
            && !is_worker_only_jwt(tok)
            && expiry(tok).is_none_or(|exp| exp > now + 30)
        {
            return Some(tok.to_string());
        }
    }
    let bin_name = if cfg!(windows) { "hanzo.exe" } else { "hanzo" };
    let cli_bin = dirs::home_dir()
        .and_then(|h| {
            [
                h.join(".local/bin").join(bin_name),
                h.join(".hanzo/bin").join(bin_name),
            ]
            .into_iter()
            .find(|p| p.is_file())
        })
        .unwrap_or_else(|| std::path::PathBuf::from(bin_name));
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(&cli_bin);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000);
    }
    if let Ok(out) = cmd.args(["auth", "token"]).output() {
        if out.status.success() {
            let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !tok.is_empty() && !is_worker_only_jwt(&tok) {
                return Some(tok);
            }
        }
    }
    if let Ok(raw) = std::env::var("HANZO_TOKEN_COMMAND") {
        let cmd_str = raw.trim();
        if !cmd_str.is_empty() {
            if let Ok(out) = std::process::Command::new("sh").arg("-c").arg(cmd_str).output() {
                if out.status.success() {
                    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !tok.is_empty() && !is_worker_only_jwt(&tok) {
                        return Some(tok);
                    }
                }
            }
        }
    }
    std::env::var("HANZO_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Exact argv `dev` / `hanzo-gen` can re-invoke to refresh an expiring 1h user IAM JWT.
/// Never returns a `hanzo-grid` fleet-only token command or an unsigned CLI command.
pub fn refresh_command(hanzo_bin: Option<std::path::PathBuf>) -> Vec<String> {
    if let Ok(raw) = std::env::var("HANZO_TOKEN_COMMAND") {
        let cmd = raw.trim();
        if !cmd.is_empty() {
            if let Ok(out) = std::process::Command::new("sh").arg("-c").arg(cmd).output() {
                if out.status.success() {
                    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !tok.is_empty() && !is_worker_only_jwt(&tok) {
                        if std::path::Path::new(cmd).is_file() {
                            return vec![cmd.to_string()];
                        }
                        if cfg!(windows) {
                            return vec!["cmd.exe".into(), "/C".into(), cmd.to_string()];
                        }
                        return vec!["/bin/sh".into(), "-c".into(), cmd.to_string()];
                    }
                }
            }
        }
    }
    if let Some(bin) = hanzo_bin {
        #[allow(unused_mut)]
        let mut probe = std::process::Command::new(&bin);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            probe.creation_flags(0x0800_0000);
        }
        if probe
            .args(["auth", "token"])
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return vec![bin.display().to_string(), "auth".into(), "token".into()];
        }
    }
    Vec::new()
}

/// Obtain a fresh IAM token via `HANZO_TOKEN_COMMAND` or `hanzo auth token` when
/// the vault token is missing or near expiry.
pub fn refresh_token() -> Option<String> {
    std::env::var("HANZO_TOKEN_COMMAND")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .and_then(|cmd| std::process::Command::new("sh").arg("-c").arg(cmd).output().ok())
        .filter(|o| o.status.success())
        .or_else(|| {
            let bin_name = if cfg!(windows) { "hanzo.exe" } else { "hanzo" };
            let cli_bin = dirs::home_dir()
                .and_then(|h| {
                    [
                        h.join(".local/bin").join(bin_name),
                        h.join(".hanzo/bin").join(bin_name),
                    ]
                    .into_iter()
                    .find(|p| p.is_file())
                })
                .unwrap_or_else(|| std::path::PathBuf::from(bin_name));
            #[allow(unused_mut)]
            let mut cmd = std::process::Command::new(cli_bin);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt as _;
                cmd.creation_flags(0x0800_0000);
            }
            cmd.args(["auth", "token"])
                .output()
                .ok()
                .filter(|o| o.status.success())
        })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Active unexpired IAM access token, falling back to [`refresh_token`] or `HANZO_API_KEY`.
pub fn fresh_token() -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Some((_, t)) = tokens() {
        let tok = t.access_token.trim();
        if !tok.is_empty() && expiry(tok).is_none_or(|exp| exp > now + 30) {
            return Some(tok.to_string());
        }
    }
    if let Some(refreshed) = refresh_token() {
        return Some(refreshed);
    }
    std::env::var("HANZO_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
