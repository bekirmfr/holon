//! Where a secret's value comes from when it isn't sealed in the store (§4.2 of
//! docs/PLATFORM_LAYOUT.md): a reference the host resolves, so the value stays in the system
//! that already manages it and the store keeps only where to find it.
//!
//! - `env:HOLON_SECRET_<NAME>`: one of the server's environment variables, or a line of its
//!   `--dotenv` file (for development: the real environment wins, as dotenv tools do). Only
//!   `HOLON_SECRET_…` variables can be named: the server's environment holds other credentials
//!   (a master key, a cloud token, VAULT_TOKEN) that no flow should be able to ask for.
//! - `vault:<mount>/<path>#<field>`: a field of a HashiCorp Vault KV v2 secret, read with
//!   `VAULT_ADDR`, `VAULT_TOKEN` and optionally `VAULT_NAMESPACE`, from the environment or the
//!   dotenv file. Read when set, at startup, and again every [`REFRESH`]; a value that can't be
//!   read is kept for [`STALE_AFTER`] at most, so a revoked one stops working.

use std::collections::BTreeMap;
use std::path::PathBuf;
use crate::sync::Mutex;
use std::time::Duration;

use zeroize::Zeroizing;

/// The prefix of every environment variable a secret can name.
pub(crate) const ENV_PREFIX: &str = "HOLON_SECRET_";

/// How often references are read again.
pub(crate) const REFRESH: Duration = Duration::from_secs(60);

/// How long a value that can no longer be read is kept.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(300);

/// A secret's source, other than `local` (sealed in the store).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reference {
    Env(String),
    Vault { mount: String, path: String, field: String },
}

impl Reference {
    pub(crate) fn parse(source: &str) -> Result<Reference, String> {
        if let Some(var) = source.strip_prefix("env:") {
            if !var.starts_with(ENV_PREFIX) || var.len() == ENV_PREFIX.len() {
                return Err(format!("an env: source names a variable starting {ENV_PREFIX} (e.g. env:{ENV_PREFIX}STRIPE_KEY)"));
            }
            if !var.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') {
                return Err("an environment variable name is upper-case letters, digits and '_'".into());
            }
            return Ok(Reference::Env(var.into()));
        }
        if let Some(rest) = source.strip_prefix("vault:") {
            let (location, field) = rest.split_once('#').ok_or("a vault: source is vault:<mount>/<path>#<field>")?;
            let (mount, path) = location.split_once('/').ok_or("a vault: source is vault:<mount>/<path>#<field>")?;
            let fine = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b)) && !s.contains("..");
            if !fine(mount) || !fine(path) || !fine(field) || mount.contains('/') {
                return Err("a vault: source's mount, path and field are letters, digits, '-', '_', '.' and '/'".into());
            }
            return Ok(Reference::Vault { mount: mount.into(), path: path.into(), field: field.into() });
        }
        Err(format!("unknown source {source:?}: local, env:{ENV_PREFIX}… or vault:<mount>/<path>#<field>"))
    }

    pub(crate) fn is_remote(&self) -> bool {
        matches!(self, Reference::Vault { .. })
    }
}

/// What references are resolved against: the server's environment and its dotenv file.
pub(crate) struct Sources {
    dotenv_path: Option<PathBuf>,
    dotenv: Mutex<BTreeMap<String, Zeroizing<String>>>,
    /// Whether the process environment is read (not in tests, which give their own).
    process_env: bool,
}

impl Sources {
    /// No environment at all: an in-memory store's, and tests'.
    pub(crate) fn none() -> Sources {
        Sources { dotenv_path: None, dotenv: Mutex::new(BTreeMap::new()), process_env: false }
    }

    /// The server's environment, and the dotenv file at `dotenv` if given (a missing file is empty).
    pub(crate) fn new(dotenv: Option<PathBuf>) -> Result<Sources, String> {
        let sources = Sources { dotenv_path: dotenv, dotenv: Mutex::new(BTreeMap::new()), process_env: true };
        sources.reload()?;
        Ok(sources)
    }

