//! Where the root key is kept, and the plain file layer under encryption at rest (`at_rest.rs`,
//! docs/L4s/at-rest-encryption-L4.md §3.1).
//!
//! - The root key lives in the OS key store by default (Credential Manager on Windows, the
//!   Keychain on macOS), or comes from an environment variable (`env:VAR`: containers, a cloud
//!   KMS or secret manager injecting it) or a key file (`file:PATH`, owner-only; the default where
//!   there is no OS key store, kept in `~/.holon/keys`, beside the data folder, not in it).
//! - It is named by the id `keys.json` records (`holon` / `root-key-<id>`, `<id>.key`), so the
//!   data folder can be moved, renamed or restored from a backup and still find its root.
//! - What writes a file here writes it owner-only from its first byte: `write_private`,
//!   `write_aside`, `append_private`. `at_rest` seals what goes through them.

use std::io::Write;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

/// A 256-bit key, wiped from memory when dropped.
pub(crate) type MasterKey = Zeroizing<[u8; 32]>;

/// Where key files were kept before `~/.holon/keys`: the user's config directory.
pub(crate) fn old_keys_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("holon").join("keys"))
}

/// A fresh random master key.
pub(crate) fn new_key() -> MasterKey {
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::fill(key.as_mut_slice()).expect("the OS random source failed");
    key
}

/// Where the root key is kept (`--root-key`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum KeySource {
    /// The OS key store, under an entry named for the keyring it opens.
    Os { account: String },
    /// An environment variable holding the key as 64 hex digits; never created here.
    Env(String),
    /// A key file holding 64 hex digits, owner-only; created when there's no keyring yet.
    File(PathBuf),
}

impl KeySource {
    /// `arg` (`os`, `env:VAR` or `file:PATH`), or the default for the keyring whose id is `id`:
    /// the OS key store where there is one, else a key file in `~/.holon/keys`.
    pub(crate) fn parse(arg: Option<&str>, id: &str) -> Result<KeySource, String> {
        match arg {
            Some("os") => Ok(KeySource::Os { account: format!("root-key-{id}") }),
            Some(a) if a.starts_with("env:") && a.len() > 4 => Ok(KeySource::Env(a[4..].to_string())),
            Some(a) if a.starts_with("file:") && a.len() > 5 => Ok(KeySource::File(PathBuf::from(&a[5..]))),
            Some(a) => Err(format!("--root-key {a:?}: expected os, env:VAR or file:PATH")),
            None if cfg!(any(windows, target_os = "macos")) => Ok(KeySource::Os { account: format!("root-key-{id}") }),
            None => {
                let keys = crate::local::holon_home()
                    .map(|h| h.join("keys"))
                    .ok_or("no OS key store here, and no HOME to keep a key file under: pass --root-key env:VAR or file:PATH")?;
                Ok(KeySource::File(keys.join(format!("{id}.key"))))
            }
        }
    }

    /// The key, if it is kept there yet.
    pub(crate) fn read(&self) -> Result<Option<MasterKey>, String> {
        let text: Option<Zeroizing<String>> = match self {
            KeySource::Os { account } => os_store::read(account)?,
            KeySource::Env(var) => std::env::var(var).ok().map(Zeroizing::new),
            KeySource::File(path) => match std::fs::read_to_string(path) {
                Ok(t) => Some(Zeroizing::new(t)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(format!("could not read the key file {path:?}: {e}")),
            },
        };
        let Some(text) = text else { return Ok(None) };
        let bytes = Zeroizing::new(from_hex(text.trim()).filter(|b| b.len() == 32).ok_or_else(|| format!("the key in {self} isn't 64 hex digits"))?);
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&bytes);
        Ok(Some(key))
    }

    /// Keep a new key here. An environment variable can't be written: the operator sets it.
    pub(crate) fn create(&self) -> Result<MasterKey, String> {
        let key = new_key();
        self.keep(&key)?;
        Ok(key)
    }

