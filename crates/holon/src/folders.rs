//! Named folders (`docs/L4s/named-folders-L4.md`): files a flow reads and writes on the host.
//!
//! - **Bound by an admin.** A folder is a name an admin binds to a real folder on this machine
//!   (`--folders`, owner-only; `/admin/folders`, `holon folder`), with the most it allows (read, or
//!   read and write) and what its reads keep in the log (the bytes, or a hash). A flow names
//!   `inbox`; it never sees a path. A binding is for the whole server, or for one deployment: a
//!   deployment's own binding of a name is the one it sees ([`Scoped`]), and no other deployment
//!   sees it. The server's own data can never be bound, nor a folder holding it.
//! - **Confined.** A file's name is a relative path inside the folder, checked before it reaches
//!   the operating system (no `..`, no drive or root, no `:`, no reserved Windows names), and the
//!   file it opens must still be inside the folder once links are followed.
//! - **Reads** are answered on the spot, as text, from the folder as the run found it, only for
//!   folders the deployment was granted, and each is kept (with its node's path), as a collection
//!   read is: the log shows it, replay answers from it, and a run is checked against it before it
//!   commits. A big file's text goes to the blob store (blobs.rs: by content, in chunks), and the
//!   log holds its hash; a folder may keep those texts for a while (`keep: bytes for 30d`), and
//!   a sweep removes the ones no kept run refers to ([`sweep`]).
//! - **Big files never enter a run whole.** A handle (`inbox.file(n)`) is a file's name, size and
//!   hash, without its bytes; a copy by handle streams them host to host when the run commits. A
//!   range (`lines`, `slice`) reads only its bytes, and the log keeps with them the hashes that
//!   prove them part of the file (ranges.rs), so the run is checkable from that range alone.
//! - **Writes** (a file's whole text, a copy, or a delete) are effects, applied when the run
//!   commits: written beside the file and renamed over it, so a reader never sees half of one. The
//!   log holds what was written by its hash and size, never its text: `holon verify` runs it again.
//!   A folder may cap its size: a write that would take it past its cap doesn't land.

use std::io::{Read as _, Seek as _, Write as _};
use std::sync::RwLock;

use holon_host::{Limits, Performer};
use rouille::{Request, Response};

use crate::outbound::{self, Body, FileBody, Outbound};

use crate::http::{err, read_body};

use crate::*;

/// The most a folder allows a deployment.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FolderAccess {
    Read,
    ReadWrite,
}

/// What a folder's reads keep in the run log: the bytes (a run is checkable anywhere, later), the
/// bytes for a while and then only their hash, or only their hash (what was read is provable; it
/// runs again when the file is supplied). Written `bytes`, `bytes for 30d` and `hash`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(try_from = "String", into = "String")]
pub(crate) enum Keep {
    #[default]
    Bytes,
    /// The bytes, for this many seconds after the run.
    BytesFor(u64),
    Hash,
}

impl Keep {
    /// Whether a read's bytes are kept at all (for a while, or for good).
    pub(crate) fn keeps_bytes(self) -> bool {
        self != Keep::Hash
    }

    /// How long after a run its reads' bytes are kept, in milliseconds: none for good.
    pub(crate) fn window_ms(self) -> Option<u64> {
        match self {
            Keep::Bytes => None,
            Keep::BytesFor(secs) => Some(secs.saturating_mul(1000)),
            Keep::Hash => Some(0),
        }
    }
}

impl TryFrom<String> for Keep {
    type Error = String;
    /// `bytes`, `hash`, or `bytes for <n><s|m|h|d>` (or just `<n><s|m|h|d>`).
    fn try_from(s: String) -> Result<Keep, String> {
        match s.trim() {
            "bytes" => Ok(Keep::Bytes),
            "hash" => Ok(Keep::Hash),
            other => {
                let span = other.strip_prefix("bytes for ").unwrap_or(other).trim();
                seconds_of(span)
                    .map(Keep::BytesFor)
                    .ok_or_else(|| format!("keep is bytes, hash, or bytes for a while (bytes for 30d; s, m, h or d), not {s:?}"))
            }
        }
    }
}

impl From<Keep> for String {
    fn from(k: Keep) -> String {
        match k {
            Keep::Bytes => "bytes".into(),
            Keep::Hash => "hash".into(),
            Keep::BytesFor(secs) => format!("bytes for {}", span_text(secs)),
        }
    }
}

/// A while, as `30d`, `12h`, `90m` or `45s`: in seconds (at least one, at most a hundred years).
fn seconds_of(span: &str) -> Option<u64> {
    let unit = match span.chars().last()? {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return None,
    };
    let n: u64 = span[..span.len() - 1].parse().ok()?;
    Some(n.checked_mul(unit)?).filter(|&s| s > 0 && s <= 100 * 365 * 86_400)
}

/// A while in seconds, in the largest unit that says it exactly.
fn span_text(secs: u64) -> String {
    match secs {
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// A size in bytes as people read it: `512 bytes`, `64 KiB`, `9.8 MiB`.
pub(crate) fn size_text(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")];
    for (size, unit) in UNITS {
        if bytes >= size {
            let n = bytes as f64 / size as f64;
            return if n.fract() == 0.0 || n >= 100.0 { format!("{n:.0} {unit}") } else { format!("{n:.1} {unit}") };
        }
    }
    format!("{bytes} bytes")
}

/// One bound folder.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Folder {
    /// The real folder, as bound (absolute, links resolved).
    pub(crate) path: PathBuf,
    pub(crate) access: FolderAccess,
    #[serde(default)]
    pub(crate) keep: Keep,
    /// The most bytes its files may hold in all, if it is capped: a write past it doesn't land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cap: Option<u64>,
}

/// The largest file a run reads as text, without asking for more (`inline_file_bytes`).
#[cfg(test)]
pub(crate) const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// A read's text up to this size is in its log entry; a bigger one is in the content store, by hash.
pub(crate) const LOG_INLINE_BYTES: u64 = 4 * 1024;
/// The most files a listing looks at.
const MAX_WALK: usize = 100_000;
/// What the error of a run whose file read failed starts with: such a run answers 500, not what
/// it computed from nothing.
pub(crate) const READ_FAILED: &str = "a file read failed: ";
/// What the error of a run whose file write didn't land starts with (a folder past its cap, a
/// copy whose source changed): such a run answers 500, not what it said it did.
pub(crate) const WRITE_FAILED: &str = "not every write landed: ";
/// What the host's own temporary files start with: never a flow's.
const TMP_PREFIX: &str = ".holon-tmp-";

/// The folders bound on this server, and where they are kept.
pub(crate) struct Folders {
    /// The bindings file (owner-only), or none (in memory).
    file: Option<PathBuf>,
    /// The server's own files and folders: never bound, nor anything holding them.
    forbidden: Vec<PathBuf>,
    bound: RwLock<Bound>,
    /// The texts of the files runs read, kept for their logs (blobs.rs).
    pub(crate) blobs: blobs::Blobs,
}

/// Every binding: the server's, by name, and each deployment's own, by its slug then name. On
/// disk, the server's are the file's top level (as before there were any of a deployment's) and a
/// deployment's are under `@deployments`, which no folder name can be.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Bound {
    #[serde(rename = "@deployments", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) deployments: BTreeMap<String, BTreeMap<String, Folder>>,
    #[serde(flatten)]
    pub(crate) server: BTreeMap<String, Folder>,
}

impl std::fmt::Debug for Folders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Folders").field("bound", &self.bound.read().unwrap()).finish()
    }
}

/// Whether `name` is a folder's name: letters, digits, `-` and `_`, 1 to 64 (core's `ident`).
pub(crate) fn is_folder_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A path made absolute and, where it exists, with its links resolved.
fn real(path: &Path) -> PathBuf {
    canonical(path).unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// The path with its links resolved, as `fs::canonicalize` gives it, but without Windows'
/// verbatim prefix on a path with a drive letter: the form people write, and the one every path
/// here is compared in.
pub(crate) fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let p = std::fs::canonicalize(path)?;
    Ok(match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    })
}

impl Folders {
    /// No folders, kept nowhere: the tests, and a server told `--folders :memory:`.
    pub(crate) fn memory() -> Folders {
        Folders { file: None, forbidden: Vec::new(), bound: RwLock::new(Bound::default()), blobs: blobs::Blobs::memory() }
    }

    /// The same folders, the texts their runs read kept in `blobs` (`<store>/blobs/`).
    pub(crate) fn with_blobs(self, blobs: blobs::Blobs) -> Folders {
        Folders { blobs, ..self }
    }

