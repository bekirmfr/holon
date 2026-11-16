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
//! - Sealed here, or a reference to where the value is kept (secret_sources.rs: an environment
//!   variable, a dotenv file in development, HashiCorp Vault), resolved by the host.
//! - Versioned: setting a secret adds a version and keeps the last [`MAX_VERSIONS`], so a change
//!   can be seen and rolled back. Values a secret no longer has are still redacted.
//! - Audited: every change, and a source that stops or starts reading, is appended to an audit
//!   log beside the store (`<secrets>.audit.jsonl`, owner-only): what, when, which version and
//!   source, never a value.
//! - Redacted: every value a run logs, and every reply the control plane sends about a
//!   deployment (its state, its sessions, a trigger's or a "send live" answer), has each secret
//!   value replaced by `‹secret:name v3 #a1b2c3d4›`: its name, the version the value is of, and
//!   the value's fingerprint (keyed by a key derived from the secrets data key, at_rest.rs), so
//!   which value a run had can be told (an old key, the one just set) without the log holding it. Each run's entry also
//!   records the versions it was given (`secrets`). Only literal occurrences are caught: a value transformed
//!   first (hashed, encoded, sliced) can't be recognised. Values shorter than
//!   [`MIN_SECRET_LEN`] are refused, since they would match ordinary text.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::sync::Mutex;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::deploy::epoch_now;
use crate::recorder::LogEntry;
use crate::at_rest::{self, Keyring};

use crate::secret_sources::{Reference, STALE_AFTER, Sources};
use crate::SValue;

/// One deployment's secrets as its runs see them, kept current as they are set.
pub(crate) type Vault = Arc<Mutex<Resolved>>;

/// A deployment's secret values, in the clear, in memory only (wiped when dropped).
#[derive(Default)]
pub(crate) struct Resolved {
    /// name → its current value: what a run granted `SecretRead(name)` reads.
    pub(crate) current: BTreeMap<String, Zeroizing<String>>,
    /// Values its secrets had before (versions still kept), each with its name and how the log
    /// tells it apart: never read, still redacted, so a client still sending a rotated-out
    /// credential doesn't put it in the log.
    pub(crate) retired: Vec<(String, String, Zeroizing<String>)>,
    /// name → how the log tells its current value apart: the version it is of and its
    /// fingerprint (`v3 #a1b2c3d4`, [`mark`]).
    pub(crate) shown: BTreeMap<String, String>,
    /// name → the salt of its current value: 32 random bytes, drawn when the value was first
    /// seen and sealed in the store (`SecretStore::salt_of`). What a run's record will commit
    /// its secrets with, and key the marks of what it computes from them
    /// (docs/engineering/capabilities/secrets.md, "A salt for each value"). Not there for a value whose
    /// kept salt doesn't open.
    pub(crate) salts: BTreeMap<String, Zeroizing<[u8; 32]>>,
    /// name → what a run given its current value records of it (`recorder::Committed`): where
    /// it is kept, and the commitment to it under its salt. There where its salt is.
    pub(crate) committed: BTreeMap<String, crate::recorder::Committed>,
    /// name → why its current version has no value, or why the value it has may be stale.
    pub(crate) problems: BTreeMap<String, String>,
    /// The names whose current value is the server's ([`SERVER`]: the deployment has none of its
    /// own), each with where the server's may go, when an admin said (`holon secret bind --to`).
    pub(crate) server: BTreeMap<String, Option<BTreeSet<String>>>,
    /// name → the version its current value is of, and when it was last read from its source.
    fetched: BTreeMap<String, (u32, std::time::Instant)>,
    /// The keys that seal what the deployment keeps on disk that may hold these values (a
    /// waiting run's requests, under its own scope, `at_rest::deployment`), and fingerprint a
    /// value the log shows by a mark: the server's keyring; a store kept in memory has one that
    /// is never written, so what it sealed can't be opened after a restart, as its secrets aren't
    /// there after one. Set whenever the vault is (`vault`, `resolve`): a waiting run is never
    /// written to disk unsealed. None only for a vault no store made.
    pub(crate) keys: Option<Arc<Keyring>>,
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
    /// Where the value comes from: `local` (sealed here, in `sealed`), or a reference resolved
    /// by the host (`env:…`, `vault:…`).
    source: String,
    /// A local value, sealed under the keyring's `secrets` scope for [`context`] (a sealed line,
    /// `a1.<version>.…`): it opens only for its deployment, name and version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed: Option<String>,
}

/// The secrets file: its values sealed under the keyring (`keys.json` beside it), never a key.
#[derive(Serialize, Deserialize)]
struct SecretsFile {
    format: u32,
    deployments: BTreeMap<String, BTreeMap<String, Vec<Stored>>>,
    /// Where each of the server's secrets may go, as an admin bound it: hosts (`api.stripe.com`)
    /// and databases (`db:shop`). One not listed may go wherever its flows' approvals show.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    caps: BTreeMap<String, BTreeSet<String>>,
    /// The salt of each value a secret has had, by deployment ([`SaltOf`]). A section of its
    /// own, and not a field of a version: a reference's value changes under one version, and a
    /// salt outlives the version it was drawn for. A file from before it has none, and each is
    /// drawn as its value is next read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    salts: BTreeMap<String, Vec<SaltOf>>,
}

/// One value's salt, as the file keeps it: whose it is (the secret's name and version, and the
/// value's fingerprint, since a reference has more than one value under a version), and the 32
/// bytes sealed under the keyring's `secrets` scope for [`salt_context`].
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SaltOf {
    name: String,
    version: u32,
    print: String,
    sealed: String,
}

/// A salt in the store: the deployment, the secret's name and version, the value's fingerprint.
type SaltKey = (String, String, u32, String);

/// Where the server's own secrets are kept in the store, beside each deployment's: one bound for
/// every deployment that reads it (`holon secret bind stripe_key`), as a folder or a database is,
/// a deployment's own of that name overriding it. Not a slug, so no deployment is called it.
pub(crate) const SERVER: &str = "*";

const FORMAT: u32 = 4;

/// What a sealed value is bound to: a value sealed for one place doesn't open in another.
fn context(slug: &str, name: &str, version: u32) -> String {
    format!("holon-secret/v3\0{slug}\0{name}\0{version}")
}

/// The commitment to `value`, version `version` of `owner`'s secret `name`, under its `salt`:
/// SHA-256, in hex, of a domain, the owner (a deployment's slug, or [`SERVER`]), the name, the
/// version, the salt and the value, each with its length before it, so no two fields read as
/// one. Whoever holds the value and the salt computes it again and knows the value is the one a
/// run recorded; without the salt (32 random bytes) it says nothing of the value, however weak.
pub(crate) fn commitment(owner: &str, name: &str, version: u32, salt: &[u8; 32], value: &str) -> String {
    hashed("sha256", vec![fields(&[b"holon-secret-commitment/v1", owner.as_bytes(), name.as_bytes(), &version.to_le_bytes(), salt.as_slice(), value.as_bytes()])])
}

/// `of`, each field with its length before it, so no two fields read as one.
fn fields(of: &[&[u8]]) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::new());
    for field in of {
        out.extend_from_slice(&(field.len() as u64).to_le_bytes());
        out.extend_from_slice(field);
    }
    out
}

/// Core's `leaf` (`sha256` of one input, `hmac_sha256` of a key and a message), in hex. Core's
/// leaves take values: the bytes are moved into them, and wiped once hashed.
fn hashed(leaf: &str, inputs: Vec<Zeroizing<Vec<u8>>>) -> String {
    let mut given: Vec<holon_core::Value> = inputs.into_iter().map(|mut bytes| holon_core::Value::Bytes(std::mem::take(&mut *bytes))).collect();
    let hashed = holon_core::primitives::compute(leaf, &given.iter().collect::<Vec<_>>(), &[holon_core::Param::frozen("encoding", "hex")]);
    for value in &mut given {
        if let holon_core::Value::Bytes(bytes) = value {
            zeroize::Zeroize::zeroize(bytes);
        }
    }
    match hashed {
        Some(holon_core::Value::Text(hex)) => hex,
        _ => unreachable!("a hash of bytes, in hex, is a text"),
    }
}

/// How the mark of a value a run computed from a secret begins: `‹derived #7c1e9a02›`.
pub(crate) const DERIVED_MARK: &str = "‹derived #";

/// The key a run's marks for what it computed from its secrets are made under
/// ([`derived_hash`]): SHA-256 of a domain and, in name order, the name and the salt of each
/// secret it was given. None for a run given none with a salt: it marks nothing.
pub(crate) fn derived_key<'a>(salts: impl IntoIterator<Item = (&'a str, &'a [u8; 32])>) -> Option<Zeroizing<Vec<u8>>> {
    let mut of: Vec<&[u8]> = vec![b"holon-derived-key/v1"];
    for (name, salt) in salts {
        of.push(name.as_bytes());
        of.push(salt.as_slice());
    }
    if of.len() == 1 {
        return None;
    }
    let hex = Zeroizing::new(hashed("sha256", vec![fields(&of)]));
    holon_core::text_bytes(&hex, "hex").map(Zeroizing::new)
}

/// The hash of `value`, a text the run `run` (its id) computed from a secret: HMAC-SHA-256, in
/// hex, under the run's key ([`derived_key`]), of a domain, the run's id, the value's kind and
/// the value, each with its length before it. Its first eight digits are the value's mark. Keyed,
/// so a weak value can't be guessed from it; bound to the run, so one value has one mark through
/// a run and another in the next.
pub(crate) fn derived_hash(key: &[u8], run: &str, value: &str) -> String {
    hashed("hmac_sha256", vec![Zeroizing::new(key.to_vec()), fields(&[b"holon-derived/v1", run.as_bytes(), b"text", value.as_bytes()])])
}

/// What the run `run` computed from its secrets, as its record marks it: each text of
/// [`MIN_SECRET_LEN`] characters or more that a leaf making something of a secret gave
/// (`SecretFed::makes`, read off `timeline`), with its hash under `key`.
///
/// And whether that is all of it (`whole`): every node that may compute from a secret was
/// followed and observed (`kept`: the run kept a timeline; none deeper than it records, none
/// inside a function or a module), and each such leaf gave a text a mark can stand for. One
/// that gave a true or false, a number, bytes, a list, a short text or nothing leaves the run
/// not whole: no mark stands for those, and what is computed from a secret is then withheld
/// whole, as it is for a run with no key.
pub(crate) fn born(timeline: &[crate::recorder::NodeTiming], fed: &crate::taint::SecretFed, key: Option<&[u8]>, run: &str, kept: bool) -> crate::recorder::Born {
    let Some(key) = key.filter(|_| !fed.is_empty()) else { return Default::default() };
    let mut whole = kept && fed.followed_to(crate::recorder::Timer::DEPTH);
    let mut made: BTreeSet<&str> = BTreeSet::new();
    for node in timeline.iter().filter(|n| fed.makes(&n.path)) {
        let Some(outputs) = &node.outputs else {
            whole = false;
            continue;
        };
        for (port, output) in outputs.iter().enumerate() {
            if !fed.output(&node.path, port).is_derived() {
                continue;
            }
            let Some(SValue::Text(text)) = output else {
                whole = false;
                continue;
            };
            if text.chars().count() >= MIN_SECRET_LEN {
                made.insert(text);
                // Found by its value, and as JSON text writes it once: one that JSON escapes (a
                // quote, a backslash, a control character) reads another way again written out
                // twice, where nothing would find it.
                whole &= serde_json::to_string(text.as_str()).is_ok_and(|quoted| quoted.len() == text.len() + 2);
            } else {
                whole = false;
            }
        }
    }
    let mut values: Vec<(String, String)> = made.into_iter().map(|value| (derived_hash(key, run, value), value.to_string())).collect();
    values.sort();
    // Two values under one mark would read as one.
    whole &= values.windows(2).all(|pair| pair[0].0[..8] != pair[1].0[..8]);
    crate::recorder::Born { values, whole }
}

/// What a run given `value`, the version `v` of `owner`'s secret `name`, records of it.
fn committed(owner: &str, name: &str, v: &Stored, salt: &[u8; 32], value: &str) -> crate::recorder::Committed {
    crate::recorder::Committed {
        kind: if v.source == "local" { "local" } else { "reference" }.into(),
        commitment: commitment(owner, name, v.version, salt, value),
        server: owner == SERVER,
    }
}

/// What a sealed salt is bound to: the value it is of, by its fingerprint, beside what the value
/// is bound to. A salt moved to another value's place doesn't open.
fn salt_context(slug: &str, name: &str, version: u32, print: &str) -> String {
    format!("holon-secret-salt/v1\0{slug}\0{name}\0{version}\0{print}")
}

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
    /// Whether it is the server's ([`SERVER`]), read by a deployment that has none of its own.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) server: bool,
    /// Where the server's may go, when an admin said: hosts and databases (`db:shop`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) to: Option<BTreeSet<String>>,
}

/// One version of a secret, as its history lists it.
#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct VersionInfo {
    pub(crate) version: u32,
    pub(crate) source: String,
    pub(crate) set_at: u64,
    pub(crate) current: bool,
}

