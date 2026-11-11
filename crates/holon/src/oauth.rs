//! OAuth 2.0 tokens, for a mailbox that logs in with one instead of a password (host-capabilities
//! L4 §5.2.6): Microsoft 365 and Google Workspace take a token over IMAP and SMTP (`XOAUTH2`,
//! smtp.rs and imap.rs), and the host asks for it.
//!
//! - **The request:** a URL read from the binding's `--oauth` source and never kept, the client's
//!   id and secret as its login: `https://id:secret@login.microsoftonline.com/<tenant>/oauth2/v2.0/token`
//!   with `?scope=…` (client credentials) or `?refresh_token=…` (a refresh token), its values
//!   percent-encoded. `https://` only; plain `http://` to a loopback address, the tests'.
//! - **The token:** asked for with a form POST, the client's id and secret in the body, which
//!   Microsoft and Google both take; kept in memory until a minute before it runs out, by the
//!   request's value, so a value an admin changes asks anew. A login it was refused at drops it
//!   ([`Tokens::forget`]), and mails.rs tries that login once more with a new one. A refresh token
//!   the answer replaces is used in place of the source's for the server's life.
//! - **What a failure is:** [`Failure::Temporary`] (no connection, a 5xx or 429), else
//!   [`Failure::Permanent`], saying the answer's `error` and the start of its `error_description`.
//!   Neither holds the URL's login or query.

use std::collections::BTreeMap;
use crate::sync::Mutex;
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::smtp::{Failure, unpercent};

/// How long the token request may take.
const TIMEOUT: Duration = Duration::from_secs(30);
/// How long before a token runs out a new one is asked for.
const MARGIN: Duration = Duration::from_secs(60);
/// A token's life, in seconds, when the answer doesn't say.
const DEFAULT_LIFE: u64 = 3600;
/// The longest part of an `error_description` said.
const MAX_SAID: usize = 200;

const USAGE: &str = "the token URL is https://id:secret@host/path?scope=… or ?refresh_token=…";

/// A token request, as the binding's source writes it.
pub(crate) struct Grant {
    /// Where it is sent: `https://login.microsoftonline.com/<tenant>/oauth2/v2.0/token`.
    endpoint: String,
    client_id: Zeroizing<String>,
    client_secret: Zeroizing<String>,
    scope: Option<String>,
    refresh_token: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.endpoint)
    }
}

impl Grant {
    /// The request `url` writes, or why it isn't one. The error never holds the URL.
    pub(crate) fn parse(url: &str) -> Result<Grant, String> {
        let (scheme, rest) = url.split_once("://").ok_or(USAGE)?;
        if scheme != "https" && scheme != "http" {
            return Err(USAGE.into());
        }
        let rest = rest.split('#').next().unwrap_or("");
        let (before, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = match before.split_once('/') {
            Some((a, p)) => (a, format!("/{p}")),
            None => (before, "/".to_string()),
        };
        let (user_info, host) = authority.rsplit_once('@').ok_or("the token URL's login is the client's id and secret: https://id:secret@host/…")?;
        let (id, secret) = user_info.split_once(':').ok_or("the token URL's login is the client's id and secret: https://id:secret@host/…")?;
        let (client_id, client_secret) = (unpercent(id)?, unpercent(secret)?);
        if client_id.is_empty() {
            return Err("the token URL's login names no client".into());
        }
        let host = host.to_ascii_lowercase();
        if host.is_empty() {
            return Err("the token URL names no host".into());
        }
        if scheme == "http" && !is_loopback(&host) {
            return Err("a token is asked for over https://: plain http:// only to this machine".into());
        }
        let (mut scope, mut refresh_token) = (None, None);
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = unpercent(value).map_err(|_| format!("the token URL's {key} has a % that isn't %XX"))?;
            match key {
                "scope" => scope = Some(value.to_string()),
                "refresh_token" => refresh_token = Some(value),
                _ => return Err(format!("the token URL's query says {key:?}: it takes scope and refresh_token")),
            }
        }
        if scope.is_none() && refresh_token.is_none() {
            return Err("the token URL says scope=… (client credentials) or refresh_token=…, and says neither".into());
        }
        Ok(Grant { endpoint: format!("{scheme}://{host}{path}"), client_id, client_secret, scope, refresh_token })
    }

    /// Where it is sent, without its login or query.
    pub(crate) fn address(&self) -> String {
        self.endpoint.clone()
    }
}

