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
//!   copies sealed lines byte for byte (§4). A run log's line is sealed apart from its `seq` and
//!   `prev`, before the log's lock is taken, and they are kept in clear before it
//!   (`r2.<seq>.<prev>.<place>.<v>.<body>`): sealing a whole line under the lock cost lazy runs a
//!   third of their throughput (§7's fallback, measured in docs/engineering/themes/performance.md). The
//!   chain still hashes the whole line as made, so a seq or prev changed on disk breaks it; and
//!   `<place>`, a keyed hash of the seq, the prev and the body's nonce and tag, binds the three
//!   together, so a body moved to another line (the last line's, which no line after it names,
//!   swapped for an earlier one's) doesn't open there (release audit R37).
//! - **A whole file of one deployment's opens only where that deployment's belongs**: its reader
//!   names the scope it expects ([`Keyring::open_file_in`]), not the header alone (K8).
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
use crate::sync::{Mutex, RwLock};

/// The keyring's writes (`keys.json`, a root kept) made to fail one at a time, for the tests that
/// hold a rotation or a forget to leaving an openable folder whichever write fails. Nothing
/// outside the tests.
mod faults {
    #[cfg(test)]
    thread_local! {
        /// Which write from now fails: `Some(1)` the next one; `None`, none.
        static AT: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
    }

    #[cfg(test)]
    pub(super) fn fail_at(n: Option<u32>) {
        AT.with(|at| at.set(n));
    }

    #[cfg(test)]
    pub(super) fn write(what: &str) -> Result<(), String> {
        AT.with(|at| match at.get() {
            Some(1) => {
                at.set(None);
                Err(format!("could not write {what}: a write made to fail"))
            }
            Some(n) => {
                at.set(Some(n - 1));
                Ok(())
            }
            None => Ok(()),
        })
    }

    #[cfg(not(test))]
    #[inline(always)]
    pub(super) fn write(_what: &str) -> Result<(), String> {
        Ok(())
    }
}

/// The data folder made to fail (audit R87). Every way into this file that touches the disk or a
/// key asks [`data_folder`] first, or goes through one that does, and every read of the data
/// folder elsewhere goes through [`disk`], which asks it too (the guards: tests/at_rest.rs,
/// `every_way_into_at_rest_can_be_made_to_fail` and `no_reader_under_the_data_folder_bypasses_the_hook`).
/// So one test fails them all and holds what every route answers, and what a listener notes, to
/// saying neither where nor why (R71's rule: whoever asked is told what failed; where and why go
/// to the admin): tests/run_steps.rs, `no_answer_says_where_or_why_the_data_folder_failed`. Three
/// ways: on a thread, every file and every key ([`FAILING`] in crate::faults); a folder, every
/// file under it, on every thread ([`fail_folder`]): a listener reads what it has seen on a
/// thread of its own; or places alone, for the files under a folder ([`fail_sites`]).
/// For the tests alone: in any other build `data_folder` is `Ok(())` and there is nothing to set,
/// no error to make and no text saying one (`the_data_folder_fails_in_tests_alone`).
#[cfg(test)]
pub(crate) const FAILING: &str = "the data folder";

/// What a failure made in the data folder says: an answer that passes it on is seen, whatever it
/// wraps it in.
#[cfg(test)]
pub(crate) const MADE_TO_FAIL: &str = "the data folder made to fail";

/// Whether the data folder is made to fail for `path` (none: a key, on this thread alone): its
/// error if it is, noted where it was asked from ([`fired`]).
#[cfg(test)]
#[track_caller]
fn data_folder(path: Option<&Path>) -> std::io::Result<()> {
    let at = std::panic::Location::caller();
    let place = || {
        let file = at.file().replace('\\', "/");
        format!("{}:{}", file.rsplit_once("/src/").map_or(file.as_str(), |(_, f)| f), at.line())
    };
    let alone = |p: &Path| {
        let sites = FAILING_SITES.lock().unwrap();
        !sites.is_empty() && {
            let here = place();
            sites.iter().any(|(dir, site)| *site == here && p.starts_with(dir))
        }
    };
    if crate::faults::failing(FAILING).is_some() || path.is_some_and(failing_folder) || path.is_some_and(alone) {
        let what = path.map_or_else(
            || "a key".to_string(),
            |p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let folder = p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let name = if name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()) { format!("{folder}/<hash>") } else { name };
                format!("{name}, {}", if p.exists() { "there" } else { "new" })
            },
        );
        let on = std::thread::current().name().unwrap_or("a thread of its own").to_string();
        FIRED.lock().unwrap().push(Fired { at: place(), what, on });
        return Err(std::io::Error::other(MADE_TO_FAIL));
    }
    Ok(())
}

/// A failure made in the data folder: where the hook was asked from (`file:line`, through disk's
/// readers to their caller), what of (a file's name, or a key), and on which thread.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct Fired {
    pub(crate) at: String,
    pub(crate) what: String,
    pub(crate) on: String,
}

/// The failures made since [`fired`] was last asked, on every thread.
#[cfg(test)]
static FIRED: Mutex<Vec<Fired>> = Mutex::new(Vec::new());

/// The failures made in the data folder since this was last asked, oldest first: what lets a
/// sweep say which hook each answer came from, and which hook no answer did.
#[cfg(test)]
pub(crate) fn fired() -> Vec<Fired> {
    std::mem::take(&mut *FIRED.lock().unwrap())
}

#[cfg(not(test))]
#[inline(always)]
fn data_folder(_path: Option<&Path>) -> std::io::Result<()> {
    Ok(())
}

/// The folders whose files are made to fail, on every thread ([`fail_folder`]).
#[cfg(test)]
static FAILING_FOLDERS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Every file under `dir` fails to be read or written through this file, on every thread, until
/// [`heal_folder`]: each test's data folder is its own, so no other test's is touched.
#[cfg(test)]
pub(crate) fn fail_folder(dir: &Path) {
    FAILING_FOLDERS.lock().unwrap().push(dir.to_path_buf());
}

/// `dir`'s files are read and written again.
#[cfg(test)]
pub(crate) fn heal_folder(dir: &Path) {
    FAILING_FOLDERS.lock().unwrap().retain(|d| d != dir);
}

#[cfg(test)]
fn failing_folder(path: &Path) -> bool {
    FAILING_FOLDERS.lock().unwrap().iter().any(|d| path.starts_with(d))
}

/// Places made to fail alone, each `file:line` as [`fired`] notes it, for the files under a
/// folder, on every thread, until [`heal_sites`]: a read kept in memory, failed from before it is
/// read, or one behind another hook that passes (the sweep's second pass). Only under the folder,
/// so no other test's files fail with them.
#[cfg(test)]
static FAILING_SITES: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

/// Each of `sites` fails for the files under `dir`, on every thread.
#[cfg(test)]
pub(crate) fn fail_sites(dir: &Path, sites: &[String]) {
    FAILING_SITES.lock().unwrap().extend(sites.iter().map(|s| (dir.to_path_buf(), s.clone())));
}

/// `dir`'s sites are read and written again.
#[cfg(test)]
pub(crate) fn heal_sites(dir: &Path) {
    FAILING_SITES.lock().unwrap().retain(|(d, _)| d != dir);
}

/// The data folder read below what this file seals: a run log's lines, a listing, a file another
/// reads whole. Each asks [`data_folder`] for its path, then reads as `std::fs` does, so a read of
/// the data folder anywhere can be made to fail with the rest (R87). Every read under the data
/// folder outside this file is one of these: what may read past them, and why, is the guard's
/// READERS (tests/at_rest.rs).
pub(crate) mod disk {
    use std::path::Path;

    /// A read's error as whoever asked may be told it: [`crate::failing::FILE_UNREAD`], the kind
    /// kept; where and why for the admin (audit R87).
    fn unread(path: &Path, e: std::io::Error) -> std::io::Error {
        crate::failing::Failed::of_file(crate::failing::FILE_UNREAD, path, e)
    }

    /// `std::fs::read`, asking first.
    #[cfg_attr(test, track_caller)]
    pub(crate) fn read(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
        let path = path.as_ref();
        super::data_folder(Some(path)).and_then(|()| std::fs::read(path)).map_err(|e| unread(path, e))
    }

    /// `std::fs::read_to_string`, asking first.
    #[cfg_attr(test, track_caller)]
    pub(crate) fn read_to_string(path: impl AsRef<Path>) -> std::io::Result<String> {
        let path = path.as_ref();
        super::data_folder(Some(path)).and_then(|()| std::fs::read_to_string(path)).map_err(|e| unread(path, e))
    }

    /// `std::fs::File::open`, asking first.
    #[cfg_attr(test, track_caller)]
    pub(crate) fn open(path: impl AsRef<Path>) -> std::io::Result<std::fs::File> {
        let path = path.as_ref();
        super::data_folder(Some(path)).and_then(|()| std::fs::File::open(path)).map_err(|e| unread(path, e))
    }

    /// `std::fs::read_dir`, asking first.
    #[cfg_attr(test, track_caller)]
    pub(crate) fn read_dir(path: impl AsRef<Path>) -> std::io::Result<std::fs::ReadDir> {
        let path = path.as_ref();
        super::data_folder(Some(path)).and_then(|()| std::fs::read_dir(path)).map_err(|e| unread(path, e))
    }
}

/// A 256-bit key, wiped from memory when dropped.
pub(crate) type Key = Zeroizing<[u8; 32]>;

/// What a whole sealed file starts with.
const MAGIC: &[u8; 4] = b"HLA1";
/// What a sealed line starts with.
const LINE: &str = "a1.";
/// What a run log's line sealed apart from its seq and prev starts with: `r2.<seq>.<prev>.<place>.`
/// before its sealed body, `<place>` binding the two to that body (release audit R37).
const NUMBERED: &str = "r2.";
/// How long a numbered line's place is: 16 bytes of a keyed hash, in base64url.
const PLACE: usize = 22;
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

impl std::error::Error for Unreadable {}

impl Unreadable {
    /// Whether it is whole but its key isn't in `keys.json`: never a line cut short as it was
    /// written (that one fails to parse or to open, [`Unreadable::Damaged`]), but a `keys.json`
    /// older than the data, or not the one that goes with it. What was sealed is there still, so
    /// nothing may take it as gone: skipped, numbered again over, or cut.
    pub(crate) fn key_missing(&self) -> bool {
        matches!(self, Unreadable::NoVersion { .. })
    }
}

/// Whether `e` (a [`LineSeal::open_file`]'s) is [`Unreadable::key_missing`].
pub(crate) fn key_missing_in(e: &std::io::Error) -> bool {
    e.get_ref().and_then(|i| i.downcast_ref::<Unreadable>()).is_some_and(Unreadable::key_missing)
}

/// What whoever made keys.json be written (a rotation, a forget, a scope made) is told when it
/// isn't: where and why go to stderr (audit R87).
pub(crate) const KEYS_UNWRITTEN: &str = "the server couldn't write its keys";

/// What whoever needed a scope's key (to seal a line, a secret, a unit) is told when it can't be
/// read or made.
pub(crate) const KEY_UNREAD: &str = "the server couldn't read or make one of its keys";

/// What to do about a record whose key is missing, said once wherever one is found.
pub(crate) const RESTORE_KEYS: &str = "keys.json is older than the data, or isn't the one that goes with this data folder: \
     restore the one that does (a backup folder keeps a copy, holon-archive-keys.json); nothing of it is read or written until then";

