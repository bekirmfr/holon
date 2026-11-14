//! Encryption at rest (docs/L4s/at-rest-encryption-L4.md): the only code that seals bytes for the
//! disk or opens them from it.
//!
//! - **Keys.** One root key, kept where `--root-key` says (the OS key store, `env:VAR`,
//!   `file:PATH`), never in the data folder. In the data folder, `keys.json` keeps data keys, each
//!   wrapped by the root, for scopes: `server` (the server's own tables and store), `secrets`
//!   (secret values), `names` (the store's file names, never rotated) and `d:<slug>`, one for each
//!   deployment (what it wrote), made the first time something of it is sealed. Each scope has
//!   versions; every record names the version that sealed it.
//! - **One AEAD**, XChaCha20-Poly1305, a random 192-bit nonce per record: a run log seals a line
//!   per run, past what a 96-bit random nonce allows one key (§2.3).
//! - **Two record formats.** A whole file: `"HLA1"`, the scope's length and name, the version
//!   (u32 LE), the nonce, the ciphertext and tag. A line, or one value kept in a text: `a1.<v>.`
//!   and the nonce, ciphertext and tag in base64url, so a file of lines still splits on `\n`.
//!   Each binds what it is for (`role`) as associated data, so a record moved to another role,
//!   scope or version fails to open; a line's doesn't name its place in a log, so a cut-back
//!   copies sealed lines byte for byte (§4).
//! - **Nothing plain is read where a sealed record belongs**: [`Unreadable::NotSealed`].
//!
//! The root key is read when the keyring opens and when a scope is made, and not kept: what the
//! server holds is the data keys, each wiped when dropped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::sealing::{KeySource, new_key};
use crate::sync::RwLock;

/// A 256-bit key, wiped from memory when dropped.
pub(crate) type Key = Zeroizing<[u8; 32]>;

/// What a whole sealed file starts with.
const MAGIC: &[u8; 4] = b"HLA1";
/// What a sealed line starts with.
const LINE: &str = "a1.";
const NONCE: usize = 24;
const KEYS_FORMAT: u32 = 1;
const AEAD: &str = "xchacha20poly1305";

/// The server's own scope: its tables, the store, its host key.
pub(crate) const SERVER: &str = "server";
/// Secret values.
pub(crate) const SECRETS: &str = "secrets";
/// The store's file names (§2.6): one version, never rotated.
pub(crate) const NAMES: &str = "names";

/// The scope of what deployment `slug` wrote.
pub(crate) fn deployment(slug: &str) -> String {
    format!("d:{slug}")
}

/// Why a record can't be opened.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Unreadable {
    /// Plain bytes where a sealed record belongs: never read as they are.
    NotSealed,
    /// Its shape is wrong, or it doesn't open under the key it names (changed, or sealed under
    /// another folder's keys, or for another role).
    Damaged(String),
    /// Sealed under a version of its scope this keyring doesn't have.
    NoVersion { scope: String, version: u32 },
    /// Sealed under a scope that was forgotten (its deployment removed with `--forget`), at `at_ms`.
    Forgotten { scope: String, at_ms: u64 },
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unreadable::NotSealed => f.write_str("not sealed: a plain file where a sealed one belongs is never read"),
            Unreadable::Damaged(why) => write!(f, "damaged: {why}"),
            Unreadable::NoVersion { scope, version } => write!(f, "sealed under version {version} of {scope:?}, which this server's keys don't have"),
            Unreadable::Forgotten { scope, at_ms } => write!(f, "sealed under {scope:?}, whose keys were forgotten (at {at_ms} ms): it can't be read"),
        }
    }
}