    /// The folders bound in `file` (none yet if it doesn't exist), refusing ever to bind one of
    /// `forbidden` or anything holding it.
    pub(crate) fn open(file: &str, forbidden: &[PathBuf]) -> Result<Folders, String> {
        let forbidden: Vec<PathBuf> = forbidden.iter().map(|p| real(p)).collect();
        if file == ":memory:" {
            return Ok(Folders { forbidden, ..Folders::memory() });
        }
        let path = PathBuf::from(file);
        let bound = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{file} isn't a folders file: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Bound::default(),
            Err(e) => return Err(format!("can't read {file}: {e}")),
        };
        Ok(Folders { file: Some(path), forbidden, ..Folders::memory() }.with_bound(bound))
    }

    fn with_bound(self, bound: Bound) -> Folders {
        Folders { bound: RwLock::new(bound), ..self }
    }

    /// Every folder bound: the server's, and each deployment's.
    pub(crate) fn all(&self) -> Bound {
        self.bound.read().unwrap().clone()
    }

    /// The folder deployment `deployment` sees as `name`: its own binding, else the server's.
    pub(crate) fn resolve(&self, deployment: &str, name: &str) -> Option<Folder> {
        let bound = self.bound.read().unwrap();
        bound.deployments.get(deployment).and_then(|d| d.get(name)).or_else(|| bound.server.get(name)).cloned()
    }

    /// Bind `name` to the folder at `path` (which must exist), capped at `cap` bytes if given, for
    /// the whole server or for one deployment (`for_deployment`, its slug), replacing any binding of
    /// it there before.
    pub(crate) fn bind(&self, name: &str, path: &str, access: FolderAccess, keep: Keep, cap: Option<u64>, for_deployment: Option<&str>) -> Result<Folder, String> {
        if !is_folder_name(name) {
            return Err(format!("{name:?} isn't a folder name: letters, digits, - and _, up to 64"));
        }
        if let Some(slug) = for_deployment.filter(|s| !is_folder_name(s)) {
            return Err(format!("{slug:?} isn't a deployment's name"));
        }
        let given = PathBuf::from(path.trim());
        if !given.is_absolute() {
            return Err(format!("{path:?} isn't an absolute path: give the whole path, as this server's machine sees it"));
        }
        if !given.is_dir() {
            return Err(format!("{path:?} isn't a folder on this server's machine"));
        }
        let at = real(&given);
        if let Some(own) = self.forbidden.iter().find(|f| f.starts_with(&at) || at.starts_with(f)) {
            return Err(format!("{} can't be bound: it is, or holds, this server's own data ({})", at.display(), own.display()));
        }
        if cap == Some(0) {
            return Err("a folder's cap is a size above nothing (10M, say)".into());
        }
        let folder = Folder { path: at, access, keep, cap };
        let mut bound = self.bound.write().unwrap();
        let mut next = bound.clone();
        match for_deployment {
            Some(slug) => next.deployments.entry(slug.to_string()).or_default().insert(name.to_string(), folder.clone()),
            None => next.server.insert(name.to_string(), folder.clone()),
        };
        self.save(&next)?;
        *bound = next;
        Ok(folder)
    }

    /// Unbind `name`, the server's or one deployment's: whether it was bound. Its files stay where
    /// they are.
    pub(crate) fn unbind(&self, name: &str, for_deployment: Option<&str>) -> Result<bool, String> {
        let mut bound = self.bound.write().unwrap();
        let mut next = bound.clone();
        let removed = match for_deployment {
            Some(slug) => {
                let removed = next.deployments.get_mut(slug).and_then(|d| d.remove(name)).is_some();
                if next.deployments.get(slug).is_some_and(BTreeMap::is_empty) {
                    next.deployments.remove(slug);
                }
                removed
            }
            None => next.server.remove(name).is_some(),
        };
        if !removed {
            return Ok(false);
        }
        self.save(&next)?;
        *bound = next;
        Ok(true)
    }

    fn save(&self, bound: &Bound) -> Result<(), String> {
        let Some(file) = &self.file else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(bound).map_err(|e| e.to_string())?;
        crate::sealing::write_private(file, &bytes).map_err(|e| format!("can't write {}: {e}", file.display()))
    }
}

/// The folders one deployment sees, by name: those bound for it, else the server's. What its runs
/// read, write and copy through, and what its deploy is checked against.
#[derive(Debug)]
pub(crate) struct Scoped {
    pub(crate) all: Arc<Folders>,
    /// Its slug.
    pub(crate) deployment: String,
    /// The folders whose new files start its runs (arrivals.rs), as its watcher last said.
    watched: Mutex<BTreeSet<String>>,
    /// The files its own runs wrote or copied into those, by folder and name, as they landed (size,
    /// modified): one doesn't start a run, unless it changes again.
    landed: Mutex<BTreeMap<(String, String), Signature>>,
    /// The files its own runs deleted from those, by folder and name: one doesn't start a run.
    removed: Mutex<BTreeSet<(String, String)>>,
}

/// A file as a watcher sees it without reading it: its size and when it was modified (ms).
pub(crate) type Signature = (u64, u64);

impl Scoped {
    pub(crate) fn new(all: Arc<Folders>, deployment: &str) -> Scoped {
        Scoped {
            all,
            deployment: deployment.to_string(),
            watched: Mutex::new(BTreeSet::new()),
            landed: Mutex::new(BTreeMap::new()),
            removed: Mutex::new(BTreeSet::new()),
        }
    }

    /// Every file of folder `name` a flow may name, by its signature: the folder isn't read.
    pub(crate) fn signatures(&self, name: &str) -> Result<BTreeMap<String, Signature>, String> {
        let folder = self.folder(name)?;
        let mut names = Vec::new();
        walk(&folder.path, "", &mut names)?;
        let mut files = BTreeMap::new();
        for n in names {
            // One gone between the walk and now is simply not there.
            if let Ok(meta) = std::fs::metadata(folder.path.join(&n)) {
                files.insert(n, (meta.len(), modified(&meta)));
            }
        }
        Ok(files)
    }

    /// The folders its watcher looks at: what its runs write there is noted as its own.
    pub(crate) fn watch(&self, folders: &BTreeSet<String>) {
        let mut watched = self.watched.lock().unwrap();
        if *watched != *folders {
            watched.clone_from(folders);
            self.landed.lock().unwrap().retain(|(name, _), _| folders.contains(name));
            self.removed.lock().unwrap().retain(|(name, _)| folders.contains(name));
        }
    }

    /// The files of folder `name` its own runs deleted that are gone from `now` (forgotten); one
    /// not gone yet is kept for a later look.
    pub(crate) fn took_own_deletes(&self, name: &str, now: &BTreeMap<String, Signature>) -> BTreeSet<String> {
        let mut removed = self.removed.lock().unwrap();
        let gone: BTreeSet<String> = removed.iter().filter(|(f, n)| f == name && !now.contains_key(n)).map(|(_, n)| n.clone()).collect();
        removed.retain(|(f, n)| f != name || !gone.contains(n));
        gone
    }

    /// Whether file `file` of folder `name`, now `signature`, is as one of this deployment's runs
    /// left it; forgotten either way.
    pub(crate) fn took_own(&self, name: &str, file: &str, signature: Signature) -> bool {
        self.landed.lock().unwrap().remove(&(name.to_string(), file.to_string())) == Some(signature)
    }

    fn note_landed(&self, name: &str, file: &str, path: &Path) {
        if !self.watched.lock().unwrap().contains(name) {
            return;
        }
        if let Ok(meta) = std::fs::metadata(path) {
            self.landed.lock().unwrap().insert((name.to_string(), file.to_string()), (meta.len(), modified(&meta)));
        }
    }

    /// No folders at all: a deployment read from disk until the server's are opened.
    pub(crate) fn memory() -> Scoped {
        Scoped::new(Arc::new(Folders::memory()), "")
    }

    pub(crate) fn get(&self, name: &str) -> Option<Folder> {
        self.all.resolve(&self.deployment, name)
    }

    /// The folder bound to `name`, or why there is none.
    fn folder(&self, name: &str) -> Result<Folder, String> {
        self.get(name).ok_or_else(|| unbound(name, &self.deployment))
    }

    /// File `file` of folder `name`, opened, and its metadata: none if there is no such file.
    pub(crate) fn open_file(&self, name: &str, file: &str) -> Result<Option<(std::fs::File, std::fs::Metadata)>, String> {
        let folder = self.folder(name)?;
        let Some(path) = existing(&folder.path, file)? else { return Ok(None) };
        let meta = std::fs::metadata(&path).map_err(|e| format!("{name}/{file}: {e}"))?;
        if !meta.is_file() {
            return Ok(None);
        }
        let f = std::fs::File::open(&path).map_err(|e| format!("{name}/{file} can't be read: {e}"))?;
        Ok(Some((f, meta)))
    }

    /// The bytes of file `file` in folder `name`: none if there is no such file; refused past `max`.
    pub(crate) fn read(&self, name: &str, file: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let Some((f, meta)) = self.open_file(name, file)? else { return Ok(None) };
        if meta.len() > max {
            return Err(format!("{name}/{file} is {} bytes: more than the {max} a run reads; read a handle, lines or a slice of it", meta.len()));
        }
        let mut bytes = Vec::new();
        f.take(max + 1).read_to_end(&mut bytes).map_err(|e| format!("{name}/{file} can't be read: {e}"))?;
        Ok(Some(bytes))
    }

    /// A handle to file `file` of folder `name`, of any size: `{folder, name, size, hash,
    /// modified}`, its bytes hashed and nothing kept. None if there is no such file.
    pub(crate) fn handle(&self, name: &str, file: &str) -> Result<Option<Value>, String> {
        let Some((mut f, meta)) = self.open_file(name, file)? else { return Ok(None) };
        let mut hasher = blake3::Hasher::new();
        let size = std::io::copy(&mut f, &mut hasher).map_err(|e| format!("{name}/{file} can't be read: {e}"))?;
        Ok(Some(Value::Record(BTreeMap::from([
            ("folder".to_string(), Value::Text(name.to_string())),
            ("name".to_string(), Value::Text(file.to_string())),
            ("size".to_string(), Value::Number(size as f64)),
            ("hash".to_string(), Value::Text(hasher.finalize().to_hex().to_string())),
            ("modified".to_string(), Value::Number(modified(&meta) as f64)),
        ]))))
    }