/// A listener's sealed record of what it has seen (a mailbox's, a queue's, a feed's, a folder
/// watch's), read: none if there is no file yet. One that is there but doesn't open (a key
/// missing, damage) or doesn't parse is an error, never none: taken as none, the listener would
/// run again on what it ran, or mark as seen what waits unrun, then write over the file.
/// Its error tells what to do, in fixed words: restore keys.json when a key is missing, and
/// otherwise make the file readable or remove it. Where and why are the admin's alone (audit R91):
/// the deployment's events and its live status, which say it, are read by whoever may view the
/// deployment.
pub(crate) fn read_seen<T: serde::de::DeserializeOwned>(path: &Path, scope: &str, role: &str) -> Result<Option<T>, crate::failing::Failed> {
    // Why begins with where, once: `<path>: <why>`, as stderr says it ([`note_seen`] leaves where
    // to the warning's place).
    let unseen = |key_missing: bool, why: String| {
        let todo = if key_missing { format!("A key it was sealed under is missing: {RESTORE_KEYS}") } else { SEEN_TODO.to_string() };
        crate::failing::Failed::reason(todo, why)
    };
    let here = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = match disk::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // disk's error: why, for the admin, is the Failed's it holds, which says where already.
        Err(e) => return Err(unseen(false, crate::failing::Failed::in_io(&e).map_or_else(|| here(&e), |f| f.for_the_admin().to_string()))),
    };
    let opened = keys().open_file_in(Some(scope), role, &bytes).map_err(|u| unseen(u.key_missing(), here(&u)))?;
    serde_json::from_slice(&opened).map(Some).map_err(|e| unseen(false, here(&e)))
}

/// What to do about a listener's record that doesn't open, but for a key missing.
const SEEN_TODO: &str = "The server's warnings say which file and why: make it readable again, or remove it to start over, \
     which takes what is there now as seen";

/// What a listener says when what it has seen doesn't open ([`read_seen`]): it is held, in fixed
/// words; why is kept for stderr and the warnings ([`note_seen`]).
pub(crate) fn seen_held(what: &str, e: crate::failing::Failed) -> crate::failing::Failed {
    e.retold(|todo| format!("what it has seen of {what} can't be read: it runs nothing and marks nothing seen until it can. {todo}"))
}

/// What the admin's warnings say of a listener's record of what it has seen that doesn't open
/// (failing::note_file), its file's path the place.
pub(crate) const SEEN_UNREAD: &str = "a listener's record of what it has seen can't be read";

/// Note in the admin's warnings how `file`'s read went, for the server whose deployments file is
/// `at`: held, or read (the warning goes). The file's path is the warning's place, so why goes
/// without it ([`read_seen`]'s begins with it). A record kept in memory has nothing to note.
pub(crate) fn note_seen(at: &crate::paths::Place, file: Option<&Path>, result: Result<(), &crate::failing::Failed>) {
    let Some(file) = file else { return };
    let place = file.display().to_string();
    match result {
        Ok(()) => crate::failing::note_file(at, SEEN_UNREAD, &place, Ok(())),
        Err(held) => {
            let why = held.for_the_admin();
            let why = why.strip_prefix(&format!("{place}: ")).unwrap_or(why);
            crate::failing::note_file(at, SEEN_UNREAD, &place, Err(&crate::failing::Failed::new(SEEN_UNREAD, why)));
        }
    }
}

