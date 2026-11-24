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