    /// Part of file `file` of folder `name` (`span`), up to `max` bytes: none if there is no such
    /// file. With `prove`, the whole file is hashed and the range comes with what proves it part of
    /// the file (ranges.rs); without, only the range is read.
    pub(crate) fn range(&self, name: &str, file: &str, span: Span, max: u64, prove: bool) -> Result<Option<RangeRead>, String> {
        let Some((mut f, meta)) = self.open_file(name, file)? else { return Ok(None) };
        let size = meta.len();
        let e = |e: std::io::Error| format!("{name}/{file} can't be read: {e}");
        let (start, end) = match span {
            Span::Lines { from, limit } => ranges::line_span(&mut f, size, from, limit).map_err(e)?,
            Span::Slice { offset, length } => ranges::slice_span(size, offset, length),
        };
        if end - start > max {
            return Err(format!("{name}/{file}: the {} bytes asked for are more than the {max} a run reads", end - start));
        }
        if !prove {
            let mut bytes = vec![0u8; (end - start) as usize];
            f.seek(std::io::SeekFrom::Start(start)).and_then(|_| f.read_exact(&mut bytes)).map_err(e)?;
            return Ok(Some(RangeRead { start, end, size, bytes, hash: String::new(), before: Vec::new(), after: Vec::new(), proof: Vec::new() }));
        }
        let (a, b) = ranges::widened(start, end, size);
        let (hash, mut kept, proof) = ranges::prove(&mut f, size, a, b).map_err(e)?;
        let after = kept.split_off((end - a) as usize);
        let bytes = kept.split_off((start - a) as usize);
        Ok(Some(RangeRead { start, end, size, bytes, hash: hash.to_hex().to_string(), before: kept, after, proof }))
    }

    /// Up to `limit` files of folder `name` in name order, by name (a path inside it, `/` between
    /// folders): those starting with `prefix`, after `after`. Each is `{size, hash, modified}`.
    pub(crate) fn list(&self, name: &str, prefix: &str, after: Option<&str>, limit: usize) -> Result<BTreeMap<String, Value>, String> {
        let folder = self.folder(name)?;
        let mut names = Vec::new();
        walk(&folder.path, "", &mut names)?;
        names.retain(|n| n.starts_with(prefix) && after.is_none_or(|a| n.as_str() > a));
        names.sort();
        names.truncate(limit);
        let mut files = BTreeMap::new();
        for n in names {
            let path = folder.path.join(&n);
            let meta = std::fs::metadata(&path).map_err(|e| format!("{name}/{n}: {e}"))?;
            let item = BTreeMap::from([
                ("size".to_string(), Value::Number(meta.len() as f64)),
                ("hash".to_string(), Value::Text(hash_of(&path).map_err(|e| format!("{name}/{n}: {e}"))?)),
                ("modified".to_string(), Value::Number(modified(&meta) as f64)),
            ]);
            files.insert(n, Value::Record(item));
        }
        Ok(files)
    }

    /// Write `text` as file `file` of folder `name`, whole: beside it first, then renamed over it.
    pub(crate) fn write(&self, name: &str, file: &str, text: &str) -> Result<(), String> {
        self.write_as(name, file, text, false)
    }

    /// As [`write`](Self::write), but only where no file has the name: never a replace.
    pub(crate) fn create(&self, name: &str, file: &str, text: &str) -> Result<(), String> {
        self.write_as(name, file, text, true)
    }

    fn write_as(&self, name: &str, file: &str, text: &str, create: bool) -> Result<(), String> {
        let folder = self.writable(name)?;
        let path = for_writing(&folder.path, file)?;
        fits(name, &folder, &path, text.len() as u64).map_err(|e| format!("{name}/{file} not written: {e}"))?;
        land(&path, create, |f| f.write_all(text.as_bytes()).map_err(|e| e.to_string())).map_err(|e| format!("{name}/{file} not written: {e}"))?;
        self.note_landed(name, file, &path);
        Ok(())
    }

    /// Copy file `from_file` of folder `from` as file `file` of folder `name`, streamed beside it
    /// and renamed over it: refused if its bytes no longer hash to `hash` (the handle the run read).
    pub(crate) fn copy(&self, name: &str, file: &str, from: &str, from_file: &str, hash: &str) -> Result<(), String> {
        self.copy_as(name, file, from, from_file, hash, false)
    }

    /// As [`copy`](Self::copy), but only where no file has the name: never a replace.
    pub(crate) fn create_copy(&self, name: &str, file: &str, from: &str, from_file: &str, hash: &str) -> Result<(), String> {
        self.copy_as(name, file, from, from_file, hash, true)
    }

    fn copy_as(&self, name: &str, file: &str, from: &str, from_file: &str, hash: &str, create: bool) -> Result<(), String> {
        let folder = self.writable(name)?;
        let Some((mut src, meta)) = self.open_file(from, from_file)? else {
            return Err(format!("{from}/{from_file} isn't there any more: not copied to {name}/{file}"));
        };
        let path = for_writing(&folder.path, file)?;
        fits(name, &folder, &path, meta.len()).map_err(|e| format!("{name}/{file} not copied: {e}"))?;
        land(&path, create, |f| {
            let mut hasher = blake3::Hasher::new();
            let mut block = vec![0u8; 64 * 1024];
            loop {
                let n = src.read(&mut block).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                hasher.update(&block[..n]);
                f.write_all(&block[..n]).map_err(|e| e.to_string())?;
            }
            if hasher.finalize().to_hex().as_str() != hash {
                return Err(format!("{from}/{from_file} changed since the run read it"));
            }
            Ok(())
        })
        .map_err(|e| format!("{name}/{file} not copied: {e}"))?;
        self.note_landed(name, file, &path);
        Ok(())
    }

    /// Delete file `file` of folder `name`; nothing to do if there is none.
    pub(crate) fn delete(&self, name: &str, file: &str) -> Result<(), String> {
        let folder = self.writable(name)?;
        match existing(&folder.path, file)? {
            Some(path) if path.is_file() => {
                // Noted first, so a watcher that finds it gone knows it was its own.
                if self.watched.lock().unwrap().contains(name) {
                    self.removed.lock().unwrap().insert((name.to_string(), file.to_string()));
                }
                std::fs::remove_file(&path).map_err(|e| format!("{name}/{file} not deleted: {e}"))
            }
            _ => Ok(()),
        }
    }

    /// Why a deployment asking for `manifest` can't run here, if it can't: a folder it needs that
    /// isn't bound, or is bound read-only when it writes. A folder it can run without
    /// (`optional`) may be missing.
    pub(crate) fn refuses(&self, manifest: &Manifest) -> Option<String> {
        for item in &manifest.capabilities {
            if manifest.requirements.optional.contains(item) {
                continue;
            }
            let (name, writes) = match (item.strip_prefix("FileRead(\""), item.strip_prefix("FileWrite(\"")) {
                (Some(rest), _) => (rest.strip_suffix("\")")?, false),
                (_, Some(rest)) => (rest.strip_suffix("\")")?, true),
                _ => continue,
            };
            match self.get(name) {
                None => return Some(unbound(name, &self.deployment)),
                Some(f) if writes && f.access != FolderAccess::ReadWrite => {
                    return Some(format!("the folder {name:?} is bound read-only on this server, and this flow writes it"));
                }
                Some(_) => {}
            }
        }
        None
    }

    fn writable(&self, name: &str) -> Result<Folder, String> {
        let folder = self.folder(name)?;
        if folder.access != FolderAccess::ReadWrite {
            return Err(format!("the folder {name:?} is bound read-only on this server"));
        }
        Ok(folder)
    }
}

/// Part of a file a run reads: lines, or bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Span {
    /// `limit` lines from line `from` (from the end when negative).
    Lines { from: i64, limit: u64 },
    /// `length` bytes from `offset` (from the end when negative).
    Slice { offset: i64, length: u64 },
}

/// Part of a file, as read: where it is, its bytes, and (when proved) the file's hash, the bytes
/// around it to whole chunks, and the hashes of the rest of the file (ranges.rs).
#[derive(Clone, Debug)]
pub(crate) struct RangeRead {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) size: u64,
    pub(crate) bytes: Vec<u8>,
    pub(crate) hash: String,
    pub(crate) before: Vec<u8>,
    pub(crate) after: Vec<u8>,
    pub(crate) proof: Vec<[u8; 32]>,
}

/// When a file was last changed, in milliseconds since 1970.
fn modified(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

/// The BLAKE3 of the file at `path` (hex), read through.
fn hash_of(path: &Path) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut std::fs::File::open(path)?, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Put a file at `path` whole: `fill` writes it beside it, then it is synced and renamed over
/// `path`; with `create`, linked to `path` instead, which fails if a file has that name by then
/// (whoever put it there), so nothing is replaced. A failed fill leaves nothing behind.
fn land(path: &Path, create: bool, fill: impl FnOnce(&mut std::fs::File) -> Result<(), String>) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder")?;
    let tmp = dir.join(format!("{TMP_PREFIX}{}-{}", std::process::id(), rand::random::<u64>()));
    let landed = (|| {
        if create && std::fs::symlink_metadata(path).is_ok() {
            return Err(EXISTS.to_string());
        }
        let mut f = std::fs::File::create_new(&tmp).map_err(|e| e.to_string())?;
        fill(&mut f)?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        if !create {
            return std::fs::rename(&tmp, path).map_err(|e| e.to_string());
        }
        let linked = std::fs::hard_link(&tmp, path).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => EXISTS.to_string(),
            _ => format!("{e} (a create-only write links the file into place)"),
        });
        let _ = std::fs::remove_file(&tmp);
        linked
    })();
    if landed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    landed
}

/// Why a create-only write didn't land.
const EXISTS: &str = "a file has that name already (create-only: it isn't replaced)";

/// Whether folder `name` has room for `size` bytes as the file at `path` (replacing what is there),
/// if it is capped: refused past its cap, saying how full it is.
fn fits(name: &str, folder: &Folder, path: &Path, size: u64) -> Result<(), String> {
    let Some(cap) = folder.cap else { return Ok(()) };
    let held = usage(&folder.path)?;
    let replaced = std::fs::symlink_metadata(path).ok().filter(std::fs::Metadata::is_file).map_or(0, |m| m.len());
    let after = held.saturating_sub(replaced).saturating_add(size);
    if after > cap {
        return Err(format!(
            "the folder {name:?} is capped at {}: it holds {}, and this would make it {}",
            size_text(cap),
            size_text(held),
            size_text(after)
        ));
    }
    Ok(())
}

