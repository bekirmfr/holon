//! Signing in through an identity provider (OpenID Connect), for a server several people share:
//! started with `--oidc-issuer URL --oidc-client-id ID` (or `HOLON_OIDC_ISSUER`,
//! `HOLON_OIDC_CLIENT_ID`), it reads the provider's discovery document once, and accepts the ID
//! tokens it signs as bearer tokens. The editor signs in with the authorization code flow and
//! PKCE (a public client: no secret), told where by `GET /auth/config`.
//!
//! A token is accepted only when its signature checks against the provider's published keys
//! (JWKS: read at startup, read again for a key it doesn't know, at most once a minute), it was
//! issued by that issuer for this client, and it hasn't expired (a minute's leeway for clocks).
//! Who it is (its subject, or its email when the provider says it is verified) is then matched
//! against the users file (access.rs), which says what they may do. Signing in proves who
//! someone is; the users file still decides what they may do.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;

/// Who a token says it is: its subject, and its email when verified.
#[derive(Debug, PartialEq)]
pub(crate) struct Identity {
    pub(crate) subject: String,
    pub(crate) email: Option<String>,
}

/// The algorithms accepted: what providers sign ID tokens with. Never `none`, never HMAC (that
/// would need a secret shared with the provider, and accepting it would let anyone holding the
/// public key forge one).
const ALGORITHMS: [Algorithm; 5] = [Algorithm::RS256, Algorithm::RS384, Algorithm::RS512, Algorithm::ES256, Algorithm::ES384];

/// The least time between two reads of the provider's keys.
const REFETCH_EVERY: Duration = Duration::from_secs(60);

pub(crate) struct Oidc {
    pub(crate) issuer: String,
    pub(crate) client_id: String,
    pub(crate) authorization_endpoint: String,
    pub(crate) token_endpoint: String,
    jwks_uri: Option<String>,
    keys: RwLock<(JwkSet, Option<Instant>)>,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
}

impl Oidc {
    /// The provider at `issuer`, from its discovery document and keys; why not, if it can't be
    /// read or names another issuer.
    pub(crate) fn discover(issuer: &str, client_id: &str) -> Result<Oidc, String> {
        let issuer = issuer.trim_end_matches('/').to_string();
        let url = format!("{issuer}/.well-known/openid-configuration");
        let found: Discovery = get_json(&url).map_err(|e| format!("the identity provider's discovery document {url}: {e}"))?;
        if found.issuer.trim_end_matches('/') != issuer {
            return Err(format!("{url} names the issuer {:?}, not {issuer:?}", found.issuer));
        }
        let keys = fetch_keys(&found.jwks_uri)?;
        Ok(Oidc {
            issuer: found.issuer,
            client_id: client_id.to_string(),
            authorization_endpoint: found.authorization_endpoint,
            token_endpoint: found.token_endpoint,
            jwks_uri: Some(found.jwks_uri),
            keys: RwLock::new((keys, Some(Instant::now()))),
        })
    }

    /// A provider with these keys and nothing to fetch (the tests).
    #[cfg(test)]
    pub(crate) fn with_keys(issuer: &str, client_id: &str, keys: JwkSet) -> Oidc {
        Oidc {
            issuer: issuer.into(),
            client_id: client_id.into(),
            authorization_endpoint: format!("{issuer}/authorize"),
            token_endpoint: format!("{issuer}/token"),
            jwks_uri: None,
            keys: RwLock::new((keys, None)),
        }
    }

    /// Who `token` (an ID token) says it is, or why it isn't accepted.
    pub(crate) fn verify(&self, token: &str) -> Result<Identity, String> {
        let header = jsonwebtoken::decode_header(token).map_err(|e| format!("not an ID token: {e}"))?;
        if !ALGORITHMS.contains(&header.alg) {
            return Err(format!("signed with {:?}, which isn't accepted", header.alg));
        }
        let key = self.key_for(header.kid.as_deref())?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.client_id]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = 60;
        let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation).map_err(|e| format!("{e}"))?.claims;
        let email = claims.email.filter(|_| claims.email_verified != Some(false)).map(|e| e.to_lowercase());
        Ok(Identity { subject: claims.sub, email })
    }

    /// The key `kid` names (or the only one, when the token names none); the provider's keys read
    /// again first when it isn't known, at most once a minute.
    fn key_for(&self, kid: Option<&str>) -> Result<DecodingKey, String> {
        let find = |keys: &JwkSet| match kid {
            Some(kid) => keys.find(kid).cloned(),
            None if keys.keys.len() == 1 => keys.keys.first().cloned(),
            None => None,
        };
        if let Some(jwk) = find(&self.keys.read().unwrap().0) {
            return DecodingKey::from_jwk(&jwk).map_err(|e| format!("the provider's key: {e}"));
        }
        let stale = self.keys.read().unwrap().1.is_none_or(|at| at.elapsed() >= REFETCH_EVERY);
        if let (Some(uri), true) = (&self.jwks_uri, stale) {
            let fresh = fetch_keys(uri)?;
            *self.keys.write().unwrap() = (fresh, Some(Instant::now()));
            if let Some(jwk) = find(&self.keys.read().unwrap().0) {
                return DecodingKey::from_jwk(&jwk).map_err(|e| format!("the provider's key: {e}"));
            }
        }
        Err(format!("signed with a key the provider doesn't publish ({})", kid.unwrap_or("unnamed")))
    }

    /// What the editor needs to sign in: where, and as which client. Public: the same for
    /// anyone, and needed before signing in.
    pub(crate) fn public_config(&self) -> serde_json::Value {
        serde_json::json!({
            "issuer": self.issuer,
            "client_id": self.client_id,
            "authorization_endpoint": self.authorization_endpoint,
            "token_endpoint": self.token_endpoint,
        })
    }
}

