//! Secrets (§4.2 of docs/PLATFORM_LAYOUT.md): a per-deployment store of named values, such as an
//! API key, that a flow reads with a `secret` node (capability `SecretRead(name)`, in its manifest
//! and granted like any other).
//!
//! - Written over the control plane, never returned by it: only names are listed.
//! - Kept in a file of their own (`--secrets`), apart from the deployments file, so the flow log,
//!   deployment records and anything copied from them never carry a value. Written atomically,
//!   readable only by its owner where the OS supports it.
//! - Redacted: every value a run logs, and every reply the control plane sends about a
//!   deployment (its state, its sessions, a trigger's or a "send live" answer), has each secret
//!   value replaced by `‹secret:name›`. Only literal occurrences are caught: a value transformed
//!   first (hashed, encoded, sliced) can't be recognised. Values shorter than
//!   [`MIN_SECRET_LEN`] are refused, since they would match ordinary text.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use holon_host::write_atomically;

use crate::recorder::LogEntry;
use crate::SValue;

/// One deployment's secrets: name → value.
pub(crate) type Vault = Arc<Mutex<BTreeMap<String, String>>>;

/// The shortest value accepted: shorter ones would match ordinary text when redacting.
pub(crate) const MIN_SECRET_LEN: usize = 6;

/// Every deployment's secrets, by slug. A slug may have secrets before it is first deployed.
pub(crate) struct SecretStore {
    path: String,
    slugs: Mutex<BTreeMap<String, Vault>>,
}