/// The bytes of every file under `dir`, links not followed.
fn usage(dir: &Path) -> Result<u64, String> {
    let mut total = 0u64;
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let entries = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}

/// Why a deployment can't use the folder `name`: it isn't bound for it, nor for the server.
fn unbound(name: &str, deployment: &str) -> String {
    format!("the folder {name:?} isn't bound on this server: an admin binds it (holon folder add {name} <path>, or with --for {deployment} for this deployment only)")
}

/// Why `name` can't name a file in a folder, if it can't: a relative path, `/` between folders,
/// with nothing that leaves the folder or that Windows reads as something else.
pub(crate) fn bad_file_name(name: &str) -> Option<String> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
        "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let why = if name.is_empty() {
        "it is empty"
    } else if name.len() > 1024 {
        "it is longer than 1024 bytes"
    } else if name.contains('\\') {
        "it has a \\ (folders are separated by /)"
    } else if name.contains(':') {
        "it has a :"
    } else if name.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*')) {
        "it has a character no file name may have"
    } else if name.starts_with('/') {
        "it starts at the root (a name is inside the folder)"
    } else if name.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        "it has an empty, . or .. part"
    } else if name.split('/').any(|s| s.ends_with('.') || s.ends_with(' ')) {
        "a part of it ends in a dot or a space"
    } else if name.split('/').any(|s| RESERVED.contains(&s.split('.').next().unwrap_or("").to_ascii_uppercase().as_str())) {
        "a part of it is a name Windows reserves (CON, NUL, COM1, …)"
    } else if name.split('/').any(|s| s.starts_with(TMP_PREFIX)) {
        "it is one of the server's temporary files"
    } else {
        return None;
    };
    Some(format!("{name:?} can't name a file: {why}"))
}

/// The file `name` names in `root`, if there is one, after links: refused if it is outside `root`.
fn existing(root: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    if let Some(why) = bad_file_name(name) {
        return Err(why);
    }
    let joined = root.join(name);
    if std::fs::symlink_metadata(&joined).is_err() {
        return Ok(None);
    }
    let at = canonical(&joined).map_err(|e| format!("{name}: {e}"))?;
    if !at.starts_with(root) {
        return Err(format!("{name:?} leads outside its folder"));
    }
    Ok(Some(at))
}

/// Where to write file `name` in `root`: its folders made, and nothing on the way leading out.
fn for_writing(root: &Path, name: &str) -> Result<PathBuf, String> {
    if let Some(why) = bad_file_name(name) {
        return Err(why);
    }
    let joined = root.join(name);
    let dir = joined.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{name}: {e}"))?;
    let dir = canonical(dir).map_err(|e| format!("{name}: {e}"))?;
    if !dir.starts_with(root) {
        return Err(format!("{name:?} leads outside its folder"));
    }
    let path = dir.join(joined.file_name().ok_or("no file name")?);
    if let Ok(at) = canonical(&path)
        && !at.starts_with(root)
    {
        return Err(format!("{name:?} leads outside its folder"));
    }
    Ok(path)
}

/// The files under `dir`, by the BLAKE3 of their bytes (hex): what `holon verify --files` answers
/// a run's file reads from (a whole file, or part of one), when its log kept only their hash.
pub(crate) fn files_in(dir: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut names = Vec::new();
    walk(dir, "", &mut names)?;
    let mut files = BTreeMap::new();
    for name in names {
        let path = dir.join(&name);
        if let Ok(hash) = hash_of(&path) {
            files.insert(hash, path);
        }
    }
    Ok(files)
}

/// Every file under `dir` (as `/`-separated names after `under`) a flow may name. Links are never
/// followed, so a listing never leaves the folder.
fn walk(dir: &Path, under: &str, names: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let Some(base) = entry.file_name().to_str().map(str::to_string) else { continue };
        let name = if under.is_empty() { base } else { format!("{under}/{base}") };
        if kind.is_dir() {
            walk(&entry.path(), &name, names)?;
        } else if kind.is_file() && bad_file_name(&name).is_none() {
            names.push(name);
            if names.len() > MAX_WALK {
                return Err(format!("the folder has more than {MAX_WALK} files: list a part of it (prefix)"));
            }
        }
    }
    Ok(())
}

/// A binding as the admin panel and `holon folder list` show it: `deployment` is the slug it is
/// bound for, or null for the whole server.
fn shown(name: &str, deployment: Option<&str>, f: &Folder) -> serde_json::Value {
    serde_json::json!({ "name": name, "deployment": deployment, "path": f.path.display().to_string(), "access": f.access, "keep": f.keep, "cap": f.cap })
}

/// What a binding is called in the activity log: its name, and the deployment it is for.
fn labelled(name: &str, deployment: Option<&str>) -> String {
    deployment.map_or_else(|| name.to_string(), |d| format!("{name} (for {d})"))
}

/// `/admin/folders` (list, bind), `/admin/folders/<name>` (unbind; `?deployment=<slug>` for a
/// deployment's own), and the admin page's browse and pick (pick.rs), or None for any other path.
/// Only an admin gets this far (access.rs); what they do goes in the activity log.
pub(crate) fn route(state: &AppState, request: &Request, by: &str) -> Option<Response> {
    let url = request.url();
    let method = request.method();
    if url == "/admin/folders" {
        return Some(match method {
            "GET" => {
                let all = state.folders.all();
                let server = all.server.iter().map(|(n, f)| shown(n, None, f));
                let own = all.deployments.iter().flat_map(|(d, folders)| folders.iter().map(move |(n, f)| shown(n, Some(d), f)));
                Response::json(&server.chain(own).collect::<Vec<_>>())
            }
            "POST" => {
                #[derive(Deserialize)]
                struct Bind {
                    name: String,
                    path: String,
                    #[serde(default = "read_write")]
                    access: FolderAccess,
                    #[serde(default)]
                    keep: Keep,
                    /// Its cap: bytes, or a size (`10M`); none: uncapped.
                    #[serde(default)]
                    cap: Option<serde_json::Value>,
                    /// The deployment it is bound for; none: the whole server.
                    #[serde(default)]
                    deployment: Option<String>,
                }
                fn read_write() -> FolderAccess {
                    FolderAccess::ReadWrite
                }
                let bind = read_body(request)
                    .and_then(|b| serde_json::from_slice::<Bind>(&b).map_err(|e| err(400, format!("invalid body: {e}"))))
                    .map(|b| Bind { deployment: b.deployment.filter(|d| !d.trim().is_empty()), ..b });
                let bound = bind.and_then(|b| {
                    let cap = match &b.cap {
                        None | Some(serde_json::Value::Null) => None,
                        Some(serde_json::Value::Number(n)) => Some(n.as_u64().ok_or_else(|| err(400, "a cap is a whole number of bytes, or a size (10M)"))?),
                        Some(serde_json::Value::String(s)) if s.trim().is_empty() => None,
                        Some(serde_json::Value::String(s)) => Some(verify::budget_of(s).ok_or_else(|| err(400, format!("a cap is a size (1048576, 64K, 10M or 2G), not {s:?}")))?),
                        Some(_) => return Err(err(400, "a cap is a whole number of bytes, or a size (10M)")),
                    };
                    let folder = state.folders.bind(&b.name, &b.path, b.access, b.keep, cap, b.deployment.as_deref()).map_err(|e| err(400, e))?;
                    Ok((b, folder))
                });
                match bound {
                    Ok((b, folder)) => {
                        state.activity.record("bound the folder", &labelled(&b.name, b.deployment.as_deref()), Some(by));
                        Response::json(&shown(&b.name, b.deployment.as_deref(), &folder))
                    }
                    Err(e) => e,
                }
            }
            _ => Response::empty_404(),
        });
    }
    if url == "/admin/folders/sweep" && method == "POST" {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        return Some(match sweep(&state.folders, &state.deployments, now, SWEEP_SPARES_MS) {
            Ok(s) => {
                if s.texts > 0 || s.chunks > 0 {
                    state.activity.record("swept file texts", &format!("{} texts, {} freed", s.texts, size_text(s.bytes)), Some(by));
                }
                let held = state.folders.blobs.held();
                Response::json(&serde_json::json!({ "texts": s.texts, "chunks": s.chunks, "bytes": s.bytes, "kept": { "texts": held.texts, "chunks": held.chunks, "bytes": held.bytes } }))
            }
            Err(e) => err(500, e),
        });
    }
    if let Some(answer) = crate::pick::route(request) {
        return Some(answer);
    }
    let name = url.strip_prefix("/admin/folders/")?;
    let deployment = request.get_param("deployment").filter(|d| !d.is_empty());
    if method != "DELETE" {
        return Some(Response::empty_404());
    }
    Some(match state.folders.unbind(name, deployment.as_deref()) {
        Ok(true) => {
            state.activity.record("unbound the folder", &labelled(name, deployment.as_deref()), Some(by));
            Response::empty_204()
        }
        Ok(false) => err(404, format!("no folder {name:?} is bound{}", deployment.map_or(String::new(), |d| format!(" for {d}")))),
        Err(e) => err(500, e),
    })
}