    /// Only `vars`, as if they were the environment.
    #[cfg(test)]
    pub(crate) fn of(vars: &[(&str, &str)]) -> Sources {
        let dotenv = vars.iter().map(|(k, v)| (k.to_string(), Zeroizing::new(v.to_string()))).collect();
        Sources { dotenv_path: None, dotenv: Mutex::new(dotenv), process_env: false }
    }

    /// Read the dotenv file again: an edit to it is picked up at the next refresh.
    pub(crate) fn reload(&self) -> Result<(), String> {
        let Some(path) = &self.dotenv_path else { return Ok(()) };
        let text = match std::fs::read_to_string(path) {
            Ok(t) => Zeroizing::new(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Zeroizing::new(String::new()),
            Err(e) => return Err(format!("could not read {path:?}: {e}")),
        };
        *self.dotenv.lock().unwrap() = parse_dotenv(&text);
        Ok(())
    }

    /// The `HOLON_SECRET_…` names an env secret can be read from here, never a value: the
    /// server's environment and its dotenv file, the file read again first so that a line just
    /// added is there. Also the dotenv file's path, if the server was given one.
    pub(crate) fn names(&self) -> (Vec<String>, Option<String>) {
        if let Err(e) = self.reload() {
            eprintln!("holon: {e}");
        }
        let mut names: std::collections::BTreeSet<String> = self.dotenv.lock().unwrap().keys().filter(|k| k.starts_with(ENV_PREFIX)).cloned().collect();
        if self.process_env {
            names.extend(std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()).filter(|k| k.starts_with(ENV_PREFIX)));
        }
        (names.into_iter().collect(), self.dotenv_path.as_ref().map(|p| p.display().to_string()))
    }

    fn var(&self, name: &str) -> Option<Zeroizing<String>> {
        if self.process_env
            && let Ok(v) = std::env::var(name)
        {
            return Some(Zeroizing::new(v));
        }
        self.dotenv.lock().unwrap().get(name).cloned()
    }

    /// `reference`'s value now, or why it can't be read: the dotenv file read again first (the
    /// last read kept if it can't be), so a line just edited is the one a database or a mail
    /// binds or connects with (their sources aren't the secrets', which a refresh reads again).
    pub(crate) fn fetch(&self, reference: &Reference) -> Result<Zeroizing<String>, String> {
        if let Err(e) = self.reload() {
            eprintln!("holon: {e}");
        }
        match reference {
            Reference::Env(var) => self.var(var).ok_or_else(|| format!("{var} isn't set in the server's environment or its dotenv file")),
            Reference::Vault { mount, path, field } => {
                let addr = self.var("VAULT_ADDR").ok_or("VAULT_ADDR isn't set: the server doesn't know where Vault is")?;
                let token = self.var("VAULT_TOKEN").ok_or("VAULT_TOKEN isn't set: the server can't read from Vault")?;
                let url = format!("{}/v1/{mount}/data/{path}", addr.trim_end_matches('/'));
                warn_if_plain(&addr);
                // A Vault that sends the read on elsewhere isn't followed: its token would go along.
                let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).redirects(0).build();
                let mut request = agent.get(&url).set("X-Vault-Token", &token);
                if let Some(ns) = self.var("VAULT_NAMESPACE") {
                    request = request.set("X-Vault-Namespace", &ns);
                }
                let body: serde_json::Value = match request.call() {
                    Ok(r) if (300..400).contains(&r.status()) => {
                        return Err(format!("Vault answered {} for {mount}/{path}, sending the read elsewhere: it isn't followed, as the token would go along", r.status()));
                    }
                    Ok(r) => r
                        .into_string()
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .ok_or_else(|| format!("Vault's answer for {mount}/{path} isn't JSON"))?,
                    Err(ureq::Error::Status(code, _)) => return Err(format!("Vault answered {code} for {mount}/{path}")),
                    Err(e) => return Err(format!("Vault at {} can't be reached: {}", addr.as_str(), crate::outbound::said(&e))),
                };
                match &body["data"]["data"][field.as_str()] {
                    serde_json::Value::String(s) => Ok(Zeroizing::new(s.clone())),
                    serde_json::Value::Null => Err(format!("{mount}/{path} has no field {field:?}")),
                    _ => Err(format!("{mount}/{path}#{field} isn't text")),
                }
            }
        }
    }
}

