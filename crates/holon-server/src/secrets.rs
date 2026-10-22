//! Secrets (§4.2 of docs/PLATFORM_LAYOUT.md): a per-deployment store of named values, such as an
//! API key, that a flow reads with a `secret` node (capability `SecretRead(name)`, in its manifest
//! and granted like any other).
//!
//! - Written over the control plane, never returned by it: only names, versions and where each
//!   comes from are listed.
//! - Kept in a file of their own (`--secrets`), apart from the deployments file, so the flow log,
//!   deployment records and anything copied from them never carry a value. Each value is sealed
//!   under a master key kept elsewhere (sealing.rs: the OS key store by default), and the file is
//!   written atomically, readable only by its owner.
//! - Versioned: setting a secret adds a version and keeps the last [`MAX_VERSIONS`], so a change
//!   can be seen and undone. Values a secret no longer has are still redacted.
//! - Redacted: every value a run logs, and every reply the control plane sends about a
//!   deployment (its state, its sessions, a trigger's or a "send live" answer), has each secret
//!   value replaced by `‹secret:name›`. Only literal occurrences are caught: a value transformed
//!   first (hashed, encoded, sliced) can't be recognised. Values shorter than
//!   [`MIN_SECRET_LEN`] are refused, since they would match ordinary text.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::deploy::epoch_now;
use crate::recorder::LogEntry;
use crate::sealing::{KeySource, Sealer, context, write_private};
use crate::SValue;

/// One deployment's secrets as its runs see them, kept current as they are set.
pub(crate) type Vault = Arc<Mutex<Resolved>>;

/// A deployment's secret values, in the clear, in memory only (wiped when dropped).
#[derive(Default)]
pub(crate) struct Resolved {
    /// name → its current value: what a run granted `SecretRead(name)` reads.
    pub(crate) current: BTreeMap<String, Zeroizing<String>>,
    /// Values its secrets had before (versions still kept): never read, still redacted, so a
    /// client still sending a rotated-out credential doesn't put it in the log.
    pub(crate) retired: Vec<(String, Zeroizing<String>)>,
    /// name → why its current version has no value.
    pub(crate) problems: BTreeMap<String, String>,
}

/// The shortest value accepted: shorter ones would match ordinary text when redacting.
pub(crate) const MIN_SECRET_LEN: usize = 6;

/// How many versions of a secret are kept; older ones are dropped, sealed value and all.
pub(crate) const MAX_VERSIONS: usize = 5;

/// One version of a secret, as the file keeps it: never its value in the clear.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
    version: u32,
    set_at: u64,
    /// Where the value comes from: `local` (sealed here).
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed: Option<String>,
}

/// The secrets file. `key_check` recognises the master key without revealing it.
#[derive(Serialize, Deserialize)]
struct SecretsFile {
    format: u32,
    cipher: String,
    key_check: String,
    deployments: BTreeMap<String, BTreeMap<String, Vec<Stored>>>,
}

const FORMAT: u32 = 2;

#[derive(Default)]
struct Slug {
    /// name → its versions, oldest first; the last is current.
    secrets: BTreeMap<String, Vec<Stored>>,
    vault: Vault,
}

/// What the control plane lists about one secret: never its value.
#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct SecretInfo {
    pub(crate) name: String,
    pub(crate) version: u32,
    pub(crate) source: String,
    pub(crate) set_at: u64,
    /// Why its current version has no value, if it hasn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) problem: Option<String>,
}

/// Every deployment's secrets, by slug. A slug may have secrets before it is first deployed.
pub(crate) struct SecretStore {
    path: String,
    sealer: Sealer,
    slugs: Mutex<BTreeMap<String, Slug>>,
}