/// `holon folder add NAME PATH [--read-only] [--keep hash|30d] [--cap SIZE] | list | remove NAME |
/// sweep`: bind, show and unbind the folders of the server this user runs here (or `--server`),
/// and sweep the texts their runs read that no kept run refers to, as an admin.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut server = None;
    let mut for_deployment: Option<String> = None;
    let (mut read_only, mut keep, mut cap) = (false, Keep::Bytes, None);
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--for" => for_deployment = args.next().cloned(),
            "--read-only" => read_only = true,
            "--keep" => match args.next().map(|k| Keep::try_from(k.clone())) {
                Some(Ok(k)) => keep = k,
                _ => {
                    eprintln!("holon folder: --keep takes bytes, hash, or how long to keep the bytes (30d, 12h; s, m, h or d)");
                    return 2;
                }
            },
            "--cap" => match args.next().and_then(|v| verify::budget_of(v)) {
                Some(n) if n > 0 => cap = Some(n),
                _ => {
                    eprintln!("holon folder: --cap takes a size: 1048576, 64K, 10M or 2G");
                    return 2;
                }
            },
            "--help" | "-h" => {
                println!("{FOLDER_USAGE}");
                return 0;
            }
            _ if !a.starts_with('-') => words.push(a.as_str()),
            _ => {
                eprintln!("holon folder: no option {a:?}\n\n{FOLDER_USAGE}");
                return 2;
            }
        }
    }
    let (method, path, body) = match words.as_slice() {
        ["list"] => ("GET", "/admin/folders".to_string(), None),
        ["add", name, path] => {
            // The path as the server's machine sees it: this one, for the server this user runs.
            let path = std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.display().to_string());
            let access = if read_only { FolderAccess::Read } else { FolderAccess::ReadWrite };
            let body = serde_json::json!({ "name": name, "path": path, "access": access, "keep": keep, "cap": cap, "deployment": for_deployment });
            ("POST", "/admin/folders".to_string(), Some(body))
        }
        ["sweep"] => ("POST", "/admin/folders/sweep".to_string(), Some(serde_json::json!({}))),
        ["remove", name] => {
            let only = for_deployment.as_ref().map_or(String::new(), |d| format!("?deployment={d}"));
            ("DELETE", format!("/admin/folders/{name}{only}"), None)
        }
        _ => {
            eprintln!("{FOLDER_USAGE}");
            return 2;
        }
    };
    let target = match local::target(server.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("holon folder: {e}");
            return 1;
        }
    };
    let server = target.server.as_str();
    let mut request = ureq::request(method, &format!("{server}{path}")).timeout(Duration::from_secs(30));
    if let Some(token) = &target.token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let answered = match &body {
        Some(body) => request.set("Content-Type", "application/json").send_string(&body.to_string()),
        None => request.call(),
    };
    let answer = match answered {
        Ok(r) => r.into_string().unwrap_or_default(),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let why = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| body.trim().to_string());
            eprintln!("holon folder: {server} answered {code}: {why}");
            return 1;
        }
        Err(e) => {
            eprintln!("holon folder: can't reach {server}: {e}");
            return 1;
        }
    };
    let line = |f: &serde_json::Value| {
        let access = if f["access"] == "read" { "read only" } else { "read and write" };
        let keep = match f["keep"].as_str() {
            Some("hash") => ", reads kept by hash".to_string(),
            Some(k) if k.starts_with("bytes for ") => format!(", reads kept {}, then by hash", &k["bytes for ".len()..]),
            _ => String::new(),
        };
        let cap = f["cap"].as_u64().map_or(String::new(), |c| format!(", capped at {}", size_text(c)));
        let only = f["deployment"].as_str().map_or(String::new(), |d| format!(", for {d} only"));
        format!("{}  {}  ({access}{keep}{cap}{only})", f["name"].as_str().unwrap_or(""), f["path"].as_str().unwrap_or(""))
    };
    match words[0] {
        "list" => {
            let all: Vec<serde_json::Value> = serde_json::from_str(&answer).unwrap_or_default();
            if all.is_empty() {
                println!("no folders are bound on {server}: holon folder add NAME PATH binds one");
            }
            for f in &all {
                println!("{}", line(f));
            }
        }
        "add" => println!("bound {}", line(&serde_json::from_str(&answer).unwrap_or_default())),
        "sweep" => {
            let s: serde_json::Value = serde_json::from_str(&answer).unwrap_or_default();
            let n = |v: &serde_json::Value| v.as_u64().unwrap_or(0);
            println!(
                "swept {} file texts no kept run refers to ({} freed); it keeps {} ({})",
                n(&s["texts"]),
                size_text(n(&s["bytes"])),
                n(&s["kept"]["texts"]),
                size_text(n(&s["kept"]["bytes"]))
            );
        }
        _ => println!(
            "unbound {}{}: its files stay where they are",
            words[1],
            for_deployment.as_ref().map_or(String::new(), |d| format!(" (for {d})"))
        ),
    }
    0
}

const FOLDER_USAGE: &str = "usage: holon folder add NAME PATH [--read-only] [--keep hash|30d] [--cap SIZE] [--for SLUG]
       holon folder list
       holon folder remove NAME [--for SLUG]
       holon folder sweep
       (each takes --server PORT|URL)

  Binds a name a flow uses (folder inbox) to a real folder on the server's machine, for an admin.
  A flow reads and writes files only in the folders it names, and only by name: never a path.
  --for SLUG binds it for that deployment only: it sees that folder as NAME, whatever the
  server's NAME is, and no other deployment sees it.
  --read-only: flows may read it, not write it. --keep hash: the run log keeps a hash of what
  each run read, not its text (holon verify --files then supplies the files); --keep 30d keeps
  the text for 30 days (s, m, h or d), then only its hash. --cap 10M: a write that would take the
  folder's files past 10 MiB in all doesn't land. Removing a binding leaves the files where they
  are; deployments that use it stop until it is bound again.
  sweep removes now the texts runs read that no kept run refers to (the server does it every 10
  minutes).";

/// What a run's read of a file kept: its hash and size, and where its text is if not in the log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Seen {
    /// BLAKE3 of the file's bytes (hex): what `holon verify --files` matches a file by.
    pub(crate) hash: String,
    pub(crate) size: u64,
    /// Whether the text read (the file's, or a range's) is in the log entry itself.
    pub(crate) inline: bool,
    /// What names the text in the blob store (the BLAKE3 of the text, hex), when kept there.
    pub(crate) blob: Option<String>,
    /// For part of a file (lines, a slice): where it is, and what proves it part of the file.
    pub(crate) range: Option<Ranged>,
}

/// Part of a file a run read, as its log keeps it: where it is; its text when the entry holds it
/// (else by `blob`, or not at all for a folder that keeps hashes); and, when the folder keeps
/// bytes, the bytes around it to whole chunks and the hashes of the rest of the file (ranges.rs).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Ranged {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) text: Option<String>,
    pub(crate) before: Vec<u8>,
    pub(crate) after: Vec<u8>,
    pub(crate) proof: Vec<[u8; 32]>,
}

/// What a range read answers: a slice's text, or the lines of it.
pub(crate) fn range_answer(lines: bool, text: &str) -> Value {
    if lines { Value::List(ranges::lines_of(text).into_iter().map(Value::Text).collect()) } else { Value::Text(text.to_string()) }
}

/// The folders one run reads: which it may, and how much it has read so far.
pub(crate) struct Readable {
    pub(crate) folders: Arc<Scoped>,
    pub(crate) granted: BTreeSet<String>,
    /// What the run may read (its deployment's file limits).
    pub(crate) limits: crate::permissions::FileLimits,
    pub(crate) read_bytes: u64,
    /// The files it has read, or taken a handle to, so far.
    pub(crate) files: u64,
}

impl std::fmt::Debug for Readable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Readable").field("granted", &self.granted).field("read_bytes", &self.read_bytes).field("files", &self.files).finish()
    }
}

/// The performer of a deployment's runs: its outbound rules (outbound.rs), with a file as a
/// request's body (`http_upload`) read from a folder the deployment reads, as the run's handle
/// names it.
pub(crate) struct Sender<'a> {
    pub(crate) outbound: &'a Outbound,
    pub(crate) folders: Option<Arc<Scoped>>,
    /// The folders it reads, by name.
    pub(crate) granted: BTreeSet<String>,
    /// The route's caller, for a streamed call's answer (`http_stream`), when the run answers one.
    pub(crate) forward: Option<&'a outbound::Forwarder>,
    /// The deployment's channels, for a piped call's answer, and the ones it may send on.
    pub(crate) channels: Option<Arc<channels::Channels>>,
    pub(crate) sends: BTreeSet<String>,
}

/// A file a request sends, as the run's handle names it.
struct Sent<'a> {
    folders: &'a Scoped,
    folder: &'a str,
    name: &'a str,
    size: u64,
    hash: &'a str,
}

impl FileBody for Sent<'_> {
    fn size(&self) -> u64 {
        self.size
    }

    fn hash(&self) -> &str {
        self.hash
    }

    /// The file, checked whole against the handle before a byte of it is sent (and again as it
    /// is: outbound.rs).
    fn open(&self) -> Result<Box<dyn std::io::Read + Send + '_>, Value> {
        let (folder, name) = (self.folder, self.name);
        let unreadable = |e: std::io::Error| outbound::failure("file", format!("{folder}/{name} can't be read: {e}"));
        let changed = || outbound::failure("changed", format!("{folder}/{name} changed since the run read it"));
        let Some((mut f, meta)) = self.folders.open_file(folder, name).map_err(|e| outbound::failure("file", e))? else {
            return Err(outbound::failure("changed", format!("{folder}/{name} isn't there any more")));
        };
        if meta.len() != self.size {
            return Err(changed());
        }
        let mut hasher = blake3::Hasher::new();
        std::io::copy(&mut f, &mut hasher).map_err(unreadable)?;
        if hasher.finalize().to_hex().as_str() != self.hash {
            return Err(changed());
        }
        f.seek(std::io::SeekFrom::Start(0)).map_err(unreadable)?;
        Ok(Box::new(f))
    }
}