/// What rolling a secret back would do, as a dry run says it: the version made current again, the
/// one current now, the new version it would be, and its source. Never a value: each value's
/// fingerprint instead, as the log marks it (`#a1b2c3d4`), so the two can be told apart; the
/// current one's is absent when it can't be read now.
#[derive(Serialize, Debug, PartialEq)]
pub(crate) struct RollbackPreview {
    pub(crate) to: u32,
    pub(crate) current: u32,
    pub(crate) version: u32,
    pub(crate) source: String,
    pub(crate) to_print: String,
    pub(crate) current_print: Option<String>,
}

/// One entry of the audit log: never a value.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct SecretEvent {
    pub(crate) at: u64,
    pub(crate) slug: String,
    /// The secret's name, or `*` for all of a deployment's.
    pub(crate) name: String,
    /// `set`, `rolled back`, `removed`, `forgotten` (its deployment was deleted), `sealed` (a
    /// plain value sealed on open), `unreadable` or `readable again` (its source).
    pub(crate) action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    /// Who did it: a named user, `admin` or `secrets` (the shared tokens); absent when no one
    /// is known (an open loopback server, or the host itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) by: Option<String>,
}

/// The audit log: appended to a file, or kept in memory for an in-memory store.
struct Audit {
    /// The file, and how its lines are sealed: each under the secrets' keys (at_rest.rs, §5.2).
    path: Option<(PathBuf, at_rest::LineSeal)>,
    kept: Mutex<Vec<SecretEvent>>,
}

impl Audit {
    fn record(&self, event: SecretEvent) {
        let Some((path, seal)) = &self.path else {
            self.kept.lock().unwrap().push(event);
            return;
        };
        let Ok(line) = serde_json::to_string(&event) else { return };
        let appended = seal.append(path, &line);
        if let Err(e) = appended {
            eprintln!("holon: could not append to the secrets audit log {path:?}: {e}");
        }
    }

    /// The last `limit` events for `slug`'s `name` (and its deployment-wide ones), oldest first.
    fn read(&self, slug: &str, name: &str, limit: usize) -> Vec<SecretEvent> {
        let all: Vec<SecretEvent> = match &self.path {
            None => self.kept.lock().unwrap().clone(),
            Some((path, seal)) => seal.read_lines(path).iter().filter_map(|l| serde_json::from_str(l).ok()).collect(),
        };
        let mine: Vec<SecretEvent> = all.into_iter().filter(|e| e.slug == slug && (e.name == name || e.name == "*")).collect();
        mine[mine.len().saturating_sub(limit)..].to_vec()
    }
}

/// Every deployment's secrets, by slug. A slug may have secrets before it is first deployed.
pub(crate) struct SecretStore {
    path: String,
    keys: Arc<Keyring>,
    sources: Sources,
    audit: Audit,
    slugs: Mutex<BTreeMap<String, Slug>>,
    /// Where each of the server's secrets may go ([`SecretsFile::caps`]). Locked after `slugs`.
    caps: Mutex<BTreeMap<String, BTreeSet<String>>>,
    /// Each value's salt, sealed ([`SecretsFile::salts`]). Locked after `slugs` and `caps`.
    salts: Mutex<BTreeMap<SaltKey, String>>,
    /// Whether a salt was drawn that the file doesn't hold yet: one is drawn where a value is
    /// read (`resolve`), under `slugs`, and the file is written once that is let go
    /// (`keep_salts`).
    unsaved: std::sync::atomic::AtomicBool,
}

/// Whether `d` is where a secret may be said to go: a host, with a port if it isn't 443
/// (`api.stripe.com`, `localhost:8443`), or a database, `db:` and its name (`db:shop`).
pub(crate) fn is_destination(d: &str) -> bool {
    match d.strip_prefix("db:") {
        Some(database) => is_valid_name(database),
        None => !d.is_empty() && d.len() <= 253 && d.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b)),
    }
}

/// Whether `name` is a secret name: letters, digits, `-` and `_`, 1 to 64 (as core's `secret`).
pub(crate) fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `name`, as a secret's name, or why not.
fn check_name(name: &str) -> Result<(), String> {
    if is_valid_name(name) { Ok(()) } else { Err("a secret name is letters, digits, '-' and '_' (1 to 64)".into()) }
}