/// `keys.json`: the data keys, wrapped by the root, and nothing that opens them without it.
#[derive(Serialize, Deserialize, Clone, Default)]
struct KeysFile {
    format: u32,
    /// The id the root key is kept under (`holon` / `root-key-<id>`, `<id>.key`): random, made
    /// with the file, so the file finds its root wherever it is moved.
    key_id: String,
    /// Recognises the root without revealing it: a wrong root is told apart from damage.
    root_check: String,
    aead: String,
    scopes: BTreeMap<String, ScopeFile>,
    /// The scopes forgotten, and when: a record sealed under one says so.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    forgotten: BTreeMap<String, u64>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct ScopeFile {
    current: u32,
    versions: BTreeMap<u32, Wrapped>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Wrapped {
    /// The 24-byte nonce and the data key sealed under the root, base64url.
    wrapped: String,
    made_ms: u64,
}

/// One scope's keys as the server uses them: derived from its data keys, which are not kept.
#[derive(Default)]
struct ScopeKeys {
    current: u32,
    versions: BTreeMap<u32, Key>,
}

struct Inner {
    file: KeysFile,
    open: BTreeMap<String, ScopeKeys>,
}

/// The keys the server seals and opens with: shared, as the store is.
pub(crate) struct Keyring {
    /// Where `keys.json` is and where its root is kept; none for a keyring kept in memory only.
    kept: Option<(PathBuf, KeySource)>,
    /// A memory keyring's root: never written, so what it sealed is gone with the server.
    memory_root: Option<Key>,
    inner: RwLock<Inner>,
    /// What the store's file names are keyed by (§2.6).
    names: Key,
    /// What a secret value's fingerprint is keyed by.
    prints: Key,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn b64(bytes: &[u8]) -> String {
    holon_core::bytes_text(bytes, "base64url").expect("base64url writes any bytes")
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    holon_core::text_bytes(text, "base64url")
}

fn root_check(root: &Key) -> String {
    blake3::keyed_hash(root, b"holon root key check").to_hex()[..16].to_string()
}

fn wrap_context(key_id: &str, scope: &str, version: u32) -> String {
    format!("holon-key/v1\0{key_id}\0{scope}\0{version}")
}

fn random_nonce() -> [u8; NONCE] {
    let mut nonce = [0u8; NONCE];
    getrandom::fill(&mut nonce).expect("the OS random source failed");
    nonce
}

fn seal_with(key: &Key, aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let nonce = random_nonce();
    let sealed = XChaCha20Poly1305::new(key.as_slice().into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
        .expect("sealing an in-memory value can't fail");
    let mut out = Vec::with_capacity(NONCE + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

fn open_with(key: &Key, aad: &[u8], sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if sealed.len() < NONCE + 16 {
        return None;
    }
    let (nonce, body) = sealed.split_at(NONCE);
    XChaCha20Poly1305::new(key.as_slice().into()).decrypt(XNonce::from_slice(nonce), Payload { msg: body, aad }).ok().map(Zeroizing::new)
}

/// The key a data key seals with, and the others derived from it: none of them is kept.
fn derive(context: &str, data_key: &[u8]) -> Key {
    Zeroizing::new(blake3::derive_key(context, data_key))
}

const SEAL_CONTEXT: &str = "holon at-rest v1 seal";
const NAMES_CONTEXT: &str = "holon at-rest v1 names";
const PRINTS_CONTEXT: &str = "holon secrets fingerprints";

impl Keyring {
    /// The keyring whose `keys.json` is at `path`, its root where `root_arg` says (`os`,
    /// `env:VAR`, `file:PATH`, else the default for its id). With no `keys.json`, a new one,
    /// and a new root where none is kept yet; unless one of `sealed` is there already: those
    /// were sealed under keys this folder no longer has, or written before encryption at rest,
    /// and starting anew would make them unreadable for good. A root that's missing or not the
    /// one the file was made under refuses too, and nothing is written.
    pub(crate) fn open(path: &Path, root_arg: Option<&str>, sealed: &[&Path]) -> Result<Keyring, String> {
        let file: Option<KeysFile> = match std::fs::read(path) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(|e| format!("{} is not a keys file: {e}", path.display()))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("could not read {}: {e}", path.display())),
        };
        let Some(file) = file else {
            if let Some(there) = sealed.iter().find(|p| p.exists()) {
                return Err(format!(
                    "{} is missing, and {} is there: it was sealed under its keys, or written before the server sealed its data. \
                     Restore keys.json beside it, or move the data folder aside to start over",
                    path.display(),
                    there.display()
                ));
            }
            return Keyring::create(path, root_arg);
        };
        if file.format != KEYS_FORMAT || file.aead != AEAD {
            return Err(format!("{} is keys format {} ({}), this server reads {KEYS_FORMAT} ({AEAD})", path.display(), file.format, file.aead));
        }
        let source = KeySource::parse(root_arg, &file.key_id)?;
        let root = source.read()?.ok_or_else(|| {
            format!("the root key that opens {} isn't in {source}: without it nothing in the data folder can be read (restore the root key, or pass --root-key where it is)", path.display())
        })?;
        if root_check(&root) != file.root_check {
            return Err(format!("{source} holds a different root key from the one {} was made under", path.display()));
        }
        let mut open = BTreeMap::new();
        for (scope, kept) in &file.scopes {
            let mut keys = ScopeKeys { current: kept.current, versions: BTreeMap::new() };
            for (version, wrapped) in &kept.versions {
                let data = unb64(&wrapped.wrapped)
                    .and_then(|w| open_with(&root, wrap_context(&file.key_id, scope, *version).as_bytes(), &w))
                    .filter(|k| k.len() == 32)
                    .ok_or_else(|| format!("{}: the key of {scope:?} version {version} doesn't open under the root (damaged, or moved from another scope or folder)", path.display()))?;
                keys.versions.insert(*version, derive(SEAL_CONTEXT, &data));
            }
            open.insert(scope.clone(), keys);
        }
        let names = Keyring::unwrap_one(&root, &file, NAMES, NAMES_CONTEXT, path)?;
        let prints = Keyring::unwrap_one(&root, &file, SECRETS, PRINTS_CONTEXT, path)?;
        Ok(Keyring { kept: Some((path.to_path_buf(), source)), memory_root: None, inner: RwLock::new(Inner { file, open }), names, prints })
    }

    /// `scope`'s first version's data key, derived for `context`: what never changes with a
    /// rotation (the store's names, fingerprints).
    fn unwrap_one(root: &Key, file: &KeysFile, scope: &str, context: &str, path: &Path) -> Result<Key, String> {
        let wrapped = file.scopes.get(scope).and_then(|s| s.versions.get(&1)).ok_or_else(|| format!("{} has no {scope:?} key", path.display()))?;
        let data = unb64(&wrapped.wrapped)
            .and_then(|w| open_with(root, wrap_context(&file.key_id, scope, 1).as_bytes(), &w))
            .filter(|k| k.len() == 32)
            .ok_or_else(|| format!("{}: the key of {scope:?} doesn't open under the root", path.display()))?;
        Ok(derive(context, &data))
    }

    /// A new keyring at `path`: a random id, the root read where it is kept (made there if there
    /// is none), and the server's three scopes.
    fn create(path: &Path, root_arg: Option<&str>) -> Result<Keyring, String> {
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).expect("the OS random source failed");
        let key_id = holon_core::hex(id);
        let source = KeySource::parse(root_arg, &key_id)?;
        let root = match source.read()? {
            Some(root) => root,
            None => source.create()?,
        };
        let keyring = Keyring::fresh(&root, key_id, Some((path.to_path_buf(), source)), None);
        keyring.persist(&keyring.inner.read().unwrap().file)?;
        Ok(keyring)
    }

    /// A keyring kept in memory only, under a root that is never written: a server or a test
    /// that keeps nothing.
    pub(crate) fn memory() -> Keyring {
        let root = new_key();
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).expect("the OS random source failed");
        Keyring::fresh(&root, holon_core::hex(id), None, Some(root.clone()))
    }