impl Performer for Sender<'_> {
    fn perform(&self, request: &Value, limits: &Limits) -> Value {
        let Value::Record(r) = request else { return self.outbound.perform(request, limits) };
        if r.get("stream") == Some(&Value::Boolean(true)) {
            return self.outbound.perform_forwarding(request, limits, self.forward.and_then(|f| f.take()));
        }
        // Piped: its answer goes to the channel's subscribers as it is read.
        if let Some(Value::Text(name)) = r.get("pipe") {
            if !self.sends.contains(name) {
                return outbound::failure("blocked_channel", format!("this deployment may not send on the channel {name:?}"));
            }
            let Some(channels) = &self.channels else { return self.outbound.perform(request, limits) };
            let pipe = channels::Pipe::new(channels, name);
            return self.outbound.perform_forwarding(request, limits, Some(&pipe));
        }
        let Some(Value::Record(file)) = r.get("file") else { return self.outbound.perform(request, limits) };
        let (Some(Value::Text(method)), Some(Value::Text(url)), Some(Value::Record(headers))) = (r.get("method"), r.get("url"), r.get("headers")) else {
            return outbound::failure("bad_request", "a request needs method, url and headers");
        };
        let (Some(folder), Some(name), Some(hash), Some(Value::Number(size))) = (text(file, "folder"), text(file, "name"), text(file, "hash"), file.get("size")) else {
            return outbound::failure("bad_request", "a file sent names its folder, its name, its size and its hash");
        };
        // A handle is only a record: one can be made up, so the folder must be one it reads.
        if !self.granted.contains(folder) {
            return outbound::failure("blocked_file", format!("this deployment doesn't read the folder {folder:?}"));
        }
        let Some(folders) = &self.folders else { return outbound::failure("blocked_file", "no folders on this server") };
        let sent = Sent { folders, folder, name, size: *size as u64, hash };
        self.outbound.send(method, url, headers, Body::File(&sent), limits, None)
    }
}

fn text<'v>(r: &'v BTreeMap<String, Value>, k: &str) -> Option<&'v str> {
    match r.get(k) {
        Some(Value::Text(t)) => Some(t),
        _ => None,
    }
}

impl Readable {
    /// What its deployment lets a run read.
    pub(crate) fn new(folders: Arc<Scoped>, permit: &Permit) -> Readable {
        Readable { folders, granted: permit.folders_read(), limits: permit.files.clone(), read_bytes: 0, files: 0 }
    }

    /// One more file read: refused past `max_files_per_run`.
    fn one_more_file(&mut self) -> Result<(), String> {
        self.files += 1;
        if self.files > self.limits.max_files_per_run {
            return Err(format!("a run reads at most {} files (max_files_per_run)", self.limits.max_files_per_run));
        }
        Ok(())
    }

    /// The most the next read may take, and the limit that says so: a file's text is at most
    /// `inline_file_bytes`, and a run's reads `max_file_read_bytes` in all.
    fn read_limit(&self) -> (u64, &'static str) {
        let left = self.limits.max_file_read_bytes.saturating_sub(self.read_bytes);
        if left < self.limits.inline_file_bytes { (left, "max_file_read_bytes") } else { (self.limits.inline_file_bytes, "inline_file_bytes") }
    }

    /// The answer to `request` (`{folder, read}`, `{folder, list}`, `{folder, handle}`, `{folder,
    /// lines}` or `{folder, slice}`), and for a read of text, what it saw: an error fails the run (a
    /// folder not granted or not bound, a name that leaves it, a file too big or not text). With
    /// `keep`, a big text is put in the blob store, and a range is proved part of its file; without
    /// (checking a run before it commits), only what it answers is read.
    pub(crate) fn answer(&mut self, request: &Value, keep: bool) -> Result<(Option<Value>, Option<Seen>), String> {
        let Value::Record(r) = request else { return Ok((None, None)) };
        let Some(name) = text(r, "folder") else { return Ok((None, None)) };
        if !self.granted.contains(name) {
            return Err(format!("the folder {name:?} isn't granted to this deployment"));
        }
        if let Some(file) = text(r, "handle") {
            self.one_more_file()?;
            return Ok((self.folders.handle(name, file)?, None));
        }
        let ranged = match (r.get("lines"), r.get("slice")) {
            (Some(Value::Record(q)), _) => Some((q, true)),
            (_, Some(Value::Record(q))) => Some((q, false)),
            _ => None,
        };
        if let Some((q, lines)) = ranged {
            let whole = |k: &str| match q.get(k) {
                Some(Value::Number(n)) if n.fract() == 0.0 => Some(*n as i64),
                _ => None,
            };
            let (Some(file), Some(a), Some(b)) = (text(q, "name"), whole(if lines { "from" } else { "offset" }), whole(if lines { "limit" } else { "length" })) else {
                return Ok((None, None));
            };
            let span = if lines { Span::Lines { from: a, limit: b.max(0) as u64 } } else { Span::Slice { offset: a, length: b.max(0) as u64 } };
            self.one_more_file()?;
            let (max, limit) = self.read_limit();
            let Some(read) = self.folders.range(name, file, span, max, keep).map_err(|e| past(e, limit))? else { return Ok((None, None)) };
            self.read_bytes += read.bytes.len() as u64;
            let (start, end) = (read.start, read.end);
            let text = String::from_utf8(read.bytes).map_err(|_| format!("{name}/{file} bytes {start}..{end} aren't text (UTF-8): read a handle of it"))?;
            let answer = range_answer(lines, &text);
            if !keep {
                return Ok((Some(answer), None));
            }
            let kept = self.folders.get(name).map_or(Keep::Bytes, |f| f.keep).keeps_bytes();
            let inline = kept && text.len() as u64 <= LOG_INLINE_BYTES;
            let blob = if kept && !inline {
                Some(self.folders.all.blobs.put(text.as_bytes()).map_err(|e| format!("{name}/{file} can't be kept: {e}"))?)
            } else {
                None
            };
            let proved = kept;
            let range = Ranged {
                start,
                end,
                text: inline.then_some(text),
                before: if proved { read.before } else { Vec::new() },
                after: if proved { read.after } else { Vec::new() },
                proof: if proved { read.proof } else { Vec::new() },
            };
            return Ok((Some(answer), Some(Seen { hash: read.hash, size: read.size, inline, blob, range: Some(range) })));
        }
        if let Some(file) = text(r, "read") {
            self.one_more_file()?;
            let (max, limit) = self.read_limit();
            let Some(bytes) = self.folders.read(name, file, max).map_err(|e| past(e, limit))? else { return Ok((None, None)) };
            self.read_bytes += bytes.len() as u64;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            let size = bytes.len() as u64;
            let text = String::from_utf8(bytes).map_err(|_| format!("{name}/{file} isn't text (UTF-8)"))?;
            let kept = self.folders.get(name).map_or(Keep::Bytes, |f| f.keep).keeps_bytes();
            let inline = kept && size <= LOG_INLINE_BYTES;
            let blob = if keep && kept && !inline {
                Some(self.folders.all.blobs.put(text.as_bytes()).map_err(|e| format!("{name}/{file} can't be kept: {e}"))?)
            } else {
                None
            };
            let value = Value::Text(text);
            return Ok((Some(value), Some(Seen { hash, size, inline, blob, range: None })));
        }
        let Some(Value::Record(list)) = r.get("list") else { return Ok((None, None)) };
        let limit = match list.get("limit") {
            Some(Value::Number(n)) => *n as usize,
            _ => return Ok((None, None)),
        };
        let files = self.folders.list(name, text(list, "prefix").unwrap_or(""), text(list, "after"), limit)?;
        Ok((Some(Value::Record(files)), None))
    }
}

/// A read refused for its size, saying which limit refused it.
fn past(e: String, limit: &str) -> String {
    if e.contains("more than the") { format!("{e} ({limit})") } else { e }
}

/// Why a run's file writes are past its limits, if they are: what it writes and copies (a copy
/// is its handle's size), and how many files it touches, the `read` it read included.
pub(crate) fn past_write_limits(performed: &[(Capability, Value)], read: u64, limits: &crate::permissions::FileLimits) -> Option<String> {
    let (mut bytes, mut files) = (0u64, read);
    for (cap, payload) in performed {
        if !matches!(cap, Capability::FileWrite(_)) {
            continue;
        }
        files += 1;
        let Value::Record(r) = payload else { continue };
        bytes = bytes.saturating_add(match (r.get("text"), r.get("from")) {
            (Some(Value::Text(t)), _) => t.len() as u64,
            (_, Some(Value::Record(h))) => match h.get("size") {
                Some(Value::Number(n)) if *n >= 0.0 => *n as u64,
                _ => 0,
            },
            _ => 0,
        });
    }
    if bytes > limits.max_file_write_bytes {
        return Some(format!("it writes {bytes} bytes to files, more than the {} a run may (max_file_write_bytes)", limits.max_file_write_bytes));
    }
    if files > limits.max_files_per_run {
        return Some(format!("it touches {files} files, more than the {} a run may (max_files_per_run)", limits.max_files_per_run));
    }
    None
}

/// The text a log entry's `blob` names: from the blob store, or, for a log from before it, the
/// content store (where a read's text was kept by its value hash). None if it is no longer kept.
pub(crate) fn kept_text(folders: &Folders, store: &SharedStore, blob: &str) -> Result<Option<String>, String> {
    if let Some(bytes) = folders.blobs.get(blob)? {
        return String::from_utf8(bytes).map(Some).map_err(|_| format!("the kept text {blob} isn't text"));
    }
    let Some(hash) = parse_hash(blob) else { return Ok(None) };
    match store.0.lock().unwrap().get_value(&hash) {
        Ok(Some(Value::Text(text))) => Ok(Some(text)),
        Ok(_) => Ok(None),
        Err(e) => Err(format!("store error: {e:?}")),
    }
}

/// How often the server sweeps the texts no kept run refers to.
pub(crate) const SWEEP_EVERY: Duration = Duration::from_secs(10 * 60);
/// How new a text a sweep spares, whoever refers to it: a run that has just kept one logs it when
/// it finishes.
pub(crate) const SWEEP_SPARES_MS: u64 = 5 * 60 * 1000;

