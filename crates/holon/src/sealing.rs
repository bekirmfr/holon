//! The secrets store's encryption at rest (§4.2 of docs/PLATFORM_LAYOUT.md): envelope-style, with
//! the master key kept apart from the file it protects, so a copy of the file — a backup, a disk
//! image, a folder zipped up and sent — carries no secret anyone can read.
//!
//! - Each value is sealed with AES-256-GCM under the master key, a fresh random nonce per value,
//!   and the slug, name and version as associated data: a sealed value moved to another
//!   deployment, name or version fails to open instead of answering there.
//! - The master key lives in the OS key store by default (Credential Manager on Windows, the
//!   Keychain on macOS), or comes from an environment variable (`env:VAR`: containers, a cloud
//!   KMS or secret manager injecting it) or a key file (`file:PATH`, owner-only; the default where
//!   there is no OS key store, kept in `~/.holon/keys`, beside the data folder, not in it).
//! - The secrets file records the id its key is kept under (`key_id`), so the file can be moved,
//!   renamed or restored from a backup and still find its key (docs/L4s/holon-home-L4.md §3).
//! - The file records a check value derived from the key (never the key), so a wrong key is
//!   told apart from a damaged file, and neither ever starts the server on an empty store that
//!   would overwrite the real one.

use std::io::Write;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use zeroize::Zeroizing;

/// A 256-bit master key, wiped from memory when dropped.
pub(crate) type MasterKey = Zeroizing<[u8; 32]>;

/// Seals and opens secret values under one master key.
pub(crate) struct Sealer {
    cipher: Aes256Gcm,
    check: String,
}

impl Sealer {
    pub(crate) fn new(key: &MasterKey) -> Sealer {
        let check = blake3::keyed_hash(key, b"holon secrets key check").to_hex()[..16].to_string();
        Sealer { cipher: Aes256Gcm::new(key.as_slice().into()), check }
    }

    /// A sealer under a fresh key that is never stored: an in-memory store's.
    pub(crate) fn ephemeral() -> Sealer {
        Sealer::new(&new_key())
    }

    /// The value the file records to recognise this key by. It reveals nothing about the key.
    pub(crate) fn check(&self) -> &str {
        &self.check
    }

    /// `plain` sealed for `context` (slug, name and version): (nonce, ciphertext), hex.
    pub(crate) fn seal(&self, context: &str, plain: &str) -> (String, String) {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).expect("the OS random source failed");
        let sealed = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain.as_bytes(), aad: context.as_bytes() })
            .expect("AES-GCM sealing can't fail for an in-memory value");
        (to_hex(&nonce), to_hex(&sealed))
    }

    /// The value sealed for `context`, or why it can't be opened.
    pub(crate) fn open(&self, context: &str, nonce: &str, sealed: &str) -> Result<Zeroizing<String>, String> {
        let nonce = from_hex(nonce).filter(|n| n.len() == 12).ok_or("a damaged nonce")?;
        let sealed = from_hex(sealed).ok_or("a damaged sealed value")?;
        let plain = Zeroizing::new(
            self.cipher
                .decrypt(Nonce::from_slice(&nonce), Payload { msg: &sealed, aad: context.as_bytes() })
                .map_err(|_| "it doesn't open under this key for this deployment, name and version")?,
        );
        String::from_utf8(plain.to_vec()).map(Zeroizing::new).map_err(|_| "not text".into())
    }
}

/// What a sealed value is bound to: a value sealed for one place doesn't open in another.
pub(crate) fn context(slug: &str, name: &str, version: u32) -> String {
    format!("holon-secret/v2\0{slug}\0{name}\0{version}")
}

/// The id the key of the secrets file at `secrets_path` is kept under: the one the file records,
/// else [`path_key_id`], which is what a store records when it is made, and what a file from
/// before ids were recorded was always keyed by. Two stores made in two places have two keys.
pub(crate) fn key_id(secrets_path: &str) -> String {
    std::fs::read(secrets_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("key_id")?.as_str().map(str::to_string))
        .filter(|id| !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| path_key_id(secrets_path))
}

/// The id from where the secrets file is: a hash of its absolute path.
pub(crate) fn path_key_id(secrets_path: &str) -> String {
    let absolute = std::path::absolute(secrets_path).unwrap_or_else(|_| PathBuf::from(secrets_path));
    blake3::hash(absolute.to_string_lossy().as_bytes()).to_hex()[..16].to_string()
}

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

/// Where the master key is kept (`--secrets-key`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum KeySource {
    /// The OS key store, under an entry named for the secrets file it protects.
    Os { account: String },
    /// An environment variable holding the key as 64 hex digits; never created here.
    Env(String),
    /// A key file holding 64 hex digits, owner-only; created when there's no store yet.
    File(PathBuf),
}