    fn fresh(root: &Key, key_id: String, kept: Option<(PathBuf, KeySource)>, memory_root: Option<Key>) -> Keyring {
        let mut file = KeysFile { format: KEYS_FORMAT, key_id, root_check: root_check(root), aead: AEAD.into(), ..KeysFile::default() };
        let mut open = BTreeMap::new();
        let mut derived = BTreeMap::new();
        for scope in [SERVER, SECRETS, NAMES] {
            let (wrapped, data) = Keyring::new_version(root, &file.key_id, scope, 1);
            file.scopes.insert(scope.to_string(), ScopeFile { current: 1, versions: [(1, wrapped)].into() });
            open.insert(scope.to_string(), ScopeKeys { current: 1, versions: [(1, derive(SEAL_CONTEXT, data.as_slice()))].into() });
            derived.insert(scope, data);
        }
        let names = derive(NAMES_CONTEXT, derived[NAMES].as_slice());
        let prints = derive(PRINTS_CONTEXT, derived[SECRETS].as_slice());
        Keyring { kept, memory_root, inner: RwLock::new(Inner { file, open }), names, prints }
    }

    /// A new data key for `scope` at `version`, and it wrapped under `root`.
    fn new_version(root: &Key, key_id: &str, scope: &str, version: u32) -> (Wrapped, Key) {
        let data = new_key();
        let wrapped = seal_with(root, wrap_context(key_id, scope, version).as_bytes(), data.as_slice());
        (Wrapped { wrapped: b64(&wrapped), made_ms: now_ms() }, data)
    }

