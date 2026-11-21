//! Bearer verification. A caller's IAM access token is checked against the
//! issuer's published keys before anything is routed for it.
//!
//! The issuer and key set come from OIDC discovery, never from a compiled
//! issuer: discovery is read at `<origin>/.well-known/openid-configuration`
//! where the origin is the sign-in origin (`signin_origin` in
//! `<data>/hanzo/hanzod.json`, `https://hanzo.ai` when unset). Whatever issuer
//! discovery advertises is the issuer tokens must carry. Redirects are never
//! followed, and the key set is cached for ten minutes.

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(600);

#[derive(Deserialize)]
struct Claims {
    owner: Option<String>,
    name: Option<String>,
}

struct Cached {
    at: Instant,
    origin: String,
    issuer: String,
    keys: JwkSet,
}

pub struct Verifier {
    origin: String,
    cache: Mutex<Option<Cached>>,
}

fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(5)).build();
    let r = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    r.into_json().map_err(|e| format!("{url}: {e}"))
}

fn secure(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost")
}

impl Verifier {
    pub fn new(origin: String) -> Verifier {
        Verifier { origin, cache: Mutex::new(None) }
    }

    fn keys(&self, origin: &str) -> Result<(String, JwkSet), String> {
        if let Some(c) = self.cache.lock().unwrap().as_ref() {
            if c.origin == origin && c.at.elapsed() < TTL {
                return Ok((c.issuer.clone(), c.keys.clone()));
            }
        }
        if !secure(origin) {
            return Err("the sign-in origin is not https".into());
        }
        let disco = get_json(&format!("{}/.well-known/openid-configuration", origin.trim_end_matches('/')))?;
        let issuer = disco.get("issuer").and_then(|v| v.as_str()).ok_or("discovery names no issuer")?.to_string();
        let jwks_uri = disco.get("jwks_uri").and_then(|v| v.as_str()).ok_or("discovery names no jwks_uri")?;
        if !secure(jwks_uri) {
            return Err("jwks_uri is not https".into());
        }
        let keys: JwkSet = serde_json::from_value(get_json(jwks_uri)?).map_err(|e| e.to_string())?;
        *self.cache.lock().unwrap() =
            Some(Cached { at: Instant::now(), origin: origin.to_string(), issuer: issuer.clone(), keys: keys.clone() });
        Ok((issuer, keys))
    }

    /// Verify `bearer` and return its principal, `owner/name`. Blocking: call it
    /// from `spawn_blocking`.
    pub fn verify(&self, bearer: &str) -> Result<String, String> {
        let origin = self.origin.clone();
        let header = decode_header(bearer).map_err(|_| "malformed token")?;
        let (issuer, keys) = self.keys(&origin)?;
        let jwk = match &header.kid {
            Some(kid) => keys.find(kid),
            None => keys.keys.first(),
        }
        .ok_or("no such signing key")?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| "unusable signing key")?;
        let mut v = Validation::new(header.alg);
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256 | Algorithm::EdDSA) {
            return Err("algorithm not accepted".into());
        }
        v.set_issuer(&[issuer]);
        v.validate_aud = false;
        v.leeway = 30;
        v.set_required_spec_claims(&["exp", "iss"]);
        let claims = decode::<Claims>(bearer, &key, &v).map_err(|e| format!("token refused: {e}"))?.claims;
        principal(claims.owner.as_deref(), claims.name.as_deref())
    }
}

fn principal(owner: Option<&str>, name: Option<&str>) -> Result<String, String> {
    match (owner, name) {
        (Some(o), Some(n)) if !o.is_empty() && !n.is_empty() => Ok(format!("{o}/{n}")),
        _ => Err("token names no principal".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_bearer_is_refused_before_any_fetch() {
        let v = Verifier::new("https://iam.invalid".into());
        assert!(v.verify("a.b.c").unwrap_err().contains("malformed"));
    }

    #[test]
    fn a_non_https_origin_is_refused_before_any_fetch() {
        let v = Verifier::new("http://iam.example".into());
        // header decodes (alg none-ish garbage fails first), so use a real header
        let h = "eyJhbGciOiJSUzI1NiIsImtpZCI6ImsifQ.e30.c2ln";
        assert!(v.verify(h).unwrap_err().contains("not https"));
    }

    #[test]
    fn a_token_must_name_its_principal() {
        assert_eq!(principal(Some("hanzo"), Some("z")).unwrap(), "hanzo/z");
        assert!(principal(Some("hanzo"), None).is_err());
        assert!(principal(Some(""), Some("z")).is_err());
    }
}