/// The records under `dir` are gone (their deployment is): their warnings go with them.
pub(crate) fn seen_forgotten(at: &crate::paths::Place, dir: &Path) {
    for (told, place, _) in crate::failing::files_now(at) {
        if told == SEEN_UNREAD && Path::new(&place).starts_with(dir) {
            crate::failing::note_file(at, told, &place, Ok(()));
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
    /// The scopes forgotten, when, and through which version: a record sealed under one of those
    /// versions says so. The slug deployed again makes its scope anew, from the next version.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    forgotten: BTreeMap<String, Forgot>,
    /// A root rotation under way: every data key wrapped under the next root too, written before
    /// that root is kept, so a rotation cut short opens under either root ([`Keyring::open`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next: Option<Staged>,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct Forgot {
    at_ms: u64,
    through: u32,
}

#[derive(Serialize, Deserialize, Clone)]
struct Staged {
    root_check: String,
    /// Each scope's versions, wrapped under the next root (base64url, as [`Wrapped`]).
    scopes: BTreeMap<String, BTreeMap<u32, String>>,
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
    /// Where the root is kept; none for a keyring kept in memory only.
    source: Option<KeySource>,
    /// A memory keyring's root: never written, so what it sealed is gone with the server.
    memory_root: Option<Key>,
}

/// The keys the server seals and opens with: shared, as the store is.
pub(crate) struct Keyring {
    /// Where `keys.json` is; none for a keyring kept in memory only.
    path: Option<PathBuf>,
    inner: RwLock<Inner>,
    /// What the store's file names are keyed by (§2.6).
    names: Key,
    /// What a secret value's fingerprint is keyed by.
    prints: Key,
    /// Opened only to read (`holon verify`, `holon key export`): nothing is written, not even a
    /// rotation cut short finished ([`Keyring::open_to_read`]).
    read_only: bool,
    /// Made by this open, its root with it: the server says where that root is kept.
    made: bool,
    /// The backup folder `keys.json` was last copied to ([`Keyring::keep_copy_in`]): each write
    /// of `keys.json` after is copied there in the same step, so the copy never opens what a
    /// forget or a root rotation has just put out of reach (release audit R5).
    backup: Mutex<Option<PathBuf>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// base64url, unpadded, as `holon_core::bytes_text(_, "base64url")` writes it, by table: every
/// sealed line and value goes through these, under a log's lock, and the core's decoder looks
/// each character up in its alphabet one by one (several times slower on a 5 KB line: measured in
/// docs/engineering/themes/performance.md, encryption at rest). The test holds the two to one another.
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Each byte's place in [`B64`], or 0xff for one that isn't in it.
const UNB64: [u8; 256] = {
    let mut t = [0xffu8; 256];
    let mut i = 0;
    while i < 64 {
        t[B64[i] as usize] = i as u8;
        i += 1;
    }
    t
};

fn b64(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    b64_into(bytes, &mut out);
    String::from_utf8(out).expect("base64url is ASCII")
}

/// `bytes` in base64url, unpadded, written at the end of `out`.
fn b64_into(bytes: &[u8], out: &mut Vec<u8>) {
    out.reserve(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for c in &mut chunks {
        let n = u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]);
        out.extend_from_slice(&[B64[(n >> 18) as usize & 63], B64[(n >> 12) as usize & 63], B64[(n >> 6) as usize & 63], B64[n as usize & 63]]);
    }
    match *chunks.remainder() {
        [a] => out.extend_from_slice(&[B64[usize::from(a >> 2)], B64[usize::from(a & 3) << 4]]),
        [a, b] => out.extend_from_slice(&[B64[usize::from(a >> 2)], B64[usize::from(a & 3) << 4 | usize::from(b >> 4)], B64[usize::from(b & 15) << 2]]),
        _ => {}
    }
}

/// The bytes `text` encodes, unpadded base64url; none for any other character, a length no
/// encoding has, or leftover bits that aren't zero (one text, one decoding, as the core's).
fn unb64(text: &str) -> Option<Vec<u8>> {
    let s = text.as_bytes();
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 2);
    let mut chunks = s.chunks_exact(4);
    for c in &mut chunks {
        let (a, b, d, e) = (UNB64[c[0] as usize], UNB64[c[1] as usize], UNB64[c[2] as usize], UNB64[c[3] as usize]);
        if (a | b | d | e) & 0xc0 != 0 {
            return None;
        }
        let n = u32::from(a) << 18 | u32::from(b) << 12 | u32::from(d) << 6 | u32::from(e);
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
    }
    let rest: Vec<u8> = chunks.remainder().iter().map(|c| UNB64[*c as usize]).collect();
    if rest.iter().any(|v| v & 0xc0 != 0) {
        return None;
    }
    match *rest.as_slice() {
        [a, b] if b & 15 == 0 => out.push(a << 2 | b >> 4),
        [a, b, d] if d & 3 == 0 => out.extend_from_slice(&[a << 2 | b >> 4, (b & 15) << 4 | d >> 2]),
        [] => {}
        _ => return None,
    }
    Some(out)
}

fn root_check(root: &Key) -> String {
    blake3::keyed_hash(root, b"holon root key check").to_hex()[..16].to_string()
}

/// Whether the keys file at `path` still opens under the root whose check is `check` (made under
/// it, or in a rotation to it), as far as can be told: one gone, or under another root, doesn't;
/// one that can't be read is counted as opening, so a rotation never guesses it away.
fn opens_under(path: &Path, check: &str) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<KeysFile>(&bytes).map_or(true, |f| f.root_check == check || f.next.is_some_and(|n| n.root_check == check)),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
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
/// What a numbered line's place is keyed by, derived from the key its body is sealed with.
const PLACE_CONTEXT: &str = "holon at-rest v1 a run log line's place";

/// Where a run log's line is (its seq and the hash of the line before), bound to the body sealed
/// apart from them, as the hash `place` begins: by the body's nonce and tag, which no other
/// sealing has, under a key derived from its own. Its body moved to another line (the last line's
/// body swapped for an earlier one's, which the chain alone wouldn't show, no line naming the
/// last) no longer opens there (release audit R37). Made before the log's lock is taken, so under
/// it only the seq and prev are hashed in, a few dozen bytes whatever the line's length.
fn place_of(key: &Key, sealed: &[u8]) -> blake3::Hasher {
    let mut place = blake3::Hasher::new_keyed(&derive(PLACE_CONTEXT, key.as_slice()));
    place.update(&sealed[..NONCE]);
    place.update(&sealed[sealed.len() - 16..]);
    place
}

/// `bytes` (a place) in base64url into `out`, made nowhere else: under a log's lock.
fn b64_to(bytes: &[u8; 16], out: &mut [u8; PLACE]) {
    let mut at = 0;
    for c in bytes.chunks(3) {
        let n = u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..=c.len() {
            out[at] = B64[(n >> (18 - 6 * i)) as usize & 63];
            at += 1;
        }
    }
}

/// The place of a line at `seq` after `prev`, from what [`place_of`] began.
fn place_at(mut place: blake3::Hasher, seq: u64, prev: &str) -> [u8; 16] {
    place.update(&seq.to_le_bytes());
    place.update(prev.as_bytes());
    place.finalize().as_bytes()[..16].try_into().expect("16 bytes")
}
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
        Keyring::open_as(path, root_arg, sealed, false)
    }

    /// The keyring at `path` opened to read only, as [`Keyring::open`] but writing nothing: none
    /// made where there is none, and a rotation cut short finished in memory, not on disk. For
    /// what reads a folder a server may be running on, without its lock (`holon verify`, `holon
    /// key export`): a write here could undo a rotation the server is making.
    pub(crate) fn open_to_read(path: &Path, root_arg: Option<&str>) -> Result<Keyring, String> {
        Keyring::open_as(path, root_arg, &[], true)
    }

    fn open_as(path: &Path, root_arg: Option<&str>, sealed: &[&Path], read_only: bool) -> Result<Keyring, String> {
        let file: Option<KeysFile> = match std::fs::read(path) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(|e| format!("{} is not a keys file: {e}", path.display()))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("could not read {}: {e}", path.display())),
        };
        let Some(file) = file else {
            if read_only {
                return Err(format!("{} isn't there: this folder has no keys to read with", path.display()));
            }
            if let Some(there) = sealed.iter().find(|p| p.exists()) {
                return Err(match sealed.iter().find(|p| written_in_clear(p)) {
                    // Nothing to restore: no keys.json was ever made for it.
                    Some(clear) => format!(
                        "{} is written in clear: a release from before the server sealed its data (0.1.0-rc.35 or earlier) made this \
                         folder, and this one reads none of it, nor converts it (holon migrate doesn't either). Keep running that \
                         release on it; or move the folder aside and start over, which leaves its accounts, secrets, state and runs \
                         behind (what that release kept in the OS key store, holon / secrets-key-<id>, can then be removed)",
                        clear.display()
                    ),
                    None => format!(
                        "{} is missing, and {} is there, sealed under its keys: nothing in the folder opens without it. Restore the \
                         keys.json that goes with this folder (a backup folder keeps a copy, holon-archive-keys.json); moving the \
                         folder aside to start over leaves everything in it behind",
                        path.display(),
                        there.display()
                    ),
                });
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
        let mut file = file;
        // A root rotation cut short: finished under the next root if that is the one kept, else
        // undone (the next root was never kept, or the old one is still passed).
        let cut_short = file.next.is_some();
        if root_check(&root) != file.root_check {
            match file.next.take() {
                Some(next) if next.root_check == root_check(&root) => promote(&mut file, next),
                _ => return Err(format!("{source} holds a different root key from the one {} was made under", path.display())),
            }
        }
        file.next = None;
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
        let keyring = Keyring {
            path: Some(path.to_path_buf()),
            inner: RwLock::new(Inner { file, open, source: Some(source), memory_root: None }),
            names,
            prints,
            read_only,
            made: false,
            backup: Mutex::new(None),
        };
        if cut_short && !read_only {
            keyring.persist(&keyring.inner.read().unwrap().file)?;
        }
        if !read_only {
            keyring.register();
        }
        Ok(keyring)
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
        let mut keyring = Keyring::fresh(&root, key_id, Some(path.to_path_buf()), Some(source), None);
        keyring.persist(&keyring.inner.read().unwrap().file)?;
        keyring.made = true;
        keyring.register();
        Ok(keyring)
    }

    /// A keyring kept in memory only, under a root that is never written: a server or a test
    /// that keeps nothing.
    pub(crate) fn memory() -> Keyring {
        let root = new_key();
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).expect("the OS random source failed");
        Keyring::fresh(&root, holon_core::hex(id), None, None, Some(root.clone()))
    }

    fn fresh(root: &Key, key_id: String, path: Option<PathBuf>, source: Option<KeySource>, memory_root: Option<Key>) -> Keyring {
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
        Keyring { path, inner: RwLock::new(Inner { file, open, source, memory_root }), names, prints, read_only: false, made: false, backup: Mutex::new(None) }
    }

    /// A new data key for `scope` at `version`, and it wrapped under `root`.
    fn new_version(root: &Key, key_id: &str, scope: &str, version: u32) -> (Wrapped, Key) {
        let data = new_key();
        let wrapped = seal_with(root, wrap_context(key_id, scope, version).as_bytes(), data.as_slice());
        (Wrapped { wrapped: b64(&wrapped), made_ms: now_ms() }, data)
    }

    /// This keys file listed beside its root ([`KeySource::keyrings`]), and those no longer under
    /// it dropped: what lets a rotation in place see another folder still opened by this root (a
    /// copy of the folder, or one given the same `--root-key`). Best effort: a root kept where the
    /// server can't write (a mounted secret) lists nothing, and a rotation there can't tell.
    fn register(&self) {
        let inner = self.inner.read().unwrap();
        let (Some(path), Some(source)) = (&self.path, &inner.source) else { return };
        if matches!(source, KeySource::Env(_)) {
            return;
        }
        let Ok(me) = crate::paths::canonical(path) else { return };
        let Ok(listed) = source.keyrings() else { return };
        let mut kept: Vec<PathBuf> = listed.iter().filter(|p| **p != me && opens_under(p, &inner.file.root_check)).cloned().collect();
        kept.push(me);
        kept.sort();
        if kept != listed {
            let _ = source.keep_keyrings(&kept);
        }
    }

    /// The other keys files the root in use still opens, as listed beside it
    /// ([`Keyring::register`]): a rotation in place would leave each opened by no root, and
    /// forgetting the old root after a rotation elsewhere would too.
    pub(crate) fn others_on_root(&self) -> Result<Vec<PathBuf>, String> {
        let inner = self.inner.read().unwrap();
        self.others_under(&inner)
    }

    fn others_under(&self, inner: &Inner) -> Result<Vec<PathBuf>, String> {
        let Some(source) = inner.source.as_ref().filter(|s| !matches!(s, KeySource::Env(_))) else { return Ok(Vec::new()) };
        let me = self.path.as_ref().and_then(|p| crate::paths::canonical(p).ok());
        Ok(source.keyrings()?.into_iter().filter(|p| Some(p) != me.as_ref() && opens_under(p, &inner.file.root_check)).collect())
    }

    /// `keys.json` written, owner-only; nothing for a keyring in memory.
    fn persist(&self, file: &KeysFile) -> Result<(), String> {
        let unwritten = |why: String| crate::failing::Failed::new(KEYS_UNWRITTEN, why).said();
        let Some(path) = &self.path else { return data_folder(None).map_err(|e| unwritten(format!("keys.json: {e}"))) };
        if self.read_only {
            return Err(format!("{} was opened to read only, and isn't written", path.display()));
        }
        faults::write("keys.json")?;
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
        data_folder(Some(path)).and_then(|()| crate::sealing::write_private(path, &bytes)).map_err(|e| unwritten(format!("{}: {e}", path.display())))?;
        // The backup's copy follows at once. Left to the archiver's next look, for a second or
        // so after a forget the copy still opened the forgotten deployment's archives with the
        // old root. One that can't be written is the archiver's to retry and to say.
        if let Some(dir) = self.backup.lock().unwrap().as_ref() {
            let _ = faults::write("the backup's copy of keys.json").and_then(|()| crate::sealing::write_private(&dir.join(BACKUP_KEYS), &bytes).map_err(|e| e.to_string()));
        }
        Ok(())
    }

    /// The root, read again where it is kept (and checked): to make a scope or a version.
    fn root(&self, inner: &Inner) -> Result<Key, String> {
        if let Some(root) = &inner.memory_root {
            return Ok(root.clone());
        }
        let (path, source) = (self.path.as_deref().unwrap_or(Path::new("keys.json")), inner.source.as_ref().expect("a keyring is kept or in memory"));
        let root = source.read()?.ok_or_else(|| format!("the root key that opens {} isn't in {source} any more", path.display()))?;
        if root_check(&root) != inner.file.root_check {
            return Err(format!("{source} now holds a different root key from the one {} was made under", path.display()));
        }
        Ok(root)
    }

    /// The id the root is kept under.
    pub(crate) fn key_id(&self) -> String {
        self.inner.read().unwrap().file.key_id.clone()
    }

    /// Where the root is kept, if anywhere.
    pub(crate) fn source(&self) -> Option<KeySource> {
        self.inner.read().unwrap().source.clone()
    }

    /// Whether this open made the keyring, and its root with it: the server then says where
    /// that root is kept, and that it is backed up apart from the data folder.
    pub(crate) fn made(&self) -> bool {
        self.made
    }

    /// `keys.json` copied into `dir`, a backup folder (storage.rs), as [`BACKUP_KEYS`], when the
    /// copy there isn't the same: what the archives there are sealed under, so the backup folder
    /// and the root alone open them once the data folder is lost. It is safe there as it is in
    /// the data folder: every key in it is wrapped under the root. Read under the keyring's lock,
    /// so never half of a write. `Ok(false)`: the copy was the same, or this keyring is kept in
    /// memory only and has no file.
    pub(crate) fn keep_copy_in(&self, dir: &Path) -> Result<bool, String> {
        let Some(path) = &self.path else { return data_folder(None).map(|()| false).map_err(|e| format!("could not read keys.json: {e}")) };
        let _held = self.inner.read().unwrap();
        *self.backup.lock().unwrap() = Some(dir.to_path_buf());
        let bytes = disk::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let to = dir.join(BACKUP_KEYS);
        if disk::read(&to).is_ok_and(|kept| kept == bytes) {
            return Ok(false);
        }
        faults::write("the backup's copy of keys.json")?;
        crate::sealing::write_private(&to, &bytes).map_err(|e| format!("could not write {}: {e}", to.display()))?;
        Ok(true)
    }

    /// No backup folder any more: `keys.json`'s writes are copied nowhere.
    pub(crate) fn no_backup(&self) {
        *self.backup.lock().unwrap() = None;
    }

    /// A copy of the root, the one this keyring opens under (checked), kept in `to` (`holon key
    /// export`): the one way to back up a root kept in the OS key store, which shows no one its
    /// keys. `to` must hold no key yet, and not be where the root is kept. Nothing of the key is
    /// shown or returned.
    pub(crate) fn export_root(&self, to: &KeySource) -> Result<(), String> {
        let inner = self.inner.read().unwrap();
        let root = self.root(&inner)?;
        if matches!(to, KeySource::Env(_)) {
            return Err("an environment variable can't be written: export the root to a file (file:PATH)".into());
        }
        if inner.source.as_ref().is_some_and(|s| s.same_place(to)) {
            return Err(format!("{to} is where the root key is kept already: export it somewhere else, apart from the data folder"));
        }
        if to.read()?.is_some() {
            return Err(format!("{to} holds a key already: an export never writes over one"));
        }
        faults::write("the root key's copy")?;
        to.keep(&root)
    }

    /// `scope`'s current version and its key, the scope made (and `keys.json` written) if it is
    /// new. A forgotten scope is made anew from the version after the last one forgotten (its slug
    /// deployed again), so what was sealed before stays forgotten.
    fn current(&self, scope: &str) -> Result<(u32, Key), String> {
        // What fails here reaches whoever sealed (a run's line, a secret, a unit stored): told
        // in fixed words, where and why on stderr (audit R87).
        let unread = |why: String| crate::failing::Failed::new(KEY_UNREAD, format!("{scope:?}'s key: {why}")).said();
        data_folder(None).map_err(|e| unread(e.to_string()))?;
        {
            let inner = self.inner.read().unwrap();
            if let Some(keys) = inner.open.get(scope) {
                return Ok((keys.current, keys.versions[&keys.current].clone()));
            }
        }
        let mut inner = self.inner.write().unwrap();
        if let Some(keys) = inner.open.get(scope) {
            return Ok((keys.current, keys.versions[&keys.current].clone()));
        }
        let version = inner.file.forgotten.get(scope).map_or(1, |f| f.through + 1);
        let key = self.add_version(&mut inner, scope, version).map_err(unread)?;
        Ok((version, key))
    }

    /// `scope`'s keys made now if it has none (`keys.json` written): a deployment's, before it goes
    /// live, so that nothing it runs is the first to need the root, and none fails to be sealed
    /// for a root that can't be read then (release audit F17). The tests fail it through the data
    /// folder's hook, which `current` asks first: whether the scope was made before or not, so a
    /// test doesn't lean on the shared keyring's state (audit R87).
    pub(crate) fn make_scope(&self, scope: &str) -> Result<(), String> {
        self.current(scope).map(|_| ())
    }

    /// A new data key for `scope` at `version`, made its current: written to `keys.json` before it
    /// is used, so nothing is sealed under a key keys.json doesn't keep.
    fn add_version(&self, inner: &mut Inner, scope: &str, version: u32) -> Result<Key, String> {
        let root = self.root(inner)?;
        let (wrapped, data) = Keyring::new_version(&root, &inner.file.key_id, scope, version);
        let mut file = inner.file.clone();
        let kept = file.scopes.entry(scope.to_string()).or_default();
        kept.current = version;
        kept.versions.insert(version, wrapped);
        self.persist(&file)?;
        inner.file = file;
        let key = derive(SEAL_CONTEXT, data.as_slice());
        let open = inner.open.entry(scope.to_string()).or_default();
        open.current = version;
        open.versions.insert(version, key.clone());
        Ok(key)
    }

    /// The key of `scope` at `version`, or why there is none.
    fn version(&self, scope: &str, version: u32) -> Result<Key, Unreadable> {
        // As a key that isn't there fails: what a caller takes for a lost key (key_missing).
        data_folder(None).map_err(|e| Unreadable::NoVersion { scope: format!("{scope} ({e})"), version })?;
        let inner = self.inner.read().unwrap();
        if let Some(key) = inner.open.get(scope).and_then(|k| k.versions.get(&version)) {
            return Ok(key.clone());
        }
        match inner.file.forgotten.get(scope) {
            Some(f) if version <= f.through => Err(Unreadable::Forgotten { scope: scope.into(), at_ms: f.at_ms }),
            _ => Err(Unreadable::NoVersion { scope: scope.into(), version }),
        }
    }

    /// A new version of `scope`'s data key, sealing from the next write on (`holon key rotate
    /// <scope>`); the versions before it stay, to open what they sealed. Never `names` (§2.6).
    pub(crate) fn rotate(&self, scope: &str) -> Result<u32, String> {
        if scope == NAMES {
            return Err(
                "the names key is never rotated: the store's files are named by it, so a new one would lose every value stored before it and store each again (docs/L4s/at-rest-encryption-L4.md §2.6)"
                    .into(),
            );
        }
        let mut inner = self.inner.write().unwrap();
        let Some(kept) = inner.file.scopes.get(scope) else {
            return Err(if inner.file.forgotten.contains_key(scope) {
                format!("the keys of {scope:?} were forgotten")
            } else {
                format!("no keys for {scope:?}: holon key status lists the scopes")
            });
        };
        let version = kept.versions.keys().max().copied().unwrap_or(0) + 1;
        self.add_version(&mut inner, scope, version)?;
        Ok(version)
    }

    /// Forget a deployment's scope (`d:<slug>`): its data keys gone from `keys.json`, so nothing
    /// sealed under them opens on this server again, in the data folder, the backup folder or the
    /// removed records ([`Unreadable::Forgotten`]). Not the content store, sealed under the
    /// server's keys (`keys::forget` sweeps it). `Ok(false)`: nothing was ever sealed under it.
    /// A copy of the whole folder taken before still holds them, wrapped under the root: rotating
    /// the root after this ([`Keyring::rotate_root`]) is what puts that copy out of reach.
    pub(crate) fn forget(&self, scope: &str) -> Result<bool, String> {
        if !scope.starts_with("d:") {
            return Err(format!("only a deployment's keys are forgotten, not {scope:?}'s"));
        }
        #[cfg(test)]
        self.forgets_only_a_tests_own(scope);
        let mut inner = self.inner.write().unwrap();
        let Some(kept) = inner.file.scopes.get(scope) else { return Ok(false) };
        let through = kept.versions.keys().max().copied().unwrap_or(0);
        let mut file = inner.file.clone();
        file.scopes.remove(scope);
        file.forgotten.insert(scope.to_string(), Forgot { at_ms: now_ms(), through });
        self.persist(&file)?;
        inner.file = file;
        inner.open.remove(scope);
        Ok(true)
    }

    /// Under test, the keyring every test shares ([`keys`]: every deployment seals under it)
    /// forgets keys only of a slug one test alone uses ([`own_slug`]); any other keyring, any
    /// slug. Tests run at once in one process: a forget of a slug another test deploys too leaves
    /// that test's lines sealed before it unopenable, and a read of its log leaves them out
    /// (`recorder::Lines::each`), so it finds fewer runs than it made, now and then. That bit the
    /// removed-deployment test, on "gone", forgotten by the forget-sweep test meanwhile. A forget
    /// of nothing held forgets nothing, and is let be.
    #[cfg(test)]
    fn forgets_only_a_tests_own(&self, scope: &str) {
        let shared = std::ptr::eq(self, keys().as_ref());
        let held = self.inner.read().unwrap().file.scopes.contains_key(scope);
        let own = scope.strip_prefix("d:").is_some_and(|slug| OWN_SLUGS.lock().unwrap().contains(slug));
        assert!(
            !shared || !held || own,
            "a test forgot {scope:?} on the keyring every test shares: forget only a slug of its own (at_rest::own_slug), or on a keyring of its own"
        );
    }

    /// Every data key wrapped under a new root, kept in `to` (in place when none): the old root
    /// then opens nothing of this folder. It goes in three writes, so one cut short loses nothing:
    /// the keys wrapped under the new root are written beside the old (`next`), the new root is
    /// kept, the old wraps are dropped; [`Keyring::open`] finishes a rotation cut short under the
    /// new root, or undoes it under the old. An environment variable can't be written: `to` one
    /// must hold the new root already. Anywhere else but in place, `to` must hold no key yet: a
    /// rotation never writes over another folder's root, or any key it didn't make. The old root
    /// stays where it was kept unless that is `to`: forgetting it there is the caller's (`holon
    /// key rotate --root --forget-old`).
    ///
    /// `Ok(Some(why))`: rotated, the new root kept and in use, but the last write of `keys.json`
    /// failed (`why`): the file on disk still holds the new wraps staged, which the next start
    /// finishes under the new root, and the next write here writes them whole. Never `Err` once
    /// the new root is kept: from then on only the new root opens this folder, and saying "not
    /// rotated" would send the operator looking for an old root that is gone.
    pub(crate) fn rotate_root(&self, to: Option<&KeySource>) -> Result<Option<String>, String> {
        self.rotate_root_until(to, true)
    }

    /// [`Keyring::rotate_root`], stopped after the new root is kept when `finish` is false: a
    /// rotation cut short, for the tests.
    fn rotate_root_until(&self, to: Option<&KeySource>, finish: bool) -> Result<Option<String>, String> {
        let mut inner = self.inner.write().unwrap();
        let old = self.root(&inner)?;
        let in_place = match (to, &inner.source) {
            (None, _) => true,
            (Some(to), Some(now)) => to.same_place(now),
            (Some(_), None) => false,
        };
        if in_place && let Some(other) = self.others_under(&inner)?.first() {
            return Err(format!(
                "{} opens under this root too (a copy of this data folder, or another folder given the same root key): a new root \
                 kept in its place would leave that folder opened by none. Give that folder a root of its own first (holon key \
                 rotate --root --data <its folder> --to file:PATH), or delete it if it is no longer used; then rotate again",
                other.display()
            ));
        }
        let to = to.cloned().or_else(|| inner.source.clone());
        let new = match &to {
            Some(source @ KeySource::Env(var)) => source.read()?.ok_or_else(|| format!("{var} isn't set: set it to the new root key (holon serve --new-root-key prints one), then rotate"))?,
            Some(source) if !in_place && source.read()?.is_some() => {
                return Err(format!(
                    "{source} holds a key already: a new root is kept only where none is, so a rotation never writes over another folder's root \
                     or a key it didn't make. Pick a place that holds none; if it is left from a rotation cut short, start the server with \
                     --root-key set to it, which finishes that rotation"
                ));
            }
            _ => new_key(),
        };
        if root_check(&new) == inner.file.root_check {
            return Err("that is the root key in use already: a rotation needs a new one".into());
        }
        let mut staged = Staged { root_check: root_check(&new), scopes: BTreeMap::new() };
        for (scope, kept) in &inner.file.scopes {
            let mut versions = BTreeMap::new();
            for (version, wrapped) in &kept.versions {
                let context = wrap_context(&inner.file.key_id, scope, *version);
                let data = unb64(&wrapped.wrapped)
                    .and_then(|w| open_with(&old, context.as_bytes(), &w))
                    .ok_or_else(|| format!("the key of {scope:?} version {version} doesn't open under the root"))?;
                versions.insert(*version, b64(&seal_with(&new, context.as_bytes(), &data)));
            }
            staged.scopes.insert(scope.clone(), versions);
        }
        let mut file = inner.file.clone();
        file.next = Some(staged);
        self.persist(&file)?;
        // Kept failing, the old root still opens everything: the staged wraps are dropped on the
        // next write, or the next open.
        if let Some(source) = to.as_ref().filter(|s| !matches!(s, KeySource::Env(_))) {
            faults::write("the root key")?;
            source.keep(&new)?;
        }
        if inner.memory_root.is_some() {
            inner.memory_root = Some(new);
        }
        if !finish {
            inner.file = file;
            inner.source = to;
            return Ok(None);
        }
        // The new root is kept: from here the keyring in use is the rotated one, whatever the last
        // write does. Left on the old file (no `next`, the old check) a later write here (a forget,
        // a scope made) would drop the staged wraps from disk, and the folder would open under no
        // root at all.
        let next = file.next.take().expect("staged above");
        promote(&mut file, next);
        inner.file = file;
        inner.source = to;
        let written = self.persist(&inner.file);
        drop(inner);
        self.register();
        Ok(written.err().map(|e| {
            format!("the root key is rotated and in use, but {e}: the next start finishes writing it, under the new root, and nothing is lost")
        }))
    }

    /// What `holon key status` shows (`GET /admin/keys`): where the root is kept and the id it is
    /// kept under, each scope's versions and when each was made, the scopes forgotten. Never a
    /// key, nor anything of one.
    pub(crate) fn status(&self) -> serde_json::Value {
        let inner = self.inner.read().unwrap();
        serde_json::json!({
            "key_id": inner.file.key_id,
            "root": inner.source.as_ref().map_or_else(|| "kept in memory only".to_string(), |s| s.to_string()),
            "source": inner.source.as_ref().map(KeySource::arg),
            "scopes": inner.file.scopes.iter().map(|(scope, kept)| serde_json::json!({
                "scope": scope,
                "current": kept.current,
                "versions": kept.versions.iter().map(|(v, w)| serde_json::json!({ "version": v, "made_ms": w.made_ms })).collect::<Vec<_>>(),
                "rotates": scope != NAMES,
            })).collect::<Vec<_>>(),
            "forgotten": inner.file.forgotten.iter().map(|(scope, f)| serde_json::json!({ "scope": scope, "at_ms": f.at_ms, "through": f.through })).collect::<Vec<_>>(),
        })
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
        self.open_file_in(None, role, bytes)
    }

    /// [`Keyring::open_file`], refused unless it was sealed under `scope` when one is named: a
    /// file one deployment wrote, copied where another's belongs, opens for neither there, as its
    /// header alone would let it (release audit K8).
    pub(crate) fn open_file_in(&self, expected: Option<&str>, role: &str, bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, Unreadable> {
        let rest = bytes.strip_prefix(MAGIC.as_slice()).ok_or(Unreadable::NotSealed)?;
        let damaged = || Unreadable::Damaged("its header is cut short".into());
        let (&len, rest) = rest.split_first().ok_or_else(damaged)?;
        let len = usize::from(len);
        if rest.len() < len + 4 {
            return Err(damaged());
        }
        let scope = std::str::from_utf8(&rest[..len]).map_err(|_| Unreadable::Damaged("its scope isn't text".into()))?;
        if let Some(expected) = expected.filter(|e| *e != scope) {
            return Err(Unreadable::Damaged(format!("it is sealed under {scope:?}, where one of {expected:?} belongs (moved from another deployment's place)")));
        }
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

    /// A run log line's body (all but its seq and the hash before it) sealed under `scope` for
    /// `role`, before they are known: `<version>.<base64url>` and its newline, framed with room in
    /// front for them ([`LineSeal::number`]).
    pub(crate) fn seal_body(&self, scope: &str, role: &str, body: &[u8]) -> Result<Framed, String> {
        let (version, key) = self.current(scope)?;
        let sealed = seal_with(&key, body_aad(scope, version, role).as_bytes(), body);
        let mut buf = Vec::with_capacity(ROOM + 11 + sealed.len().div_ceil(3) * 4 + 1);
        buf.resize(ROOM, b' ');
        buf.extend_from_slice(version.to_string().as_bytes());
        buf.push(b'.');
        b64_into(&sealed, &mut buf);
        buf.push(b'\n');
        Ok(Framed { buf, start: ROOM, place: Some(place_of(&key, &sealed)) })
    }

    /// A body sealed by [`Keyring::seal_body`] (`<version>.<base64url>`), opened, if `place` is
    /// where it was numbered: at `seq` after `prev` ([`place_of`]).
    fn open_body(&self, scope: &str, role: &str, (seq, prev, place): (u64, &str, &str), sealed: &str) -> Result<Zeroizing<Vec<u8>>, Unreadable> {
        let (version, body) = sealed.split_once('.').ok_or_else(|| Unreadable::Damaged("no version".into()))?;
        let version: u32 = version.parse().map_err(|_| Unreadable::Damaged("its version isn't a number".into()))?;
        let key = self.version(scope, version)?;
        let sealed = unb64(body.trim_end()).filter(|s| s.len() >= NONCE + 16).ok_or_else(|| Unreadable::Damaged("not base64url".into()))?;
        let want = place_at(place_of(&key, &sealed), seq, prev);
        let said = unb64(place).unwrap_or_default();
        // Compared in constant time: how much of a place matches says nothing.
        if said.len() != want.len() || said.iter().zip(want).fold(0, |d, (a, b)| d | (a ^ b)) != 0 {
            return Err(Unreadable::Damaged(format!("it isn't the line sealed at {seq} after the line before it (its body moved from another line)")));
        }
        open_with(&key, body_aad(scope, version, role).as_bytes(), &sealed)
            .ok_or_else(|| Unreadable::Damaged(format!("it doesn't open as {role:?} under {scope:?} version {version} (changed, or sealed by another folder's keys)")))
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

fn body_aad(scope: &str, version: u32, role: &str) -> String {
    format!("r1\0{scope}\0{version}\0{role}")
}

/// The most a numbered line's front takes: the newline a line after a cut-back starts with, `r2.`,
/// a seq (at most 20 digits), `.`, the hash of the line before (64 hex digits), `.`, its place
/// and `.`.
const ROOM: usize = 1 + NUMBERED.len() + 20 + 1 + 64 + 1 + PLACE + 1;

/// A line for a log's file, framed to be written as it is: `buf[start..]` is the line and its
/// newline, with room before it for what goes in front under the log's lock (a body sealed ahead's
/// seq and prev, and the newline after a cut-back), so nothing is made or copied there but that.
pub(crate) struct Framed {
    buf: Vec<u8>,
    start: usize,
    /// For a body sealed ahead, its place begun ([`place_of`]): its seq and prev go in under the
    /// log's lock.
    place: Option<blake3::Hasher>,
}

impl Framed {
    /// `line` as it is, with room for a newline before it.
    pub(crate) fn of(line: &str) -> Framed {
        let mut buf = Vec::with_capacity(line.len() + 2);
        buf.push(b'\n');
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        Framed { buf, start: 1, place: None }
    }

    /// The line, without its newline.
    pub(crate) fn line(&self) -> &[u8] {
        &self.buf[self.start..self.buf.len() - 1]
    }

    /// What is written: the line and its newline, after a newline if `after_cut` (the line a
    /// cut-back left short ended).
    pub(crate) fn text(&mut self, after_cut: bool) -> &[u8] {
        if after_cut {
            self.buf[self.start - 1] = b'\n';
            &self.buf[self.start - 1..]
        } else {
            &self.buf[self.start..]
        }
    }
}

/// `n` in decimal, at the end of `buf`.
pub(crate) fn decimal(n: u64, buf: &mut [u8; 20]) -> &[u8] {
    let (mut n, mut at) = (n, buf.len());
    loop {
        at -= 1;
        buf[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &buf[at..];
        }
    }
}

/// A run log's line from its seq, the hash of the line before (hex, empty for none) and its body
/// (what follows them, from its `,`): the line as it was made, and is hashed.
pub(crate) fn rejoin(seq: u64, prev: &str, body: &str) -> String {
    let mut line = String::with_capacity(body.len() + prev.len() + 32);
    line.push_str("{\"seq\":");
    line.push_str(&seq.to_string());
    if !prev.is_empty() {
        line.push_str(",\"prev\":\"");
        line.push_str(prev);
        line.push('"');
    }
    line.push_str(body);
    line
}

/// A staged root rotation made the file's: each version's key wrapped under the next root.
fn promote(file: &mut KeysFile, next: Staged) {
    for (scope, versions) in next.scopes {
        if let Some(kept) = file.scopes.get_mut(&scope) {
            for (version, wrapped) in versions {
                if let Some(w) = kept.versions.get_mut(&version) {
                    w.wrapped = wrapped;
                }
            }
        }
    }
    file.root_check = next.root_check;
}

/// `bytes` sealed under `scope` for `role` and written to `path` atomically, owner-only. Its
/// error says only [`crate::failing::FILE_UNWRITTEN`]: where and why go to stderr (audit R87).
pub(crate) fn write_private(keys: &Keyring, path: &Path, scope: &str, role: &str, bytes: &[u8]) -> std::io::Result<()> {
    data_folder(Some(path))
        .and_then(|()| keys.seal_file(scope, role, bytes).map_err(std::io::Error::other))
        .and_then(|sealed| crate::sealing::write_private(path, &sealed))
        .map_err(|e| crate::failing::Failed::of_file(crate::failing::FILE_UNWRITTEN, path, e))
}

/// The file at `path`, sealed for `role`, opened: `Ok(None)` if there is none. A file that
/// doesn't open says why in holon's own words ([`Unreadable`]: not sealed, sealed for another
/// role, its key gone), never where: the path goes to stderr (audit R87).
pub(crate) fn read(keys: &Keyring, path: &Path, role: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    match disk::read(path) {
        Ok(bytes) => keys.open_file(role, &bytes).map(Some).map_err(|e| crate::failing::Failed::reason(format!("{}: {e}", crate::failing::FILE_UNREAD), format!("{}: {e}", path.display())).said()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        // disk's error, where and why on stderr already.
        Err(e) => Err(e.to_string()),
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

/// What `keys.json` is copied as in a backup folder ([`Keyring::keep_copy_in`]): named as every
/// file the server writes there is, `holon-archive-…`.
pub(crate) const BACKUP_KEYS: &str = "holon-archive-keys.json";

/// The server's keyring.
pub(crate) fn keys() -> &'static std::sync::Arc<Keyring> {
    KEYS.get_or_init(|| std::sync::Arc::new(Keyring::memory()))
}

/// The slugs [`own_slug`] made: those the shared keyring forgets keys of, under test.
#[cfg(test)]
static OWN_SLUGS: std::sync::LazyLock<crate::sync::Mutex<std::collections::BTreeSet<String>>> = std::sync::LazyLock::new(Default::default);

/// A slug no other test uses, `base` with a number after it: one whose keys a test may forget on
/// the keyring every test shares ([`Keyring::forget`]).
#[cfg(test)]
pub(crate) fn own_slug(base: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let slug = format!("{base}-own{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    OWN_SLUGS.lock().unwrap().insert(slug.clone());
    slug
}

/// The data folder `dir` locked (`keys.lock`, an OS lock, let go when the process ends): a server
/// holds it while it runs ([`hold_folder`]), so `holon key rotate --root`, which changes
/// `keys.json` with no server on it, can tell; and a second server on one folder, which would
/// write `keys.json` over the first's, refuses to start.
pub(crate) fn lock_folder(dir: &Path) -> Result<std::fs::File, String> {
    let path = dir.join("keys.lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    holon_host::restrict_to_owner(&path).map_err(|e| format!("could not keep {} for its owner alone: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!("a server is running on {} (it holds {}): stop it first", dir.display(), path.display())),
        Err(std::fs::TryLockError::Error(e)) => Err(format!("could not lock {}: {e}", path.display())),
    }
}

static HELD: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

/// The server's data folder locked for as long as it runs ([`lock_folder`]).
pub(crate) fn hold_folder(dir: &Path) -> Result<(), String> {
    let file = lock_folder(dir)?;
    let _ = HELD.set(file);
    Ok(())
}

/// The content store's sealing (holon-host's `AtRest`, §5.4): its units, values and packs sealed
/// under the server's keys, each for what it is, and named by the names key.
pub(crate) struct StoreKeys(pub(crate) std::sync::Arc<Keyring>);

impl holon_host::AtRest for StoreKeys {
    fn seal(&self, role: &'static str, plain: &[u8]) -> std::io::Result<Vec<u8>> {
        self.0.seal_file(SERVER, role, plain).map_err(std::io::Error::other)
    }
    fn open(&self, role: &'static str, sealed: &[u8]) -> std::io::Result<Vec<u8>> {
        // The store hands its bytes on (a unit decoded, a value to a run): not kept zeroized.
        self.0.open_file(role, sealed).map(|mut plain| std::mem::take(&mut *plain)).map_err(|e| std::io::Error::other(e.to_string()))
    }
    fn name(&self, hash: &holon_core::Hash) -> String {
        self.0.name(hash)
    }
    fn data_folder(&self, path: &Path) -> std::io::Result<()> {
        data_folder(Some(path))
    }
    fn failed(&self, path: &Path, e: std::io::Error) -> std::io::Error {
        crate::failing::Failed::of_file(crate::failing::STORE_FAILED, path, e)
    }
}

/// The store's sealing under the server's keyring.
pub(crate) fn store() -> std::sync::Arc<dyn holon_host::AtRest> {
    std::sync::Arc::new(StoreKeys(std::sync::Arc::clone(keys())))
}

/// The scope of the deployment a kept file's key names first (`<slug>/<file>`): what it saw of
/// its queues, mailboxes and feeds.
pub(crate) fn deployment_of(key: &Path) -> String {
    deployment(&key.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default())
}

/// What each file is for, bound into it as it is sealed: a file renamed to another role fails to
/// open (§4).
pub(crate) mod role {
    pub(crate) const BLOB_TEXT: &str = "blob: text";
    pub(crate) const BLOB_CHUNK: &str = "blob: chunk";
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
    pub(crate) const ACCESS_PROPOSALS: &str = "table: access proposals";
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
    pub(crate) const SPOOL_MAIL: &str = "spool: mail";
    pub(crate) const SPOOL_QUEUE: &str = "spool: queue";

    /// A deployment's state head: the hash of its snapshot, bound to its slug.
    pub(crate) fn state_head(slug: &str) -> String {
        format!("state head: {slug}")
    }

    /// A run log's sealed segment: its older runs, gzipped then sealed whole.
    pub(crate) fn segment(slug: &str) -> String {
        format!("segment: {slug}")
    }

    /// The runs a cut-back let go of, written to the backup as one file (§5.5).
    pub(crate) fn batch(slug: &str) -> String {
        format!("batch: {slug}")
    }

    /// An export kept on disk: a backup's archive, a removed deployment's records (§5.5).
    pub(crate) fn export(slug: &str) -> String {
        format!("export: {slug}")
    }
}

/// An export kept on disk (a backup's archive, a removed deployment's records), opened by
/// `keys`: sealed under its deployment's scope for `role::export` of it, so which deployment it
/// is comes from the header, and a file sealed as anything else doesn't open as one.
pub(crate) fn open_export(keys: &Keyring, bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    let scope = bytes
        .strip_prefix(MAGIC.as_slice())
        .and_then(|rest| rest.split_first())
        .and_then(|(&len, rest)| rest.get(..usize::from(len)))
        .and_then(|scope| std::str::from_utf8(scope).ok())
        .ok_or("its header is cut short")?;
    let slug = scope.strip_prefix(&deployment("")).ok_or_else(|| format!("it is sealed under {scope:?}, not a deployment's"))?;
    keys.open_file(&role::export(slug), bytes).map_err(|e| e.to_string())
}

/// Whether the file at `path` is a table written in clear, as releases before the server sealed
/// its data (0.1.0-rc.35 and earlier) wrote them: there, not empty, and not sealed. A folder
/// is a log folder, not a table.
fn written_in_clear(path: &Path) -> bool {
    path.is_file() && std::fs::read(path).is_ok_and(|b| !b.is_empty() && !is_sealed(&b))
}

/// Whether `bytes` are sealed whole (§4): what `holon verify` needs keys for.
pub(crate) fn is_sealed(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC.as_slice())
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

    /// A run log line's body (from the `,` after its seq and prev) sealed before they are known,
    /// so outside the log's lock: for [`LineSeal::number`] under it.
    pub(crate) fn seal_body(&self, body: &str) -> std::io::Result<Framed> {
        self.keys.seal_body(&self.scope, &self.role, body.as_bytes()).map_err(std::io::Error::other)
    }

    /// A body sealed ahead made the line on disk: its seq, prev and place in clear before it
    /// (`r2.<seq>.<prev>.<place>.`), written into the room left in front of it, so under the log's
    /// lock nothing is made or copied but those, and the place's hash is of a few dozen bytes
    /// (docs/engineering/themes/performance.md).
    pub(crate) fn number(framed: &mut Framed, seq: u64, prev: &str) {
        let mut digits = [0u8; 20];
        let mut place = [0u8; PLACE];
        let at = place_at(framed.place.take().expect("a body sealed ahead"), seq, prev);
        b64_to(&at, &mut place);
        let parts = [NUMBERED.as_bytes(), decimal(seq, &mut digits), b".", prev.as_bytes(), b".", &place, b"."];
        let len: usize = parts.iter().map(|p| p.len()).sum();
        if len >= framed.start {
            // A prev longer than a hash (none is): made again, with room.
            let rest = framed.buf[framed.start..].to_vec();
            framed.buf = vec![b' '; len + 1];
            framed.buf.extend_from_slice(&rest);
            framed.start = len + 1;
        }
        let mut at = framed.start - len;
        for p in parts {
            framed.buf[at..at + p.len()].copy_from_slice(p);
            at += p.len();
        }
        framed.start -= len;
    }

    /// A line as it is on disk, opened: none if it doesn't (cut short as it was written, changed,
    /// or plain where a sealed one belongs). A run log's line sealed apart from its seq and prev
    /// is opened to the whole line, as it was made.
    pub(crate) fn open(&self, line: &str) -> Option<String> {
        self.read(line).ok()
    }

    /// [`LineSeal::open`], saying why a line doesn't open: a key missing
    /// ([`Unreadable::key_missing`]) is told apart from a line cut short or changed.
    pub(crate) fn read(&self, line: &str) -> Result<String, Unreadable> {
        let damaged = |why: &str| Unreadable::Damaged(why.into());
        let text = |bytes: &[u8]| std::str::from_utf8(bytes).map(str::to_string).map_err(|_| damaged("it isn't text"));
        if let Some(numbered) = line.strip_prefix(NUMBERED) {
            let (seq, rest) = numbered.split_once('.').ok_or_else(|| damaged("no seq"))?;
            let (prev, rest) = rest.split_once('.').ok_or_else(|| damaged("no prev"))?;
            let (place, sealed) = rest.split_once('.').ok_or_else(|| damaged("no place"))?;
            let seq: u64 = seq.parse().map_err(|_| damaged("its seq isn't a number"))?;
            if !prev.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(damaged("its prev isn't hex"));
            }
            let body = self.keys.open_body(&self.scope, &self.role, (seq, prev, place), sealed)?;
            return Ok(rejoin(seq, prev, &text(&body)?));
        }
        text(&self.keys.open_line(&self.scope, &self.role, line)?)
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
        disk::read_to_string(path).map(|t| self.open_lines(&t).collect()).unwrap_or_default()
    }

    /// The file at `path`'s lines that open, opened: none if there is no file. An error if it is
    /// there and can't be read, or a line's key is missing: what reads every line to tell what
    /// they name (the sweep) fails rather than take them as none (release audit R92). A line cut
    /// short or changed is left out, as [`LineSeal::open_lines`] leaves it.
    pub(crate) fn try_read_lines(&self, path: &Path) -> std::io::Result<Vec<String>> {
        let text = match disk::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(std::io::Error::new(e.kind(), format!("its file can't be read: {e}"))),
        };
        let mut lines = Vec::new();
        for line in text.lines() {
            match self.read(line) {
                Ok(plain) => lines.push(plain),
                Err(e) if e.key_missing() => return Err(std::io::Error::other(format!("a line of its file is {e}: {RESTORE_KEYS}"))),
                Err(_) => {}
            }
        }
        Ok(lines)
    }

    /// `plain` (one line) sealed and appended to the file at `path`, owner-only.
    pub(crate) fn append(&self, path: &Path, plain: &str) -> std::io::Result<()> {
        use std::io::Write;
        data_folder(Some(path))?;
        let line = self.seal(plain)? + "\n";
        crate::sealing::append_private(path)?.write_all(line.as_bytes())
    }

    /// A whole file of the same scope (a segment), sealed for `role`.
    pub(crate) fn seal_file(&self, role: &str, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
        self.keys.seal_file(&self.scope, role, bytes).map_err(std::io::Error::other)
    }

    /// A whole file sealed for `role`, opened.
    pub(crate) fn open_file(&self, role: &str, bytes: &[u8]) -> std::io::Result<Zeroizing<Vec<u8>>> {
        // The reason kept inside, for [`key_missing_in`].
        self.keys.open_file(role, bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
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

/// [`read_sealed`], refused unless it was sealed under `scope` ([`Keyring::open_file_in`]): for a
/// file of one deployment's, in a folder that holds every deployment's.
pub(crate) fn read_sealed_in(path: &Path, scope: &str, role: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    match disk::read(path) {
        Ok(bytes) => keys().open_file_in(Some(scope), role, &bytes).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
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

    /// A version of a scope no keyring here has: what a `keys.json` older than the data lacks.
    const MISSING: u32 = 999;

    /// A line whole on disk but sealed under a version of its scope the keys don't have: a run
    /// log's (numbered, seq `seq` after `prev`), or any other line log's.
    pub(crate) fn line_of_missing_key(numbered: Option<(u64, &str)>) -> String {
        let body = b64(&[7u8; 48]);
        match numbered {
            Some((seq, prev)) => format!("{NUMBERED}{seq}.{prev}.{}.{MISSING}.{body}", "A".repeat(PLACE)),
            None => format!("{LINE}{MISSING}.{body}"),
        }
    }

    /// A whole file sealed under `scope` at a version the keys don't have.
    pub(crate) fn file_of_missing_key(scope: &str) -> Vec<u8> {
        let mut file = file_header(scope, MISSING);
        file.extend_from_slice(&[7u8; 48]);
        file
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
        // keys.json gone, data sealed under it beside it: restore it.
        let data = dir.join("deployments.json");
        std::fs::write(&data, &sealed).unwrap();
        std::fs::remove_file(&path).unwrap();
        let gone = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[&data]).err().unwrap();
        assert!(gone.contains("is missing") && gone.contains("deployments.json") && gone.contains("holon-archive-keys.json"), "{gone}");
        assert!(!path.exists(), "no new keyring made over sealed data");
        // A table in clear beside it: a folder an older release wrote (0.1.0-rc.35 or earlier),
        // told apart, since there is no keys.json to restore (R10).
        std::fs::write(&data, br#"{"deployments":[]}"#).unwrap();
        let older = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[&dir.join("host.key"), &data]).err().unwrap();
        assert!(older.contains("0.1.0-rc.35") && older.contains("deployments.json") && !older.contains("Restore"), "{older}");
        assert!(!path.exists(), "no new keyring made over an older release's data");
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

    /// Guards: a root rotation leaves every record opening under the new root, and the old root
    /// opening none of the folder's keys; one cut short after the new root is kept opens under
    /// the new root (finished) or the old (undone), never neither.
    /// Why: a rotation that loses a data key loses everything sealed under it; one the old root
    /// still opens protects nothing (§2.4).
    #[test]
    fn a_rotated_root_opens_everything_and_the_old_root_nothing() {
        let dir = temp("rotate-root");
        let path = dir.join("keys.json");
        let made = Keyring::open(&path, Some(&root_arg(&dir, "old.key")), &[]).unwrap();
        let table = made.seal_file(SERVER, "table: test", b"kept").unwrap();
        let line = made.seal_line(&deployment("shop"), "log", b"a run").unwrap();
        made.rotate(&deployment("shop")).unwrap();
        let later = made.seal_line(&deployment("shop"), "log", b"a later run").unwrap();
        let name = made.name(&[7; 32]);
        let new = KeySource::File(dir.join("new.key"));
        made.rotate_root(Some(&new)).unwrap();
        assert_eq!(made.source(), Some(new.clone()));
        // The keyring in use goes on, and makes scopes under the new root.
        let after = made.seal_line(&deployment("other"), "log", b"x").unwrap();
        let file: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(file.get("next").is_none(), "nothing left staged: {file}");
        let check = |keys: &Keyring| {
            assert_eq!(keys.open_file("table: test", &table).unwrap().as_slice(), b"kept");
            assert_eq!(keys.open_line(&deployment("shop"), "log", &line).unwrap().as_slice(), b"a run");
            assert_eq!(keys.open_line(&deployment("shop"), "log", &later).unwrap().as_slice(), b"a later run");
            assert_eq!(keys.open_line(&deployment("other"), "log", &after).unwrap().as_slice(), b"x");
            assert_eq!(keys.name(&[7; 32]), name, "the store's names are the same");
        };
        check(&Keyring::open(&path, Some(&root_arg(&dir, "new.key")), &[]).unwrap());
        let old = Keyring::open(&path, Some(&root_arg(&dir, "old.key")), &[]).err().unwrap();
        assert!(old.contains("different root key"), "{old}");
        // A rotation in place (to where the root is kept), cut short after the new root is kept:
        // the next open finishes it.
        let again = Keyring::open(&path, Some(&root_arg(&dir, "new.key")), &[]).unwrap();
        let before = std::fs::read(dir.join("new.key")).unwrap();
        again.rotate_root_until(None, false).unwrap();
        assert_ne!(std::fs::read(dir.join("new.key")).unwrap(), before, "the new root kept in place");
        let file: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(file.get("next").is_some(), "cut short: still staged");
        check(&Keyring::open(&path, Some(&root_arg(&dir, "new.key")), &[]).unwrap());
        let file: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(file.get("next").is_none(), "finished on open: {file}");
        // One cut short with the new root kept elsewhere, opened under the old: undone.
        let third = Keyring::open(&path, Some(&root_arg(&dir, "new.key")), &[]).unwrap();
        third.rotate_root_until(Some(&KeySource::File(dir.join("third.key"))), false).unwrap();
        check(&Keyring::open(&path, Some(&root_arg(&dir, "new.key")), &[]).unwrap());
        assert!(Keyring::open(&path, Some(&root_arg(&dir, "third.key")), &[]).is_err(), "undone: the new root opens nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: a forget and the root rotation it makes, failing at any one of their writes
    /// (`keys.json` forgotten, the next root's wraps staged, the root kept, the wraps promoted),
    /// leave a folder that opens under the root its source holds, everything sealed before it
    /// still opening; and so does every write the same keyring makes after it (another forget and
    /// rotation, a scope made). Once the new root is kept the rotation says so, never "not
    /// rotated".
    /// Why: a rotation whose last write failed left the keyring in use on the old file (the old
    /// root's check, nothing staged) while its source held the new root; the next forget wrote
    /// that over the staged wraps on disk, and the folder opened under no root at all.
    #[test]
    fn a_forget_and_its_rotation_failing_at_any_write_leave_a_folder_that_opens() {
        // Four writes; the fifth round fails none.
        for n in 1..=5 {
            let dir = temp(&format!("rotate-fault-{n}"));
            let path = dir.join("keys.json");
            let arg = root_arg(&dir, "root.key");
            let keys = Keyring::open(&path, Some(&arg), &[]).unwrap();
            let table = keys.seal_file(SERVER, "table: test", b"kept").unwrap();
            let line = keys.seal_line(&deployment("shop"), "log", b"a run").unwrap();
            keys.seal_line(&deployment("gone"), "log", b"x").unwrap();
            keys.seal_line(&deployment("later"), "log", b"y").unwrap();
            // What keys.rs's forget does: the scope forgotten, then the root rotated in place.
            faults::fail_at(Some(n));
            let rotated = keys.forget(&deployment("gone")).and_then(|_| keys.rotate_root(None));
            faults::fail_at(None);
            match n {
                4 => assert!(matches!(&rotated, Ok(Some(why)) if why.contains("rotated and in use")), "write {n}: {rotated:?}"),
                5 => assert!(matches!(rotated, Ok(None)), "write {n}: {rotated:?}"),
                _ => assert!(rotated.is_err(), "write {n}: {rotated:?}"),
            }
            // The same keyring writes on.
            keys.forget(&deployment("later")).unwrap();
            assert_eq!(keys.rotate_root(None).unwrap(), None, "write {n}");
            let fresh = keys.seal_line(&deployment("fresh"), "log", b"z").unwrap();
            let again = Keyring::open(&path, Some(&arg), &[]).unwrap_or_else(|e| panic!("write {n} failed, then: {e}"));
            assert_eq!(again.open_file("table: test", &table).unwrap().as_slice(), b"kept", "write {n}");
            assert_eq!(again.open_line(&deployment("shop"), "log", &line).unwrap().as_slice(), b"a run", "write {n}");
            assert_eq!(again.open_line(&deployment("fresh"), "log", &fresh).unwrap().as_slice(), b"z", "write {n}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Guards: a keyring opened to read writes nothing, ever: not a keyring where there is none,
    /// not a rotation cut short (finished in memory only), not a scope or a version asked of it;
    /// and the root it exports, to a place that holds none, opens the folder on its own, while
    /// an export over a key, to where the root is kept, or to a variable is refused.
    /// Why: `holon verify` and `holon key export` read a folder a server may be running on,
    /// without its lock; a write from them could undo a rotation the server is making. And the
    /// export is the one way to back up a root kept in the OS key store: one that didn't open
    /// the folder would be found out only when it is needed.
    #[test]
    fn a_keyring_opened_to_read_writes_nothing_and_its_exported_root_opens_the_folder() {
        let dir = temp("read-only");
        let path = dir.join("keys.json");
        let arg = root_arg(&dir, "root.key");
        let missing = Keyring::open_to_read(&path, Some(&arg)).err().unwrap();
        assert!(missing.contains("isn't there") && !path.exists() && !dir.join("root.key").exists(), "{missing}");
        let keys = Keyring::open(&path, Some(&arg), &[]).unwrap();
        assert!(keys.made());
        let table = keys.seal_file(SERVER, "table: test", b"kept").unwrap();
        // A rotation in place cut short: staged on disk, the new root kept.
        keys.rotate_root_until(None, false).unwrap();
        let staged = std::fs::read(&path).unwrap();
        let read = Keyring::open_to_read(&path, Some(&arg)).unwrap();
        assert!(!read.made());
        assert_eq!(read.open_file("table: test", &table).unwrap().as_slice(), b"kept", "finished in memory");
        assert!(read.seal_line(&deployment("new"), "log", b"x").is_err(), "no scope made");
        assert!(read.rotate(SERVER).is_err(), "no version made");
        assert_eq!(std::fs::read(&path).unwrap(), staged, "keys.json as it was");
        // Its root exported, then the folder opened with that copy alone.
        let copy = KeySource::File(dir.join("backup").join("root.copy"));
        read.export_root(&copy).unwrap();
        let alone = Keyring::open(&path, Some(&format!("file:{}", dir.join("backup").join("root.copy").display())), &[]).unwrap();
        assert_eq!(alone.open_file("table: test", &table).unwrap().as_slice(), b"kept");
        let again = read.export_root(&copy).err().unwrap();
        assert!(again.contains("holds a key already"), "{again}");
        let kept = read.export_root(&KeySource::File(dir.join(".").join("root.key"))).err().unwrap();
        assert!(kept.contains("where the root key is kept"), "{kept}");
        assert!(read.export_root(&KeySource::Env("HOLON_NEVER_SET".into())).is_err());
        // A copy that can't be written: refused, and said.
        faults::fail_at(Some(1));
        assert!(read.export_root(&KeySource::File(dir.join("other.copy"))).is_err());
        faults::fail_at(None);
        assert!(!dir.join("other.copy").exists());
        assert!(crate::sealing::inside(&dir.join("x").join("..").join("root.key"), &dir) && !crate::sealing::inside(&dir.join("root.key"), &dir.join("backup")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: a backup folder keeps a copy of keys.json, written again in the same step as each
    /// write of keys.json (and not when it hasn't changed), and every archive in it opens with that folder and the root
    /// alone once the data folder is gone; a copy that can't be written is said, and the one there
    /// before is left whole.
    /// Why: archives are sealed under per-deployment keys that lived only in the data folder's
    /// keys.json, so the backup was lost with the disk it was meant to replace.
    #[test]
    fn an_archive_opens_with_its_backup_folder_and_the_root_alone() {
        let dir = temp("backup-keys");
        let (data, backup) = (dir.join("data"), dir.join("backup"));
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(backup.join("shop")).unwrap();
        let arg = root_arg(&dir, "root.key");
        let keys = Keyring::open(&data.join("keys.json"), Some(&arg), &[]).unwrap();
        let archive = |name: &str, plain: &[u8]| {
            let p = backup.join("shop").join(name);
            std::fs::write(&p, keys.seal_file(&deployment("shop"), &role::export("shop"), plain).unwrap()).unwrap();
            (p, plain.to_vec())
        };
        let mut archives = vec![archive("holon-archive-shop-runs-0-9.json.gz", b"runs 0-9")];
        assert!(keys.keep_copy_in(&backup).unwrap());
        assert!(!keys.keep_copy_in(&backup).unwrap(), "the same: not written again");
        // Each change to keys.json reaches the copy in the same step, before the archiver looks
        // again: a new version of a scope, a new scope, a scope forgotten, the root rotated in
        // place. Left to the archiver, the copy opened a forgotten deployment's archives with the
        // old root for a second or so after the forget (release audit R5).
        let copy = || std::fs::read(backup.join(BACKUP_KEYS)).unwrap();
        let file = || std::fs::read(data.join("keys.json")).unwrap();
        keys.rotate(&deployment("shop")).unwrap();
        assert_eq!(copy(), file(), "a new version");
        archives.push(archive("holon-archive-shop-runs-10-19.json.gz", b"runs 10-19"));
        keys.seal_line(&deployment("gone"), "log", b"a run").unwrap();
        assert_eq!(copy(), file(), "a new scope");
        assert!(keys.forget(&deployment("gone")).unwrap());
        assert_eq!(copy(), file(), "a scope forgotten");
        keys.rotate_root(None).unwrap();
        assert_eq!(copy(), file(), "the root rotated in place");
        assert!(!keys.keep_copy_in(&backup).unwrap(), "nothing left for the archiver");
        // A copy that can't be written: keys.json is, the copy before is left whole, and the
        // archiver's next look writes it, or says why it can't.
        let kept = copy();
        faults::fail_at(Some(2));
        keys.rotate(SERVER).unwrap();
        faults::fail_at(None);
        assert_eq!(copy(), kept, "left whole");
        assert_ne!(file(), kept);
        faults::fail_at(Some(1));
        assert!(keys.keep_copy_in(&backup).is_err());
        faults::fail_at(None);
        assert_eq!(copy(), kept);
        assert!(keys.keep_copy_in(&backup).unwrap());
        assert_eq!(copy(), file());
        // No backup any more: keys.json's writes are copied nowhere.
        keys.no_backup();
        keys.rotate(SERVER).unwrap();
        assert_ne!(copy(), file());
        assert!(keys.keep_copy_in(&backup).unwrap());
        assert!(!Keyring::memory().keep_copy_in(&backup).unwrap(), "a keyring in memory has no file to copy");
        // The data folder lost: the backup folder and the root open every archive.
        std::fs::remove_dir_all(&data).unwrap();
        for (p, plain) in &archives {
            let beside = crate::verify::keys_beside(p, Some(&arg)).expect("the copy found").unwrap();
            assert_eq!(open_export(&beside, &std::fs::read(p).unwrap()).unwrap().as_slice(), plain.as_slice(), "{}", p.display());
        }
        assert!(crate::verify::keys_beside(&dir.join("elsewhere").join("x.json.gz"), Some(&arg)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: a root rotation to a place that holds a key already writes nothing there, nor
    /// anything else; one `--to` the key file in use, its path spelled another way, is a rotation
    /// in place ([`KeySource::same_place`]).
    /// Why: a rotation `--to` another folder's root, or a key kept for something else, wrote over
    /// it, and that folder or that key was gone; and `--forget-old`, deciding "old" by spelling,
    /// removed the new root it had just kept in place.
    #[test]
    fn a_new_root_is_kept_only_where_none_is_and_a_place_is_known_however_spelled() {
        let dir = temp("rotate-to-kept");
        let path = dir.join("keys.json");
        let arg = root_arg(&dir, "root.key");
        let keys = Keyring::open(&path, Some(&arg), &[]).unwrap();
        let table = keys.seal_file(SERVER, "table: test", b"kept").unwrap();
        let other = KeySource::File(dir.join("other.key"));
        other.create().unwrap();
        let (theirs, file) = (std::fs::read(dir.join("other.key")).unwrap(), std::fs::read(&path).unwrap());
        let said = keys.rotate_root(Some(&other)).err().unwrap();
        assert!(said.contains("holds a key already"), "{said}");
        assert_eq!(std::fs::read(dir.join("other.key")).unwrap(), theirs, "the other key untouched");
        assert_eq!(std::fs::read(&path).unwrap(), file, "keys.json untouched");
        // The key file in use, spelled another way.
        let used = KeySource::File(dir.join("root.key"));
        let spelled = KeySource::File(dir.join(".").join("..").join(dir.file_name().unwrap()).join("root.key"));
        assert!(spelled.same_place(&used) && used.same_place(&spelled));
        assert!(!other.same_place(&used));
        assert!(KeySource::File(dir.join("not-yet.key")).same_place(&KeySource::File(dir.join(".").join("not-yet.key"))), "a file not made yet");
        let before = std::fs::read(dir.join("root.key")).unwrap();
        assert_eq!(keys.rotate_root(Some(&spelled)).unwrap(), None, "in place, not refused");
        assert_ne!(std::fs::read(dir.join("root.key")).unwrap(), before, "a new root, kept in place");
        let again = Keyring::open(&path, Some(&arg), &[]).unwrap();
        assert_eq!(again.open_file("table: test", &table).unwrap().as_slice(), b"kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: a root that opens two data folders (a copy of the folder, or one given the same
    /// `--root-key`) is never rotated in place: the rotation is refused, naming the other folder,
    /// and both still open; once the other has a root of its own, or is gone, it goes ahead.
    /// Why: a new root kept in the old one's place opens only the folder rotated, and every other
    /// folder under the old root (a backup copy, a second server) would open under none (K2).
    #[test]
    fn a_root_shared_by_two_folders_is_never_rotated_in_place_from_under_one() {
        let dir = temp("rotate-shared");
        let arg = root_arg(&dir, "root.key");
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir_all(d).unwrap();
        }
        let first = Keyring::open(&a.join("keys.json"), Some(&arg), &[]).unwrap();
        let table = first.seal_file(SERVER, "table: test", b"kept").unwrap();
        // b and c: copies of a, each opened (a server started on it).
        std::fs::copy(a.join("keys.json"), b.join("keys.json")).unwrap();
        std::fs::copy(a.join("keys.json"), c.join("keys.json")).unwrap();
        let second = Keyring::open(&b.join("keys.json"), Some(&arg), &[]).unwrap();
        drop(Keyring::open(&c.join("keys.json"), Some(&arg), &[]).unwrap());
        assert_eq!(first.others_on_root().unwrap().len(), 2, "b and c, as listed beside the root");
        let root = std::fs::read(dir.join("root.key")).unwrap();
        for to in [None, Some(KeySource::File(dir.join("root.key")))] {
            let said = first.rotate_root(to.as_ref()).err().unwrap();
            assert!(said.contains("opens under this root too"), "{said}");
            assert_eq!(std::fs::read(dir.join("root.key")).unwrap(), root, "the shared root untouched");
        }
        // A forget asks for the rotation, which is refused: the scope is still forgotten.
        first.seal_line(&deployment("gone"), "log", b"a run").unwrap();
        assert!(first.forget(&deployment("gone")).unwrap());
        assert!(first.rotate_root(None).is_err());
        for d in [&a, &b, &c] {
            let again = Keyring::open(&d.join("keys.json"), Some(&arg), &[]).unwrap();
            assert_eq!(again.open_file("table: test", &table).unwrap().as_slice(), b"kept", "{} still opens", d.display());
        }
        // b given a root of its own (a rotation elsewhere, which leaves the old root where it is),
        // c deleted: a rotates in place.
        assert_eq!(second.rotate_root(Some(&KeySource::File(dir.join("b.key")))).unwrap(), None);
        std::fs::remove_dir_all(&c).unwrap();
        assert!(first.others_on_root().unwrap().is_empty());
        assert_eq!(first.rotate_root(None).unwrap(), None);
        assert_ne!(std::fs::read(dir.join("root.key")).unwrap(), root, "a new root, kept in place");
        let opened_a = Keyring::open(&a.join("keys.json"), Some(&arg), &[]).unwrap();
        let opened_b = Keyring::open(&b.join("keys.json"), Some(&root_arg(&dir, "b.key")), &[]).unwrap();
        for opened in [opened_a, opened_b] {
            assert_eq!(opened.open_file("table: test", &table).unwrap().as_slice(), b"kept");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: a new version of a scope's data key seals what is written after it, what was
    /// sealed under the versions before still opens, then and after the server starts again; the
    /// names key is never rotated.
    /// Why: a rotation that made old records unreadable would lose the run log; one that didn't
    /// seal new writes under the new key would rotate nothing.
    #[test]
    fn a_new_data_key_version_seals_new_writes_and_old_records_still_open() {
        let dir = temp("rotate-scope");
        let path = dir.join("keys.json");
        let keys = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).unwrap();
        let first = keys.seal_file(SERVER, "table: test", b"one").unwrap();
        let line = keys.seal_line(&deployment("shop"), "log", b"a run").unwrap();
        assert_eq!(keys.rotate(SERVER).unwrap(), 2);
        assert_eq!(keys.rotate(&deployment("shop")).unwrap(), 2);
        let second = keys.seal_file(SERVER, "table: test", b"two").unwrap();
        let later = keys.seal_line(&deployment("shop"), "log", b"a later run").unwrap();
        assert!(second.starts_with(&file_header(SERVER, 2)), "sealed under version 2");
        assert!(later.starts_with("a1.2."), "{later}");
        for keys in [&keys, &Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).unwrap()] {
            assert_eq!(keys.open_file("table: test", &first).unwrap().as_slice(), b"one");
            assert_eq!(keys.open_file("table: test", &second).unwrap().as_slice(), b"two");
            assert_eq!(keys.open_line(&deployment("shop"), "log", &line).unwrap().as_slice(), b"a run");
            assert_eq!(keys.open_line(&deployment("shop"), "log", &later).unwrap().as_slice(), b"a later run");
        }
        let names = keys.rotate(NAMES).err().unwrap();
        assert!(names.contains("never rotated"), "{names}");
        assert!(keys.rotate("d:never-made").is_err(), "no scope made by a rotation");
        let status = keys.status();
        assert_eq!(status["scopes"].as_array().unwrap().iter().find(|s| s["scope"] == SERVER).unwrap()["current"], 2);
        assert!(!status.to_string().contains("wrapped"), "no key, wrapped or not, in what status shows");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: the keyring every test shares forgets keys only of a slug one test alone uses
    /// (`own_slug`): a slug any test may deploy is refused, loudly, its keys left as they were; a
    /// slug of its own is forgotten; one with nothing sealed forgets nothing, whoever's. Any other
    /// keyring forgets any slug.
    /// Why: a test forgetting on the shared keyring a slug another test deploys meanwhile leaves
    /// that test's first lines unopenable, and it finds fewer runs than it made, by chance.
    #[test]
    fn the_shared_keyring_forgets_keys_only_of_a_slug_its_test_alone_uses() {
        let shared = keys();
        let anyones = deployment("shared-keyring-probe");
        let line = shared.seal_line(&anyones, role::LOG, b"a run").unwrap();
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shared.forget(&anyones)));
        assert!(refused.is_err(), "a slug any test may use isn't forgotten on the shared keyring");
        assert_eq!(shared.open_line(&anyones, role::LOG, &line).unwrap().as_slice(), b"a run", "its keys stay");
        let own = deployment(&own_slug("probe"));
        shared.seal_line(&own, role::LOG, b"a run").unwrap();
        assert_eq!(shared.forget(&own), Ok(true));
        assert_eq!(shared.forget(&deployment("never-sealed-probe")), Ok(false));
        let private = Keyring::memory();
        private.seal_line(&anyones, role::LOG, b"a run").unwrap();
        assert_eq!(private.forget(&anyones), Ok(true));
    }

    /// Guards: a forgotten deployment's records don't open, and say they were forgotten (not
    /// damaged), then and after a restart; the slug deployed again seals under a new version,
    /// and the forgotten ones stay forgotten; the server's own scopes are never forgotten.
    /// Why: forgetting is the operator's way to make a removed deployment's data unreadable
    /// everywhere it was copied to by the server (§2.4).
    #[test]
    fn a_forgotten_deployment_is_unreadable_everywhere_and_says_so() {
        let dir = temp("forget");
        let path = dir.join("keys.json");
        let keys = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).unwrap();
        let line = keys.seal_line(&deployment("shop"), "log", b"a run").unwrap();
        let export = keys.seal_file(&deployment("shop"), "export: shop", b"its records").unwrap();
        let kept = keys.seal_line(&deployment("kept"), "log", b"another's run").unwrap();
        assert!(keys.forget(&deployment("shop")).unwrap());
        assert!(!keys.forget(&deployment("never")).unwrap(), "nothing to forget");
        assert!(keys.forget(SERVER).is_err() && keys.forget(SECRETS).is_err() && keys.forget(NAMES).is_err());
        let again = Keyring::open(&path, Some(&root_arg(&dir, "root.key")), &[]).unwrap();
        for keys in [&keys, &again] {
            assert!(matches!(keys.open_line(&deployment("shop"), "log", &line), Err(Unreadable::Forgotten { .. })));
            let said = keys.open_file("export: shop", &export).err().unwrap();
            assert!(matches!(said, Unreadable::Forgotten { .. }) && said.to_string().contains("forgotten"), "{said}");
            assert_eq!(keys.open_line(&deployment("kept"), "log", &kept).unwrap().as_slice(), b"another's run");
        }
        let file: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(file["scopes"].get("d:shop").is_none() && file["forgotten"]["d:shop"]["through"] == 1, "its keys gone from keys.json: {file}");
        // The slug deployed again: a new version, and the old records still forgotten.
        let anew = again.seal_line(&deployment("shop"), "log", b"a new run").unwrap();
        assert!(anew.starts_with("a1.2."), "{anew}");
        assert_eq!(again.open_line(&deployment("shop"), "log", &anew).unwrap().as_slice(), b"a new run");
        assert!(matches!(again.open_line(&deployment("shop"), "log", &line), Err(Unreadable::Forgotten { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards: the table-driven base64url here writes what the core's writes, for every length,
    /// and reads back exactly what the core's reads: the same texts refused (a character outside
    /// the alphabet, a length none has, leftover bits set), the same bytes from the rest.
    /// Why: it replaced the core's on every sealed line for speed; a text one reads and the other
    /// doesn't would make a sealed record unreadable, or two texts open as one.
    #[test]
    fn base64url_here_is_the_cores() {
        let mut bytes = vec![0u8; 300];
        getrandom::fill(&mut bytes).unwrap();
        for len in 0..bytes.len() {
            let b = &bytes[..len];
            let core = holon_core::bytes_text(b, "base64url").unwrap();
            assert_eq!(b64(b), core, "{len} bytes");
            assert_eq!(unb64(&core).as_deref(), Some(b), "{len} bytes back");
            // Every last character, and characters outside the alphabet, read as the core reads them.
            if let Some(last) = core.len().checked_sub(1) {
                for c in B64.iter().chain(b"=+/ .".iter()) {
                    let mut t = core.clone().into_bytes();
                    t[last] = *c;
                    let t = String::from_utf8(t).unwrap();
                    assert_eq!(unb64(&t), holon_core::text_bytes(&t, "base64url").filter(|_| !t.contains('=')), "{t:?}");
                }
            }
        }
        for t in ["A", "AAAAA", "A=", "AA=="] {
            assert_eq!(unb64(t), None, "{t:?}");
        }
    }

    /// Guards: what is sealed opens only where it was sealed (release audit R37). A run log line
    /// sealed apart from its seq and prev (before the log's lock) opens to the very line that was
    /// made and hashed, and nowhere else: of two lines, any mix of one's seq, prev, place and body
    /// with the other's doesn't open (the last line's body swapped for an earlier one's among
    /// them, which the chain alone wouldn't show); nor does it under another deployment's log or
    /// another role. A whole file of one deployment's opens where that deployment's belongs, not
    /// where another's does, whatever its header says (K8).
    /// Why: sealing outside the lock keeps seq and prev in clear; their place, keyed and bound to
    /// the body's nonce and tag, is what holds them to it (docs/L4s/at-rest-encryption-L4.md §7).
    #[test]
    fn what_is_sealed_opens_only_where_it_was_sealed() {
        let keys = std::sync::Arc::new(Keyring::memory());
        let seal = LineSeal::with(std::sync::Arc::clone(&keys), &deployment("shop"), role::LOG);
        let numbered = |seq: u64, prev: &str, body: &str| {
            let mut framed = seal.seal_body(body).unwrap();
            LineSeal::number(&mut framed, seq, prev);
            assert_eq!(framed.text(false).last(), Some(&b'\n'));
            assert_eq!(framed.text(true)[0], b'\n', "room for the newline after a cut-back");
            String::from_utf8(framed.line().to_vec()).unwrap()
        };
        let body = r#","at_ms":1,"answer":"a secret answer"}"#;
        let prev = "ab".repeat(32);
        let line = numbered(7, &prev, body);
        assert!(line.starts_with(&format!("r2.7.{prev}.")) && !line.contains("secret"), "{line}");
        assert_eq!(seal.open(&line).unwrap(), rejoin(7, &prev, body));
        assert_eq!(rejoin(7, &prev, body), format!(r#"{{"seq":7,"prev":"{prev}","at_ms":1,"answer":"a secret answer"}}"#));
        assert_eq!(seal.open(&numbered(0, "", body)).unwrap(), format!(r#"{{"seq":0{body}"#), "the first line has no prev");
        assert_eq!(seal.open(&numbered(u64::MAX, &prev, body)).unwrap(), rejoin(u64::MAX, &prev, body), "the longest seq fits the room");

        // Two lines, the second after the first; each of its four parts from one or the other.
        let parts = |line: &str| -> [String; 4] {
            let rest = line.strip_prefix(NUMBERED).unwrap();
            let mut it = rest.splitn(4, '.');
            [(); 4].map(|_| it.next().unwrap().to_string())
        };
        let later = numbered(8, &crate::recorder::line_hash(&rejoin(7, &prev, body)), r#","at_ms":2,"answer":"a refusal"}"#);
        let (a, b) = (parts(&line), parts(&later));
        for mix in 0..16u8 {
            let pick = |i: usize| if mix & (1 << i) == 0 { &a[i] } else { &b[i] };
            let made = format!("{NUMBERED}{}.{}.{}.{}", pick(0), pick(1), pick(2), pick(3));
            let opens = seal.open(&made).is_some();
            assert_eq!(opens, mix == 0 || mix == 15, "seq, prev, place, body from {mix:04b} (1: the later line's) opened: {opens}");
        }
        assert!(seal.open(&line.replacen(&prev, "not-hex", 1)).is_none(), "a prev that isn't a hash");
        let said = seal.read(&format!("{NUMBERED}{}.{}.{}.{}", b[0], b[1], b[2], a[3])).err().unwrap();
        assert!(said.to_string().contains("moved from another line"), "{said}");
        // Another log, another role.
        assert!(LineSeal::with(std::sync::Arc::clone(&keys), &deployment("other"), role::LOG).open(&line).is_none());
        assert!(LineSeal::with(std::sync::Arc::clone(&keys), &deployment("shop"), role::EVENTS).open(&line).is_none());

        // A whole file of one deployment's, where it belongs and where another's does.
        for role in [role::QUEUE_SEEN, role::MAIL_SEEN, role::FEED_SEEN, role::ARRIVALS, role::SPOOL_MAIL, role::SPOOL_QUEUE] {
            let file = keys.seal_file(&deployment("shop"), role, b"what shop has seen").unwrap();
            assert_eq!(keys.open_file_in(Some(&deployment("shop")), role, &file).unwrap().as_slice(), b"what shop has seen", "{role}");
            let moved = keys.open_file_in(Some(&deployment("other")), role, &file).err().unwrap();
            assert!(matches!(moved, Unreadable::Damaged(_)) && moved.to_string().contains("moved from another deployment"), "{role}: {moved}");
        }
    }
}