    /// `keys.json` written, owner-only; nothing for a keyring in memory.
    fn persist(&self, file: &KeysFile) -> Result<(), String> {
        let Some((path, _)) = &self.kept else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
        crate::sealing::write_private(path, &bytes).map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    /// The root, read again where it is kept (and checked): to make a scope.
    fn root(&self, file: &KeysFile) -> Result<Key, String> {
        if let Some(root) = &self.memory_root {
            return Ok(root.clone());
        }
        let (path, source) = self.kept.as_ref().expect("a keyring is kept or in memory");
        let root = source.read()?.ok_or_else(|| format!("the root key that opens {} isn't in {source} any more", path.display()))?;
        if root_check(&root) != file.root_check {
            return Err(format!("{source} now holds a different root key from the one {} was made under", path.display()));
        }
        Ok(root)
    }

    /// The id the root is kept under (`holon key status`, step 7).
    #[allow(dead_code)]
    pub(crate) fn key_id(&self) -> String {
        self.inner.read().unwrap().file.key_id.clone()
    }

    /// Where the root is kept, if anywhere.
    pub(crate) fn source(&self) -> Option<&KeySource> {
        self.kept.as_ref().map(|(_, s)| s)
    }

    /// `scope`'s current version and its key, the scope made (and `keys.json` written) if it is
    /// new. A forgotten scope is never made again.
    fn current(&self, scope: &str) -> Result<(u32, Key), String> {
        {
            let inner = self.inner.read().unwrap();
            if let Some(keys) = inner.open.get(scope) {
                return Ok((keys.current, keys.versions[&keys.current].clone()));
            }
            if inner.file.forgotten.contains_key(scope) {
                return Err(format!("the keys of {scope:?} were forgotten: nothing more is sealed under them"));
            }
        }
        let mut inner = self.inner.write().unwrap();
        if let Some(keys) = inner.open.get(scope) {
            return Ok((keys.current, keys.versions[&keys.current].clone()));
        }
        let root = self.root(&inner.file)?;
        let (wrapped, data) = Keyring::new_version(&root, &inner.file.key_id, scope, 1);
        let mut file = inner.file.clone();
        file.scopes.insert(scope.to_string(), ScopeFile { current: 1, versions: [(1, wrapped)].into() });
        // Written before it is used: nothing is sealed under a key keys.json doesn't keep.
        self.persist(&file)?;
        inner.file = file;
        let key = derive(SEAL_CONTEXT, data.as_slice());
        inner.open.insert(scope.to_string(), ScopeKeys { current: 1, versions: [(1, key.clone())].into() });
        Ok((1, key))
    }

    /// The key of `scope` at `version`, or why there is none.
    fn version(&self, scope: &str, version: u32) -> Result<Key, Unreadable> {
        let inner = self.inner.read().unwrap();
        if let Some(at_ms) = inner.file.forgotten.get(scope) {
            return Err(Unreadable::Forgotten { scope: scope.into(), at_ms: *at_ms });
        }
        inner.open.get(scope).and_then(|k| k.versions.get(&version)).cloned().ok_or(Unreadable::NoVersion { scope: scope.into(), version })
    }

    /// `plain` sealed as a whole file under `scope`, for `role`.
    pub(crate) fn seal_file(&self, scope: &str, role: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
        let (version, key) = self.current(scope)?;
        let header = file_header(scope, version);
        let sealed = seal_with(&key, &file_aad(&header, role), plain);
        let mut out = header;
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// A whole file sealed for `role`, opened.
    pub(crate) fn open_file(&self, role: &str, bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, Unreadable> {
        let rest = bytes.strip_prefix(MAGIC.as_slice()).ok_or(Unreadable::NotSealed)?;
        let damaged = || Unreadable::Damaged("its header is cut short".into());
        let (&len, rest) = rest.split_first().ok_or_else(damaged)?;
        let len = usize::from(len);
        if rest.len() < len + 4 {
            return Err(damaged());
        }
        let scope = std::str::from_utf8(&rest[..len]).map_err(|_| Unreadable::Damaged("its scope isn't text".into()))?;
        let version = u32::from_le_bytes(rest[len..len + 4].try_into().expect("four bytes"));
        let key = self.version(scope, version)?;
        let header = &bytes[..MAGIC.len() + 1 + len + 4];
        open_with(&key, &file_aad(header, role), &rest[len + 4..]).ok_or_else(|| Unreadable::Damaged(format!("it doesn't open as {role:?} under {scope:?} version {version} (changed, or sealed by another folder's keys)")))
    }

    /// `plain` sealed as one line (or one value kept in a text) under `scope`, for `role`: no
    /// newline in it.
    pub(crate) fn seal_line(&self, scope: &str, role: &str, plain: &[u8]) -> Result<String, String> {
        let (version, key) = self.current(scope)?;
        Ok(format!("{LINE}{version}.{}", b64(&seal_with(&key, line_aad(scope, version, role).as_bytes(), plain))))
    }

    /// A line sealed under `scope` for `role`, opened.
    pub(crate) fn open_line(&self, scope: &str, role: &str, line: &str) -> Result<Zeroizing<Vec<u8>>, Unreadable> {
        let rest = line.strip_prefix(LINE).ok_or(Unreadable::NotSealed)?;
        let (version, body) = rest.split_once('.').ok_or_else(|| Unreadable::Damaged("no version".into()))?;
        let version: u32 = version.parse().map_err(|_| Unreadable::Damaged("its version isn't a number".into()))?;
        let key = self.version(scope, version)?;
        let sealed = unb64(body.trim_end()).ok_or_else(|| Unreadable::Damaged("not base64url".into()))?;
        open_with(&key, line_aad(scope, version, role).as_bytes(), &sealed)
            .ok_or_else(|| Unreadable::Damaged(format!("it doesn't open as {role:?} under {scope:?} version {version} (changed, or sealed by another folder's keys)")))
    }

    /// `value`'s fingerprint: 8 hex, keyed, so the log can tell which value a run had without
    /// holding it, and a guess can't be checked against it without the keys.
    pub(crate) fn fingerprint(&self, value: &str) -> String {
        blake3::keyed_hash(&self.prints, value.as_bytes()).to_hex()[..8].to_string()
    }

    /// What a content hash is named on disk (§2.6): keyed, 64 lowercase hex.
    #[allow(dead_code)] // the store's names, step 4
    pub(crate) fn name(&self, hash: &[u8; 32]) -> String {
        blake3::keyed_hash(&self.names, hash).to_hex().to_string()
    }
}

/// A whole file's header: magic, the scope's length and name, the version.
fn file_header(scope: &str, version: u32) -> Vec<u8> {
    let scope = scope.as_bytes();
    assert!(scope.len() <= usize::from(u8::MAX), "a scope's name fits its length byte");
    let mut header = Vec::with_capacity(MAGIC.len() + 1 + scope.len() + 4);
    header.extend_from_slice(MAGIC);
    header.push(scope.len() as u8);
    header.extend_from_slice(scope);
    header.extend_from_slice(&version.to_le_bytes());
    header
}

fn file_aad(header: &[u8], role: &str) -> Vec<u8> {
    let mut aad = header.to_vec();
    aad.push(0);
    aad.extend_from_slice(role.as_bytes());
    aad
}

fn line_aad(scope: &str, version: u32, role: &str) -> String {
    format!("a1\0{scope}\0{version}\0{role}")
}

/// `bytes` sealed under `scope` for `role` and written to `path` atomically, owner-only.
pub(crate) fn write_private(keys: &Keyring, path: &Path, scope: &str, role: &str, bytes: &[u8]) -> std::io::Result<()> {
    let sealed = keys.seal_file(scope, role, bytes).map_err(std::io::Error::other)?;
    crate::sealing::write_private(path, &sealed)
}

/// The file at `path`, sealed for `role`, opened: `Ok(None)` if there is none.
pub(crate) fn read(keys: &Keyring, path: &Path, role: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => keys.open_file(role, &bytes).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

/// The server's keyring, for what every store writes: set once, at startup, by `install`, before
/// anything is read or written. A process that installs none (a test, a command that keeps
/// nothing) has one in memory, as the resume tokens' host key is (`durable::HOST_KEY`).
static KEYS: std::sync::OnceLock<std::sync::Arc<Keyring>> = std::sync::OnceLock::new();

/// Make `keys` the server's keyring. An error if one was already in use: something sealed under
/// another before this ran.
pub(crate) fn install(keys: std::sync::Arc<Keyring>) -> Result<(), String> {
    KEYS.set(keys).map_err(|_| "a keyring was in use before the server's opened".to_string())
}

/// The server's keyring.
pub(crate) fn keys() -> &'static std::sync::Arc<Keyring> {
    KEYS.get_or_init(|| std::sync::Arc::new(Keyring::memory()))
}

/// The scope of the deployment a kept file's key names first (`<slug>/<file>`): what it saw of
/// its queues, mailboxes and feeds.
pub(crate) fn deployment_of(key: &Path) -> String {
    deployment(&key.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default())
}

/// What each file is for, bound into it as it is sealed: a file renamed to another role fails to
/// open (§4).
pub(crate) mod role {
    pub(crate) const DEPLOYMENTS: &str = "table: deployments";
    pub(crate) const FLOWS: &str = "table: flows";
    pub(crate) const FLOW_PROPOSALS: &str = "table: flow proposals";
    pub(crate) const SECRETS: &str = "table: secrets";
    pub(crate) const FOLDERS: &str = "table: folders";
    pub(crate) const DATABASES: &str = "table: databases";
    pub(crate) const MAILS: &str = "table: mails";
    pub(crate) const QUEUES: &str = "table: queues";
    pub(crate) const FEEDS: &str = "table: feeds";
    pub(crate) const ACCOUNTS: &str = "table: accounts";
    pub(crate) const ACCESS: &str = "table: access";
    pub(crate) const ACCESS_HISTORY: &str = "access history";
    pub(crate) const QUEUE_SEEN: &str = "seen: queue";
    pub(crate) const MAIL_SEEN: &str = "seen: mail";
    pub(crate) const FEED_SEEN: &str = "seen: feed";
    pub(crate) const ARRIVALS: &str = "arrivals";
    pub(crate) const LOG: &str = "log";
    pub(crate) const EVENTS: &str = "events";
    pub(crate) const CONSOLE: &str = "console";
    pub(crate) const ACTIVITY: &str = "activity";
    pub(crate) const SECRETS_AUDIT: &str = "secrets audit";

    /// A run log's sealed segment: its older runs, gzipped then sealed whole.
    pub(crate) fn segment(slug: &str) -> String {
        format!("segment: {slug}")
    }
}

/// How a line log is sealed (§4, §5.1, §5.2): each line on its own, under `scope`, for `role`, so
/// the file still splits on `\n` and a reader opens each line it reads. Its whole files (a run
/// log's segments) are sealed under the same scope. What it opens is the line as it was made:
/// the chain hashes that, never what is on disk (§2.2).
#[derive(Clone)]
pub(crate) struct LineSeal {
    keys: std::sync::Arc<Keyring>,
    scope: String,
    role: String,
}

impl std::fmt::Debug for LineSeal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LineSeal({:?}, {:?})", self.scope, self.role)
    }
}

impl LineSeal {
    /// Lines under `scope` for `role`, with the server's keyring.
    pub(crate) fn new(scope: &str, role: &str) -> LineSeal {
        LineSeal::with(std::sync::Arc::clone(keys()), scope, role)
    }

