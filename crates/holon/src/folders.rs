//! Named folders (`docs/L4s/named-folders-L4.md`): files a flow reads and writes on the host.
//!
//! - **Bound by an admin.** A folder is a name an admin binds to a real folder on this machine
//!   (`--folders`, owner-only; `/admin/folders`, `holon folder`), with the most it allows (read, or
//!   read and write) and what its reads keep in the log (the bytes, or a hash). A flow names
//!   `inbox`; it never sees a path. The server's own data can never be bound, nor a folder holding
//!   it.
//! - **Confined.** A file's name is a relative path inside the folder, checked before it reaches
//!   the operating system (no `..`, no drive or root, no `:`, no reserved Windows names), and the
//!   file it opens must still be inside the folder once links are followed.
//! - **Reads** are answered on the spot, as text, from the folder as the run found it, only for
//!   folders the deployment was granted, and each is kept (with its node's path), as a collection
//!   read is: the log shows it, replay answers from it, and a run is checked against it before it
//!   commits. A big file's text goes to the content store, and the log holds its hash.
//! - **Writes** (a file's whole text, or a delete) are effects, applied when the run commits:
//!   written beside the file and renamed over it, so a reader never sees half of one. The log holds
//!   what was written by its hash and size, never its text: `holon verify` runs it again.

use std::io::Write as _;
use std::sync::RwLock;

use rouille::{Request, Response};

use crate::http::{err, read_body};

use crate::*;

/// The most a folder allows a deployment.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FolderAccess {
    Read,
    ReadWrite,
}

/// What a folder's reads keep in the run log: the bytes (a run is checkable anywhere, later), or
/// only their hash (what was read is provable; it runs again when the file is supplied).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Keep {
    #[default]
    Bytes,
    Hash,
}

/// One bound folder.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Folder {
    /// The real folder, as bound (absolute, links resolved).
    pub(crate) path: PathBuf,
    pub(crate) access: FolderAccess,
    #[serde(default)]
    pub(crate) keep: Keep,
}

/// The largest file a run reads as text.
pub(crate) const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// The most a run reads from files, in all.
pub(crate) const MAX_RUN_READ_BYTES: u64 = 64 * 1024 * 1024;
/// A read's text up to this size is in its log entry; a bigger one is in the content store, by hash.
pub(crate) const LOG_INLINE_BYTES: u64 = 4 * 1024;
/// The most files a listing looks at.
const MAX_WALK: usize = 100_000;
/// What the error of a run whose file read failed starts with: such a run answers 500, not what
/// it computed from nothing.
pub(crate) const READ_FAILED: &str = "a file read failed: ";
/// What the host's own temporary files start with: never a flow's.
const TMP_PREFIX: &str = ".holon-tmp-";

/// The folders bound on this server, and where they are kept.
pub(crate) struct Folders {
    /// The bindings file (owner-only), or none (in memory).
    file: Option<PathBuf>,
    /// The server's own files and folders: never bound, nor anything holding them.
    forbidden: Vec<PathBuf>,
    bound: RwLock<BTreeMap<String, Folder>>,
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
fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let p = std::fs::canonicalize(path)?;
    Ok(match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    })
}