    /// Keep `key` here, in place of any kept before (a root rotation, at_rest.rs). An environment
    /// variable can't be written: the operator sets it.
    pub(crate) fn keep(&self, key: &MasterKey) -> Result<(), String> {
        let hex = Zeroizing::new(to_hex(key.as_slice()));
        match self {
            KeySource::Os { account } => os_store::write(account, &hex)?,
            KeySource::Env(var) => {
                return Err(format!(
                    "{var} isn't set: set it to a key (holon serve --new-root-key prints one) and keep it safe, apart from the data folder; without it nothing in the folder can be read"
                ));
            }
            KeySource::File(path) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {dir:?}: {e}"))?;
                }
                write_private(path, hex.as_bytes()).map_err(|e| format!("could not write the key file {path:?}: {e}"))?;
            }
        }
        Ok(())
    }

    /// Whether `other` names the same place: a key file however its path is spelled (relative or
    /// not, `.` and `..`, a link, a drive letter's case), else the same entry or variable. What a
    /// rotation in place and `--forget-old` decide by: told apart by spelling, a rotation `--to`
    /// the file in use, written another way, would forget the new root it just kept there.
    pub(crate) fn same_place(&self, other: &KeySource) -> bool {
        match (self, other) {
            (KeySource::File(a), KeySource::File(b)) => a == b || matches!((resolved(a), resolved(b)), (Some(a), Some(b)) if a == b),
            _ => self == other,
        }
    }

    /// The keys files opened under the root kept here, as the file system resolves them: kept
    /// beside the root (`<key file>.keyrings`, or `<account>-keyrings` in the OS key store), so a
    /// rotation in place can tell when another data folder still opens under this root
    /// ([`crate::at_rest`]). None for a variable, which the server can't write. Not a key: paths.
    pub(crate) fn keyrings(&self) -> Result<Vec<PathBuf>, String> {
        let text: Option<String> = match self {
            KeySource::Os { account } => os_store::read(&format!("{account}-keyrings"))?.map(|t| t.to_string()),
            KeySource::Env(_) => return Ok(Vec::new()),
            KeySource::File(path) => match std::fs::read_to_string(keyrings_file(path)) {
                Ok(t) => Some(t),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(format!("could not read {}: {e}", keyrings_file(path).display())),
            },
        };
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default())
    }

    /// [`KeySource::keyrings`] kept as `keyrings`.
    pub(crate) fn keep_keyrings(&self, keyrings: &[PathBuf]) -> Result<(), String> {
        let text = serde_json::to_string(keyrings).map_err(|e| e.to_string())?;
        match self {
            KeySource::Os { account } => os_store::write(&format!("{account}-keyrings"), &text),
            KeySource::Env(_) => Ok(()),
            KeySource::File(path) => write_private(&keyrings_file(path), text.as_bytes()).map_err(|e| format!("could not write {}: {e}", keyrings_file(path).display())),
        }
    }

    /// Drop the key (`uninstall --purge`), and the list of keyrings kept beside it. `Ok(false)`: there was
    /// none here, or it is an environment variable, which only the operator can unset.
    pub(crate) fn forget(&self) -> Result<bool, String> {
        // The list goes with its key; one left behind would only name folders.
        match self {
            KeySource::Os { account } => {
                let _ = os_store::delete(&format!("{account}-keyrings"));
            }
            KeySource::Env(_) => {}
            KeySource::File(path) => {
                let _ = std::fs::remove_file(keyrings_file(path));
            }
        }
        match self {
            KeySource::Os { account } => os_store::delete(account),
            KeySource::Env(_) => Ok(false),
            KeySource::File(path) => match std::fs::remove_file(path) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(format!("could not remove the key file {path:?}: {e}")),
            },
        }
    }
}

/// Where a release from before the server sealed its data (0.1.0-rc.35 and earlier) kept the key
/// of the secrets file at `secrets`, as it chose it with no `--secrets-key`: under the id the file
/// records (written in clear then), else a hash of the file's absolute path; in the OS key store
/// as `secrets-key-<id>`, else as `~/.holon/keys/<id>.key`. None for a file this release sealed,
/// or none there. What `uninstall --purge` removes with the folder, so it isn't left behind.
pub(crate) fn old_secrets_key(secrets: &Path) -> Option<KeySource> {
    let bytes = std::fs::read(secrets).ok()?;
    if crate::at_rest::is_sealed(&bytes) {
        return None;
    }
    let id = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("key_id")?.as_str().map(str::to_string))
        .filter(|id| !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| {
            let absolute = std::path::absolute(secrets).unwrap_or_else(|_| secrets.to_path_buf());
            blake3::hash(absolute.to_string_lossy().as_bytes()).to_hex()[..16].to_string()
        });
    if cfg!(any(windows, target_os = "macos")) {
        Some(KeySource::Os { account: format!("secrets-key-{id}") })
    } else {
        crate::local::holon_home().map(|h| KeySource::File(h.join("keys").join(format!("{id}.key"))))
    }
}

/// Where a key file's keyrings are listed ([`KeySource::keyrings`]): beside it.
fn keyrings_file(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".keyrings");
    PathBuf::from(name)
}

/// Whether `path` is in `dir` or below it, as the file system resolves both: a root key file in
/// the data folder is refused, since a copy of the folder would then open itself.
pub(crate) fn inside(path: &Path, dir: &Path) -> bool {
    match (resolved(path), std::fs::canonicalize(dir)) {
        (Some(p), Ok(d)) => p.starts_with(d),
        _ => false,
    }
}