/// Remove from the blob store every text (and the chunks only it used) that no run a deployment's
/// log keeps refers to within its folder's window (`keep: bytes for 30d`; a folder now kept by
/// hash keeps none; one no longer bound keeps them all), except those kept in the last
/// `spare_ms`. A run whose text is swept stays provable by its hash; `holon verify` then runs it
/// again only with the file given.
pub(crate) fn sweep(folders: &Folders, deployments: &Mutex<BTreeMap<String, Deployment>>, now_ms: u64, spare_ms: u64) -> Result<blobs::Swept, String> {
    let logs: Vec<(String, Vec<String>)> = deployments.lock().unwrap().iter().map(|(slug, d)| (slug.clone(), d.log.lines())).collect();
    let mut live = BTreeSet::new();
    for (slug, lines) in &logs {
        for entry in lines.iter().filter_map(|l| serde_json::from_str::<LogEntry>(l).ok()) {
            for read in &entry.reads {
                let Some(blob) = read.file.as_ref().and_then(|f| f.blob.as_ref()) else { continue };
                let folder = match &read.request {
                    SValue::Record(pairs) => pairs.iter().find(|(k, _)| k == "folder").and_then(|(_, v)| match v {
                        SValue::Text(t) => Some(t.as_str()),
                        _ => None,
                    }),
                    _ => None,
                };
                let window = folder.and_then(|n| folders.resolve(slug, n)).and_then(|f| f.keep.window_ms());
                if window.is_none_or(|w| entry.at_ms.saturating_add(w) > now_ms) {
                    live.insert(blob.clone());
                }
            }
        }
    }
    folders.blobs.sweep(&live, now_ms.saturating_sub(spare_ms))
}

/// The folder a copy's handle names, for a performed `FileWrite` that is a copy (`{name, from}`):
/// a folder the deployment must be granted to read.
pub(crate) fn copied_from(payload: &Value) -> Option<&str> {
    let Value::Record(copy) = payload else { return None };
    let Some(Value::Record(from)) = copy.get("from") else { return None };
    text(from, "folder")
}

/// Apply a performed `FileWrite(name)`: a write's `{name, text}`, a copy's `{name, from}` (a
/// handle), or a delete's name.
pub(crate) fn apply(folders: &Scoped, name: &str, payload: &Value) -> Result<(), String> {
    match payload {
        Value::Record(file) if file.contains_key("from") => {
            let (Some(Value::Text(file)), Some(Value::Record(from))) = (file.get("name"), file.get("from")) else {
                return Err("a copy needs a name and a handle".into());
            };
            let (Some(folder), Some(source), Some(hash)) = (text(from, "folder"), text(from, "name"), text(from, "hash")) else {
                return Err("a copy's handle names a folder, a file and its hash".into());
            };
            if creates(payload) {
                folders.create_copy(name, file, folder, source, hash)
            } else {
                folders.copy(name, file, folder, source, hash)
            }
        }
        Value::Record(file) => {
            let (Some(Value::Text(file)), Some(Value::Text(text))) = (file.get("name"), file.get("text")) else {
                return Err("a write needs a name and a text".into());
            };
            if creates(payload) {
                folders.create(name, file, text)
            } else {
                folders.write(name, file, text)
            }
        }
        Value::Text(file) => folders.delete(name, file),
        _ => Err("not a write or a delete".into()),
    }
}

/// Whether a write's or a copy's payload is create-only (`file_create`, `file_create_copy`).
fn creates(payload: &Value) -> bool {
    matches!(payload, Value::Record(r) if r.get("create") == Some(&Value::Boolean(true)))
}

/// What the log keeps of a performed effect: a file write as `{name, size, hash}`, never its text;
/// everything else as it is.
pub(crate) fn loggable(cap: &Capability, payload: &Value) -> Value {
    match cap {
        Capability::FileWrite(_) => by_hash(payload),
        _ => payload.clone(),
    }
}

/// A file write's `{name, text}` as `{name, size, hash}` (and `create`, for a create-only one);
/// anything else as it is.
fn by_hash(payload: &Value) -> Value {
    if let Value::Record(file) = payload
        && let (Some(name), Some(Value::Text(text))) = (file.get("name"), file.get("text"))
    {
        let mut kept = BTreeMap::from([
            ("name".to_string(), name.clone()),
            ("size".to_string(), Value::Number(text.len() as f64)),
            ("hash".to_string(), Value::Text(blake3::hash(text.as_bytes()).to_hex().to_string())),
        ]);
        if creates(payload) {
            kept.insert("create".to_string(), Value::Boolean(true));
        }
        return Value::Record(kept);
    }
    payload.clone()
}

/// A run's output as the log keeps it: each file write it carries (an effect, as settled) by its
/// hash, as [`loggable`] keeps the write performed.
pub(crate) fn loggable_output(v: &Value) -> Value {
    match v {
        Value::Effect { sink, payload } if sink.starts_with("file_write:") || sink.starts_with("file_create:") => {
            Value::Effect { sink: sink.clone(), payload: Box::new(by_hash(payload)) }
        }
        Value::Record(r) => Value::Record(r.iter().map(|(k, v)| (k.clone(), loggable_output(v))).collect()),
        Value::List(l) => Value::List(l.iter().map(loggable_output).collect()),
        Value::Sum { arm, payload } => Value::Sum { arm: *arm, payload: Box::new(loggable_output(payload)) },
        v => v.clone(),
    }
}