impl Folders {
    /// No folders, kept nowhere: the tests, and a server told `--folders :memory:`.
    pub(crate) fn memory() -> Folders {
        Folders { file: None, forbidden: Vec::new(), bound: RwLock::new(BTreeMap::new()) }
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("can't read {file}: {e}")),
        };
        Ok(Folders { file: Some(path), forbidden, bound: RwLock::new(bound) })
    }

    /// Every folder bound, by name.
    pub(crate) fn all(&self) -> BTreeMap<String, Folder> {
        self.bound.read().unwrap().clone()
    }

    pub(crate) fn get(&self, name: &str) -> Option<Folder> {
        self.bound.read().unwrap().get(name).cloned()
    }

    /// Bind `name` to the folder at `path` (which must exist), replacing any binding before.
    pub(crate) fn bind(&self, name: &str, path: &str, access: FolderAccess, keep: Keep) -> Result<Folder, String> {
        if !is_folder_name(name) {
            return Err(format!("{name:?} isn't a folder name: letters, digits, - and _, up to 64"));
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
        let folder = Folder { path: at, access, keep };
        let mut bound = self.bound.write().unwrap();
        let mut next = bound.clone();
        next.insert(name.to_string(), folder.clone());
        self.save(&next)?;
        *bound = next;
        Ok(folder)
    }

    /// Unbind `name`: whether it was bound. Its files stay where they are.
    pub(crate) fn unbind(&self, name: &str) -> Result<bool, String> {
        let mut bound = self.bound.write().unwrap();
        if !bound.contains_key(name) {
            return Ok(false);
        }
        let mut next = bound.clone();
        next.remove(name);
        self.save(&next)?;
        *bound = next;
        Ok(true)
    }

    fn save(&self, bound: &BTreeMap<String, Folder>) -> Result<(), String> {
        let Some(file) = &self.file else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(bound).map_err(|e| e.to_string())?;
        crate::sealing::write_private(file, &bytes).map_err(|e| format!("can't write {}: {e}", file.display()))
    }

    /// The folder bound to `name`, or why there is none.
    fn folder(&self, name: &str) -> Result<Folder, String> {
        self.get(name).ok_or_else(|| format!("the folder {name:?} isn't bound on this server: an admin binds it (holon folder add {name} <path>)"))
    }

    /// The bytes of file `file` in folder `name`: none if there is no such file; refused past `max`.
    pub(crate) fn read(&self, name: &str, file: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let folder = self.folder(name)?;
        let Some(path) = existing(&folder.path, file)? else { return Ok(None) };
        let meta = std::fs::metadata(&path).map_err(|e| format!("{name}/{file}: {e}"))?;
        if !meta.is_file() {
            return Ok(None);
        }
        if meta.len() > max {
            return Err(format!("{name}/{file} is {} bytes: more than the {max} a run reads", meta.len()));
        }
        std::fs::read(&path).map(Some).map_err(|e| format!("{name}/{file} can't be read: {e}"))
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
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
            let mut hasher = blake3::Hasher::new();
            let mut f = std::fs::File::open(&path).map_err(|e| format!("{name}/{n}: {e}"))?;
            std::io::copy(&mut f, &mut hasher).map_err(|e| format!("{name}/{n}: {e}"))?;
            let item = BTreeMap::from([
                ("size".to_string(), Value::Number(meta.len() as f64)),
                ("hash".to_string(), Value::Text(hasher.finalize().to_hex().to_string())),
                ("modified".to_string(), Value::Number(modified as f64)),
            ]);
            files.insert(n, Value::Record(item));
        }
        Ok(files)
    }

    /// Write `text` as file `file` of folder `name`, whole: beside it first, then renamed over it.
    pub(crate) fn write(&self, name: &str, file: &str, text: &str) -> Result<(), String> {
        let folder = self.writable(name)?;
        let path = for_writing(&folder.path, file)?;
        let dir = path.parent().ok_or("no folder")?;
        let tmp = dir.join(format!("{TMP_PREFIX}{}-{}", std::process::id(), rand::random::<u64>()));
        let written = (|| {
            let mut f = std::fs::File::create_new(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &path)
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{name}/{file} not written: {e}"));
        }
        Ok(())
    }

    /// Delete file `file` of folder `name`; nothing to do if there is none.
    pub(crate) fn delete(&self, name: &str, file: &str) -> Result<(), String> {
        let folder = self.writable(name)?;
        match existing(&folder.path, file)? {
            Some(path) if path.is_file() => std::fs::remove_file(&path).map_err(|e| format!("{name}/{file} not deleted: {e}")),
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
                None => return Some(format!("the folder {name:?} isn't bound on this server: an admin binds it (holon folder add {name} <path>)")),
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

/// The text files under `dir`, by the BLAKE3 of their bytes (hex): what `holon verify --files`
/// answers a run's file reads from, when its log kept only their hash.
pub(crate) fn texts_in(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut names = Vec::new();
    walk(dir, "", &mut names)?;
    let mut texts = BTreeMap::new();
    for name in names {
        let path = dir.join(&name);
        if std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            texts.insert(blake3::hash(text.as_bytes()).to_hex().to_string(), text);
        }
    }
    Ok(texts)
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

/// A binding as the admin panel and `holon folder list` show it.
fn shown(name: &str, f: &Folder) -> serde_json::Value {
    serde_json::json!({ "name": name, "path": f.path.display().to_string(), "access": f.access, "keep": f.keep })
}

/// `/admin/folders` (list, bind) and `/admin/folders/<name>` (unbind), or None for any other path.
/// Only an admin gets this far (access.rs); what they do goes in the activity log.
pub(crate) fn route(state: &AppState, request: &Request, by: &str) -> Option<Response> {
    let url = request.url();
    let method = request.method();
    if url == "/admin/folders" {
        return Some(match method {
            "GET" => Response::json(&state.folders.all().iter().map(|(n, f)| shown(n, f)).collect::<Vec<_>>()),
            "POST" => {
                #[derive(Deserialize)]
                struct Bind {
                    name: String,
                    path: String,
                    #[serde(default = "read_write")]
                    access: FolderAccess,
                    #[serde(default)]
                    keep: Keep,
                }
                fn read_write() -> FolderAccess {
                    FolderAccess::ReadWrite
                }
                let bind = read_body(request)
                    .and_then(|b| serde_json::from_slice::<Bind>(&b).map_err(|e| err(400, format!("invalid body: {e}"))));
                match bind.and_then(|b| state.folders.bind(&b.name, &b.path, b.access, b.keep).map(|f| (b.name, f)).map_err(|e| err(400, e))) {
                    Ok((name, folder)) => {
                        state.activity.record("bound the folder", &name, Some(by));
                        Response::json(&shown(&name, &folder))
                    }
                    Err(e) => e,
                }
            }
            _ => Response::empty_404(),
        });
    }
    let name = url.strip_prefix("/admin/folders/")?;
    Some(match (method, state.folders.unbind(name)) {
        ("DELETE", Ok(true)) => {
            state.activity.record("unbound the folder", name, Some(by));
            Response::empty_204()
        }
        ("DELETE", Ok(false)) => err(404, format!("no folder {name:?} is bound")),
        ("DELETE", Err(e)) => err(500, e),
        _ => Response::empty_404(),
    })
}

/// `holon folder add NAME PATH [--read-only] [--keep hash] | list | remove NAME`: bind, show and
/// unbind the folders of the server this user runs here (or `--server`), as an admin.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut server = None;
    let (mut read_only, mut keep) = (false, Keep::Bytes);
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--read-only" => read_only = true,
            "--keep" => match args.next().map(String::as_str) {
                Some("bytes") => keep = Keep::Bytes,
                Some("hash") => keep = Keep::Hash,
                other => {
                    eprintln!("holon folder: --keep takes bytes or hash, not {other:?}");
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
            let body = serde_json::json!({ "name": name, "path": path, "access": access, "keep": keep });
            ("POST", "/admin/folders".to_string(), Some(body))
        }
        ["remove", name] => ("DELETE", format!("/admin/folders/{name}"), None),
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
        let keep = if f["keep"] == "hash" { ", reads kept by hash" } else { "" };
        format!("{}  {}  ({access}{keep})", f["name"].as_str().unwrap_or(""), f["path"].as_str().unwrap_or(""))
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
        _ => println!("unbound {}: its files stay where they are", words[1]),
    }
    0
}

const FOLDER_USAGE: &str = "usage: holon folder add NAME PATH [--read-only] [--keep hash]
       holon folder list
       holon folder remove NAME
       (each takes --server PORT|URL)

  Binds a name a flow uses (folder inbox) to a real folder on the server's machine, for an admin.
  A flow reads and writes files only in the folders it names, and only by name: never a path.
  --read-only: flows may read it, not write it. --keep hash: the run log keeps a hash of what
  each run read, not its text (holon verify --files then supplies the files). Removing a binding
  leaves the files where they are; deployments that use it stop until it is bound again.";

/// What a run's read of a file kept: its hash and size, and where its text is if not in the log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Seen {
    /// BLAKE3 of the file's bytes (hex): what `holon verify --files` matches a file by.
    pub(crate) hash: String,
    pub(crate) size: u64,
    /// Whether the text is in the log entry itself.
    pub(crate) inline: bool,
    /// The text's value hash in the content store (hex), when kept there.
    pub(crate) blob: Option<String>,
}

/// The folders one run reads: which it may, and how much it has read so far.
pub(crate) struct Readable {
    pub(crate) folders: Arc<Folders>,
    pub(crate) granted: BTreeSet<String>,
    pub(crate) store: SharedStore,
    pub(crate) read_bytes: u64,
}

impl std::fmt::Debug for Readable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Readable").field("granted", &self.granted).field("read_bytes", &self.read_bytes).finish()
    }
}