/// Whether `name` is a secret name: letters, digits, `-` and `_`, 1 to 64 (as core's `secret`).
pub(crate) fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl SecretStore {
    /// An empty store that is never written (`":memory:"`), under a key that is never kept.
    pub(crate) fn memory() -> SecretStore {
        SecretStore { path: ":memory:".into(), sealer: Sealer::ephemeral(), slugs: Mutex::new(BTreeMap::new()) }
    }

    /// The store at `path`, its master key from `key` (created there when there's nothing to
    /// open yet). A file that exists but doesn't parse, or a key that's missing or doesn't match,
    /// is an error: starting empty would lose every secret at the next write. A file from before
    /// sealing (plain values) is sealed and rewritten.
    pub(crate) fn open(path: &str, key: &KeySource) -> Result<SecretStore, String> {
        if path == ":memory:" {
            return Ok(SecretStore::memory());
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => {
                // One written before files were restricted on every OS is restricted now.
                let _ = crate::sealing::restrict_to_owner(Path::new(path));
                Some(b)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("could not read {path:?}: {e}")),
        };
        let parsed: Option<serde_json::Value> = match &bytes {
            Some(b) => Some(serde_json::from_slice(b).map_err(|e| format!("{path:?} is not a valid secrets file: {e}"))?),
            None => None,
        };
        let Some(parsed) = parsed else {
            let master = match key.read()? {
                Some(k) => k,
                None => key.create()?,
            };
            return Ok(SecretStore { path: path.into(), sealer: Sealer::new(&master), slugs: Mutex::new(BTreeMap::new()) });
        };
        if parsed.get("format").is_none() {
            // Plain values, from before sealing: seal them under a key (kept from now on).
            let plain: BTreeMap<String, BTreeMap<String, String>> =
                serde_json::from_value(parsed).map_err(|e| format!("{path:?} is not a valid secrets file: {e}"))?;
            let master = match key.read()? {
                Some(k) => k,
                None => key.create()?,
            };
            let store = SecretStore { path: path.into(), sealer: Sealer::new(&master), slugs: Mutex::new(BTreeMap::new()) };
            let count: usize = plain.values().map(|names| names.len()).sum();
            for (slug, names) in plain {
                for (name, value) in names {
                    let value = Zeroizing::new(value);
                    store.add_version(&slug, &name, "local", Some(&value), false)?;
                }
            }
            for (slug, entry) in store.slugs.lock().unwrap().iter() {
                store.resolve(slug, entry);
            }
            store.persist()?;
            eprintln!("holon-server: sealed {count} secret(s) in {path:?} under {key}");
            return Ok(store);
        }
        let file: SecretsFile = serde_json::from_value(parsed).map_err(|e| format!("{path:?} is not a valid secrets file: {e}"))?;
        if file.format != FORMAT {
            return Err(format!("{path:?} is secrets format {}, this server reads {FORMAT}", file.format));
        }
        let has_secrets = file.deployments.values().any(|names| !names.is_empty());
        let master = match key.read()? {
            Some(k) => k,
            None if !has_secrets => key.create()?,
            None => {
                return Err(format!(
                    "the key that seals {path:?} isn't in {key}: without it its secrets can't be read (restore the key, or move the file aside to start over)"
                ));
            }
        };
        let sealer = Sealer::new(&master);
        if has_secrets && sealer.check() != file.key_check {
            return Err(format!("{key} holds a different key from the one that sealed {path:?}"));
        }
        let store = SecretStore { path: path.into(), sealer, slugs: Mutex::new(BTreeMap::new()) };
        {
            let mut slugs = store.slugs.lock().unwrap();
            for (slug, secrets) in file.deployments {
                let entry = slugs.entry(slug.clone()).or_default();
                entry.secrets = secrets;
                store.resolve(&slug, entry);
            }
        }
        Ok(store)
    }

    /// `slug`'s vault, created empty if it has none: a deployment holds this, and its runs read it.
    pub(crate) fn vault(&self, slug: &str) -> Vault {
        Arc::clone(&self.slugs.lock().unwrap().entry(slug.to_string()).or_default().vault)
    }

    /// The names set for `slug`, never their values.
    pub(crate) fn names(&self, slug: &str) -> Vec<String> {
        self.slugs.lock().unwrap().get(slug).map(|s| s.secrets.keys().cloned().collect()).unwrap_or_default()
    }

    /// What there is to know about `slug`'s secrets, short of their values.
    pub(crate) fn list(&self, slug: &str) -> Vec<SecretInfo> {
        let slugs = self.slugs.lock().unwrap();
        let Some(s) = slugs.get(slug) else { return Vec::new() };
        let problems = s.vault.lock().unwrap().problems.clone();
        s.secrets
            .iter()
            .filter_map(|(name, versions)| {
                let v = versions.last()?;
                Some(SecretInfo {
                    name: name.clone(),
                    version: v.version,
                    source: v.source.clone(),
                    set_at: v.set_at,
                    problem: problems.get(name).cloned(),
                })
            })
            .collect()
    }

    /// Set `slug`'s secret `name` to `value`, as a new version; that version's number.
    pub(crate) fn set(&self, slug: &str, name: &str, value: &str) -> Result<u32, String> {
        if value.chars().count() < MIN_SECRET_LEN {
            return Err(format!("a secret must be at least {MIN_SECRET_LEN} characters, or it can't be told apart from ordinary text to redact it"));
        }
        let version = self.add_version(slug, name, "local", Some(value), true)?;
        self.persist()?;
        Ok(version)
    }

    /// Add a version of `name` from `source` (`value`: a local one's, to seal), keeping the last
    /// [`MAX_VERSIONS`]; re-resolve the vault when `resolve`.
    fn add_version(&self, slug: &str, name: &str, source: &str, value: Option<&str>, resolve: bool) -> Result<u32, String> {
        if !is_valid_name(name) {
            return Err("a secret name is letters, digits, '-' and '_' (1 to 64)".into());
        }
        let mut slugs = self.slugs.lock().unwrap();
        let entry = slugs.entry(slug.to_string()).or_default();
        let versions = entry.secrets.entry(name.to_string()).or_default();
        let version = versions.last().map_or(1, |v| v.version + 1);
        let (nonce, sealed) = match value {
            Some(value) => {
                let (n, s) = self.sealer.seal(&context(slug, name, version), value);
                (Some(n), Some(s))
            }
            None => (None, None),
        };
        versions.push(Stored { version, set_at: epoch_now(), source: source.into(), nonce, sealed });
        if versions.len() > MAX_VERSIONS {
            versions.drain(..versions.len() - MAX_VERSIONS);
        }
        if resolve {
            self.resolve(slug, entry);
        }
        Ok(version)
    }

    /// Remove `slug`'s secret `name`, every version of it; whether there was one.
    pub(crate) fn remove(&self, slug: &str, name: &str) -> Result<bool, String> {
        let removed = {
            let mut slugs = self.slugs.lock().unwrap();
            let entry = slugs.entry(slug.to_string()).or_default();
            let removed = entry.secrets.remove(name).is_some();
            self.resolve(slug, entry);
            removed
        };
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    /// Forget all of `slug`'s secrets: its deployment was deleted.
    pub(crate) fn forget(&self, slug: &str) -> Result<(), String> {
        let removed = self.slugs.lock().unwrap().remove(slug);
        if let Some(s) = removed {
            *s.vault.lock().unwrap() = Resolved::default();
            self.persist()?;
        }
        Ok(())
    }

    /// One stored version's value, or why there is none.
    fn value_of(&self, slug: &str, name: &str, v: &Stored) -> Result<Zeroizing<String>, String> {
        match v.source.as_str() {
            "local" => match (&v.nonce, &v.sealed) {
                (Some(nonce), Some(sealed)) => self.sealer.open(&context(slug, name, v.version), nonce, sealed),
                _ => Err("no sealed value".into()),
            },
            other => Err(format!("unknown source {other:?}")),
        }
    }

    /// Recompute `slug`'s vault from its versions, in place: the deployments holding it see it.
    fn resolve(&self, slug: &str, entry: &Slug) {
        let mut resolved = Resolved::default();
        for (name, versions) in &entry.secrets {
            let Some((current, older)) = versions.split_last() else { continue };
            match self.value_of(slug, name, current) {
                Ok(value) if value.chars().count() >= MIN_SECRET_LEN => {
                    resolved.current.insert(name.clone(), value);
                }
                Ok(_) => {
                    resolved.problems.insert(name.clone(), format!("shorter than {MIN_SECRET_LEN} characters, too short to redact"));
                }
                Err(e) => {
                    resolved.problems.insert(name.clone(), e);
                }
            }
            for v in older {
                if let Ok(value) = self.value_of(slug, name, v)
                    && value.chars().count() >= MIN_SECRET_LEN
                {
                    resolved.retired.push((name.clone(), value));
                }
            }
        }
        *entry.vault.lock().unwrap() = resolved;
    }

    fn persist(&self) -> Result<(), String> {
        if self.path == ":memory:" {
            return Ok(());
        }
        let file = SecretsFile {
            format: FORMAT,
            cipher: "AES-256-GCM".into(),
            key_check: self.sealer.check().into(),
            deployments: self
                .slugs
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, s)| !s.secrets.is_empty())
                .map(|(slug, s)| (slug.clone(), s.secrets.clone()))
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        write_private(Path::new(&self.path), &bytes).map_err(|e| format!("could not write {:?}: {e}", self.path))
    }
}