/// `KEY=value` lines: blank lines and `#` comments skipped, an `export ` prefix allowed, a value
/// in matching single or double quotes taken as is, an unquoted one up to a ` #` comment.
pub(crate) fn parse_dotenv(text: &str) -> BTreeMap<String, Zeroizing<String>> {
    let mut vars = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        let value = match value.as_bytes().first() {
            Some(q @ (b'"' | b'\'')) if value.len() >= 2 && value.as_bytes()[value.len() - 1] == *q => &value[1..value.len() - 1],
            _ => value.split(" #").next().unwrap_or("").trim_end(),
        };
        vars.insert(key.trim().to_string(), Zeroizing::new(value.to_string()));
    }
    vars
}

/// Say once, on the server's output, [`plain_vault_warning`].
fn warn_if_plain(addr: &str) {
    static SAID: std::sync::Once = std::sync::Once::new();
    if let Some(warning) = plain_vault_warning(addr) {
        SAID.call_once(|| eprintln!("{warning}"));
    }
}

/// What the server says when `VAULT_ADDR` is plain `http://` (in any case, as a URL's scheme is)
/// to another machine: its token and every secret read go in the clear. Vault's dev server is
/// plain `http://`, so it is said, not refused. None for `https://` or this machine. A name and
/// password before an `@` in the address are never said.
fn plain_vault_warning(addr: &str) -> Option<String> {
    let rest = addr.get(..7).filter(|scheme| scheme.eq_ignore_ascii_case("http://")).map(|_| &addr[7..])?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host).to_ascii_lowercase();
    (!crate::oauth::is_loopback(&host)).then(|| format!("holon: VAULT_ADDR is plain http:// to {host}: its token and the secrets read from it go in the clear (https:// keeps them)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Guards:** a `VAULT_ADDR` that is plain `http://` to another machine is said, naming the
    /// host and nothing before an `@`; one to this machine, or `https://`, is not.
    /// **Why:** Vault's token and every secret read from it cross the network in the clear over
    /// `http://`. The warning was an `eprintln` nothing tested, and it missed `HTTP://`.
    /// **If it fails:** the row named is said where it shouldn't be, or not where it should, or
    /// the warning says what an address holds before its `@` (a name and a password).
    #[test]
    fn a_plain_http_vault_on_another_machine_is_said_and_no_other_is() {
        for (addr, said) in [
            ("http://127.0.0.1:8200", None),
            ("http://localhost:8200/", None),
            ("http://LOCALHOST:8200", None),
            ("http://[::1]:8200", None),
            ("https://vault.example.com", None),
            ("HTTPS://vault.example.com:8200", None),
            ("vault.example.com:8200", None),
            ("http://vault.example.com:8200", Some("vault.example.com:8200")),
            ("HTTP://Vault.Example.com/v1", Some("vault.example.com")),
            ("http://10.0.0.5:8200?x", Some("10.0.0.5:8200")),
            ("http://ops:s3cret@vault.example.com", Some("vault.example.com")),
        ] {
            let warning = plain_vault_warning(addr);
            assert_eq!(warning.is_some(), said.is_some(), "{addr}: {warning:?}");
            if let (Some(warning), Some(host)) = (&warning, said) {
                assert!(warning.contains(&format!("plain http:// to {host}:")), "{addr}: {warning}");
                assert!(!warning.contains("s3cret") && !warning.contains("ops"), "{addr}: {warning}");
            }
        }
    }

    #[test]
    fn references_parse_and_only_holon_variables_can_be_named() {
        assert_eq!(Reference::parse("env:HOLON_SECRET_STRIPE").unwrap(), Reference::Env("HOLON_SECRET_STRIPE".into()));
        for refused in ["env:VAULT_TOKEN", "env:HOLON_MASTER_KEY", "env:HOLON_SECRET_", "env:holon_secret_x", "vault:secret", "vault:a/b", "vault:a/../b#f", "aws:x"] {
            assert!(Reference::parse(refused).is_err(), "{refused}");
        }
        assert_eq!(
            Reference::parse("vault:secret/payments/stripe#signing").unwrap(),
            Reference::Vault { mount: "secret".into(), path: "payments/stripe".into(), field: "signing".into() }
        );
    }

    #[test]
    fn a_dotenv_file_reads_like_the_usual_tools() {
        let vars = parse_dotenv("# comment\n\nexport HOLON_SECRET_A=plain value # note\nB=\"quoted # kept\"\nC='single'\nbroken line\n");
        let get = |k: &str| vars.get(k).map(|v| v.to_string());
        assert_eq!(get("HOLON_SECRET_A").as_deref(), Some("plain value"));
        assert_eq!(get("B").as_deref(), Some("quoted # kept"));
        assert_eq!(get("C").as_deref(), Some("single"));
        assert_eq!(vars.len(), 3);
    }

    #[test]
    fn the_names_an_env_secret_can_use_are_listed_never_their_values() {
        let path = std::env::temp_dir().join(format!("holon-names-{}.env", std::process::id()));
        std::fs::write(&path, "HOLON_SECRET_GEMINI_API_KEY=AIza-value\nVAULT_TOKEN=t0ken\n").unwrap();
        let sources = Sources { dotenv_path: Some(path.clone()), dotenv: Mutex::new(BTreeMap::new()), process_env: false };
        let (names, file) = sources.names();
        assert_eq!(names, ["HOLON_SECRET_GEMINI_API_KEY"], "only the names a secret can use");
        assert_eq!(file.as_deref(), Some(path.display().to_string().as_str()));
        // A line just added is listed, and readable, without waiting for the next refresh.
        std::fs::write(&path, "HOLON_SECRET_GEMINI_API_KEY=AIza-value\nHOLON_SECRET_SLACK_URL=https://hooks\n").unwrap();
        assert_eq!(sources.names().0, ["HOLON_SECRET_GEMINI_API_KEY", "HOLON_SECRET_SLACK_URL"]);
        assert_eq!(*sources.fetch(&Reference::parse("env:HOLON_SECRET_SLACK_URL").unwrap()).unwrap(), "https://hooks");
        // A value edited is the one fetched next, with no refresh or listing between.
        std::fs::write(&path, "HOLON_SECRET_SLACK_URL=https://hooks/edited\n").unwrap();
        assert_eq!(*sources.fetch(&Reference::parse("env:HOLON_SECRET_SLACK_URL").unwrap()).unwrap(), "https://hooks/edited");
        let _ = std::fs::remove_file(&path);
        assert_eq!(Sources::none().names(), (Vec::new(), None));
    }

    #[test]
    fn a_vault_field_is_read_with_the_token() {
        let server = rouille::Server::new("127.0.0.1:0", |request| {
            if request.header("X-Vault-Token") != Some("t0ken") {
                return rouille::Response::text("denied").with_status_code(403);
            }
            match request.url().as_str() {
                "/v1/secret/data/payments" => rouille::Response::json(&serde_json::json!({ "data": { "data": { "signing": "whsec_from_vault" } } })),
                _ => rouille::Response::empty_404(),
            }
        })
        .unwrap();
        let addr = format!("http://{}", server.server_addr());
        let (_handle, stop) = server.stoppable();
        let sources = Sources::of(&[("VAULT_ADDR", &addr), ("VAULT_TOKEN", "t0ken")]);
        let get = |s: &str| sources.fetch(&Reference::parse(s).unwrap());
        assert_eq!(*get("vault:secret/payments#signing").unwrap(), "whsec_from_vault");
        assert!(get("vault:secret/payments#other").unwrap_err().contains("no field"));
        assert!(get("vault:secret/missing#f").unwrap_err().contains("404"));
        let wrong = Sources::of(&[("VAULT_ADDR", &addr), ("VAULT_TOKEN", "nope")]);
        assert!(wrong.fetch(&Reference::parse("vault:secret/payments#signing").unwrap()).unwrap_err().contains("403"));
        assert!(Sources::none().fetch(&Reference::parse("vault:secret/payments#signing").unwrap()).unwrap_err().contains("VAULT_ADDR"));
        let _ = stop.send(());
    }
}
