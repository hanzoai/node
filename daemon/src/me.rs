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