/// Whether `name` is a secret name: letters, digits, `-` and `_`, 1 to 64 (as core's `secret`).
pub(crate) fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl SecretStore {
    /// An empty store that is never written (`":memory:"`).
    pub(crate) fn memory() -> SecretStore {
        SecretStore { path: ":memory:".into(), slugs: Mutex::new(BTreeMap::new()) }
    }

    /// The store at `path` (empty when there is no such file, or for `":memory:"`). A file that
    /// exists but doesn't parse is an error: starting empty would lose every secret at the next
    /// write.
    pub(crate) fn load(path: &str) -> Result<SecretStore, String> {
        if path == ":memory:" {
            return Ok(SecretStore::memory());
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SecretStore { path: path.into(), slugs: Mutex::new(BTreeMap::new()) });
            }
            Err(e) => return Err(format!("could not read {path:?}: {e}")),
        };
        let all: BTreeMap<String, BTreeMap<String, String>> =
            serde_json::from_slice(&bytes).map_err(|e| format!("{path:?} is not a valid secrets file: {e}"))?;
        let slugs = all.into_iter().map(|(slug, names)| (slug, Arc::new(Mutex::new(names)))).collect();
        Ok(SecretStore { path: path.into(), slugs: Mutex::new(slugs) })
    }

    /// `slug`'s vault, created empty if it has none: a deployment holds this, and its runs read it.
    pub(crate) fn vault(&self, slug: &str) -> Vault {
        Arc::clone(self.slugs.lock().unwrap().entry(slug.to_string()).or_default())
    }

    /// The names set for `slug`, never their values.
    pub(crate) fn names(&self, slug: &str) -> Vec<String> {
        self.slugs.lock().unwrap().get(slug).map(|v| v.lock().unwrap().keys().cloned().collect()).unwrap_or_default()
    }

    /// Set `slug`'s secret `name` to `value`.
    pub(crate) fn set(&self, slug: &str, name: &str, value: &str) -> Result<(), String> {
        if !is_valid_name(name) {
            return Err("a secret name is letters, digits, '-' and '_' (1 to 64)".into());
        }
        if value.chars().count() < MIN_SECRET_LEN {
            return Err(format!("a secret must be at least {MIN_SECRET_LEN} characters, or it can't be told apart from ordinary text to redact it"));
        }
        self.vault(slug).lock().unwrap().insert(name.to_string(), value.to_string());
        self.persist()
    }

    /// Remove `slug`'s secret `name`; whether there was one.
    pub(crate) fn remove(&self, slug: &str, name: &str) -> Result<bool, String> {
        let removed = self.vault(slug).lock().unwrap().remove(name).is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    /// Forget all of `slug`'s secrets: its deployment was deleted.
    pub(crate) fn forget(&self, slug: &str) -> Result<(), String> {
        if self.slugs.lock().unwrap().remove(slug).is_some() {
            self.persist()?;
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), String> {
        if self.path == ":memory:" {
            return Ok(());
        }
        let all: BTreeMap<String, BTreeMap<String, String>> = self
            .slugs
            .lock()
            .unwrap()
            .iter()
            .map(|(slug, v)| (slug.clone(), v.lock().unwrap().clone()))
            .filter(|(_, names)| !names.is_empty())
            .collect();
        let bytes = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
        write_atomically(Path::new(&self.path), &bytes).map_err(|e| format!("could not write {:?}: {e}", self.path))?;
        restrict_to_owner(&self.path);
        Ok(())
    }
}

#[cfg(unix)]
pub(crate) fn restrict_to_owner(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
pub(crate) fn restrict_to_owner(_path: &str) {}

/// The name of a secret in `vault` whose value occurs in `v`, if any: a state write carrying one
/// is refused, so a secret never reaches the state store, its snapshots or the disk.
pub(crate) fn secret_in(v: &holon_core::Value, vault: &Vault) -> Option<String> {
    use holon_core::Value;
    fn walk(v: &Value, found: &dyn Fn(&str) -> bool) -> bool {
        match v {
            Value::Text(s) => found(s),
            Value::List(xs) => xs.iter().any(|x| walk(x, found)),
            Value::Record(fields) => fields.iter().any(|(k, x)| found(k) || walk(x, found)),
            Value::Effect { payload, .. } | Value::Sum { payload, .. } => walk(payload, found),
            Value::Number(_) | Value::Boolean(_) => false,
        }
    }
    let vault = vault.lock().unwrap();
    vault
        .iter()
        .find(|(_, value)| walk(v, &|s: &str| s.contains(value.as_str())))
        .map(|(name, _)| name.clone())
}

/// Replaces each secret value with `‹secret:name›`.
pub(crate) struct Redactor {
    /// (value, mark), longest value first, so a value containing another is replaced whole.
    pairs: Vec<(String, String)>,
}

impl Redactor {
    pub(crate) fn of(vault: &Vault) -> Redactor {
        let mut pairs: Vec<(String, String)> =
            vault.lock().unwrap().iter().map(|(name, value)| (value.clone(), format!("‹secret:{name}›"))).collect();
        pairs.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        Redactor { pairs }
    }

    /// A run's tokens, as `‹token:name›`: a token is a bearer credential for its waiting point.
    pub(crate) fn tokens(tokens: &BTreeMap<String, String>) -> Redactor {
        let mut pairs: Vec<(String, String)> =
            tokens.iter().map(|(name, token)| (token.clone(), format!("‹token:{name}›"))).collect();
        pairs.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        Redactor { pairs }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    pub(crate) fn text(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (value, mark) in &self.pairs {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), mark);
            }
        }
        out
    }

    pub(crate) fn svalue(&self, v: &mut SValue) {
        match v {
            SValue::Text(s) => *s = self.text(s),
            SValue::List(xs) => xs.iter_mut().for_each(|x| self.svalue(x)),
            SValue::Record(fields) => fields.iter_mut().for_each(|(_, x)| self.svalue(x)),
            SValue::Effect { payload, .. } | SValue::Sum { payload, .. } => self.svalue(payload),
            SValue::Number(_) | SValue::Boolean(_) => {}
        }
    }

    /// Every string in a JSON reply, keys included.
    pub(crate) fn json(&self, v: &mut serde_json::Value) {
        match v {
            serde_json::Value::String(s) => *s = self.text(s),
            serde_json::Value::Array(xs) => xs.iter_mut().for_each(|x| self.json(x)),
            serde_json::Value::Object(map) => {
                let old = std::mem::take(map);
                for (k, mut x) in old {
                    self.json(&mut x);
                    map.insert(self.text(&k), x);
                }
            }
            _ => {}
        }
    }

    /// Everything a log entry carries that a run could have put a secret into.
    pub(crate) fn entry(&self, e: &mut LogEntry) {
        if self.is_empty() {
            return;
        }
        let r = |v: &mut SValue| self.svalue(v);
        e.request.iter_mut().for_each(r);
        e.sourced.iter_mut().for_each(|(_, v)| self.svalue(v));
        e.state_before.values_mut().for_each(|v| self.svalue(v));
        e.outputs.iter_mut().flatten().for_each(|v| self.svalue(v));
        e.performed.iter_mut().for_each(|p| self.svalue(&mut p.payload));
        e.events.values_mut().for_each(|v| self.svalue(v));
        for r in &mut e.reads {
            self.svalue(&mut r.request);
            r.answer.iter_mut().for_each(|v| self.svalue(v));
        }
        for t in &mut e.timeline {
            t.outputs.iter_mut().flatten().flatten().for_each(|v| self.svalue(v));
        }
        for round in &mut e.rounds {
            for x in &mut round.exchanges {
                self.svalue(&mut x.request);
                self.svalue(&mut x.response);
            }
        }
        if let Some(body) = &mut e.response_body {
            *body = self.text(body);
        }
        if let Some(error) = &mut e.error {
            *error = self.text(error);
        }
        if let Some(url) = &mut e.url {
            *url = self.text(url);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_keeps_values_to_itself_and_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("holon-secrets-{}.json", std::process::id()));
        let path = path.to_str().unwrap().to_string();
        let _ = std::fs::remove_file(&path);
        let store = SecretStore::load(&path).unwrap();
        store.set("api", "stripe_key", "sk_live_123456").unwrap();
        assert!(store.set("api", "bad name", "longenough").is_err());
        assert!(store.set("api", "short", "12345").unwrap_err().contains("at least 6"));
        assert_eq!(store.names("api"), vec!["stripe_key"]);
        let vault = store.vault("api");

        let again = SecretStore::load(&path).unwrap();
        assert_eq!(again.vault("api").lock().unwrap()["stripe_key"], "sk_live_123456");
        assert!(store.remove("api", "stripe_key").unwrap());
        assert!(vault.lock().unwrap().is_empty(), "a running deployment's vault sees the change");
        store.set("api", "k", "longenough").unwrap();
        store.forget("api").unwrap();
        assert!(SecretStore::load(&path).unwrap().names("api").is_empty());
        std::fs::write(&path, "{not json").unwrap();
        assert!(SecretStore::load(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn redaction_replaces_every_literal_occurrence() {
        let vault: Vault = Arc::new(Mutex::new(BTreeMap::from([
            ("key".to_string(), "sk_123456".to_string()),
            ("long".to_string(), "sk_123456_extended".to_string()),
        ])));
        let r = Redactor::of(&vault);
        assert_eq!(r.text("Bearer sk_123456"), "Bearer ‹secret:key›");
        assert_eq!(r.text("x sk_123456_extended y"), "x ‹secret:long› y", "the longer value is replaced whole");
        let mut v = SValue::Record(vec![("auth".into(), SValue::List(vec![SValue::Text("sk_123456".into())]))]);
        r.svalue(&mut v);
        assert_eq!(v, SValue::Record(vec![("auth".into(), SValue::List(vec![SValue::Text("‹secret:key›".into())]))]));
        let mut j = serde_json::json!({ "sk_123456": ["token sk_123456"] });
        r.json(&mut j);
        assert_eq!(j, serde_json::json!({ "‹secret:key›": ["token ‹secret:key›"] }));
    }
}