fn text<'v>(r: &'v BTreeMap<String, Value>, k: &str) -> Option<&'v str> {
    match r.get(k) {
        Some(Value::Text(t)) => Some(t),
        _ => None,
    }
}

impl Readable {
    /// The answer to `request` (`{folder, read}` or `{folder, list}`), and for a read, what it saw:
    /// an error fails the run (a folder not granted or not bound, a name that leaves it, a file too
    /// big or not text). With `keep`, a big file's text is put in the content store.
    pub(crate) fn answer(&mut self, request: &Value, keep: bool) -> Result<(Option<Value>, Option<Seen>), String> {
        let Value::Record(r) = request else { return Ok((None, None)) };
        let Some(name) = text(r, "folder") else { return Ok((None, None)) };
        if !self.granted.contains(name) {
            return Err(format!("the folder {name:?} isn't granted to this deployment"));
        }
        if let Some(file) = text(r, "read") {
            let left = MAX_RUN_READ_BYTES.saturating_sub(self.read_bytes);
            let Some(bytes) = self.folders.read(name, file, MAX_FILE_BYTES.min(left))? else { return Ok((None, None)) };
            self.read_bytes += bytes.len() as u64;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            let size = bytes.len() as u64;
            let text = String::from_utf8(bytes).map_err(|_| format!("{name}/{file} isn't text (UTF-8)"))?;
            let kept = self.folders.get(name).map_or(Keep::Bytes, |f| f.keep);
            let inline = kept == Keep::Bytes && size <= LOG_INLINE_BYTES;
            let value = Value::Text(text);
            let blob = if keep && kept == Keep::Bytes && !inline {
                let hash = self.store.0.lock().unwrap().put_value(&value).map_err(|e| format!("{name}/{file} can't be kept: {e:?}"))?;
                Some(to_hex(&hash))
            } else {
                None
            };
            return Ok((Some(value), Some(Seen { hash, size, inline, blob })));
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

/// Apply a performed `FileWrite(name)`: a write's `{name, text}`, or a delete's name.
pub(crate) fn apply(folders: &Folders, name: &str, payload: &Value) -> Result<(), String> {
    match payload {
        Value::Record(file) => {
            let (Some(Value::Text(file)), Some(Value::Text(text))) = (file.get("name"), file.get("text")) else {
                return Err("a write needs a name and a text".into());
            };
            folders.write(name, file, text)
        }
        Value::Text(file) => folders.delete(name, file),
        _ => Err("not a write or a delete".into()),
    }
}

/// What the log keeps of a performed effect: a file write as `{name, size, hash}`, never its text;
/// everything else as it is.
pub(crate) fn loggable(cap: &Capability, payload: &Value) -> Value {
    match cap {
        Capability::FileWrite(_) => by_hash(payload),
        _ => payload.clone(),
    }
}

/// A file write's `{name, text}` as `{name, size, hash}`; anything else as it is.
fn by_hash(payload: &Value) -> Value {
    if let Value::Record(file) = payload
        && let (Some(name), Some(Value::Text(text))) = (file.get("name"), file.get("text"))
    {
        return Value::Record(BTreeMap::from([
            ("name".to_string(), name.clone()),
            ("size".to_string(), Value::Number(text.len() as f64)),
            ("hash".to_string(), Value::Text(blake3::hash(text.as_bytes()).to_hex().to_string())),
        ]));
    }
    payload.clone()
}

/// A run's output as the log keeps it: each file write it carries (an effect, as settled) by its
/// hash, as [`loggable`] keeps the write performed.
pub(crate) fn loggable_output(v: &Value) -> Value {
    match v {
        Value::Effect { sink, payload } if sink.starts_with("file_write:") => Value::Effect { sink: sink.clone(), payload: Box::new(by_hash(payload)) },
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
        let folders = Folders::open(":memory:", std::slice::from_ref(&data)).unwrap();
        let inbox = temp("inbox");
        assert!(folders.bind("inbox", "relative/path", FolderAccess::Read, Keep::Bytes).unwrap_err().contains("absolute"));
        assert!(folders.bind("inbox", data.to_str().unwrap(), FolderAccess::Read, Keep::Bytes).unwrap_err().contains("own data"));
        assert!(folders.bind("all", data.parent().unwrap().to_str().unwrap(), FolderAccess::Read, Keep::Bytes).unwrap_err().contains("holds"));
        folders.bind("inbox", inbox.to_str().unwrap(), FolderAccess::Read, Keep::Bytes).unwrap();
        let out = temp("out");
        folders.bind("out", out.to_str().unwrap(), FolderAccess::ReadWrite, Keep::Hash).unwrap();

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

        let write = Value::Record(BTreeMap::from([("name".into(), Value::Text("a.md".into())), ("text".into(), Value::Text("hi".into()))]));
        let logged = loggable(&Capability::FileWrite("out".into()), &write);
        let Value::Record(l) = logged else { panic!() };
        assert_eq!((l.get("text"), l["size"].clone()), (None, Value::Number(2.0)), "the log keeps no text");
        for d in [data, inbox, out] {
            let _ = std::fs::remove_dir_all(d);
        }
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
            let folders = Folders::memory();
            folders.bind("inbox", inbox.to_str().unwrap(), FolderAccess::ReadWrite, Keep::Bytes).unwrap();
            assert!(folders.read("inbox", "out/secret.txt", MAX_FILE_BYTES).unwrap_err().contains("outside"));
            assert!(folders.write("inbox", "out/x.txt", "x").unwrap_err().contains("outside"));
            assert!(folders.list("inbox", "", None, 10).unwrap().is_empty(), "a listing doesn't follow the link");
        }
        let _ = std::fs::remove_dir_all(inbox);
        let _ = std::fs::remove_dir_all(outside);
    }
}