impl SecretStore {
    /// An empty store that is never written (`":memory:"`), under a key that is never kept.
    pub(crate) fn memory() -> SecretStore {
        SecretStore {
            path: ":memory:".into(),
            keys: Arc::new(Keyring::memory()),
            sources: Sources::none(),
            audit: Audit { path: None, kept: Mutex::new(Vec::new()) },
            slugs: Mutex::new(BTreeMap::new()),
            caps: Mutex::new(BTreeMap::new()),
            salts: Mutex::new(BTreeMap::new()),
            unsaved: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The `HOLON_SECRET_…` names an env secret can be read from, and the dotenv file read, never
    /// a value (`Sources::names`).
    pub(crate) fn env_names(&self) -> (Vec<String>, Option<String>) {
        self.sources.names()
    }

    /// This store, resolving references against `sources`.
    #[cfg(test)]
    pub(crate) fn with_sources(mut self, sources: Sources) -> SecretStore {
        self.sources = sources;
        self
    }

    /// The store at `path`, its values sealed under `keys`. A file that exists but doesn't
    /// parse is an error: starting empty would lose every secret at the next write.
    pub(crate) fn open(path: &str, keys: Arc<Keyring>, sources: Sources) -> Result<SecretStore, String> {
        if path == ":memory:" {
            return Ok(SecretStore { keys, ..SecretStore::memory() });
        }
        let seal = at_rest::LineSeal::with(Arc::clone(&keys), at_rest::SECRETS, at_rest::role::SECRETS_AUDIT);
        let audit = Audit { path: Some((Path::new(path).with_extension("audit.jsonl"), seal)), kept: Mutex::new(Vec::new()) };
        let store = |caps, salts| SecretStore {
            path: path.into(),
            keys: Arc::clone(&keys),
            sources,
            audit,
            slugs: Mutex::new(BTreeMap::new()),
            caps: Mutex::new(caps),
            salts: Mutex::new(salts),
            unsaved: std::sync::atomic::AtomicBool::new(false),
        };
        // Sealed whole under the server's keys (which secrets each deployment has, their sources),
        // each local value in it under the secrets scope.
        let Some(bytes) = at_rest::read(&keys, Path::new(path), at_rest::role::SECRETS)? else { return Ok(store(BTreeMap::new(), BTreeMap::new())) };
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("{path:?} is not a valid secrets file: {e}"))?;
        let format = parsed.get("format").and_then(serde_json::Value::as_u64);
        if format != Some(u64::from(FORMAT)) {
            return Err(format!("{path:?} is secrets format {}, this server reads {FORMAT}", format.map_or("(none)".into(), |f| f.to_string())));
        }
        let file: SecretsFile = serde_json::from_value(parsed).map_err(|e| format!("{path:?} is not a valid secrets file: {e}"))?;
        let salts = file.salts.into_iter().flat_map(|(slug, salts)| salts.into_iter().map(move |s| ((slug.clone(), s.name, s.version, s.print), s.sealed))).collect();
        let store = store(file.caps, salts);
        {
            let mut slugs = store.slugs.lock().unwrap();
            for (slug, secrets) in file.deployments {
                slugs.entry(slug).or_default().secrets = secrets;
            }
            store.resolve_all(&slugs);
        }
        // A value read for the first time (a file from before salts, a reference's new value)
        // had its salt drawn: it is kept before a run is given it.
        store.keep_salts();
        Ok(store)
    }

    /// `slug`'s vault, created empty if it has none: a deployment holds this, and its runs read it.
    pub(crate) fn vault(&self, slug: &str) -> Vault {
        let vault = {
            let mut slugs = self.slugs.lock().unwrap();
            // A deployment new here reads the server's secrets until it has its own.
            if !slugs.contains_key(slug) {
                slugs.insert(slug.to_string(), Slug::default());
                self.resolve_after(&slugs, slug);
            }
            Arc::clone(&slugs[slug].vault)
        };
        self.keep_salts();
        {
            let mut v = vault.lock().unwrap();
            v.keys.get_or_insert_with(|| Arc::clone(&self.keys));
        }
        vault
    }

    /// The names set for `slug`, its own and the server's it reads, never their values.
    pub(crate) fn names(&self, slug: &str) -> Vec<String> {
        self.list(slug).into_iter().map(|s| s.name).collect()
    }

    /// What there is to know about `slug`'s secrets, short of their values: its own, and the
    /// server's it reads for want of its own (for [`SERVER`], the server's, with where each may go).
    pub(crate) fn list(&self, slug: &str) -> Vec<SecretInfo> {
        let slugs = self.slugs.lock().unwrap();
        let caps = self.caps.lock().unwrap();
        let empty = Slug::default();
        let own = slugs.get(slug).unwrap_or(&empty);
        let server = slugs.get(SERVER).filter(|_| slug != SERVER).unwrap_or(&empty);
        let problems = own.vault.lock().unwrap().problems.clone();
        let server_problems = server.vault.lock().unwrap().problems.clone();
        let mut listed: Vec<SecretInfo> = own
            .secrets
            .iter()
            .map(|(name, versions)| (name, versions, false))
            .chain(server.secrets.iter().filter(|(name, _)| !own.secrets.contains_key(*name)).map(|(name, versions)| (name, versions, true)))
            .filter_map(|(name, versions, theirs)| {
                let v = versions.last()?;
                let shared = theirs || slug == SERVER;
                Some(SecretInfo {
                    name: name.clone(),
                    version: v.version,
                    source: v.source.clone(),
                    set_at: v.set_at,
                    problem: if theirs { &server_problems } else { &problems }.get(name).cloned(),
                    server: theirs,
                    to: caps.get(name).filter(|_| shared).cloned(),
                })
            })
            .collect();
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        listed
    }

    /// Say where the server's secret `name` may go (hosts, `db:` databases), or, with `None`,
    /// that it may go wherever its flows' approvals show. A deploy that sends it elsewhere is
    /// refused ([`SecretStore::refuses`]), and so is a request a run makes with its value.
    pub(crate) fn set_cap(&self, name: &str, to: Option<BTreeSet<String>>, by: Option<&str>) -> Result<(), String> {
        let slugs = self.slugs.lock().unwrap();
        if !slugs.get(SERVER).is_some_and(|s| s.secrets.contains_key(name)) {
            return Err(format!("the server has no secret {name:?}: bind it first"));
        }
        if let Some(bad) = to.iter().flatten().find(|d| !is_destination(d)) {
            return Err(format!("{bad:?} isn't where a secret goes: a host (api.stripe.com, with :port if not 443) or a database (db:shop)"));
        }
        let detail = match &to {
            Some(to) if to.is_empty() => "it may go nowhere".to_string(),
            Some(to) => format!("it may go only to {}", to.iter().map(|d| holon_author::describe::describe_destination(d)).collect::<Vec<_>>().join(", ")),
            None => "it may go wherever its flows' approvals show".to_string(),
        };
        match to {
            Some(to) => self.caps.lock().unwrap().insert(name.to_string(), to),
            None => self.caps.lock().unwrap().remove(name),
        };
        self.resolve_after(&slugs, SERVER);
        drop(slugs);
        self.persist()?;
        self.event(SERVER, name, "capped", None, None, Some(detail), by);
        Ok(())
    }

    /// Why deploying a flow to `slug` that sends its secrets where `sent` says (core's
    /// `sent_secrets`) can't be granted, if it can't: it would send a secret it reads from the
    /// server (having none of its own) somewhere the admin didn't say the server's may go.
    pub(crate) fn refuses(&self, slug: &str, sent: &BTreeMap<String, BTreeSet<String>>) -> Option<String> {
        let slugs = self.slugs.lock().unwrap();
        let caps = self.caps.lock().unwrap();
        for (name, to) in sent {
            if slugs.get(slug).is_some_and(|s| s.secrets.contains_key(name)) || !slugs.get(SERVER).is_some_and(|s| s.secrets.contains_key(name)) {
                continue;
            }
            let Some(cap) = caps.get(name) else { continue };
            let outside: BTreeSet<String> = to.iter().filter(|d| holon_core::is_own_destination(d) && !cap.contains(*d)).cloned().collect();
            if !outside.is_empty() {
                let (may, sends) = (holon_author::describe::describe_sent_to(cap).own, holon_author::describe::describe_sent_to(&outside).own);
                return Some(format!(
                    "the server's secret {name:?} may go only to {}, and this flow sends it to {} (set {slug:?}'s own {name}, or ask an admin to let the server's go there)",
                    may.unwrap_or_else(|| "nowhere".into()),
                    sends.unwrap_or_default()
                ));
            }
        }
        None
    }

    /// Set `slug`'s secret `name` to `value`, as a new version; that version's number.
    pub(crate) fn set(&self, slug: &str, name: &str, value: &str, by: Option<&str>) -> Result<u32, String> {
        check_name(name)?;
        if value.chars().count() < MIN_SECRET_LEN {
            return Err(format!("a secret must be at least {MIN_SECRET_LEN} characters, or it can't be told apart from ordinary text to redact it"));
        }
        let version = self.add_version(slug, name, "local", Some(value), true)?;
        self.persist()?;
        self.event(slug, name, "set", Some(version), Some("local"), None, by);
        Ok(version)
    }

    /// Point `slug`'s secret `name` at `source` (`env:…`, `vault:…`), as a new version; that
    /// version's number. Refused unless it can be read now, and long enough to redact.
    pub(crate) fn set_from(&self, slug: &str, name: &str, source: &str, by: Option<&str>) -> Result<u32, String> {
        // The name first: a source is read only for a secret that can be kept, so what reading it
        // says (unset, too short) is never told for nothing.
        check_name(name)?;
        let value = self.sources.fetch(&Reference::parse(source)?)?;
        if value.chars().count() < MIN_SECRET_LEN {
            return Err(format!("{source} is shorter than {MIN_SECRET_LEN} characters, too short to redact"));
        }
        let version = self.add_version(slug, name, source, None, true)?;
        self.persist()?;
        self.event(slug, name, "set", Some(version), Some(source), None, by);
        Ok(version)
    }

    /// Make version `to` of `slug`'s `name` current again, as a new version (history is kept);
    /// that version's number. Refused unless `to` is kept, isn't current, and still reads.
    pub(crate) fn rollback(&self, slug: &str, name: &str, to: u32, by: Option<&str>) -> Result<u32, String> {
        let (old, _) = self.rollback_from(slug, name, to)?;
        let value = self.value_of(slug, name, &old).map_err(|e| format!("version {to} can't be read: {e}"))?;
        let sealed = (old.source == "local").then_some(value);
        let version = self.add_version(slug, name, &old.source, sealed.as_deref().map(|v| v.as_str()), true)?;
        self.persist()?;
        self.event(slug, name, "rolled back", Some(version), Some(&old.source), Some(format!("to version {to}")), by);
        Ok(version)
    }

    /// What `rollback` would do, changing nothing: refused as it would be (`to` not kept, already
    /// current, or not readable now).
    pub(crate) fn rollback_preview(&self, slug: &str, name: &str, to: u32) -> Result<RollbackPreview, String> {
        let (old, current) = self.rollback_from(slug, name, to)?;
        let value = self.value_of(slug, name, &old).map_err(|e| format!("version {to} can't be read: {e}"))?;
        let current_print = current.as_ref().and_then(|c| self.value_of(slug, name, c).ok()).map(|v| self.keys.fingerprint(&v));
        let current = current.map_or(0, |c| c.version);
        Ok(RollbackPreview { to, current, version: current + 1, source: old.source, to_print: self.keys.fingerprint(&value), current_print })
    }

    /// The kept version `to` of `slug`'s `name`, and the one current now; refused when `to` isn't
    /// kept or is the current one.
    fn rollback_from(&self, slug: &str, name: &str, to: u32) -> Result<(Stored, Option<Stored>), String> {
        let slugs = self.slugs.lock().unwrap();
        let versions = slugs.get(slug).and_then(|s| s.secrets.get(name)).ok_or_else(|| format!("{slug:?} has no secret {name:?}"))?;
        let current = versions.last().cloned();
        if current.as_ref().is_some_and(|c| c.version == to) {
            return Err(format!("version {to} is already current"));
        }
        let old = versions.iter().find(|v| v.version == to).cloned().ok_or_else(|| format!("version {to} isn't kept (the last {MAX_VERSIONS} are)"))?;
        Ok((old, current))
    }

    /// `slug`'s secret `name`: its kept versions, newest first, and its last audit events.
    pub(crate) fn history(&self, slug: &str, name: &str) -> Option<(Vec<VersionInfo>, Vec<SecretEvent>)> {
        let versions: Vec<VersionInfo> = {
            let slugs = self.slugs.lock().unwrap();
            let versions = slugs.get(slug).and_then(|s| s.secrets.get(name));
            let last = versions.and_then(|v| v.last()).map(|v| v.version);
            versions
                .into_iter()
                .flatten()
                .rev()
                .map(|v| VersionInfo { version: v.version, source: v.source.clone(), set_at: v.set_at, current: Some(v.version) == last })
                .collect()
        };
        let events = self.audit.read(slug, name, 50);
        (!versions.is_empty() || !events.is_empty()).then_some((versions, events))
    }

    /// `by`: who asked for it (access.rs's caller), when someone did.
    #[allow(clippy::too_many_arguments)]
    fn event(&self, slug: &str, name: &str, action: &str, version: Option<u32>, source: Option<&str>, detail: Option<String>, by: Option<&str>) {
        self.audit.record(SecretEvent {
            at: epoch_now(),
            slug: slug.into(),
            name: name.into(),
            action: action.into(),
            version,
            source: source.map(String::from),
            detail,
            by: by.map(String::from),
        });
    }

    /// Read every reference again (and the dotenv file): a value changed or revoked where it is
    /// kept reaches the runs within a refresh.
    pub(crate) fn refresh(&self) {
        if let Err(e) = self.sources.reload() {
            eprintln!("holon: {e}");
        }
        let slugs = self.slugs.lock().unwrap();
        let remote = |entry: &Slug| entry.secrets.values().flatten().any(|v| v.source != "local");
        let server = slugs.get(SERVER);
        let server_remote = server.is_some_and(remote);
        for (slug, entry) in slugs.iter() {
            if remote(entry) || server_remote {
                self.resolve(slug, entry, server.filter(|_| slug != SERVER));
            }
        }
        drop(slugs);
        // A reference whose value changed where it is kept has a salt of its own.
        self.keep_salts();
    }

    /// Recompute the vaults a change to `slug`'s secrets reaches: its own, and for the server's
    /// ([`SERVER`]), every deployment's too.
    fn resolve_after(&self, slugs: &BTreeMap<String, Slug>, slug: &str) {
        let server = slugs.get(SERVER);
        for (s, entry) in slugs.iter().filter(|(s, _)| slug == SERVER || s.as_str() == slug) {
            self.resolve(s, entry, server.filter(|_| s != SERVER));
        }
    }

    /// Recompute every vault.
    fn resolve_all(&self, slugs: &BTreeMap<String, Slug>) {
        self.resolve_after(slugs, SERVER);
    }

    /// Add a version of `name` from `source` (`value`: a local one's, to seal), keeping the last
    /// [`MAX_VERSIONS`]; re-resolve the vault when `resolve`.
    fn add_version(&self, slug: &str, name: &str, source: &str, value: Option<&str>, resolve: bool) -> Result<u32, String> {
        check_name(name)?;
        let mut slugs = self.slugs.lock().unwrap();
        let entry = slugs.entry(slug.to_string()).or_default();
        let versions = entry.secrets.entry(name.to_string()).or_default();
        let version = versions.last().map_or(1, |v| v.version + 1);
        let sealed = match value {
            Some(value) => Some(self.keys.seal_line(at_rest::SECRETS, &context(slug, name, version), value.as_bytes())?),
            None => None,
        };
        versions.push(Stored { version, set_at: epoch_now(), source: source.into(), sealed });
        if versions.len() > MAX_VERSIONS {
            versions.drain(..versions.len() - MAX_VERSIONS);
        }
        if resolve {
            self.resolve_after(&slugs, slug);
        }
        Ok(version)
    }

    /// Remove `slug`'s secret `name`, every version of it; whether there was one.
    pub(crate) fn remove(&self, slug: &str, name: &str, by: Option<&str>) -> Result<bool, String> {
        let removed = {
            let mut slugs = self.slugs.lock().unwrap();
            let removed = slugs.entry(slug.to_string()).or_default().secrets.remove(name).is_some();
            if slug == SERVER {
                self.caps.lock().unwrap().remove(name);
            }
            self.resolve_after(&slugs, slug);
            removed
        };
        if removed {
            self.persist()?;
            self.event(slug, name, "removed", None, None, None, by);
        }
        Ok(removed)
    }

    /// Forget all of `slug`'s secrets: its deployment was deleted.
    pub(crate) fn forget(&self, slug: &str, by: Option<&str>) -> Result<(), String> {
        let removed = self.slugs.lock().unwrap().remove(slug);
        // Its salts with it: nothing of a deleted deployment is kept. (A secret removed, or a
        // version let go of, keeps its salt: a run that named it is still held to its value.)
        self.salts.lock().unwrap().retain(|(of, ..), _| of != slug);
        if let Some(s) = removed {
            *s.vault.lock().unwrap() = Resolved::default();
            self.persist()?;
            self.event(slug, "*", "forgotten", None, None, Some("its deployment was deleted".into()), by);
        }
        Ok(())
    }

    /// One stored version's value, or why there is none.
    fn value_of(&self, slug: &str, name: &str, v: &Stored) -> Result<Zeroizing<String>, String> {
        match v.source.as_str() {
            "local" => match &v.sealed {
                Some(sealed) => {
                    let plain = self.keys.open_line(at_rest::SECRETS, &context(slug, name, v.version), sealed).map_err(|e| e.to_string())?;
                    String::from_utf8(plain.to_vec()).map(Zeroizing::new).map_err(|_| "not text".into())
                }
                None => Err("no sealed value".into()),
            },
            other => self.sources.fetch(&Reference::parse(other)?),
        }
    }

    /// Recompute `slug`'s vault from its versions, in place: the deployments holding it see it.
    /// A value from a remote source that can't be read now is kept, for [`STALE_AFTER`] at most.
    /// `server`: the server's secrets ([`SERVER`]), read where the deployment has none of its own.
    fn resolve(&self, slug: &str, entry: &Slug, server: Option<&Slug>) {
        // Its keys too: a vault rebuilt without them (a secret set, a source read again) left a
        // waiting run to be written to disk unsealed.
        let mut resolved = Resolved { keys: Some(Arc::clone(&self.keys)), ..Resolved::default() };
        let mut before = std::mem::take(&mut *entry.vault.lock().unwrap());
        let now = std::time::Instant::now();
        let caps = self.caps.lock().unwrap();
        let theirs = server.into_iter().flat_map(|s| &s.secrets).filter(|(name, _)| !entry.secrets.contains_key(*name)).map(|(name, versions)| (SERVER, name, versions));
        for (owner, name, versions) in entry.secrets.iter().map(|(name, versions)| (slug, name, versions)).chain(theirs) {
            let Some((current, older)) = versions.split_last() else { continue };
            if owner == SERVER && slug != SERVER {
                resolved.server.insert(name.clone(), caps.get(name).cloned());
            }
            match self.value_of(owner, name, current) {
                Ok(value) if value.chars().count() >= MIN_SECRET_LEN => {
                    resolved.shown.insert(name.clone(), self.shown(current.version, &value));
                    if let Some(salt) = self.salt_of(owner, name, current.version, &value) {
                        resolved.committed.insert(name.clone(), committed(owner, name, current, &salt, &value));
                        resolved.salts.insert(name.clone(), salt);
                    }
                    resolved.current.insert(name.clone(), value);
                    resolved.fetched.insert(name.clone(), (current.version, now));
                }
                Ok(_) => {
                    resolved.problems.insert(name.clone(), format!("shorter than {MIN_SECRET_LEN} characters, too short to redact"));
                }
                Err(e) => match (before.fetched.get(name).copied(), before.current.remove(name)) {
                    (Some((version, at)), Some(value)) if version == current.version && now.duration_since(at) < STALE_AFTER => {
                        let ago = now.duration_since(at).as_secs();
                        resolved.problems.insert(name.clone(), format!("{e}; using the value read {ago}s ago"));
                        resolved.shown.insert(name.clone(), self.shown(version, &value));
                        if let Some(salt) = self.salt_of(owner, name, version, &value) {
                            resolved.committed.insert(name.clone(), committed(owner, name, current, &salt, &value));
                            resolved.salts.insert(name.clone(), salt);
                        }
                        resolved.current.insert(name.clone(), value);
                        resolved.fetched.insert(name.clone(), (version, at));
                    }
                    _ => {
                        resolved.problems.insert(name.clone(), e);
                    }
                },
            }
            // Older versions are read again only from here or the environment, never fetched.
            for v in older.iter().filter(|v| !Reference::parse(&v.source).is_ok_and(|r| r.is_remote())) {
                if let Ok(value) = self.value_of(owner, name, v)
                    && value.chars().count() >= MIN_SECRET_LEN
                {
                    resolved.retired.push((name.clone(), self.shown(v.version, &value), value));
                }
            }
        }
        drop(caps);
        // A source that stops or starts reading is audited once, not at every refresh (the
        // server's, once, where it is kept).
        for (name, problem) in &resolved.problems {
            if !before.problems.contains_key(name) && entry.secrets.contains_key(name) {
                let source = entry.secrets.get(name).and_then(|v| v.last()).map(|v| v.source.as_str());
                self.event(slug, name, "unreadable", None, source, Some(problem.clone()), None);
            }
        }
        for name in before.problems.keys().filter(|n| !resolved.problems.contains_key(*n) && resolved.current.contains_key(*n)) {
            self.event(slug, name, "readable again", None, None, None, None);
        }
        *entry.vault.lock().unwrap() = resolved;
    }

    /// The salt of `value`, version `version` of `owner`'s secret `name`: drawn the first time
    /// the value is read, and sealed in the store from then on, so it is the same after a
    /// restart, and for a reference's value that comes back after another. A reference's value
    /// changes under one version, so a salt is a value's, told by its fingerprint. None when the
    /// one kept doesn't open (a damaged file, another keyring): another is never drawn in its
    /// place, which would give one value two salts.
    fn salt_of(&self, owner: &str, name: &str, version: u32, value: &str) -> Option<Zeroizing<[u8; 32]>> {
        let print = self.keys.fingerprint(value);
        let role = salt_context(owner, name, version, &print);
        let mut salts = self.salts.lock().unwrap();
        let mut salt = Zeroizing::new([0u8; 32]);
        match salts.entry((owner.to_string(), name.to_string(), version, print)) {
            std::collections::btree_map::Entry::Occupied(kept) => {
                let plain = self.keys.open_line(at_rest::SECRETS, &role, kept.get()).ok()?;
                if plain.len() != salt.len() {
                    return None;
                }
                salt.copy_from_slice(&plain);
            }
            std::collections::btree_map::Entry::Vacant(new) => {
                getrandom::fill(salt.as_mut_slice()).expect("the OS random source failed");
                new.insert(self.keys.seal_line(at_rest::SECRETS, &role, salt.as_slice()).ok()?);
                self.unsaved.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        Some(salt)
    }

    /// Write the file if a salt was drawn that it doesn't hold. Called with `slugs` let go.
    fn keep_salts(&self) {
        if self.unsaved.load(std::sync::atomic::Ordering::SeqCst)
            && let Err(e) = self.persist()
        {
            eprintln!("holon: a secret's salt isn't kept yet: {e}");
            self.unsaved.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// How the log tells `value`, version `version` of a secret, apart: `v3 #a1b2c3d4`.
    fn shown(&self, version: u32, value: &str) -> String {
        format!("v{version} #{}", self.keys.fingerprint(value))
    }

    fn persist(&self) -> Result<(), String> {
        if self.path == ":memory:" {
            return Ok(());
        }
        // Whatever was drawn is in what is written now; one drawn while it is written is said
        // again.
        self.unsaved.store(false, std::sync::atomic::Ordering::SeqCst);
        let mut salts: BTreeMap<String, Vec<SaltOf>> = BTreeMap::new();
        let file = SecretsFile {
            format: FORMAT,
            deployments: self
                .slugs
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, s)| !s.secrets.is_empty())
                .map(|(slug, s)| (slug.clone(), s.secrets.clone()))
                .collect(),
            caps: self.caps.lock().unwrap().clone(),
            salts: {
                for ((slug, name, version, print), sealed) in self.salts.lock().unwrap().iter() {
                    salts.entry(slug.clone()).or_default().push(SaltOf { name: name.clone(), version: *version, print: print.clone(), sealed: sealed.clone() });
                }
                salts
            },
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        at_rest::write_private(&self.keys, Path::new(&self.path), at_rest::SERVER, at_rest::role::SECRETS, &bytes).map_err(|e| format!("could not write {:?}: {e}", self.path))
    }
}

/// The name of a secret in `vault` whose value, current or retired, occurs in `v`, if any: a
/// state write carrying one is refused, so a secret never reaches the state store, its
/// snapshots or the disk.
pub(crate) fn secret_in(v: &holon_core::Value, vault: &Vault) -> Option<String> {
    secrets_in(v, vault).into_iter().next()
}

/// The names of every secret in `vault` whose value, current or retired, occurs in `v`, in the
/// order [`secret_in`] finds the first.
pub(crate) fn secrets_in(v: &holon_core::Value, vault: &Vault) -> Vec<String> {
    let vault = vault.lock().unwrap();
    let mut found: Vec<String> = vec![];
    for (name, value) in vault.current.iter().chain(vault.retired.iter().map(|(name, _, value)| (name, value))) {
        if !found.contains(name) && value_holds(v, &|s: &str| s.contains(value.as_str())) {
            found.push(name.clone());
        }
    }
    found
}

/// Whether any text in `v`, a record's keys included, satisfies `found`.
pub(crate) fn value_holds(v: &holon_core::Value, found: &dyn Fn(&str) -> bool) -> bool {
    use holon_core::Value;
    match v {
        Value::Text(s) => found(s),
        Value::List(xs) => xs.iter().any(|x| value_holds(x, found)),
        Value::Record(fields) => fields.iter().any(|(k, x)| found(k) || value_holds(x, found)),
        Value::Effect { payload, .. } | Value::Sum { payload, .. } => value_holds(payload, found),
        Value::Bytes(b) => bytes_hold(b, found),
        Value::Number(_) | Value::Boolean(_) => false,
    }
}

/// Whether a run of valid UTF-8 in `b` satisfies `found`. A secret's UTF-8 inside bytes lies
/// within one such run (a text never starts with a continuation byte, nor ends inside a
/// character), so bytes holding a secret's UTF-8 hold the secret, as a text holding it does.
pub(crate) fn bytes_hold(b: &[u8], found: &dyn Fn(&str) -> bool) -> bool {
    b.utf8_chunks().any(|run| found(run.valid()))
}

/// Each run of valid UTF-8 in `b`, in place, through `f`, every other byte kept as it was: what
/// [`texts_mut`] replaces in a text, this replaces in bytes ([`bytes_hold`] says why it finds it).
pub(crate) fn bytes_texts_mut(b: &mut Vec<u8>, f: &mut impl FnMut(&mut String)) {
    let mut out = Vec::with_capacity(b.len());
    for run in b.utf8_chunks() {
        let mut text = run.valid().to_string();
        f(&mut text);
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(run.invalid());
    }
    *b = out;
}

/// The fields of a call's answer whose values are credentials, as an API names them (an OAuth
/// token exchange answers with these): a value under one is kept out of the log by its value.
const ANSWER_FIELDS: &[&str] = &["access_token", "refresh_token", "id_token", "session_token", "token", "client_secret", "api_key", "secret", "password"];

/// The credentials in the answers to the calls that sent a secret, as (`answer field`, value):
/// each value of a field in [`ANSWER_FIELDS`] in a JSON answer, to a request that held a secret's
/// value or was computed from one. Such a value is kept out of the log by its value, as a
/// secret is, and written nowhere, as a secret isn't: a token a flow gets with a secret is as
/// good as the secret until it expires.
pub(crate) fn answered_tokens(rounds: &[holon_host::Round], vault: &Vault, fed: &crate::taint::SecretFed) -> Vec<(String, String)> {
    use holon_core::Value;
    fn walk(v: &serde_json::Value, out: &mut Vec<(String, String)>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, x) in map {
                    match x {
                        serde_json::Value::String(s) if ANSWER_FIELDS.contains(&k.as_str()) && s.chars().count() >= MIN_SECRET_LEN => {
                            out.push((format!("answer {k}"), s.clone()))
                        }
                        _ => walk(x, out),
                    }
                }
            }
            serde_json::Value::Array(xs) => xs.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for x in rounds.iter().flat_map(|r| &r.exchanges) {
        let sent_a_secret = secret_in(&x.request, vault).is_some() || (0..3).any(|port| fed.input(&x.path, port) > crate::taint::Taint::Clean);
        let Value::Record(answer) = &x.response else { continue };
        let Some(Value::Text(body)) = answer.get("body") else { continue };
        if let (true, Ok(json)) = (sent_a_secret, serde_json::from_str::<serde_json::Value>(body)) {
            walk(&json, &mut out);
        }
    }
    // One that is a secret's own value is marked as that secret.
    out.retain(|(_, value)| secret_in(&Value::Text(value.clone()), vault).is_none());
    out.sort();
    out.dedup();
    out
}

/// The mark the log writes for a secret's value: `‹secret:name v3 #a1b2c3d4›`, or `‹secret:name›`
/// for one with no version to show.
pub(crate) fn mark(name: &str, shown: Option<&str>) -> String {
    match shown {
        Some(shown) => format!("‹secret:{name} {shown}›"),
        None => format!("‹secret:{name}›"),
    }
}

/// Replaces each secret value with its mark ([`mark`]).
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
            .map(|(name, value)| (mark(name, vault.shown.get(name).map(String::as_str)), value))
            .chain(vault.retired.iter().map(|(name, shown, value)| (mark(name, Some(shown)), value)))
            .flat_map(|(mark, value)| {
                // As it is, and as it reads inside JSON text (a request's body, a value written
                // out with `json_stringify`), where a quote, a backslash or a newline in it is
                // escaped and the value as it is no longer appears.
                let escaped = serde_json::to_string(value.as_str()).ok().map(|q| q[1..q.len() - 1].to_string()).filter(|e| e != value.as_str());
                std::iter::once((value.to_string(), mark.clone())).chain(escaped.map(|e| (e, mark)))
            })
            .collect();
        pairs.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        Redactor { pairs }
    }

    /// The credentials a run's calls answered ([`answered_tokens`]), as `‹token:answer field #a1b2c3d4›`:
    /// the fingerprint tells which one a run had, as a secret's does.
    pub(crate) fn answers(tokens: &[(String, String)], vault: &Vault) -> Redactor {
        let prints = vault.lock().unwrap().keys.clone();
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (name, value) in tokens {
            let mark = match &prints {
                Some(prints) => format!("‹token:{name} #{}›", prints.fingerprint(value)),
                None => format!("‹token:{name}›"),
            };
            let escaped = serde_json::to_string(value.as_str()).ok().map(|q| q[1..q.len() - 1].to_string()).filter(|e| e != value.as_str());
            pairs.extend(std::iter::once((value.clone(), mark.clone())).chain(escaped.map(|e| (e, mark))));
        }
        pairs.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        Redactor { pairs }
    }

    /// What a run computed from its secrets ([`born`]), each as `‹derived #7c1e9a02›`, the start
    /// of its hash: found by its value wherever the run's record holds it, as a secret is, and
    /// as it reads inside JSON text.
    pub(crate) fn born(born: &crate::recorder::Born) -> Redactor {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (hash, value) in &born.values {
            let mark = format!("{DERIVED_MARK}{}›", &hash[..8]);
            let escaped = serde_json::to_string(value.as_str()).ok().map(|q| q[1..q.len() - 1].to_string()).filter(|e| e != value);
            pairs.extend(std::iter::once((value.clone(), mark.clone())).chain(escaped.map(|e| (e, mark))));
        }
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

    /// A run's tainted values (`data::tainted`), each as `‹taint:key›`, the key it was found at;
    /// as it reads inside JSON text too (a body, a value written out with `json_stringify`), and a
    /// number that is one as it is written.
    pub(crate) fn tainted(values: &[(String, String)]) -> Redactor {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (key, value) in values {
            let mark = format!("{TAINT_MARK}{key}›");
            let escaped = serde_json::to_string(value.as_str()).ok().map(|q| q[1..q.len() - 1].to_string()).filter(|e| e != value);
            pairs.extend(std::iter::once((value.clone(), mark.clone())).chain(escaped.map(|e| (e, mark))));
        }
        pairs.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        Redactor { pairs }
    }

    /// The tokens a run's starts were handed (`calls::handed`), each as `‹token:name›`, as the
    /// run's own are: the same name may come from several starts.
    pub(crate) fn handed(handed: &[(String, String)]) -> Redactor {
        let mut pairs: Vec<(String, String)> = handed.iter().map(|(name, token)| (token.clone(), format!("‹token:{name}›"))).collect();
        pairs.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        Redactor { pairs }
    }

    /// This and `other` together: an entry is redacted once, so a mark one of them writes is
    /// never read by the other as a run's own text.
    pub(crate) fn and(mut self, other: Redactor) -> Redactor {
        self.pairs.extend(other.pairs);
        self.pairs.sort_by_key(|(value, _)| std::cmp::Reverse(value.len()));
        self
    }

    /// Whether it has nothing to replace.
    pub(crate) fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Whether `s` holds any of its values.
    pub(crate) fn holds(&self, s: &str) -> bool {
        self.pairs.iter().any(|(value, _)| !value.is_empty() && s.contains(value.as_str()))
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
            SValue::Record(fields) => {
                for (k, x) in fields.iter_mut() {
                    *k = self.text(k);
                    self.svalue(x);
                }
                fields.sort_by(|a, b| a.0.cmp(&b.0));
            }
            SValue::Effect { payload, .. } | SValue::Sum { payload, .. } => self.svalue(payload),
            SValue::Bytes(b) => bytes_texts_mut(b, &mut |s: &mut String| *s = self.text(s)),
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

    /// `s` as the log writes it: each value replaced by its mark (the leftmost first, the longest
    /// where two start together), and a look-alike of a mark in the run's own text escaped
    /// ([`write_marked`]). Whether one was.
    fn logged(&self, s: &str) -> Option<(String, bool)> {
        // Nothing to mark and nothing that reads as a mark: the text stays as it is, uncopied
        // (interpreter-path-L4.md, step 2).
        if !s.contains('‹') && self.pairs.iter().all(|(value, _)| value.is_empty() || !s.contains(value.as_str())) {
            return None;
        }
        // JSON holding a tainted number as a number (a body's `"number": 4111…`): its mark, a
        // text, goes where the number was, and the text stays JSON, so what reads it reads the rest.
        let t = s.trim_start();
        if (t.starts_with('{') || t.starts_with('['))
            && self.pairs.iter().any(|(value, mark)| mark.starts_with(TAINT_MARK) && value.parse::<f64>().is_ok() && s.contains(value.as_str()))
            && let Ok(mut json) = serde_json::from_str::<serde_json::Value>(t)
        {
            let mut escaped = false;
            self.logged_json(&mut json, &mut escaped);
            return Some((json.to_string(), escaped));
        }
        let mut next: Vec<Option<usize>> = self.pairs.iter().map(|(value, _)| (!value.is_empty()).then(|| s.find(value.as_str())).flatten()).collect();
        let mut pieces = Vec::new();
        let mut at = 0;
        loop {
            for (k, (value, _)) in self.pairs.iter().enumerate() {
                if next[k].is_some_and(|p| p < at) {
                    next[k] = s[at..].find(value.as_str()).map(|p| p + at);
                }
            }
            // Pairs are longest first, so the first found at the leftmost place is the longest.
            let Some((k, p)) = next.iter().enumerate().filter_map(|(k, p)| p.map(|p| (k, p))).min_by_key(|&(_, p)| p) else { break };
            pieces.push((&s[at..p], false));
            pieces.push((self.pairs[k].1.as_str(), true));
            at = p + self.pairs[k].0.len();
        }
        pieces.push((&s[at..], false));
        Some(write_marked(&pieces))
    }

    /// `json` as the log writes it: each text as [`Redactor::logged`] writes it, its keys too, and
    /// each number that is a tainted value as its mark.
    fn logged_json(&self, json: &mut serde_json::Value, escaped: &mut bool) {
        let text = |s: &str, escaped: &mut bool| match self.logged(s) {
            Some((out, e)) => {
                *escaped |= e;
                out
            }
            None => s.to_string(),
        };
        match json {
            serde_json::Value::String(s) => *s = text(s, escaped),
            serde_json::Value::Number(n) => {
                let written = n.to_string();
                if let Some((_, mark)) = self.pairs.iter().find(|(value, mark)| mark.starts_with(TAINT_MARK) && *value == written) {
                    *json = serde_json::Value::String(mark.clone());
                }
            }
            serde_json::Value::Array(xs) => xs.iter_mut().for_each(|x| self.logged_json(x, escaped)),
            serde_json::Value::Object(map) => {
                for (k, mut x) in std::mem::take(map) {
                    self.logged_json(&mut x, escaped);
                    map.insert(text(&k, escaped), x);
                }
            }
            serde_json::Value::Bool(_) | serde_json::Value::Null => {}
        }
    }

    fn logged_svalue(&self, v: &mut SValue) -> bool {
        let mut escaped = false;
        texts_mut(v, &mut |s| {
            if let Some((out, e)) = self.logged(s) {
                *s = out;
                escaped |= e;
            }
        });
        if self.pairs.iter().any(|(_, mark)| mark.starts_with(TAINT_MARK)) {
            self.tainted_numbers(v);
        }
        escaped
    }

    /// Each number in `v` that is a tainted value as it is written (a card's number in a JSON
    /// body), as its mark: a text, where the number was.
    fn tainted_numbers(&self, v: &mut SValue) {
        match v {
            SValue::Number(n) => {
                let written = format!("{n}");
                if let Some((_, mark)) = self.pairs.iter().find(|(value, mark)| mark.starts_with(TAINT_MARK) && *value == written) {
                    *v = SValue::Text(mark.clone());
                }
            }
            SValue::List(xs) => xs.iter_mut().for_each(|x| self.tainted_numbers(x)),
            SValue::Record(fields) => fields.iter_mut().for_each(|(_, x)| self.tainted_numbers(x)),
            SValue::Effect { payload, .. } | SValue::Sum { payload, .. } => self.tainted_numbers(payload),
            SValue::Text(_) | SValue::Boolean(_) | SValue::Bytes(_) => {}
        }
    }

    /// Everything a log entry carries that a run could have put a secret into, as the log writes
    /// it: with each value marked, and the run's own look-alikes of a mark escaped. Applied once,
    /// where the entry is logged, with every value the entry is redacted of (`and`).
    pub(crate) fn entry(&self, e: &mut LogEntry) {
        let mut escaped = false;
        e.values_mut(&mut |v: &mut SValue| escaped |= self.logged_svalue(v));
        for read in &mut e.reads {
            // A part of a file is kept as read, proved by its hashes and the bytes around it to
            // whole chunks: one whose text or bytes around hold a secret isn't kept at all (a mark
            // would no longer prove), so verify asks for the file instead.
            if let Some(file) = read.file.as_mut()
                && let Some(range) = file.range.as_mut()
            {
                let around = |hex: &str| crate::ranges::unhex(hex).unwrap_or_default();
                let bytes = [around(&range.before), range.text.clone().unwrap_or_default().into_bytes(), around(&range.after)].concat();
                if bytes_hold(&bytes, &|s: &str| self.holds(s)) {
                    range.text = None;
                    range.before.clear();
                    range.after.clear();
                    range.proof.clear();
                    file.blob = None;
                }
            }
        }
        for s in e.texts_mut() {
            if let Some((out, e)) = self.logged(s) {
                *s = out;
                escaped |= e;
            }
        }
        e.escaped |= escaped;
    }
}

/// Each text in `v`, in place, a record's keys included (a secret can be a key: a map from API
/// keys to accounts), each record kept in key order after.
pub(crate) fn texts_mut(v: &mut SValue, f: &mut impl FnMut(&mut String)) {
    match v {
        SValue::Text(s) => f(s),
        SValue::List(xs) => xs.iter_mut().for_each(|x| texts_mut(x, f)),
        SValue::Record(fields) => {
            for (k, x) in fields.iter_mut() {
                f(k);
                texts_mut(x, f);
            }
            // A stable sort leaves keys already in order as they are: only sorted when not.
            if !fields.is_sorted_by(|a, b| a.0 <= b.0) {
                fields.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        SValue::Effect { sink, payload } => {
            f(sink);
            texts_mut(payload, f)
        }
        SValue::Sum { payload, .. } => texts_mut(payload, f),
        SValue::Bytes(b) => bytes_texts_mut(b, f),
        SValue::Number(_) | SValue::Boolean(_) => {}
    }
}

/// What follows a mark's `‹`: the kinds of value the log writes a mark in place of.
const MARKED: [&str; 4] = ["secret:", "token:", "taint:", "derived #"];

/// How a tainted value's mark begins: `‹taint:number›`.
const TAINT_MARK: &str = "‹taint:";

/// Whether `s` starts with what reads as a mark the log writes: a run of `‹` before `secret:`,
/// `token:`, `taint:` or `derived #`.
fn reads_as_mark(s: &str) -> bool {
    let rest = s.trim_start_matches('‹');
    rest.len() < s.len() && MARKED.iter().any(|kind| rest.starts_with(kind))
}

/// Text as the log writes it, from its pieces: the run's own text, and the marks (`true`) put in
/// place of values the log never holds. Each `‹` of the run's own that reads as part of a mark (a
/// run of them before `secret:` or `token:`, a mark's own included) is written twice, so that a
/// mark in the log is only ever one written here: a caller who sends `‹token:paid›` can't pass
/// for a token (`holon verify` trusts only the log's own). Whether one was written twice.
pub(crate) fn write_marked(pieces: &[(&str, bool)]) -> (String, bool) {
    let mut whole = String::new();
    let mut marks = Vec::new();
    for (text, mark) in pieces {
        if *mark {
            marks.push(whole.len());
        }
        whole.push_str(text);
    }
    let mut out = String::with_capacity(whole.len());
    let mut escaped = false;
    for (at, c) in whole.char_indices() {
        if c == '‹' && marks.binary_search(&at).is_err() && reads_as_mark(&whole[at..]) {
            out.push('‹');
            escaped = true;
        }
        out.push(c);
    }
    (out, escaped)
}

/// Text the log wrote, as its pieces again: the run's own text, and the marks (`true`, as
/// `‹secret:name›`). `escaped`: its entry says a look-alike was escaped ([`write_marked`]);
/// otherwise there is none to tell apart (or it was logged before they were escaped, and every
/// look-alike reads as a mark, as it did then).
pub(crate) fn read_marked(s: &str, escaped: bool) -> Vec<(String, bool)> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('‹') {
        text.push_str(&rest[..i]);
        let after = rest[i..].trim_start_matches('‹');
        let n = (rest.len() - i - after.len()) / '‹'.len_utf8();
        rest = after;
        if !MARKED.iter().any(|kind| after.starts_with(kind)) {
            text.extend(std::iter::repeat_n('‹', n));
            continue;
        }
        let (literal, mark) = if escaped { (n / 2, n % 2 == 1) } else { (n - 1, true) };
        match after.find('›').filter(|_| mark) {
            Some(end) => {
                text.extend(std::iter::repeat_n('‹', literal));
                pieces.push((std::mem::take(&mut text), false));
                let end = end + '›'.len_utf8();
                pieces.push((format!("‹{}", &after[..end]), true));
                rest = &after[end..];
            }
            // One the log didn't write: no mark is left open.
            None => text.extend(std::iter::repeat_n('‹', literal + usize::from(mark))),
        }
    }
    text.push_str(rest);
    pieces.push((text, false));
    pieces
}

/// A logged value with its look-alikes of a mark as the run had them (and the log's own marks as
/// they read): what a channel's subscriber gets.
pub(crate) fn unescaped(v: &mut SValue) {
    texts_mut(v, &mut |s| *s = read_marked(s, true).into_iter().map(|(t, _)| t).collect());
}

#[cfg(test)]
mod tests {
    /// A secret's UTF-8 inside bytes is found and marked as in a text, every other byte kept; a
    /// look-alike of a mark inside them is escaped as in a text, and read back as it was.
    #[test]
    fn a_secret_inside_bytes_is_marked_as_in_a_text() {
        use crate::wire::SValue;
        let redactor = Redactor::tokens(&BTreeMap::from([("t".to_string(), "sk_live_abc".to_string())]));
        let held = [&[0xff, b'k', b'='][..], b"sk_live_abc", &[0x00, 0xc3]].concat();
        assert!(bytes_hold(&held, &|s: &str| s.contains("sk_live_abc")));
        assert!(!bytes_hold(&[0xff, 0x00], &|s: &str| s.contains("sk")));
        let mut v = SValue::Bytes(held);
        redactor.svalue(&mut v);
        assert_eq!(v, SValue::Bytes([&[0xff, b'k', b'='][..], "‹token:t›".as_bytes(), &[0x00, 0xc3]].concat()));
        let mut v = SValue::Bytes([&[0xfe][..], "x‹secret:k›".as_bytes()].concat());
        let mut none = SValue::Bytes(vec![0xfe, b'x']);
        let empty = Redactor::tokens(&BTreeMap::new());
        assert!(!empty.logged_svalue(&mut none) && none == SValue::Bytes(vec![0xfe, b'x']), "nothing to mark: as they are");
        assert!(empty.logged_svalue(&mut v), "a look-alike is escaped");
        assert_ne!(v, SValue::Bytes([&[0xfe][..], "x‹secret:k›".as_bytes()].concat()));
        assert_eq!(v.clone().into_core().ty(), holon_core::Type::Bytes);
    }

    /// A redactor with nothing to mark leaves every text without a `‹` as it is (D3): what the log's
    /// shortcut (`firing::redacted_line`) counts on. Thousands of texts of the awkward kinds: quotes,
    /// backslashes, `›`, the words a mark is made of, text of several bytes a character.
    #[test]
    fn with_nothing_to_mark_a_text_without_a_mark_sign_stays_as_it_is() {
        let none = Redactor::tokens(&BTreeMap::new());
        assert!(none.is_empty());
        let pieces = ["a", " ", "\"", "\\", "\n", "›", "secret:", "token:", "é", "😀", "{", "}", "#", "x1"];
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut next = move |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        for _ in 0..5000 {
            let text: String = (0..next(12)).map(|_| pieces[next(pieces.len() as u64) as usize]).collect();
            assert_eq!(none.logged(&text), None, "{text:?}");
        }
        // And one with a `‹` is looked at (escaped where it reads as a mark).
        assert_eq!(none.logged("sent ‹token:paid›"), Some(("sent ‹‹token:paid›".to_string(), true)));
    }

    use super::*;

    /// A secrets file in a folder of its own, and the keyring beside it, its root in a key file.
    fn temp(name: &str) -> (String, Arc<Keyring>) {
        let dir = std::env::temp_dir().join(format!("holon-secrets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = format!("file:{}", dir.join("root.key").display());
        let keys = Keyring::open(&dir.join("keys.json"), Some(&root), &[]).unwrap();
        (dir.join("secrets.json").to_str().unwrap().to_string(), Arc::new(keys))
    }

    /// The secrets file's text, opened under `keys`.
    fn opened(keys: &Keyring, path: &str) -> String {
        String::from_utf8(at_rest::read(keys, Path::new(path), at_rest::role::SECRETS).unwrap().unwrap().to_vec()).unwrap()
    }

    /// `bytes` written as the secrets file, sealed under `keys`.
    fn seal(keys: &Keyring, path: &str, bytes: &[u8]) {
        at_rest::write_private(keys, Path::new(path), at_rest::SERVER, at_rest::role::SECRETS, bytes).unwrap();
    }

    #[test]
    fn a_store_keeps_values_sealed_and_survives_a_restart() {
        let (path, key) = temp("restart");
        let store = SecretStore::open(&path, Arc::clone(&key), Sources::none()).unwrap();
        assert_eq!(store.set("api", "stripe_key", "sk_live_123456", None).unwrap(), 1);
        assert!(store.set("api", "bad name", "longenough", None).is_err());
        assert!(store.set("api", "short", "12345", None).unwrap_err().contains("at least 6"));
        assert_eq!(store.names("api"), vec!["stripe_key"]);
        let vault = store.vault("api");
        assert!(!opened(&key, &path).contains("sk_live"), "each value sealed in the file");
        assert!(!std::fs::read_to_string(&path).unwrap_or_default().contains("sk_live"), "and the file itself");

        let again = SecretStore::open(&path, Arc::clone(&key), Sources::none()).unwrap();
        assert_eq!(*again.vault("api").lock().unwrap().current["stripe_key"], "sk_live_123456");
        assert!(store.remove("api", "stripe_key", None).unwrap());
        assert!(vault.lock().unwrap().current.is_empty(), "a running deployment's vault sees the change");
        store.set("api", "k", "longenough", None).unwrap();
        store.forget("api", None).unwrap();
        assert!(SecretStore::open(&path, Arc::clone(&key), Sources::none()).unwrap().names("api").is_empty());
        std::fs::write(&path, "{not json").unwrap();
        assert!(SecretStore::open(&path, Arc::clone(&key), Sources::none()).err().unwrap().contains("not sealed"));
    }

    /// Guards: each value of a secret has one salt, for as long as the store is kept. Drawn when
    /// the value is first read, the same after a restart, another for another version (a
    /// roll-back's too: it is a new version), and for a reference one for each value it has had,
    /// the first again when that value comes back. Sealed in the file, gone with a deleted
    /// deployment, kept when a secret is removed.
    /// Why: a run's record will commit its secrets with it and key the marks of what it computes
    /// from them. A salt drawn again for a value it already had would make two records of one
    /// value read as two values, and one kept in the clear lets a weak value be guessed.
    /// If it fails: the case named says which of those no longer holds.
    #[test]
    fn a_value_has_one_salt_drawn_once_sealed_and_kept_across_restarts() {
        let salt = |store: &SecretStore, slug: &str, name: &str| store.vault(slug).lock().unwrap().salts.get(name).map(|s| s.to_vec());
        let hex = |salt: &[u8]| salt.iter().map(|b| format!("{b:02x}")).collect::<String>();

        // A value set here.
        let (path, key) = temp("salts");
        let open = |sources: Sources| SecretStore::open(&path, Arc::clone(&key), sources).unwrap();
        let store = open(Sources::none());
        store.set("api", "k", "the-first-value", None).unwrap();
        let first = salt(&store, "api", "k").expect("a value set has a salt");
        assert_eq!(first.len(), 32);
        assert_ne!(first, vec![0u8; 32], "drawn, not left empty");
        assert_eq!(salt(&store, "api", "k"), Some(first.clone()), "read again, the same");
        let text = opened(&key, &path);
        assert!(text.contains("\"salts\"") && !text.contains(&hex(&first)), "kept in the file, sealed: {text}");
        assert_eq!(salt(&open(Sources::none()), "api", "k"), Some(first.clone()), "after a restart, the same");

        // Another version has another; a roll-back is a new version of the old value.
        store.set("api", "k", "the-second-value", None).unwrap();
        let second = salt(&store, "api", "k").unwrap();
        assert_ne!(second, first);
        store.rollback("api", "k", 1, None).unwrap();
        let back = salt(&store, "api", "k").unwrap();
        assert!(back != first && back != second, "version 3 holds version 1's value, and is version 3");
        // Removed, its salts are kept (a run that named it is still held to its value); the
        // deployment deleted, they are gone.
        assert!(store.remove("api", "k", None).unwrap());
        assert!(opened(&key, &path).contains("\"salts\""));
        store.set("api", "k", "the-first-value", None).unwrap();
        assert_eq!(salt(&store, "api", "k"), Some(first.clone()), "the same name, version and value: the salt it had");
        store.forget("api", None).unwrap();
        assert!(!opened(&key, &path).contains("\"salts\""), "{}", opened(&key, &path));

        // A reference: its value changes where it is kept, under one version.
        let env = |value: &str| Sources::of(&[("HOLON_SECRET_K", value)]);
        let store = open(env("value-one-from-env"));
        assert_eq!(store.set_from("web", "k", "env:HOLON_SECRET_K", None).unwrap(), 1);
        let one = salt(&store, "web", "k").unwrap();
        let store = open(env("value-two-from-env"));
        let two = salt(&store, "web", "k").unwrap();
        assert_ne!(two, one, "another value, another salt, under the same version");
        assert_eq!(salt(&open(env("value-two-from-env")), "web", "k"), Some(two), "kept as it was drawn, at the open that read it");
        assert_eq!(salt(&open(env("value-one-from-env")), "web", "k"), Some(one), "the first value back: its first salt");

        // The server's secret, read by a deployment with none of its own: one salt, the server's.
        let store = open(Sources::none());
        store.set(SERVER, "shared", "the-servers-value", None).unwrap();
        assert_eq!(salt(&store, "one", "shared"), salt(&store, "two", "shared"));
        assert!(salt(&store, "one", "shared").is_some());

        // A file from before salts opens, and each value gets one as it is read.
        let (old_path, old_key) = temp("salts-before");
        let old = SecretStore::open(&old_path, Arc::clone(&old_key), Sources::none()).unwrap();
        old.set("api", "k", "a-value-from-before", None).unwrap();
        let mut file: serde_json::Value = serde_json::from_str(&opened(&old_key, &old_path)).unwrap();
        file.as_object_mut().unwrap().remove("salts").expect("it had the section");
        seal(&old_key, &old_path, &serde_json::to_vec(&file).unwrap());
        let again = SecretStore::open(&old_path, Arc::clone(&old_key), Sources::none()).unwrap();
        let drawn = salt(&again, "api", "k").expect("drawn at the open that first reads it");
        assert!(opened(&old_key, &old_path).contains("\"salts\""), "and written then");
        assert_eq!(salt(&SecretStore::open(&old_path, Arc::clone(&old_key), Sources::none()).unwrap(), "api", "k"), Some(drawn.clone()));

        // One that doesn't open (sealed for somewhere else) is never drawn again: the value has
        // no salt, at this open and the next, and what the file holds is left as it is.
        let mut file: serde_json::Value = serde_json::from_str(&opened(&old_key, &old_path)).unwrap();
        let elsewhere = old_key.seal_line(at_rest::SECRETS, "sealed for somewhere else", &[7u8; 32]).unwrap();
        file["salts"]["api"][0]["sealed"] = serde_json::json!(elsewhere);
        seal(&old_key, &old_path, &serde_json::to_vec(&file).unwrap());
        for _ in 0..2 {
            let damaged = SecretStore::open(&old_path, Arc::clone(&old_key), Sources::none()).unwrap();
            assert_eq!(salt(&damaged, "api", "k"), None, "never another in its place");
            assert_eq!(*damaged.vault("api").lock().unwrap().current["k"], "a-value-from-before", "the value itself still reads");
            assert!(opened(&old_key, &old_path).contains(&elsewhere));
        }
        // One filed under another value's fingerprint isn't this value's: this one gets its own.
        file["salts"]["api"][0]["print"] = serde_json::json!("00000000");
        seal(&old_key, &old_path, &serde_json::to_vec(&file).unwrap());
        let moved = SecretStore::open(&old_path, Arc::clone(&old_key), Sources::none()).unwrap();
        assert!(salt(&moved, "api", "k").is_some_and(|s| s != drawn));
        assert!(opened(&old_key, &old_path).contains(&elsewhere), "and the other is left as it was");
    }

    /// Guards: what a run records of a secret it is given (`Resolved::committed`): where the
    /// value is kept, and a commitment that whoever holds the value and its salt computes again
    /// (`commitment`), and that is another for another value, version, owner or salt.
    /// Why: it is what later holds the server to the value a run had. One that left a field out
    /// would be the same for two values; one made without the salt could be guessed from.
    /// If it fails: the case named is a commitment that no longer tells two things apart, or a
    /// kind that says the wrong place.
    #[test]
    fn what_a_run_records_of_a_secret_is_where_it_is_kept_and_a_commitment_under_its_salt() {
        let (path, key) = temp("committed");
        let store = SecretStore::open(&path, Arc::clone(&key), Sources::of(&[("HOLON_SECRET_K", "kept-in-the-environment")])).unwrap();
        store.set("api", "here", "kept-in-the-store", None).unwrap();
        store.set_from("api", "there", "env:HOLON_SECRET_K", None).unwrap();
        store.set(SERVER, "shared", "the-servers-value", None).unwrap();
        let vault = store.vault("api");
        let v = vault.lock().unwrap();
        let of = |name: &str| (v.committed[name].clone(), *v.salts[name].clone());
        for (name, kind, server, owner, value) in [("here", "local", false, "api", "kept-in-the-store"), ("there", "reference", false, "api", "kept-in-the-environment"), ("shared", "local", true, SERVER, "the-servers-value")] {
            let (said, salt) = of(name);
            assert_eq!((said.kind.as_str(), said.server), (kind, server), "{name}");
            assert_eq!(said.commitment, commitment(owner, name, 1, &salt, value), "{name}: computed again from the value and its salt");
            assert_eq!(said.commitment.len(), 64);
            // Another for anything else: a value, a version, a name, an owner, a salt.
            let mut other_salt = salt;
            other_salt[0] ^= 1;
            for (what, another) in [
                ("another value", commitment(owner, name, 1, &salt, "kept-somewhere-else")),
                ("another version", commitment(owner, name, 2, &salt, value)),
                ("another name", commitment(owner, "other", 1, &salt, value)),
                ("another owner", commitment("web", name, 1, &salt, value)),
                ("another salt", commitment(owner, name, 1, &other_salt, value)),
            ] {
                assert_ne!(said.commitment, another, "{name}: {what}");
            }
        }
        // No field reads as part of the next: the lengths are in it.
        let salt = [7u8; 32];
        assert_ne!(commitment("ab", "c", 1, &salt, "value-1"), commitment("a", "bc", 1, &salt, "value-1"));
        // The same value under two names, or in two deployments, isn't one commitment. (The
        // vault is let go first: setting a secret takes it.)
        drop(v);
        store.set("api", "twin", "kept-in-the-store", None).unwrap();
        store.set("web", "here", "kept-in-the-store", None).unwrap();
        let (api, web) = (store.vault("api"), store.vault("web"));
        let here = api.lock().unwrap().committed["here"].commitment.clone();
        assert_ne!(here, api.lock().unwrap().committed["twin"].commitment);
        assert_ne!(here, web.lock().unwrap().committed["here"].commitment);
        // And it is the same after a restart, as its salt is.
        let again = SecretStore::open(&path, Arc::clone(&key), Sources::of(&[("HOLON_SECRET_K", "kept-in-the-environment")])).unwrap();
        assert_eq!(again.vault("api").lock().unwrap().committed["here"].commitment, here);
    }

    /// The server's secret is read by a deployment with none of its own, overridden by one it
    /// has, kept sealed with where it may go, and seen by a running deployment as it changes.
    #[test]
    fn the_servers_secret_is_read_where_a_deployment_has_none_and_keeps_its_cap() {
        let (path, key) = temp("server");
        let store = SecretStore::open(&path, Arc::clone(&key), Sources::none()).unwrap();
        let shop = store.vault("shop");
        store.set(SERVER, "stripe_key", "sk_live_server_1", None).unwrap();
        store.set_cap("stripe_key", Some(BTreeSet::from(["api.stripe.com".to_string()])), Some("ada")).unwrap();
        assert!(store.set_cap("nope", None, None).unwrap_err().contains("bind it first"));
        assert!(store.set_cap("stripe_key", Some(BTreeSet::from(["https://x".to_string()])), None).is_err(), "a host, not a URL");
        assert_eq!(*shop.lock().unwrap().current["stripe_key"], "sk_live_server_1", "a running deployment sees it");
        assert_eq!(shop.lock().unwrap().server["stripe_key"], Some(BTreeSet::from(["api.stripe.com".to_string()])));
        assert!(store.list("shop")[0].server);
        // Its own overrides it, and isn't held to the cap.
        store.set("billing", "stripe_key", "sk_live_billing_2", None).unwrap();
        let billing = store.vault("billing");
        assert_eq!(*billing.lock().unwrap().current["stripe_key"], "sk_live_billing_2");
        assert!(billing.lock().unwrap().server.is_empty());
        let sent = BTreeMap::from([("stripe_key".to_string(), BTreeSet::from(["hooks.slack.com".to_string()]))]);
        assert!(store.refuses("shop", &sent).unwrap().contains("may go only to https://api.stripe.com, and this flow sends it to https://hooks.slack.com"));
        assert_eq!(store.refuses("billing", &sent), None, "its own");
        assert!(!opened(&key, &path).contains("sk_live"), "each value sealed in the file");
        assert!(!std::fs::read_to_string(&path).unwrap_or_default().contains("sk_live"), "and the file itself");
        // After a restart: the same, a deployment new to it reads it.
        let again = SecretStore::open(&path, Arc::clone(&key), Sources::none()).unwrap();
        assert_eq!(*again.vault("new").lock().unwrap().current["stripe_key"], "sk_live_server_1");
        assert_eq!(again.list(SERVER)[0].to, Some(BTreeSet::from(["api.stripe.com".to_string()])));
        assert_eq!(again.history(SERVER, "stripe_key").unwrap().1.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), ["set", "capped"]);
        // Removed, so is its cap, and the deployments read nothing for it.
        assert!(again.remove(SERVER, "stripe_key", None).unwrap());
        assert!(again.vault("new").lock().unwrap().current.is_empty());
        assert!(SecretStore::open(&path, Arc::clone(&key), Sources::none()).unwrap().list(SERVER).is_empty());
    }

    /// Guards: a sealed value opens only under the keys that sealed it, and only for the
    /// deployment, name and version it was sealed for.
    /// Why: a value copied to another deployment's entry in the file (by whoever can write it)
    /// must not be read by that deployment's runs.
    #[test]
    fn a_value_opens_only_under_its_keys_and_for_its_place() {
        let (path, key) = temp("place");
        SecretStore::open(&path, Arc::clone(&key), Sources::none()).unwrap().set("api", "k", "sk_live_123456", None).unwrap();
        assert!(SecretStore::open(&path, Arc::new(Keyring::memory()), Sources::none()).is_err(), "another keyring opens nothing");
        // Its sealed value put under another deployment, and another name.
        let mut v: serde_json::Value = serde_json::from_str(&opened(&key, &path)).unwrap();
        let sealed = v["deployments"]["api"]["k"].clone();
        v["deployments"]["shop"] = serde_json::json!({ "k": sealed.clone() });
        v["deployments"]["api"]["j"] = sealed;
        seal(&key, &path, &serde_json::to_vec_pretty(&v).unwrap());
        let store = SecretStore::open(&path, Arc::clone(&key), Sources::none()).unwrap();
        assert_eq!(*store.vault("api").lock().unwrap().current["k"], "sk_live_123456", "where it was sealed");
        assert!(!store.vault("shop").lock().unwrap().current.contains_key("k"), "not for another deployment");
        assert!(!store.vault("api").lock().unwrap().current.contains_key("j"), "nor another name");
    }

    /// A secrets file of another format (plain values, or sealed under the key the server kept
    /// before its keyring) is refused, never read or rewritten: whether it is plain on disk or
    /// sealed as this server seals it.
    #[test]
    fn a_secrets_file_of_another_format_is_refused() {
        let (path, key) = temp("format");
        for text in [r#"{"sign-payload": {"signing_key": "sample-signing-key-42"}}"#, r#"{"format": 3, "cipher": "AES-256-GCM", "key_check": "00", "deployments": {}}"#] {
            std::fs::write(&path, text).unwrap();
            let refused = SecretStore::open(&path, Arc::clone(&key), Sources::none()).err().unwrap();
            assert!(refused.contains("not sealed"), "{refused}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "left as it was");
            seal(&key, &path, text.as_bytes());
            let on_disk = std::fs::read(&path).unwrap();
            let refused = SecretStore::open(&path, Arc::clone(&key), Sources::none()).err().unwrap();
            assert!(refused.contains("this server reads 4"), "{refused}");
            assert_eq!(std::fs::read(&path).unwrap(), on_disk, "left as it was");
        }
    }

    #[test]
    fn versions_are_kept_and_old_values_still_redacted() {
        let store = SecretStore::memory();
        for n in 1..=7 {
            assert_eq!(store.set("api", "k", &format!("value-number-{n}"), None).unwrap(), n);
        }
        let vault = store.vault("api");
        let v = vault.lock().unwrap();
        assert_eq!(*v.current["k"], "value-number-7");
        let retired: Vec<&str> = v.retired.iter().map(|(_, _, value)| value.as_str()).collect();
        assert_eq!(retired, ["value-number-3", "value-number-4", "value-number-5", "value-number-6"], "the last {MAX_VERSIONS} are kept");
        drop(v);
        assert!(Redactor::of(&vault).text("old value-number-4").starts_with("old ‹secret:k v4 #"), "marked with its own version");
        assert_eq!(store.list("api")[0].version, 7);
    }

    /// A mark names the version the value is of and its fingerprint: a value set again has the
    /// fingerprint it had, another value another one, and another store's key makes another.
    #[test]
    fn a_mark_names_the_version_and_a_fingerprint_of_the_value() {
        let store = SecretStore::memory();
        let marked = |store: &SecretStore, text: &str| Redactor::of(&store.vault("api")).text(text);
        let print = |mark: &str| mark.rsplit_once('#').map(|(_, p)| p.trim_end_matches('›').to_string()).unwrap();
        store.set("api", "k", "first-value-1", None).unwrap();
        let first = marked(&store, "first-value-1");
        assert!(first.starts_with("‹secret:k v1 #") && first.ends_with('›'), "{first}");
        let p1 = print(&first);
        assert!(p1.len() == 8 && p1.bytes().all(|b| b.is_ascii_hexdigit()), "{p1}");
        store.set("api", "k", "second-value-2", None).unwrap();
        let second = marked(&store, "second-value-2");
        assert!(second.starts_with("‹secret:k v2 #") && print(&second) != p1, "{second}");
        assert_eq!(marked(&store, "first-value-1"), first, "a retired value keeps its version and print");
        store.set("api", "k", "first-value-1", None).unwrap();
        let again = marked(&store, "first-value-1");
        assert_eq!((again.starts_with("‹secret:k v3 #"), print(&again)), (true, p1.clone()), "set back: the same print");
        assert_eq!(store.vault("api").lock().unwrap().shown["k"], format!("v3 #{p1}"));
        let other = SecretStore::memory();
        other.set("api", "k", "first-value-1", None).unwrap();
        assert_ne!(print(&marked(&other, "first-value-1")), p1, "keyed: another key, another print");
    }

    #[test]
    fn a_secret_can_come_from_the_environment_and_is_redacted_like_any_other() {
        let store = SecretStore::memory().with_sources(Sources::of(&[("HOLON_SECRET_API", "env-value-123"), ("HOLON_SECRET_TINY", "abc")]));
        assert!(store.set_from("api", "k", "env:PATH", None).unwrap_err().contains(crate::secret_sources::ENV_PREFIX));
        assert!(store.set_from("api", "k", "env:HOLON_SECRET_MISSING", None).unwrap_err().contains("isn't set"));
        assert!(store.set_from("api", "k", "env:HOLON_SECRET_TINY", None).unwrap_err().contains("too short"));
        // A name that can't be kept is refused before any source is read: whatever is wrong with
        // the name, an unset variable, a short one and a good one answer alike, so asking teaches
        // nothing of the environment.
        let long = "x".repeat(65);
        for name in ["", "has space", "a/b", long.as_str()] {
            for source in ["env:HOLON_SECRET_MISSING", "env:HOLON_SECRET_TINY", "env:HOLON_SECRET_API", "vault:secret/x#y"] {
                assert!(store.set_from("api", name, source, None).unwrap_err().contains("a secret name is"), "{name:?} from {source}");
            }
        }
        assert!(store.names("api").is_empty(), "a refused source adds nothing");
        assert_eq!(store.set_from("api", "k", "env:HOLON_SECRET_API", None).unwrap(), 1);
        let vault = store.vault("api");
        assert_eq!(*vault.lock().unwrap().current["k"], "env-value-123");
        assert_eq!(store.list("api")[0].source, "env:HOLON_SECRET_API");
        // Replaced by a sealed value: the env one is retired, still redacted.
        store.set("api", "k", "sealed-value-456", None).unwrap();
        let marked = Redactor::of(&vault).text("env-value-123 / sealed-value-456");
        assert!(marked.starts_with("‹secret:k v1 #") && marked.contains("› / ‹secret:k v2 #"), "each marked with its version: {marked}");
    }

    #[test]
    fn a_vault_value_follows_its_source_and_outlives_an_outage_only_briefly() {
        let value: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Some("whsec_first".into())));
        let served = Arc::clone(&value);
        let server = rouille::Server::new("127.0.0.1:0", move |_| match served.lock().unwrap().clone() {
            Some(v) => rouille::Response::json(&serde_json::json!({ "data": { "data": { "signing": v } } })),
            None => rouille::Response::text("sealed").with_status_code(503),
        })
        .unwrap();
        let addr = format!("http://{}", server.server_addr());
        let (_handle, stop) = server.stoppable();
        let store = SecretStore::memory().with_sources(Sources::of(&[("VAULT_ADDR", &addr), ("VAULT_TOKEN", "t")]));
        store.set_from("pay", "signing", "vault:secret/stripe#signing", None).unwrap();
        let vault = store.vault("pay");
        let current = || vault.lock().unwrap().current.get("signing").map(|v| v.to_string());

        *value.lock().unwrap() = Some("whsec_second".into());
        store.refresh();
        assert_eq!(current().as_deref(), Some("whsec_second"), "a change where it's kept reaches the runs");

        *value.lock().unwrap() = None;
        store.refresh();
        assert_eq!(current().as_deref(), Some("whsec_second"), "kept through a short outage");
        assert!(store.list("pay")[0].problem.as_deref().unwrap().contains("503"));

        // Past STALE_AFTER without a successful read, it's dropped: a revoked value stops working.
        if let Some(long_ago) = std::time::Instant::now().checked_sub(STALE_AFTER + std::time::Duration::from_secs(1)) {
            vault.lock().unwrap().fetched.get_mut("signing").unwrap().1 = long_ago;
            store.refresh();
            assert_eq!(current(), None);
        }
        let _ = stop.send(());
    }

    #[test]
    fn a_rollback_restores_an_older_version_as_a_new_one_and_everything_is_audited() {
        let (path, key) = temp("rollback");
        let store = SecretStore::open(&path, Arc::clone(&key), Sources::of(&[("HOLON_SECRET_K", "from-the-env")])).unwrap();
        store.set("api", "k", "first-value", None).unwrap();
        store.set_from("api", "k", "env:HOLON_SECRET_K", None).unwrap();
        store.set("api", "k", "third-value", None).unwrap();
        assert!(store.rollback("api", "k", 3, None).unwrap_err().contains("already current"));
        assert!(store.rollback("api", "k", 9, None).unwrap_err().contains("isn't kept"));
        // A dry run says what it would do, and does nothing.
        assert!(store.rollback_preview("api", "k", 3).unwrap_err().contains("already current"));
        let preview = store.rollback_preview("api", "k", 2).unwrap();
        let print = |v: &str| store.keys.fingerprint(v);
        assert_eq!(
            preview,
            RollbackPreview { to: 2, current: 3, version: 4, source: "env:HOLON_SECRET_K".into(), to_print: print("from-the-env"), current_print: Some(print("third-value")) }
        );
        assert_eq!(store.list("api")[0].version, 3);
        assert_eq!(store.rollback("api", "k", 1, None).unwrap(), 4);
        assert_eq!(*store.vault("api").lock().unwrap().current["k"], "first-value", "re-sealed for its new version");
        assert_eq!(store.rollback("api", "k", 2, None).unwrap(), 5);
        assert_eq!(store.list("api")[0].source, "env:HOLON_SECRET_K");

        let (versions, events) = store.history("api", "k").unwrap();
        assert_eq!(versions.iter().map(|v| (v.version, v.current)).collect::<Vec<_>>(), [(5, true), (4, false), (3, false), (2, false), (1, false)]);
        let actions: Vec<(&str, Option<u32>)> = events.iter().map(|e| (e.action.as_str(), e.version)).collect();
        assert_eq!(actions, [("set", Some(1)), ("set", Some(2)), ("set", Some(3)), ("rolled back", Some(4)), ("rolled back", Some(5))]);
        assert_eq!(events[3].detail.as_deref(), Some("to version 1"));
        let log = std::fs::read_to_string(Path::new(&path).with_extension("audit.jsonl")).unwrap();
        assert!(!log.contains("first-value") && !log.contains("third-value") && !log.contains("from-the-env"), "never a value: {log}");

        store.remove("api", "k", None).unwrap();
        let (versions, events) = store.history("api", "k").unwrap();
        assert!(versions.is_empty());
        assert_eq!(events.last().unwrap().action, "removed");
        assert!(store.history("api", "never").is_none());
    }

    #[test]
    fn a_source_that_stops_reading_is_audited_once() {
        let store = SecretStore::memory().with_sources(Sources::of(&[("VAULT_ADDR", "http://127.0.0.1:9"), ("VAULT_TOKEN", "t")]));
        {
            // Stored as if set while Vault was up; it isn't now.
            let mut slugs = store.slugs.lock().unwrap();
            let entry = slugs.entry("pay".into()).or_default();
            entry.secrets.insert("s".into(), vec![Stored { version: 1, set_at: 0, source: "vault:secret/x#f".into(), sealed: None }]);
        }
        store.refresh();
        store.refresh();
        let (_, events) = store.history("pay", "s").unwrap();
        assert_eq!(events.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), ["unreadable"]);
        assert!(events[0].detail.as_deref().unwrap().contains("can't be reached"));
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
        // A key too, in a reply and in the log, the record kept in key order.
        let keyed = || SValue::Record(vec![("a".into(), SValue::Text("x".into())), ("sk_123456".into(), SValue::Text("acme".into()))]);
        let redacted = SValue::Record(vec![("a".into(), SValue::Text("x".into())), ("‹secret:key›".into(), SValue::Text("acme".into()))]);
        let mut v = keyed();
        r.svalue(&mut v);
        assert_eq!(v, redacted);
        let mut v = keyed();
        assert!(!r.logged_svalue(&mut v));
        assert_eq!(v, redacted, "the log writes the key's mark");
    }

    /// In a log entry, a run's own text that reads like a mark is escaped (each `‹` before one
    /// written twice), so the marks read back are only the log's own; text without one is as it was.
    #[test]
    fn a_look_alike_of_a_mark_is_escaped_and_reads_back_as_text() {
        let vault: Vault = Arc::new(Mutex::new(Resolved {
            current: BTreeMap::from([("key".to_string(), Zeroizing::new("sk_123456".to_string()))]),
            ..Resolved::default()
        }));
        let r = Redactor::of(&vault).and(Redactor::tokens(&BTreeMap::from([("paid".to_string(), "tok-abcdef".to_string())])));
        let read = |s: &str, escaped| read_marked(s, escaped).into_iter().map(|(t, m)| if m { format!("[{t}]") } else { t }).collect::<String>();
        for (run, logged, marks) in [
            ("plain ‹quoted› text", "plain ‹quoted› text", "plain ‹quoted› text"),
            ("Bearer sk_123456 for tok-abcdef", "Bearer ‹secret:key› for ‹token:paid›", "Bearer [‹secret:key›] for [‹token:paid›]"),
            ("sent ‹token:paid›", "sent ‹‹token:paid›", "sent ‹token:paid›"),
            ("‹‹secret:key› and ‹sk_123456", "‹‹‹‹secret:key› and ‹‹‹secret:key›", "‹‹secret:key› and ‹[‹secret:key›]"),
            ("open ‹token:paid", "open ‹‹token:paid", "open ‹token:paid"),
        ] {
            // Unchanged comes back as none: the text as it is, nothing escaped.
            let (out, escaped) = r.logged(run).unwrap_or_else(|| (run.to_string(), false));
            assert_eq!(out, logged, "{run:?}");
            assert_eq!(escaped, logged != run.replace("sk_123456", "‹secret:key›").replace("tok-abcdef", "‹token:paid›"), "{run:?}");
            assert_eq!(read(&out, escaped), marks, "{run:?}");
        }
        // An entry logged before look-alikes were escaped: each reads as a mark, as it did then.
        assert_eq!(read("sent ‹token:paid› and ‹‹secret:key›", false), "sent [‹token:paid›] and ‹[‹secret:key›]");

        let mut e = LogEntry { request: Some(SValue::Text("‹token:paid›".into())), outputs: vec![Some(SValue::Text("tok-abcdef".into()))], ..LogEntry::default() };
        r.entry(&mut e);
        assert!(e.escaped);
        assert_eq!(e.request, Some(SValue::Text("‹‹token:paid›".into())));
        assert_eq!(e.outputs, vec![Some(SValue::Text("‹token:paid›".into()))]);
        let mut plain = e.request.clone().unwrap();
        unescaped(&mut plain);
        assert_eq!(plain, SValue::Text("‹token:paid›".into()), "as the run had it");
        let mut e = LogEntry { request: Some(SValue::Text("sk_123456".into())), ..LogEntry::default() };
        r.entry(&mut e);
        assert!(!e.escaped, "nothing to escape: the line is as it was");
    }

    /// Guards: the hash a run's record keeps of a value it computed from a secret
    /// ([`derived_hash`]) is one value's in one run under one set of salts, and nobody else's;
    /// and the redactor made of such values ([`Redactor::born`]) writes each as the start of its
    /// hash wherever it is, a secret's own mark first where a value is both.
    /// Why: `holon verify` follows a value through a run by its mark, and reads two marks as two
    /// values; and a hash that didn't take the salts, or the run, could be guessed from, or
    /// matched across runs.
    /// If it fails: the row named says which: a mark that moves between runs of one value in
    /// one run, a key made of no salt, or a derived value that hides a secret's mark.
    #[test]
    fn a_value_computed_from_a_secret_has_one_mark_in_a_run_keyed_by_the_runs_salts() {
        let (a, b) = ([7u8; 32], [9u8; 32]);
        let key = |salts: &[(&str, &[u8; 32])]| derived_key(salts.iter().copied());
        assert!(key(&[]).is_none(), "no salt, no key: nothing is marked");
        let one = key(&[("k", &a)]).unwrap();
        assert_eq!(one.len(), 32);
        assert_eq!(*one, *key(&[("k", &a)]).unwrap(), "the same salts, the same key");
        for (what, other) in [("another salt", key(&[("k", &b)])), ("another name", key(&[("j", &a)])), ("one more secret", key(&[("k", &a), ("m", &b)]))] {
            assert_ne!(*one, *other.unwrap(), "{what}");
        }
        // A name and a salt don't read as another name and salt: each has its length before it.
        assert_ne!(*key(&[("ab", &a)]).unwrap(), *key(&[("a", &a)]).unwrap());

        let hash = derived_hash(&one, "run-1", "QmFzaWMgdmFsdWU=");
        assert!(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "{hash}");
        assert_eq!(hash, derived_hash(&one, "run-1", "QmFzaWMgdmFsdWU="), "one value, one run: one hash");
        assert_ne!(hash, derived_hash(&one, "run-2", "QmFzaWMgdmFsdWU="), "another run");
        assert_ne!(hash, derived_hash(&one, "run-1", "QmFzaWMgdmFsdWU"), "another value");
        assert_ne!(hash, derived_hash(&key(&[("k", &b)]).unwrap(), "run-1", "QmFzaWMgdmFsdWU="), "another key");

        // Marked by its value, wherever it is; and where it is a secret's own value, as the secret.
        let secret = "sk_live_both_ways";
        let vault: Vault = Arc::new(Mutex::new(Resolved { current: BTreeMap::from([("k".to_string(), Zeroizing::new(secret.to_string()))]), ..Resolved::default() }));
        let born = crate::recorder::Born {
            values: vec![(hash.clone(), "QmFzaWMgdmFsdWU=".to_string()), (derived_hash(&one, "run-1", secret), secret.to_string())],
            whole: true,
        };
        let redactor = Redactor::of(&vault).and(Redactor::born(&born));
        let mark = format!("{DERIVED_MARK}{}›", &hash[..8]);
        assert_eq!(redactor.text("Basic QmFzaWMgdmFsdWU= and QmFzaWMgdmFsdWU="), format!("Basic {mark} and {mark}"));
        assert_eq!(redactor.text(secret), "‹secret:k›", "a secret's value reads as the secret, though a step gave it too");
        let mut e = LogEntry { request: Some(SValue::Text(format!("{mark} QmFzaWMgdmFsdWU="))), ..LogEntry::default() };
        redactor.entry(&mut e);
        assert_eq!(e.request, Some(SValue::Text(format!("‹{mark} {mark}"))), "a look-alike the run wrote itself is escaped, as a secret's is");
        assert!(e.escaped);
        assert_eq!(read_marked(&format!("‹{mark} {mark}"), true), vec![(format!("{mark} "), false), (mark.clone(), true), (String::new(), false)]);
    }

    /// Guards: every text a log entry holds goes through the redactor ([`Redactor::entry`]), or is
    /// named below as one that holds no run data: an id or a hash the host makes, a word it
    /// writes, a name the flow's text gives.
    /// Why: what the redactor goes over is a list kept by hand (`LogEntry::values_mut`,
    /// `texts_mut`), and a field left off it kept what a run had in the log as it was: a node's
    /// `reason` let `throw new Error("key is " + k)` write the secret `k` into the log, the
    /// export and `explain`, in the clear.
    /// If it fails: a field the log writes carries what a run had past the redactor. Walk it in
    /// `values_mut` or `texts_mut`, or name it here with why it never holds run data. A new
    /// field of the entry, or of anything in it, doesn't compile here until it is given a value.
    #[test]
    fn every_text_a_log_entry_holds_is_redacted_or_named_as_no_run_data() {
        use crate::recorder::{AnswerTokens, Commit, LoggedClaim, LoggedExchange, LoggedFile, LoggedRange, LoggedRead, LoggedRound, NodeTiming};
        use crate::wire::PerformedEntry;
        const S: &str = "sk_sentinel_4242";
        let vault: Vault = Arc::new(Mutex::new(Resolved {
            current: BTreeMap::from([("key".to_string(), Zeroizing::new(S.to_string()))]),
            ..Resolved::default()
        }));
        let t = || S.to_string();
        // A value holding it everywhere a value can: a record's key and field, a list, bytes, an
        // effect's sink and payload, a sum's payload.
        let v = || {
            SValue::Record(vec![(
                t(),
                SValue::List(vec![
                    SValue::Text(t()),
                    SValue::Bytes(t().into_bytes()),
                    SValue::Effect { sink: t(), payload: Box::new(SValue::Text(t())) },
                    SValue::Sum { arm: 0, payload: Box::new(SValue::Text(t())) },
                ]),
            )])
        };
        let performed = || PerformedEntry { capability: t(), payload: v() };
        // A read of part of a file, its text `text` and the file's bytes around it `near`: one
        // whose text or bytes around hold it isn't kept (verify asks for the file), the hashes of
        // one whose don't are.
        let read = |text: &str, near: &str| LoggedRead {
            path: vec![0],
            request: v(),
            answer: Some(v()),
            file: Some(LoggedFile {
                hash: t(),
                size: 1,
                blob: Some(t()),
                range: Some(LoggedRange {
                    start: 0,
                    end: 1,
                    text: Some(text.to_string()),
                    before: crate::ranges::hex(near.as_bytes()),
                    after: crate::ranges::hex(near.as_bytes()),
                    proof: vec![t()],
                }),
            }),
        };
        let mut e = LogEntry {
            seq: 1,
            prev: t(),
            lost_since: Some(1),
            session: 1,
            at_ms: 1,
            duration_ms: 1.0,
            kind: t(),
            route: Some(t()),
            method: Some(t()),
            url: Some(t()),
            status: Some(200),
            request: Some(v()),
            sourced: vec![(t(), v())],
            state_before: BTreeMap::from([(t(), v())]),
            state_after: Some(t()),
            commit: Some(Commit { cell: t(), after: Some(1), n: Some(2) }),
            outputs: vec![Some(v())],
            performed: vec![performed()],
            response_body: Some(t()),
            error: Some(t()),
            timeline: vec![NodeTiming { path: vec![0], start_us: 0.0, end_us: 1.0, round: 0, outputs: Some(vec![Some(v())]), reason: Some(t()) }],
            rounds: vec![LoggedRound { exchanges: vec![LoggedExchange { path: vec![0], request: v(), response: v(), start_us: 0.0, end_us: 1.0 }] }],
            run_id: t(),
            unit: t(),
            nonce: t(),
            grant: t(),
            parked: vec![t()],
            progress: vec![performed()],
            events: BTreeMap::from([(t(), v())]),
            reads: vec![read(S, "plain"), read("plain", S), read("plain", "plain")],
            retries: 1,
            claims: vec![LoggedClaim { key: t(), waited_ms: 1.0, overtaken: false, held_ms: Some(1.0) }],
            by: Some(t()),
            secrets: BTreeMap::from([(t(), t())]),
            committed: BTreeMap::from([(t(), crate::recorder::Committed { kind: t(), commitment: t(), server: true })]),
            escaped: false,
            // Never written with the line: each goes its own way, redacted there (firing.rs's
            // `push_run`, mails.rs, queues.rs).
            answered: AnswerTokens(vec![(t(), t())]),
            born: crate::recorder::Born { values: vec![(t(), t())], whole: true },
            derived: vec![t()],
            mails: vec![(t(), holon_core::Value::Text(t()))],
            queued: vec![(t(), holon_core::Value::Text(t()))],
            lines: vec![(t(), holon_core::Value::Text(t()))],
        };
        Redactor::of(&vault).entry(&mut e);
        // A part of a file whose text or bytes around hold it isn't kept at all, so verify asks
        // for the file.
        for read in &e.reads[..2] {
            let kept = read.file.as_ref().unwrap();
            assert!(kept.blob.is_none(), "{kept:?}");
            assert_eq!(kept.range.as_ref().map(|r| (&r.text, r.before.as_str(), r.after.as_str(), r.proof.len())), Some((&None, "", "", 0)), "{kept:?}");
        }
        let line = serde_json::to_value(&e).unwrap();

        // Where it is still there, as text, hex or base64: each place by its fields, `[]` for any
        // item and `{}` for any key.
        let forms = [S.to_string(), crate::ranges::hex(S.as_bytes()), holon_core::bytes_text(S.as_bytes(), "base64").unwrap()];
        fn left(v: &serde_json::Value, at: &str, forms: &[String], out: &mut BTreeSet<String>) {
            let holds = |s: &str| forms.iter().any(|f| s.contains(f.as_str()));
            match v {
                serde_json::Value::String(s) if holds(s) => {
                    out.insert(at.to_string());
                }
                serde_json::Value::Array(xs) => xs.iter().for_each(|x| left(x, &format!("{at}[]"), forms, out)),
                serde_json::Value::Object(fields) => {
                    for (k, x) in fields {
                        let at = if holds(k) { format!("{at}.{{}}") } else { format!("{at}.{k}") };
                        if holds(k) {
                            out.insert(at.clone());
                        }
                        left(x, &at, forms, out);
                    }
                }
                _ => {}
            }
        }
        let mut found = BTreeSet::new();
        left(&line, "", &forms, &mut found);
        let named: BTreeSet<String> = [
            // The chain, the run's id and what it ran under: hashes and ids the host makes.
            ".prev", ".run_id", ".unit", ".nonce", ".grant", ".state_after", ".commit.cell",
            // Words the host writes and names the flow's text gives: the kind of run, the route as
            // the flow names it, each capability, waiting point and state.
            ".kind", ".route", ".sourced[][]", ".performed[].capability", ".progress[].capability", ".parked[]", ".state_before.{}", ".events.{}",
            // Who sent it (a principal's name), and the secrets it was given, by name, version and
            // fingerprint; and what each is held to, a word for where it is kept and a hash the
            // host makes of it under a salt.
            ".by", ".secrets.{}", ".committed.{}", ".committed.{}.kind", ".committed.{}.commitment",
            // What it computed from a secret, each by a keyed hash the host makes of it.
            ".derived[]",
            // A file read's hashes: the file's, its text's in the blob store, the proof's.
            ".reads[].file.hash", ".reads[].file.blob", ".reads[].file.range.proof[]",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(found, named, "left in the line: {line}");
    }
}