/// Whether `host` (`127.0.0.1:8080`, `[::1]`, `localhost`) is this machine.
fn is_loopback(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(h, _)| h),
    };
    name == "localhost" || name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The tokens the host holds, in memory only, by the request each was asked with.
#[derive(Default)]
pub(crate) struct Tokens(Mutex<BTreeMap<[u8; 32], Kept>>);

#[derive(Default)]
struct Kept {
    /// The token, and when it runs out.
    access: Option<(Zeroizing<String>, Instant)>,
    /// A refresh token an answer gave in place of the request's.
    refresh: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tokens({})", self.0.lock().unwrap().len())
    }
}

impl Tokens {
    /// A token for the request `url`: the one kept, unless it runs out within a minute; else a new
    /// one, asked for now.
    pub(crate) fn token(&self, url: &str) -> Result<Zeroizing<String>, Failure> {
        let key = *blake3::hash(url.as_bytes()).as_bytes();
        let refresh = match self.0.lock().unwrap().get(&key) {
            Some(Kept { access: Some((token, until)), .. }) if Instant::now() + MARGIN < *until => return Ok(token.clone()),
            Some(kept) => kept.refresh.clone(),
            None => None,
        };
        let grant = Grant::parse(url).map_err(Failure::Permanent)?;
        let (token, life, replaced) = ask(&grant, refresh.as_ref().map(|r| r.as_str()))?;
        let mut kept = self.0.lock().unwrap();
        let k = kept.entry(key).or_default();
        k.access = Some((token.clone(), Instant::now() + Duration::from_secs(life)));
        if replaced.is_some() {
            k.refresh = replaced;
        }
        Ok(token)
    }

    /// Drop the token kept for the request `url`: a login refused it, so the next asks anew.
    pub(crate) fn forget(&self, url: &str) {
        if let Some(k) = self.0.lock().unwrap().get_mut(blake3::hash(url.as_bytes()).as_bytes()) {
            k.access = None;
        }
    }
}

/// What an endpoint gives: the token, its life in seconds, and the refresh token it gave in place
/// of the one sent.
type Given = (Zeroizing<String>, u64, Option<Zeroizing<String>>);

