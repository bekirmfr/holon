//! Signing in through an identity provider (OpenID Connect), optional: an admin sets its issuer
//! and client id in the admin panel (accounts.rs keeps them), and the server reads the
//! provider's discovery document, and accepts the ID tokens it signs as bearer tokens. The
//! editor signs in with the authorization code flow and PKCE (a public client: no secret), told
//! where by `GET /auth/config`.
//!
//! A token is accepted only when its signature checks against the provider's published keys
//! (JWKS: read when set, read again for a key it doesn't know, at most once a minute whether or
//! not the provider answered), it was issued by that issuer for this client (and, when it names a
//! party it was issued to, `azp`, to this client; it must when it has several audiences), and it
//! hasn't expired (a minute's leeway for clocks). The issuer, and every address its discovery
//! document gives, is https, or plain http to this machine only (`secure`): keys that crossed a
//! network unprotected could be anyone's.
//! Who it is (its email, only when the provider says it verified it: `email_verified` true) is
//! then matched to an account
//! (access.rs), whose role says what they may do. Signing in proves who someone is; their
//! account decides what they may do.

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
    /// An email the token gave that the provider didn't say it verified: not who it is, but said
    /// back when it matches no one, so the person knows why.
    pub(crate) unverified: Option<String>,
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
    /// Who it is for: one audience, or a list of them.
    #[serde(default)]
    aud: serde_json::Value,
    /// The party it was issued to.
    #[serde(default)]
    azp: Option<String>,
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
        secured("the issuer", &issuer)?;
        let url = format!("{issuer}/.well-known/openid-configuration");
        let found: Discovery = get_json(&url).map_err(|e| format!("the identity provider's discovery document {url}: {e}"))?;
        if found.issuer.trim_end_matches('/') != issuer {
            return Err(format!("{url} names the issuer {:?}, not {issuer:?}", found.issuer));
        }
        for (what, at) in [("its keys", &found.jwks_uri), ("where to sign in", &found.authorization_endpoint), ("where to get tokens", &found.token_endpoint)] {
            secured(&format!("{url} gives {what}, and that"), at)?;
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
        // The party it was issued to, when it names one, is this client; and it must name one
        // when the token is for several audiences (OpenID Connect Core 3.1.3.7).
        let several = claims.aud.as_array().is_some_and(|all| all.len() > 1);
        match claims.azp.as_deref() {
            Some(azp) if azp != self.client_id => return Err(format!("issued to {azp:?}, not this client")),
            None if several => return Err("for several audiences, without saying which it was issued to (azp)".into()),
            _ => {}
        }
        // Only an email the provider says it verified: an account is matched by its email
        // (access.rs), so one a person merely typed at the provider would sign them in as whoever
        // has it. A provider that doesn't say (no `email_verified`) signs nobody in by email.
        let verified = claims.email_verified == Some(true);
        let email = claims.email.map(|e| e.to_lowercase());
        let (email, unverified) = if verified { (email, None) } else { (None, email) };
        Ok(Identity { subject: claims.sub, email, unverified })
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
        // Asked at most once a minute, counted from the last time it was asked, whether or not it
        // answered: a token naming a key nobody publishes can't make every request wait on it.
        let due = {
            let mut keys = self.keys.write().unwrap();
            let due = keys.1.is_none_or(|at| at.elapsed() >= REFETCH_EVERY);
            if due {
                keys.1 = Some(Instant::now());
            }
            due
        };
        if let (Some(uri), true) = (&self.jwks_uri, due) {
            let fresh = fetch_keys(uri)?;
            self.keys.write().unwrap().0 = fresh;
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

/// Whether `url` may carry what signing in rests on: https, or plain http to this machine only (a
/// provider run beside the server), which nothing on a network can change on the way.
fn secure(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // `http://localhost:80@elsewhere/` is elsewhere's.
    if authority.contains('@') {
        return false;
    }
    let host = if authority.starts_with("[::1]") { "[::1]" } else { authority.split(':').next().unwrap_or("") };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// `url`, if `secure`; else why it isn't taken, `what` it is.
fn secured(what: &str, url: &str) -> Result<(), String> {
    if secure(url) {
        Ok(())
    } else {
        Err(format!("{what} ({url}) must be https (plain http only to this machine): keys read over plain http could be anyone's"))
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
        signed("test-key", claims)
    }

    /// As `token`, its header naming the key `kid`.
    fn signed(kid: &str, claims: serde_json::Value) -> String {
        let now = crate::deploy::epoch_now();
        let mut all = serde_json::json!({ "iss": ISSUER, "aud": CLIENT, "sub": "user-1", "iat": now, "exp": now + 600 });
        for (k, v) in claims.as_object().unwrap() {
            all[k] = v.clone();
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.into());
        jsonwebtoken::encode(&header, &all, &EncodingKey::from_rsa_pem(include_bytes!("testdata/oidc-test-key.pem")).unwrap()).unwrap()
    }

    #[test]
    fn a_token_the_provider_signed_for_this_client_says_who() {
        let who = provider().verify(&token(serde_json::json!({ "email": "Ada@Example.com", "email_verified": true }))).unwrap();
        assert_eq!(who, Identity { subject: "user-1".into(), email: Some("ada@example.com".into()), unverified: None });
        // An email the provider hasn't verified isn't taken; nor one it doesn't say of.
        let unverified = provider().verify(&token(serde_json::json!({ "email": "ada@example.com", "email_verified": false }))).unwrap();
        assert_eq!(unverified.email, None);
        let unsaid = provider().verify(&token(serde_json::json!({ "email": "ada@example.com" }))).unwrap();
        assert_eq!((unsaid.email, unsaid.unverified.as_deref()), (None, Some("ada@example.com")), "no email_verified: not taken");
    }

    /// Which ID tokens are taken: each row a token that differs from a good one in one way (its
    /// audience, the party it was issued to, its issuer, its expiry, its signature, its key), and
    /// what is said of it: taken, or refused for that reason (OpenID Connect Core 3.1.3.7).
    #[test]
    fn which_id_tokens_are_taken() {
        use serde_json::json;
        let p = provider();
        let now = crate::deploy::epoch_now();
        let good = token(json!({}));
        let part = |t: &str, i: usize| t.split('.').nth(i).unwrap().to_string();
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            ("as issued", good.clone(), None),
            ("for another client", token(json!({ "aud": "someone-else" })), Some("InvalidAudience")),
            ("for this client, as a list", token(json!({ "aud": [CLIENT] })), None),
            ("for this client, issued to it", token(json!({ "azp": CLIENT })), None),
            ("for this client, issued to another", token(json!({ "azp": "someone-else" })), Some("not this client")),
            ("for this client and another, issued to it", token(json!({ "aud": [CLIENT, "other"], "azp": CLIENT })), None),
            ("for this client and another, issued to the other", token(json!({ "aud": [CLIENT, "other"], "azp": "other" })), Some("not this client")),
            ("for this client and another, not saying to whom", token(json!({ "aud": [CLIENT, "other"] })), Some("azp")),
            ("by another issuer", token(json!({ "iss": "https://evil.test" })), Some("InvalidIssuer")),
            ("expired within the clocks' leeway", token(json!({ "exp": now - 30 })), None),
            ("expired an hour ago", token(json!({ "exp": now - 3600 })), Some("ExpiredSignature")),
            ("its payload changed", format!("{}.{}.{}", part(&good, 0), part(&token(json!({ "sub": "admin" })), 1), part(&good, 2)), Some("InvalidSignature")),
            ("signed with a key the provider doesn't publish", signed("no-such-key", json!({})), Some("doesn't publish")),
            (
                "signed with HMAC",
                jsonwebtoken::encode(&Header::new(Algorithm::HS256), &json!({ "iss": ISSUER, "aud": CLIENT, "sub": "x", "exp": now + 60 }), &EncodingKey::from_secret(b"k")).unwrap(),
                Some("isn't accepted"),
            ),
            // `{"alg":"none"}`, unsigned.
            ("not signed at all", format!("eyJhbGciOiJub25lIn0.{}.", part(&good, 1)), Some("not an ID token")),
        ];
        for (what, token, refused) in rows {
            match (p.verify(&token), refused) {
                (Ok(_), None) => {}
                (Err(e), Some(why)) if e.contains(why) => {}
                (got, _) => panic!("a token {what}: {got:?}, where {}", refused.map_or("it is taken".to_string(), |why| format!("refused: {why}"))),
            }
        }
    }

    /// The issuer, and every address its discovery document gives, comes over https or from this
    /// machine; anything else is refused before it is read.
    #[test]
    fn what_signing_in_rests_on_comes_over_https_or_from_this_machine() {
        for (url, taken) in [
            ("https://id.example.com/realms/x", true),
            ("HTTPS://id.example.com", true),
            ("http://127.0.0.1:7530", true),
            ("http://localhost:7530/realms/x", true),
            ("http://[::1]:7530/x", true),
            ("http://id.example.com", false),
            ("http://localhost.example.com", false),
            ("http://127.0.0.1.example.com", false),
            ("http://localhost:80@example.com/", false),
            ("ftp://id.example.com", false),
            ("id.example.com", false),
        ] {
            assert_eq!(secure(url), taken, "{url}");
        }
        assert!(Oidc::discover("http://id.example.com", CLIENT).err().unwrap().contains("must be https"), "refused before anything is read");
    }

    #[test]
    fn a_key_nobody_publishes_asks_the_provider_once_a_minute_whether_or_not_it_answers() {
        // Nothing listens on the discard port here: asking fails at once.
        let p = Oidc { jwks_uri: Some("http://127.0.0.1:9/keys".into()), ..provider() };
        let unknown = signed("no-such-key", serde_json::json!({}));
        assert!(p.verify(&unknown).unwrap_err().contains("the identity provider's keys"), "asked, and no answer");
        assert!(p.verify(&unknown).unwrap_err().contains("doesn't publish"), "not asked again within the minute");
    }

    #[test]
    fn only_jwt_shaped_bearers_are_taken_for_id_tokens() {
        assert!(looks_like_jwt(&token(serde_json::json!({}))));
        assert!(!looks_like_jwt("admin-token-0123456789abcdef"));
        assert!(!looks_like_jwt("a.b"));
    }
}