/// The texts of the files a run wrote that are past the inline size: the log keeps them by hash
/// only, so nothing else in the entry may carry them.
pub(crate) fn written_texts(performed: &[(Capability, Value)]) -> Vec<&str> {
    performed
        .iter()
        .filter_map(|(c, p)| match (c, p) {
            (Capability::FileWrite(_), Value::Record(f)) => match f.get("text") {
                Some(Value::Text(t)) if t.len() as u64 > LOG_INLINE_BYTES => Some(t.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-folders-{tag}-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        real(&dir)
    }

    #[test]
    fn a_name_stays_inside_its_folder() {
        for ok in ["a.csv", "2026/10/a.csv", "x y.txt", "über.md", ".env"] {
            assert_eq!(bad_file_name(ok), None, "{ok}");
        }
        for bad in [
            "", "/etc/passwd", "../x", "a/../../x", "a//b", "./a", "C:x", "a\\b", "con", "NUL.txt", "a/COM1.log", "a.", "b ",
            "x?", ".holon-tmp-1",
        ] {
            assert!(bad_file_name(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn a_bound_folder_reads_writes_lists_and_deletes_and_nothing_leaves_it() {
        let data = temp("data");
        let all = Arc::new(Folders::open(":memory:", std::slice::from_ref(&data)).unwrap());
        let folders = Scoped::new(Arc::clone(&all), "app");
        let inbox = temp("inbox");
        assert!(all.bind("inbox", "relative/path", FolderAccess::Read, Keep::Bytes, None, None).unwrap_err().contains("absolute"));
        assert!(all.bind("inbox", data.to_str().unwrap(), FolderAccess::Read, Keep::Bytes, None, None).unwrap_err().contains("own data"));
        assert!(all.bind("all", data.parent().unwrap().to_str().unwrap(), FolderAccess::Read, Keep::Bytes, None, None).unwrap_err().contains("holds"));
        all.bind("inbox", inbox.to_str().unwrap(), FolderAccess::Read, Keep::Bytes, None, None).unwrap();
        let out = temp("out");
        all.bind("out", out.to_str().unwrap(), FolderAccess::ReadWrite, Keep::Hash, None, None).unwrap();

        std::fs::create_dir_all(inbox.join("2026")).unwrap();
        std::fs::write(inbox.join("2026/a.csv"), "x,1\ny,2\n").unwrap();
        std::fs::write(inbox.join("b.txt"), "b").unwrap();
        assert_eq!(folders.read("inbox", "2026/a.csv", MAX_FILE_BYTES).unwrap().unwrap(), b"x,1\ny,2\n");
        assert_eq!(folders.read("inbox", "none.csv", MAX_FILE_BYTES).unwrap(), None);
        assert!(folders.read("inbox", "2026/a.csv", 3).unwrap_err().contains("more than the 3"));
        assert!(folders.read("inbox", "../out/x", MAX_FILE_BYTES).is_err());
        assert!(folders.read("nope", "a", MAX_FILE_BYTES).unwrap_err().contains("isn't bound"));
        let listed = folders.list("inbox", "", None, 10).unwrap();
        assert_eq!(listed.keys().collect::<Vec<_>>(), ["2026/a.csv", "b.txt"]);
        assert_eq!(folders.list("inbox", "2026/", None, 10).unwrap().len(), 1);
        assert_eq!(folders.list("inbox", "", Some("2026/a.csv"), 10).unwrap().keys().collect::<Vec<_>>(), ["b.txt"]);

        assert!(folders.write("inbox", "c.txt", "no").unwrap_err().contains("read-only"));
        folders.write("out", "r/report.md", "# total 3").unwrap();
        assert_eq!(std::fs::read_to_string(out.join("r/report.md")).unwrap(), "# total 3");
        folders.write("out", "r/report.md", "# total 4").unwrap();
        assert_eq!(std::fs::read_to_string(out.join("r/report.md")).unwrap(), "# total 4", "replaced whole");
        assert_eq!(folders.list("out", "", None, 10).unwrap().len(), 1, "no temporary file left");
        folders.delete("out", "r/report.md").unwrap();
        folders.delete("out", "r/report.md").unwrap();
        assert!(!out.join("r/report.md").exists());

        // A copy by handle: whole, and only while the file still has the handle's hash.
        let Some(Value::Record(h)) = folders.handle("inbox", "2026/a.csv").unwrap() else { panic!() };
        let Some(Value::Text(hash)) = h.get("hash") else { panic!() };
        assert_eq!((h["size"].clone(), hash.as_str()), (Value::Number(8.0), blake3::hash(b"x,1\ny,2\n").to_hex().as_str()));
        assert_eq!(folders.handle("inbox", "none.csv").unwrap(), None);
        folders.copy("out", "copy.csv", "inbox", "2026/a.csv", hash).unwrap();
        assert_eq!(std::fs::read(out.join("copy.csv")).unwrap(), b"x,1\ny,2\n");
        std::fs::write(inbox.join("2026/a.csv"), "x,1\ny,3\n").unwrap();
        assert!(folders.copy("out", "late.csv", "inbox", "2026/a.csv", hash).unwrap_err().contains("changed since"));
        assert!(!out.join("late.csv").exists() && folders.list("out", "", None, 10).unwrap().len() == 1, "nothing left of it");
        let tail = folders.range("inbox", "2026/a.csv", Span::Lines { from: -1, limit: 1 }, MAX_FILE_BYTES, true).unwrap().unwrap();
        assert_eq!((tail.bytes.as_slice(), tail.start, tail.end), (&b"y,3\n"[..], 4, 8));
        assert!(folders.range("inbox", "2026/a.csv", Span::Slice { offset: 0, length: 8 }, 3, true).unwrap_err().contains("more than the 3"));
        folders.delete("out", "copy.csv").unwrap();

        let write = Value::Record(BTreeMap::from([("name".into(), Value::Text("a.md".into())), ("text".into(), Value::Text("hi".into()))]));
        let logged = loggable(&Capability::FileWrite("out".into()), &write);
        let Value::Record(l) = logged else { panic!() };
        assert_eq!((l.get("text"), l["size"].clone()), (None, Value::Number(2.0)), "the log keeps no text");
        for d in [data, inbox, out] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn a_deployment_sees_its_own_binding_of_a_name_and_no_one_else_does() {
        let (shared, own) = (temp("shared"), temp("own"));
        let file = std::env::temp_dir().join(format!("holon-folders-{}-{}.json", std::process::id(), rand::random::<u32>()));
        let all = Arc::new(Folders::open(file.to_str().unwrap(), &[]).unwrap());
        all.bind("inbox", shared.to_str().unwrap(), FolderAccess::Read, Keep::Bytes, None, None).unwrap();
        all.bind("inbox", own.to_str().unwrap(), FolderAccess::ReadWrite, Keep::Hash, None, Some("billing")).unwrap();
        assert!(all.bind("inbox", own.to_str().unwrap(), FolderAccess::Read, Keep::Bytes, None, Some("../x")).is_err(), "a slug is a name");
        let (billing, other) = (Scoped::new(Arc::clone(&all), "billing"), Scoped::new(Arc::clone(&all), "other"));
        assert_eq!(billing.get("inbox").unwrap().path, own);
        assert_eq!(other.get("inbox").unwrap().path, shared, "the server's, for everyone else");
        billing.write("inbox", "a.txt", "mine").unwrap();
        assert!(other.write("inbox", "a.txt", "no").unwrap_err().contains("read-only"));
        assert_eq!(other.read("inbox", "a.txt", MAX_FILE_BYTES).unwrap(), None, "nor does it see billing's files");

        // Kept as the file was before (the server's at the top), a deployment's beside them.
        let on_disk: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert!(on_disk["inbox"]["path"].is_string() && on_disk["@deployments"]["billing"]["inbox"]["path"].is_string(), "{on_disk}");
        let again = Folders::open(file.to_str().unwrap(), &[]).unwrap();
        assert_eq!(again.all(), all.all());

        assert!(all.unbind("inbox", Some("billing")).unwrap());
        assert!(!all.unbind("inbox", Some("billing")).unwrap());
        assert_eq!(billing.get("inbox").unwrap().path, shared, "unbound: the server's again");
        assert!(all.all().deployments.is_empty());
        let only_billing = Arc::new(Folders::memory());
        only_billing.bind("logs", own.to_str().unwrap(), FolderAccess::Read, Keep::Bytes, None, Some("billing")).unwrap();
        assert!(Scoped::new(only_billing, "other").read("logs", "a.txt", 10).unwrap_err().contains("--for other"));
        for d in [shared, own] {
            let _ = std::fs::remove_dir_all(d);
        }
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_link_out_of_the_folder_is_never_followed() {
        let inbox = temp("links");
        let outside = temp("outside");
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, inbox.join("out")).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, inbox.join("out")).is_ok();
        if linked {
            let folders = Scoped::memory();
            folders.all.bind("inbox", inbox.to_str().unwrap(), FolderAccess::ReadWrite, Keep::Bytes, None, None).unwrap();
            assert!(folders.read("inbox", "out/secret.txt", MAX_FILE_BYTES).unwrap_err().contains("outside"));
            assert!(folders.write("inbox", "out/x.txt", "x").unwrap_err().contains("outside"));
            assert!(folders.list("inbox", "", None, 10).unwrap().is_empty(), "a listing doesn't follow the link");
        }
        let _ = std::fs::remove_dir_all(inbox);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn keep_is_bytes_hash_or_bytes_for_a_while() {
        let keep = |s: &str| Keep::try_from(s.to_string());
        assert_eq!(keep("bytes"), Ok(Keep::Bytes));
        assert_eq!(keep("hash"), Ok(Keep::Hash));
        assert_eq!(keep("bytes for 30d"), Ok(Keep::BytesFor(30 * 86_400)));
        assert_eq!(keep("12h"), Ok(Keep::BytesFor(12 * 3600)), "the short form --keep takes");
        for bad in ["bytes for", "bytes for 0d", "30", "30w", "bytes for -1d", "forever"] {
            assert!(keep(bad).is_err(), "{bad}");
        }
        for k in [Keep::Bytes, Keep::Hash, Keep::BytesFor(86_400 * 7), Keep::BytesFor(5400), Keep::BytesFor(45)] {
            let text = serde_json::to_string(&k).unwrap();
            assert_eq!(serde_json::from_str::<Keep>(&text).unwrap(), k, "{text}");
        }
        assert_eq!(String::from(Keep::BytesFor(5400)), "bytes for 90m");
        assert_eq!((Keep::Bytes.window_ms(), Keep::Hash.window_ms(), Keep::BytesFor(2).window_ms()), (None, Some(0), Some(2000)));
        assert_eq!((size_text(512), size_text(64 * 1024), size_text(10 << 20), size_text(1536 << 10)), ("512 bytes".into(), "64 KiB".into(), "10 MiB".into(), "1.5 MiB".into()));
        // A folders file from before windows and caps reads as it did.
        let old: Folder = serde_json::from_str(r#"{"path": "/srv/x", "access": "read", "keep": "bytes"}"#).unwrap();
        assert_eq!((old.keep, old.cap), (Keep::Bytes, None));
    }

    #[test]
    fn a_capped_folder_takes_a_write_or_a_copy_only_while_it_fits() {
        let (logs, archive) = (temp("cap-logs"), temp("cap-archive"));
        std::fs::write(logs.join("app.log"), "x".repeat(600)).unwrap();
        let folders = Scoped::memory();
        folders.all.bind("logs", logs.to_str().unwrap(), FolderAccess::Read, Keep::Bytes, None, None).unwrap();
        folders.all.bind("archive", archive.to_str().unwrap(), FolderAccess::ReadWrite, Keep::Bytes, Some(1000), None).unwrap();
        let hash = blake3::hash("x".repeat(600).as_bytes()).to_hex().to_string();
        folders.copy("archive", "a.log", "logs", "app.log", &hash).unwrap();
        let full = folders.copy("archive", "b.log", "logs", "app.log", &hash).unwrap_err();
        assert!(full.contains("capped at 1000 bytes") && full.contains("it holds 600 bytes"), "{full}");
        assert!(!archive.join("b.log").exists());
        folders.write("archive", "a.log", &"y".repeat(1000)).unwrap();
        assert!(folders.write("archive", "a.log", &"y".repeat(1001)).unwrap_err().contains("capped"), "replacing a file counts only the new one");
        folders.write("archive", "notes/n.txt", "").unwrap();
        assert!(folders.write("archive", "notes/m.txt", "z").unwrap_err().contains("capped"), "files in folders under it count too");
        for d in [logs, archive] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn a_create_only_write_or_copy_never_replaces_a_file() {
        let (logs, out) = (temp("create-logs"), temp("create-out"));
        std::fs::write(logs.join("app.log"), "abc").unwrap();
        let folders = Scoped::memory();
        folders.all.bind("logs", logs.to_str().unwrap(), FolderAccess::Read, Keep::Bytes, None, None).unwrap();
        folders.all.bind("out", out.to_str().unwrap(), FolderAccess::ReadWrite, Keep::Bytes, None, None).unwrap();
        folders.create("out", "2026/a.json", "first").unwrap();
        let again = folders.create("out", "2026/a.json", "second").unwrap_err();
        assert!(again.contains("out/2026/a.json not written") && again.contains("create-only"), "{again}");
        assert_eq!(std::fs::read_to_string(out.join("2026/a.json")).unwrap(), "first", "the first stays");
        let hash = blake3::hash(b"abc").to_hex().to_string();
        folders.create_copy("out", "app.log", "logs", "app.log", &hash).unwrap();
        assert!(folders.create_copy("out", "app.log", "logs", "app.log", &hash).unwrap_err().contains("create-only"));
        folders.write("out", "app.log", "replaced").unwrap();
        assert_eq!(std::fs::read_to_string(out.join("app.log")).unwrap(), "replaced", "a plain write still replaces");
        let left: Vec<_> = std::fs::read_dir(&out).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(left.iter().all(|n| !n.starts_with(TMP_PREFIX)), "nothing left beside them: {left:?}");
        for d in [logs, out] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