fn fetch_keys(uri: &str) -> Result<JwkSet, String> {
    get_json(uri).map_err(|e| format!("the identity provider's keys {uri}: {e}"))
}

/// `url`'s JSON, read with a timeout.
fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    let body = ureq::get(url).timeout(Duration::from_secs(10)).call().map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// Whether `token` has the shape of a JWT (three base64url parts), as an ID token does; any
/// other bearer is a plain token (access.rs).
pub(crate) fn looks_like_jwt(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header};

    pub(crate) const ISSUER: &str = "https://id.example.test";
    pub(crate) const CLIENT: &str = "holon-editor";

    pub(crate) fn provider() -> Oidc {
        Oidc::with_keys(ISSUER, CLIENT, serde_json::from_str(include_str!("testdata/oidc-test-jwks.json")).unwrap())
    }

    /// An ID token signed with the test key, with these claims over the usual ones.
    pub(crate) fn token(claims: serde_json::Value) -> String {
        let now = crate::deploy::epoch_now();
        let mut all = serde_json::json!({ "iss": ISSUER, "aud": CLIENT, "sub": "user-1", "iat": now, "exp": now + 600 });
        for (k, v) in claims.as_object().unwrap() {
            all[k] = v.clone();
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-key".into());
        jsonwebtoken::encode(&header, &all, &EncodingKey::from_rsa_pem(include_bytes!("testdata/oidc-test-key.pem")).unwrap()).unwrap()
    }

    #[test]
    fn a_token_the_provider_signed_for_this_client_says_who() {
        let who = provider().verify(&token(serde_json::json!({ "email": "Ada@Example.com", "email_verified": true }))).unwrap();
        assert_eq!(who, Identity { subject: "user-1".into(), email: Some("ada@example.com".into()) });
        // An email the provider hasn't verified isn't taken.
        let unverified = provider().verify(&token(serde_json::json!({ "email": "ada@example.com", "email_verified": false }))).unwrap();
        assert_eq!(unverified.email, None);
    }

    #[test]
    fn a_token_for_another_client_issuer_expired_or_forged_is_refused() {
        let p = provider();
        assert!(p.verify(&token(serde_json::json!({ "aud": "someone-else" }))).unwrap_err().contains("InvalidAudience"));
        assert!(p.verify(&token(serde_json::json!({ "iss": "https://evil.test" }))).unwrap_err().contains("InvalidIssuer"));
        let past = crate::deploy::epoch_now() - 3600;
        assert!(p.verify(&token(serde_json::json!({ "exp": past }))).unwrap_err().contains("ExpiredSignature"));
        // A changed payload no longer matches its signature.
        let good = token(serde_json::json!({}));
        let parts: Vec<&str> = good.split('.').collect();
        let forged_claims = token(serde_json::json!({ "sub": "admin" }));
        let forged = format!("{}.{}.{}", parts[0], forged_claims.split('.').nth(1).unwrap(), parts[2]);
        assert!(p.verify(&forged).unwrap_err().contains("InvalidSignature"));
        // HMAC and `none` aren't accepted at all.
        let hs = jsonwebtoken::encode(&Header::new(Algorithm::HS256), &serde_json::json!({ "iss": ISSUER, "aud": CLIENT, "sub": "x", "exp": crate::deploy::epoch_now() + 60 }), &EncodingKey::from_secret(b"k")).unwrap();
        assert!(p.verify(&hs).unwrap_err().contains("isn't accepted"));
    }

    #[test]
    fn only_jwt_shaped_bearers_are_taken_for_id_tokens() {
        assert!(looks_like_jwt(&token(serde_json::json!({}))));
        assert!(!looks_like_jwt("admin-token-0123456789abcdef"));
        assert!(!looks_like_jwt("a.b"));
    }
}