/// `path` as the file system resolves it: its folder resolved (links, `..`, case) and its name
/// joined, so a key file not made yet still compares; None when the folder isn't there either.
fn resolved(path: &Path) -> Option<PathBuf> {
    if let Ok(p) = std::fs::canonicalize(path) {
        return Some(p);
    }
    let name = path.file_name()?;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    Some(std::fs::canonicalize(parent).ok()?.join(name))
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::Os { account } => write!(f, "the OS key store (holon / {account})"),
            KeySource::Env(var) => write!(f, "the environment variable {var}"),
            KeySource::File(path) => write!(f, "the key file {}", path.display()),
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod os_store {
    use zeroize::Zeroizing;

    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new("holon", account).map_err(|e| format!("the OS key store: {e}"))
    }

    pub(super) fn read(account: &str) -> Result<Option<Zeroizing<String>>, String> {
        match entry(account)?.get_password() {
            Ok(p) => Ok(Some(Zeroizing::new(p))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("could not read the key from the OS key store: {e}")),
        }
    }

    pub(super) fn write(account: &str, hex: &str) -> Result<(), String> {
        entry(account)?.set_password(hex).map_err(|e| format!("could not keep the key in the OS key store: {e}"))
    }

    pub(super) fn delete(account: &str) -> Result<bool, String> {
        match entry(account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(format!("could not remove the key from the OS key store: {e}")),
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod os_store {
    use zeroize::Zeroizing;

    const NONE: &str = "there is no OS key store on this platform: pass --root-key env:VAR or file:PATH";

    pub(super) fn read(_account: &str) -> Result<Option<Zeroizing<String>>, String> {
        Err(NONE.into())
    }

    pub(super) fn write(_account: &str, _hex: &str) -> Result<(), String> {
        Err(NONE.into())
    }

    pub(super) fn delete(_account: &str) -> Result<bool, String> {
        Ok(false)
    }
}

/// Write `bytes` to `path` atomically, readable only by its owner from the first byte: the
/// temporary file is restricted before anything is written to it, and the rename keeps that.
/// `bytes` written beside `path`, owner-only and synced, under a name of its own, and not moved into
/// place: the caller renames it over `path` when it is ready (a log's cut-back, recorder.rs).
pub(crate) fn write_aside(path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".cut-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        restrict_to_owner(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    match result {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// `path` opened for appending, made if it isn't there, for its owner only (`0o600` where the
/// system has modes, an owner-only DACL on Windows): the server's own logs (its activity, the
/// witness's queue and receipts, console lines).
pub(crate) fn append_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let new = !path.exists();
    let file = options.open(path)?;
    if new {
        restrict_to_owner(path)?;
    }
    Ok(file)
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        restrict_to_owner(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        holon_host::sync_dir_of(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Make `path` readable and writable by its owner only (`0o600`, or a protected owner-only DACL on
/// Windows): kept in `holon-host`, whose store writes files too.
pub(crate) use holon_host::restrict_to_owner;
#[cfg(all(test, windows))]
use holon_host::verbatim;

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    holon_core::hex(bytes)
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path past 260 characters is given to Windows' own calls in the form they take at any
    /// length; one already in that form is left as it is.
    #[cfg(windows)]
    #[test]
    fn windows_is_given_a_name_of_any_length() {
        let v = |p: &str| verbatim(Path::new(p)).unwrap().to_string_lossy().into_owned();
        assert_eq!(v(r"C:\data\logs\a.jsonl"), r"\\?\C:\data\logs\a.jsonl");
        assert_eq!(v(r"\\nas\runs\a.jsonl"), r"\\?\UNC\nas\runs\a.jsonl");
        assert_eq!(v(r"\\?\C:\data\a.jsonl"), r"\\?\C:\data\a.jsonl");
        assert!(v("relative.jsonl").starts_with(r"\\?\") && v("relative.jsonl").ends_with(r"\relative.jsonl"));
    }

    #[test]
    fn a_key_file_is_created_once_owner_only_and_read_back() {
        let dir = std::env::temp_dir().join(format!("holon-key-{}", std::process::id()));
        let path = dir.join("nested").join("k.key");
        let source = KeySource::File(path.clone());
        assert!(source.read().unwrap().is_none());
        let key = source.create().unwrap();
        assert_eq!(source.read().unwrap().unwrap().as_slice(), key.as_slice());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&path, "not hex").unwrap();
        assert!(source.read().unwrap_err().contains("64 hex digits"));
        assert!(KeySource::Env("HOLON_TEST_UNSET_KEY_VAR".into()).create().unwrap_err().contains("--new-root-key"));
        assert!(KeySource::parse(Some("vault"), "0123456789abcdef").is_err());
        assert_eq!(KeySource::parse(Some("env:K"), "0123456789abcdef").unwrap(), KeySource::Env("K".into()));
        assert_eq!(KeySource::parse(Some("os"), "0123456789abcdef").unwrap(), KeySource::Os { account: "root-key-0123456789abcdef".into() });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