/// The name of a secret in `vault` whose value, current or retired, occurs in `v`, if any: a
/// state write carrying one is refused, so a secret never reaches the state store, its
/// snapshots or the disk.
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
        .current
        .iter()
        .chain(vault.retired.iter().map(|(name, value)| (name, value)))
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
        let vault = vault.lock().unwrap();
        let mut pairs: Vec<(String, String)> = vault
            .current
            .iter()
            .chain(vault.retired.iter().map(|(name, value)| (name, value)))
            .map(|(name, value)| (value.to_string(), format!("‹secret:{name}›")))
            .collect();
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

    fn temp(name: &str) -> (String, KeySource) {
        let dir = std::env::temp_dir().join(format!("holon-secrets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join("secrets.json").to_str().unwrap().to_string(), KeySource::File(dir.join("secrets.key")))
    }

    #[test]
    fn a_store_keeps_values_sealed_and_survives_a_restart() {
        let (path, key) = temp("restart");
        let store = SecretStore::open(&path, &key).unwrap();
        assert_eq!(store.set("api", "stripe_key", "sk_live_123456").unwrap(), 1);
        assert!(store.set("api", "bad name", "longenough").is_err());
        assert!(store.set("api", "short", "12345").unwrap_err().contains("at least 6"));
        assert_eq!(store.names("api"), vec!["stripe_key"]);
        let vault = store.vault("api");
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("sk_live"), "sealed on disk: {on_disk}");

        let again = SecretStore::open(&path, &key).unwrap();
        assert_eq!(*again.vault("api").lock().unwrap().current["stripe_key"], "sk_live_123456");
        assert!(store.remove("api", "stripe_key").unwrap());
        assert!(vault.lock().unwrap().current.is_empty(), "a running deployment's vault sees the change");
        store.set("api", "k", "longenough").unwrap();
        store.forget("api").unwrap();
        assert!(SecretStore::open(&path, &key).unwrap().names("api").is_empty());
        std::fs::write(&path, "{not json").unwrap();
        assert!(SecretStore::open(&path, &key).is_err());
    }

    #[test]
    fn a_missing_or_wrong_key_refuses_instead_of_starting_empty() {
        let (path, key) = temp("wrongkey");
        SecretStore::open(&path, &key).unwrap().set("api", "k", "sk_live_123456").unwrap();
        let KeySource::File(key_file) = &key else { unreachable!() };
        let kept = std::fs::read(key_file).unwrap();
        std::fs::remove_file(key_file).unwrap();
        assert!(SecretStore::open(&path, &key).err().unwrap().contains("isn't in"));
        key.create().unwrap();
        assert!(SecretStore::open(&path, &key).err().unwrap().contains("different key"));
        std::fs::write(key_file, kept).unwrap();
        assert_eq!(SecretStore::open(&path, &key).unwrap().names("api"), vec!["k"]);
    }

    #[test]
    fn versions_are_kept_and_old_values_still_redacted() {
        let store = SecretStore::memory();
        for n in 1..=7 {
            assert_eq!(store.set("api", "k", &format!("value-number-{n}")).unwrap(), n);
        }
        let vault = store.vault("api");
        let v = vault.lock().unwrap();
        assert_eq!(*v.current["k"], "value-number-7");
        let retired: Vec<&str> = v.retired.iter().map(|(_, value)| value.as_str()).collect();
        assert_eq!(retired, ["value-number-3", "value-number-4", "value-number-5", "value-number-6"], "the last {MAX_VERSIONS} are kept");
        drop(v);
        assert_eq!(Redactor::of(&vault).text("old value-number-4"), "old ‹secret:k›");
        assert_eq!(store.list("api")[0].version, 7);
    }

    #[test]
    fn a_file_of_plain_values_is_sealed_on_open() {
        let (path, key) = temp("migrate");
        std::fs::write(&path, r#"{"sign-payload": {"signing_key": "sample-signing-key-42"}}"#).unwrap();
        let store = SecretStore::open(&path, &key).unwrap();
        assert_eq!(*store.vault("sign-payload").lock().unwrap().current["signing_key"], "sample-signing-key-42");
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("\"format\": 2") && !on_disk.contains("sample-signing"), "{on_disk}");
        assert_eq!(SecretStore::open(&path, &key).unwrap().list("sign-payload")[0].version, 1);
    }

    #[test]
    fn redaction_replaces_every_literal_occurrence() {
        let vault: Vault = Arc::new(Mutex::new(Resolved {
            current: BTreeMap::from([
                ("key".to_string(), Zeroizing::new("sk_123456".to_string())),
                ("long".to_string(), Zeroizing::new("sk_123456_extended".to_string())),
            ]),
            ..Resolved::default()
        }));
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