    /// Lines under `scope` for `role`, with `keys` (a store that holds its own, the secrets').
    pub(crate) fn with(keys: std::sync::Arc<Keyring>, scope: &str, role: &str) -> LineSeal {
        LineSeal { keys, scope: scope.into(), role: role.into() }
    }

    /// Deployment `slug`'s lines for `role`: its run log, its events, its console lines.
    pub(crate) fn of(slug: &str, role: &str) -> LineSeal {
        LineSeal::new(&deployment(slug), role)
    }

    /// `plain` (one line, no newline) sealed.
    pub(crate) fn seal(&self, plain: &str) -> std::io::Result<String> {
        self.keys.seal_line(&self.scope, &self.role, plain.as_bytes()).map_err(std::io::Error::other)
    }

    /// A line as it is on disk, opened: none if it doesn't (cut short as it was written, changed,
    /// or plain where a sealed one belongs).
    pub(crate) fn open(&self, line: &str) -> Option<String> {
        let plain = self.keys.open_line(&self.scope, &self.role, line).ok()?;
        String::from_utf8(plain.to_vec()).ok()
    }

    /// Each line of `text` (lines ending in `\n`) sealed, `\n` after each.
    pub(crate) fn seal_lines(&self, text: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(text.len() * 3 / 2);
        for line in text.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            let line = std::str::from_utf8(line).map_err(std::io::Error::other)?;
            out.extend_from_slice(self.seal(line)?.as_bytes());
            out.push(b'\n');
        }
        Ok(out)
    }