/// Ask `grant`'s endpoint for a token, with `refresh` in place of the grant's refresh token.
fn ask(grant: &Grant, refresh: Option<&str>) -> Result<Given, Failure> {
    let mut form: Vec<(&str, &str)> = vec![("client_id", grant.client_id.as_str()), ("client_secret", grant.client_secret.as_str())];
    let sent = refresh.or(grant.refresh_token.as_ref().map(|r| r.as_str()));
    match sent {
        Some(r) => form.extend([("grant_type", "refresh_token"), ("refresh_token", r)]),
        None => form.push(("grant_type", "client_credentials")),
    }
    if let Some(scope) = &grant.scope {
        form.push(("scope", scope));
    }
    let at = grant.address();
    // A token endpoint that sends the client's secret on elsewhere isn't followed.
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build();
    let answer = match agent.post(&grant.endpoint).set("Accept", "application/json").send_form(&form) {
        Ok(a) if a.status() == 200 => a,
        Ok(a) => return Err(Failure::Permanent(format!("{at} answered {} to the token request, not a token", a.status()))),
        Err(ureq::Error::Status(code, a)) => return Err(refused(&at, code, a)),
        Err(ureq::Error::Transport(t)) => return Err(Failure::Temporary(format!("{at} can't be reached: {}", t.message().map_or_else(|| t.kind().to_string(), str::to_string)))),
    };
    let body = Zeroizing::new(answer.into_string().map_err(|e| Failure::Temporary(format!("{at}'s answer broke off: {e}")))?);
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|_| Failure::Permanent(format!("{at} answered something that isn't a token")))?;
    let token = json["access_token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| Failure::Permanent(format!("{at} answered without an access_token")))?;
    if let Some(kind) = json["token_type"].as_str().filter(|k| !k.eq_ignore_ascii_case("bearer")) {
        return Err(Failure::Permanent(format!("{at} gave a {kind} token, not a bearer one")));
    }
    let life = match &json["expires_in"] {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .unwrap_or(DEFAULT_LIFE);
    let replaced = json["refresh_token"].as_str().filter(|r| !r.is_empty() && Some(*r) != sent).map(|r| Zeroizing::new(r.to_string()));
    Ok((Zeroizing::new(token.to_string()), life, replaced))
}

/// What an endpoint's refusal says: its `error`, and its `error_description`'s first line, cut
/// short. A 5xx or 429 is worth trying again.
fn refused(at: &str, code: u16, answer: ureq::Response) -> Failure {
    let json: serde_json::Value = answer.into_string().ok().and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default();
    let error = json["error"].as_str().unwrap_or("no error said");
    let description: String = json["error_description"].as_str().and_then(|d| d.lines().next()).unwrap_or("").chars().take(MAX_SAID).collect();
    let said = if description.is_empty() { format!("{at} refused the token request ({code}): {error}") } else { format!("{at} refused the token request ({code}): {error}: {description}") };
    if code >= 500 || code == 429 { Failure::Temporary(said) } else { Failure::Permanent(said) }
}

/// A token endpoint for the tests, plain on loopback: it gives its token to the client it was made
/// for, and refuses another (401 `invalid_client`), keeping each request's form.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use crate::sync::Mutex;

    pub(crate) struct Endpoint {
        pub(crate) port: u16,
        /// Each request's form, in order.
        pub(crate) asked: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
        /// The life each token is given, in seconds.
        pub(crate) life: Arc<AtomicU64>,
        /// The token it gives.
        pub(crate) token: Arc<Mutex<String>>,
    }

    impl Endpoint {
        /// Its URL with the client's login and `query`: `http://id:secret@127.0.0.1:port/token?query`.
        pub(crate) fn url(&self, id: &str, secret: &str, query: &str) -> String {
            format!("http://{id}:{secret}@127.0.0.1:{}/token?{query}", self.port)
        }
    }

    /// One that gives `token` to the client `id` with `secret`, and with a refresh token, a new one
    /// each time (`rotated-1`, `rotated-2`…).
    pub(crate) fn start(id: &str, secret: &str, token: &str) -> Endpoint {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let life = Arc::new(AtomicU64::new(3600));
        let token = Arc::new(Mutex::new(token.to_string()));
        let (kept, lives, given, id, secret) = (Arc::clone(&asked), Arc::clone(&life), Arc::clone(&token), id.to_string(), secret.to_string());
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut out = conn.try_clone().unwrap();
                let mut reader = BufReader::new(conn);
                let (mut line, mut length) = (String::new(), 0usize);
                let _ = reader.read_line(&mut line);
                let path_ok = line.starts_with("POST /token ");
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                        break;
                    }
                    if let Some((_, value)) = header.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("content-length")) {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let form: BTreeMap<String, String> = String::from_utf8_lossy(&body)
                    .split('&')
                    .filter_map(|p| p.split_once('='))
                    .map(|(k, v)| (decoded(k), decoded(v)))
                    .collect();
                let mut all = kept.lock().unwrap();
                all.push(form.clone());
                let n = all.len();
                drop(all);
                let client = form.get("client_id") == Some(&id) && form.get("client_secret") == Some(&secret);
                let token = given.lock().unwrap().clone();
                let (status, json) = if !path_ok {
                    ("404 Not Found", serde_json::json!({ "error": "not_here" }))
                } else if !client {
                    ("401 Unauthorized", serde_json::json!({ "error": "invalid_client", "error_description": "AADSTS7000215: Invalid client secret provided.\r\nTrace ID: 0000" }))
                } else if form.get("grant_type").map(String::as_str) == Some("refresh_token") {
                    ("200 OK", serde_json::json!({ "access_token": token, "token_type": "Bearer", "expires_in": lives.load(Ordering::SeqCst), "refresh_token": format!("rotated-{n}") }))
                } else {
                    ("200 OK", serde_json::json!({ "access_token": token, "token_type": "Bearer", "expires_in": lives.load(Ordering::SeqCst) }))
                };
                let body = json.to_string();
                let _ = out.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        Endpoint { port, asked, life, token }
    }

    /// A form's value: `+` a space, `%XX` its byte.
    fn decoded(s: &str) -> String {
        let s = s.replace('+', " ");
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            match (b[i], s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
                (b'%', Some(byte)) => {
                    out.push(byte);
                    i += 3;
                }
                (c, _) => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    #[test]
    fn a_token_url_says_where_and_how_and_never_shows_its_login() {
        let g = Grant::parse("https://app-id:s3cr%40t@login.microsoftonline.com/acme.example/oauth2/v2.0/token?scope=https%3A%2F%2Foutlook.office365.com%2F.default").unwrap();
        assert_eq!(g.address(), "https://login.microsoftonline.com/acme.example/oauth2/v2.0/token");
        assert_eq!(format!("{g:?}"), g.address());
        assert_eq!((g.client_id.as_str(), g.client_secret.as_str()), ("app-id", "s3cr@t"));
        assert_eq!(g.scope.as_deref(), Some("https://outlook.office365.com/.default"));
        assert!(g.refresh_token.is_none());
        let r = Grant::parse("https://id.apps.googleusercontent.com:GOCSPX-s3cret@oauth2.googleapis.com/token?refresh_token=1%2F%2F0gR3fresh").unwrap();
        assert_eq!(r.refresh_token.as_deref().map(|t| t.as_str()), Some("1//0gR3fresh"));
        assert_eq!(r.address(), "https://oauth2.googleapis.com/token");
        assert!(Grant::parse("http://id:s3cret@127.0.0.1:9000/token?scope=x").is_ok());
        for bad in [
            "https://login.example.com/token?scope=x",
            "https://id@login.example.com/token?scope=x",
            "https://id:s3cret@login.example.com/token",
            "https://id:s3cret@login.example.com/token?scope=x&password=s3cret",
            "http://id:s3cret@login.example.com/token?scope=x",
            "ftp://id:s3cret@login.example.com/token?scope=x",
            "https://id:s3cret@/token?scope=x",
        ] {
            let e = Grant::parse(bad).unwrap_err();
            assert!(!e.contains("s3cret") && !e.contains("login.example.com"), "{bad}: {e}");
        }
    }

    /// A token is asked for once and kept, asked again once it runs out within a minute or a login
    /// refused it; a wrong client is refused, saying why but not its secret; a refresh token an
    /// answer replaces is the one sent next.
    #[test]
    fn a_token_is_kept_until_it_runs_out_and_asked_for_again() {
        let endpoint = stand_in::start("app", "s3cret", "t0ken");
        let url = endpoint.url("app", "s3cret", "scope=mail");
        let tokens = Tokens::default();
        assert_eq!(tokens.token(&url).unwrap().as_str(), "t0ken");
        assert_eq!(tokens.token(&url).unwrap().as_str(), "t0ken");
        assert_eq!(endpoint.asked.lock().unwrap().len(), 1, "kept");
        let first = endpoint.asked.lock().unwrap()[0].clone();
        assert_eq!((first["grant_type"].as_str(), first["scope"].as_str(), first["client_id"].as_str()), ("client_credentials", "mail", "app"));
        tokens.forget(&url);
        tokens.token(&url).unwrap();
        assert_eq!(endpoint.asked.lock().unwrap().len(), 2, "forgotten");
        endpoint.life.store(30, Ordering::SeqCst);
        tokens.forget(&url);
        tokens.token(&url).unwrap();
        tokens.token(&url).unwrap();
        assert_eq!(endpoint.asked.lock().unwrap().len(), 4, "a token that runs out within a minute isn't kept");

        let wrong = endpoint.url("app", "n0pe", "scope=mail");
        match tokens.token(&wrong) {
            Err(Failure::Permanent(why)) => {
                assert!(why.contains("refused the token request (401): invalid_client: AADSTS7000215: Invalid client secret provided."), "{why}");
                assert!(!why.contains("n0pe") && !why.contains("Trace ID"), "{why}");
            }
            other => panic!("{other:?}"),
        }

        let refreshing = endpoint.url("app", "s3cret", "refresh_token=g1ven");
        endpoint.life.store(3600, Ordering::SeqCst);
        tokens.token(&refreshing).unwrap();
        tokens.forget(&refreshing);
        tokens.token(&refreshing).unwrap();
        let asked = endpoint.asked.lock().unwrap();
        let n = asked.len();
        assert_eq!(asked[n - 2]["refresh_token"], "g1ven");
        assert_eq!(asked[n - 1]["refresh_token"], format!("rotated-{}", n - 1), "the one the answer gave");
    }

    #[test]
    fn an_endpoint_that_isnt_there_is_tried_again() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let url = format!("http://app:s3cret@127.0.0.1:{port}/token?scope=mail");
        assert!(matches!(Tokens::default().token(&url), Err(Failure::Temporary(w)) if w.contains("can't be reached") && !w.contains("s3cret")));
    }
}