impl KeySource {
    /// `arg` (`os`, `env:VAR` or `file:PATH`), or the default for `secrets_path`: the OS key
    /// store where there is one, else a key file in `~/.holon/keys`. Either is named by the id
    /// the secrets file records ([`key_id`]).
    pub(crate) fn parse(arg: Option<&str>, secrets_path: &str) -> Result<KeySource, String> {
        let id = key_id(secrets_path);
        match arg {
            Some("os") => Ok(KeySource::Os { account: format!("secrets-key-{id}") }),
            Some(a) if a.starts_with("env:") && a.len() > 4 => Ok(KeySource::Env(a[4..].to_string())),
            Some(a) if a.starts_with("file:") && a.len() > 5 => Ok(KeySource::File(PathBuf::from(&a[5..]))),
            Some(a) => Err(format!("--secrets-key {a:?}: expected os, env:VAR or file:PATH")),
            None if cfg!(any(windows, target_os = "macos")) => Ok(KeySource::Os { account: format!("secrets-key-{id}") }),
            None => {
                let keys = crate::local::holon_home()
                    .map(|h| h.join("keys"))
                    .ok_or("no OS key store here, and no HOME to keep a key file under: pass --secrets-key env:VAR or file:PATH")?;
                let file = keys.join(format!("{id}.key"));
                // One kept where releases before 0.1.0-rc.17 put it, until `holon migrate` moves it.
                let old = old_keys_dir().map(|d| d.join(format!("{id}.key"))).filter(|o| !file.exists() && o.exists());
                Ok(KeySource::File(old.unwrap_or(file)))
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
        let hex = Zeroizing::new(to_hex(key.as_slice()));
        match self {
            KeySource::Os { account } => os_store::write(account, &hex)?,
            KeySource::Env(var) => {
                return Err(format!(
                    "{var} isn't set: set it to a key (holon serve --new-secrets-key prints one) and keep it safe; without it the secrets can't be read"
                ));
            }
            KeySource::File(path) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {dir:?}: {e}"))?;
                }
                write_private(path, hex.as_bytes()).map_err(|e| format!("could not write the key file {path:?}: {e}"))?;
            }
        }
        Ok(key)
    }

    /// Drop the key (`uninstall --purge`). `Ok(false)`: there was none here, or it is an
    /// environment variable, which only the operator can unset.
    pub(crate) fn forget(&self) -> Result<bool, String> {
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

    const NONE: &str = "there is no OS key store on this platform: pass --secrets-key env:VAR or file:PATH";

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
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Make `path` readable and writable by its owner only.
#[cfg(unix)]
pub(crate) fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Make `path` readable and writable by its owner only: a protected DACL (nothing inherited from
/// the folder) granting full control to the owner and to SYSTEM, and nobody else.
#[cfg(windows)]
pub(crate) fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SetFileSecurityW};

    let sddl: Vec<u16> = "D:P(A;;FA;;;OW)(A;;FA;;;SY)".encode_utf16().chain(Some(0)).collect();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: both strings are NUL-terminated and outlive the calls; `sd` is allocated by the
    // first call and freed with LocalFree, as its documentation asks.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let ok = SetFileSecurityW(wide.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, sd);
        let error = std::io::Error::last_os_error();
        LocalFree(sd as _);
        if ok == 0 {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn restrict_to_owner(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

    #[test]
    fn a_sealed_value_opens_only_where_it_was_sealed() {
        let sealer = Sealer::ephemeral();
        let (nonce, sealed) = sealer.seal(&context("api", "key", 1), "sk_live_123456");
        assert!(!sealed.contains(&to_hex(b"sk_live")), "the value isn't in the clear");
        assert_eq!(*sealer.open(&context("api", "key", 1), &nonce, &sealed).unwrap(), "sk_live_123456");
        for elsewhere in [context("other", "key", 1), context("api", "other", 1), context("api", "key", 2)] {
            assert!(sealer.open(&elsewhere, &nonce, &sealed).is_err(), "moved to {elsewhere:?}");
        }
        assert!(Sealer::ephemeral().open(&context("api", "key", 1), &nonce, &sealed).is_err(), "another key");
        let (again, _) = sealer.seal(&context("api", "key", 1), "sk_live_123456");
        assert_ne!(nonce, again, "a fresh nonce each time");
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
        assert!(KeySource::Env("HOLON_TEST_UNSET_KEY_VAR".into()).create().unwrap_err().contains("--new-secrets-key"));
        assert!(KeySource::parse(Some("vault"), "x.json").is_err());
        assert_eq!(KeySource::parse(Some("env:K"), "x.json").unwrap(), KeySource::Env("K".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