    /// Each line of `text` that opens, opened, oldest first.
    pub(crate) fn open_lines<'a>(&'a self, text: &'a str) -> impl Iterator<Item = String> + 'a {
        text.lines().filter_map(|l| self.open(l))
    }

    /// The file at `path`'s lines that open, opened: none if there is no file.
    pub(crate) fn read_lines(&self, path: &Path) -> Vec<String> {
        std::fs::read_to_string(path).map(|t| self.open_lines(&t).collect()).unwrap_or_default()
    }

    /// `plain` (one line) sealed and appended to the file at `path`, owner-only.
    pub(crate) fn append(&self, path: &Path, plain: &str) -> std::io::Result<()> {
        use std::io::Write;
        let line = self.seal(plain)? + "\n";
        crate::sealing::append_private(path)?.write_all(line.as_bytes())
    }

    /// A whole file of the same scope (a segment), sealed for `role`.
    pub(crate) fn seal_file(&self, role: &str, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
        self.keys.seal_file(&self.scope, role, bytes).map_err(std::io::Error::other)
    }

    /// A whole file sealed for `role`, opened.
    pub(crate) fn open_file(&self, role: &str, bytes: &[u8]) -> std::io::Result<Zeroizing<Vec<u8>>> {
        self.keys.open_file(role, bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}

/// A table of the server's (`scope` server), sealed for `role` and written to `path` atomically,
/// owner-only.
pub(crate) fn write_table(path: &Path, role: &str, bytes: &[u8]) -> std::io::Result<()> {
    write_private(keys(), path, SERVER, role, bytes)
}

/// `bytes` sealed under `scope` for `role` and written to `path` atomically, owner-only, under
/// the server's keyring.
pub(crate) fn write_sealed(path: &Path, scope: &str, role: &str, bytes: &[u8]) -> std::io::Result<()> {
    write_private(keys(), path, scope, role, bytes)
}

/// The file at `path` sealed for `role`, opened under the server's keyring: `Ok(None)` if there
/// is none; an error if it doesn't open (a plain one where a sealed one belongs included).
pub(crate) fn read_sealed(path: &Path, role: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    read(keys(), path, role)
}

/// For tests: a server table's text as it was sealed (`role` one of [`role`]), opened under the
/// process's keys; panics where there is none or it doesn't open.
#[cfg(test)]
pub(crate) fn opened(path: impl AsRef<Path>, role: &str) -> String {
    let path = path.as_ref();
    let bytes = read_sealed(path, role).unwrap_or_else(|e| panic!("{}: {e}", path.display())).unwrap_or_else(|| panic!("{} is missing", path.display()));
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// For tests that read or change a line log as it is on disk: its lines opened, and written back
/// sealed, so each keeps testing what it tests (a cut line, a changed one, a broken chain).
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// How the line log at `path` is sealed, by its name as the server names it: `<slug>.jsonl` a
    /// run log, `<slug>.events.jsonl` events, `<slug>.console[.n].jsonl` console lines,
    /// `….activity.jsonl` the activity.
    pub(crate) fn seal_of(path: &Path) -> LineSeal {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let slug = name.split('.').next().unwrap_or_default();
        if name.ends_with("activity.jsonl") {
            LineSeal::new(SERVER, role::ACTIVITY)
        } else if name.ends_with(".events.jsonl") {
            LineSeal::of(slug, role::EVENTS)
        } else if name.contains(".console.") {
            LineSeal::of(slug, role::CONSOLE)
        } else {
            LineSeal::of(slug, role::LOG)
        }
    }

    /// The line log at `path`'s lines, opened; one that doesn't open (cut short, or plain) as it
    /// is on disk. None if there is no file.
    pub(crate) fn open_lines(path: &Path) -> Vec<String> {
        let seal = seal_of(path);
        std::fs::read_to_string(path).unwrap_or_default().lines().map(|l| seal.open(l).unwrap_or_else(|| l.to_string())).collect()
    }

    /// The line log at `path` written as `lines`, each sealed, `\n` after each.
    pub(crate) fn write_lines(path: &Path, lines: &[impl AsRef<str>]) {
        let seal = seal_of(path);
        let text: String = lines.iter().map(|l| seal.seal(l.as_ref()).unwrap() + "\n").collect();
        std::fs::write(path, text).unwrap();
    }

    /// `line` as the line log at `path` holds it, sealed (no newline).
    pub(crate) fn sealed(path: &Path, line: &str) -> String {
        seal_of(path).seal(line).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-at-rest-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_arg(dir: &Path, name: &str) -> String {
        format!("file:{}", dir.join(name).display())
    }

    /// Guards: a keyring opens only under the root it was made under: a wrong root, a missing
    /// one, or a keys.json missing beside sealed files each refuse, and nothing is written.
    /// Why: starting on a new keyring beside data sealed under the old one would make that data
    /// unreadable at the first write; starting under the wrong root would too.
    #[test]
    fn a_keyring_opens_only_under_its_root() {
        let dir = temp("root");
        let path = dir.join("keys.json");
        let made = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).unwrap();
        let sealed = made.seal_file(SERVER, "table: test", b"kept").unwrap();
        let line = made.seal_line(&deployment("shop"), "log", b"a run").unwrap();
        let kept = std::fs::read(&path).unwrap();
        // Read again under its root: what it sealed opens, the scope it made on the way too.
        let again = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).unwrap();
        assert_eq!(again.open_file("table: test", &sealed).unwrap().as_slice(), b"kept");
        assert_eq!(again.open_line(&deployment("shop"), "log", &line).unwrap().as_slice(), b"a run");
        assert_eq!(again.key_id(), made.key_id());
        assert_eq!(std::fs::read(&path).unwrap(), kept, "opening writes nothing");
        // Another root, in another key file.
        crate::sealing::KeySource::File(dir.join("other.key")).create().unwrap();
        let wrong = Keyring::open(&path, Some(&root_arg(&dir, "other.key")), &[]).err().unwrap();
        assert!(wrong.contains("different root key"), "{wrong}");
        // No root where it says.
        let missing = Keyring::open(&path, Some(&root_arg(&dir, "absent.key")), &[]).err().unwrap();
        assert!(missing.contains("isn't in"), "{missing}");
        assert!(!dir.join("absent.key").exists(), "no root made beside a keyring that has one");
        assert_eq!(std::fs::read(&path).unwrap(), kept, "nothing written");
        // keys.json gone, data beside it.
        let data = dir.join("deployments.json");
        std::fs::write(&data, b"sealed").unwrap();
        std::fs::remove_file(&path).unwrap();
        let gone = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[&data]).err().unwrap();
        assert!(gone.contains("is missing") && gone.contains("deployments.json"), "{gone}");
        assert!(!path.exists(), "no new keyring made over sealed data");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: a wrapped data key opens only for the scope and version it was made for, and only
    /// in its folder (its key id): one copied to another scope or version refuses to open.
    /// Why: else whoever can write keys.json could make one deployment's key open as another's.
    #[test]
    fn a_wrapped_key_opens_only_for_its_scope_and_version() {
        let dir = temp("wrapped");
        let path = dir.join("keys.json");
        let made = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).unwrap();
        made.seal_line(&deployment("a"), "log", b"x").unwrap();
        let mut file: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let a = file["scopes"]["d:a"]["versions"]["1"].clone();
        // a's key put where b's would be, and as version 2 of a.
        file["scopes"]["d:b"] = serde_json::json!({ "current": 1, "versions": { "1": a.clone() } });
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let moved = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).err().unwrap();
        assert!(moved.contains("\"d:b\""), "{moved}");
        file["scopes"].as_object_mut().unwrap().remove("d:b");
        file["scopes"]["d:a"]["versions"]["2"] = a;
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).is_err(), "another version");
        // Another folder's id.
        file["scopes"]["d:a"]["versions"].as_object_mut().unwrap().remove("2");
        file["key_id"] = serde_json::json!("0123456789abcdef");
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).is_err(), "another folder's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: a record opens only as what it was sealed for: its role, its scope, its version,
    /// unchanged; plain bytes where a sealed record belongs are refused, never read.
    /// Why: a sealed file renamed to another role (a segment put where the deployments table is),
    /// or a line moved to another deployment's log, must not open there; and a plain file planted
    /// in the folder must not be taken for the server's own.
    #[test]
    fn a_value_sealed_for_one_role_does_not_open_as_another() {
        let keys = Keyring::memory();
        let file = keys.seal_file(SERVER, "table: deployments", b"{}").unwrap();
        assert_eq!(keys.open_file("table: deployments", &file).unwrap().as_slice(), b"{}");
        assert!(matches!(keys.open_file("table: flows", &file), Err(Unreadable::Damaged(_))), "another role");
        let mut changed = file.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert!(matches!(keys.open_file("table: deployments", &changed), Err(Unreadable::Damaged(_))), "a changed byte");
        let mut renamed = file.clone();
        renamed[5..5 + SERVER.len()].copy_from_slice(b"secret");
        assert!(keys.open_file("table: deployments", &renamed).is_err(), "another scope named in its header");
        assert_eq!(keys.open_file("table: deployments", b"{\"plain\": true}"), Err(Unreadable::NotSealed));
        assert_eq!(keys.open_file("table: deployments", b""), Err(Unreadable::NotSealed));

        let line = keys.seal_line(&deployment("a"), "log", b"{\"seq\":1}").unwrap();
        assert!(line.starts_with("a1.1.") && !line.contains('\n'), "{line}");
        assert_eq!(keys.open_line(&deployment("a"), "log", &line).unwrap().as_slice(), b"{\"seq\":1}");
        assert!(keys.open_line(&deployment("a"), "events", &line).is_err(), "another role");
        assert!(keys.open_line(&deployment("b"), "log", &line).is_err(), "another deployment's log");
        assert_eq!(keys.open_line(&deployment("a"), "log", "{\"seq\":1}"), Err(Unreadable::NotSealed));
        assert_eq!(keys.open_line(&deployment("a"), "log", &line.replacen("a1.1.", "a1.7.", 1)), Err(Unreadable::NoVersion { scope: "d:a".into(), version: 7 }));
        // Two seals of one value differ (a fresh nonce each), and another keyring opens neither.
        assert_ne!(keys.seal_line(SERVER, "x", b"same").unwrap(), keys.seal_line(SERVER, "x", b"same").unwrap());
        assert!(Keyring::memory().open_file("table: deployments", &file).is_err());
    }

    #[test]
    fn fingerprints_and_names_are_keyed_and_stable() {
        let keys = Keyring::memory();
        assert_eq!(keys.fingerprint("sk_live_1"), keys.fingerprint("sk_live_1"));
        assert_ne!(keys.fingerprint("sk_live_1"), Keyring::memory().fingerprint("sk_live_1"), "keyed");
        let hash = [7u8; 32];
        let name = keys.name(&hash);
        assert!(name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{name}");
        assert_ne!(name, holon_core::hex(hash), "not the hash itself");
    }
}
