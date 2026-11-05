//! Paths as each platform writes them.
//!
//! A path comes in as a person wrote it (a flag, a setting, a folder or a database to bind, a
//! folder to browse) or as the code joins it, and is kept, compared and shown in its platform's
//! own form, whichever way it was written:
//!
//! - **Windows:** `/` and `\` both separate its parts, written `\`; `.` goes and `..` takes the
//!   part before it, as Windows itself reads a path (`GetFullPathNameW`); a verbatim path (`\\?\…`)
//!   is taken as written. A whole path starts at a drive (`D:\archive`) or a share (`\\nas\runs`):
//!   `\archive` (the current drive's) and `D:archive` (that drive's current folder's) aren't.
//! - **Unix:** only `/` separates (`\` may be in a name); `.` and a doubled `/` go and `..` stays,
//!   since the folder it leads up from may be a link. A whole path starts at `/`.
//!
//! Either way a separator at the end goes. The reading is the platform's own
//! (`std::path::absolute`), not one of ours, so a path can't mean one thing here and another to
//! the system. `canonical` resolves links too, on a path that is there.
//!
//! A path the server keeps (where its deployments, flows and policy are, a folder or a database
//! bound, the backup) is a [`HostPath`], which only these make: one kept is never relative, nor
//! written with the other platform's separators. Where it may be kept nowhere, it is a [`Place`].

use std::ffi::OsStr;
use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A whole path in its platform's form, as [`normalize`] writes it, and made only by it (or by
/// [`whole`] or [`canonical`]). It reads as a `Path`; `join`, `with_extension` and `parent` keep it
/// one. Kept in a file as its text, and read back through `normalize`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct HostPath(PathBuf);

impl HostPath {
    /// `path` whole, in its platform's form ([`normalize`]).
    pub(crate) fn new(path: impl AsRef<Path>) -> HostPath {
        HostPath(normalize(path))
    }

    /// The whole path a person wrote ([`whole`]); none if it isn't whole here.
    pub(crate) fn whole(text: &str) -> Option<HostPath> {
        whole(text).map(HostPath)
    }

    /// `path` with its links resolved ([`canonical`]), if it is there.
    pub(crate) fn canonical(path: impl AsRef<Path>) -> std::io::Result<HostPath> {
        canonical(path.as_ref()).map(HostPath)
    }

    /// `part` below it, read as its platform reads it (`a/b` is two parts on Windows).
    pub(crate) fn join(&self, part: impl AsRef<Path>) -> HostPath {
        HostPath::new(self.0.join(part))
    }

    /// The file beside it with this extension (`deployments.logs` beside `deployments.json`).
    pub(crate) fn with_extension(&self, extension: impl AsRef<OsStr>) -> HostPath {
        HostPath(self.0.with_extension(extension))
    }

    /// The folder it is in; none for a root.
    pub(crate) fn parent(&self) -> Option<HostPath> {
        self.0.parent().map(|p| HostPath(p.to_path_buf()))
    }
}

impl Deref for HostPath {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for HostPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl From<HostPath> for PathBuf {
    fn from(path: HostPath) -> PathBuf {
        path.0
    }
}

/// As a `Path` is: quoted, as messages say a path.
impl fmt::Debug for HostPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for HostPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(f)
    }
}

impl serde::Serialize for HostPath {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for HostPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<HostPath, D::Error> {
        PathBuf::deserialize(d).map(HostPath::new)
    }
}

/// Where the server keeps a file: at a whole path, or nowhere, only in memory (`:memory:`: the
/// tests, and a server told so).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Place {
    Nowhere,
    At(HostPath),
}

impl Place {
    /// As an option names it: `:memory:` for nowhere, else a path, made whole.
    pub(crate) fn parse(text: &str) -> Place {
        if text == ":memory:" { Place::Nowhere } else { Place::At(HostPath::new(text)) }
    }

    /// Its path, if it is kept anywhere.
    pub(crate) fn at(&self) -> Option<&HostPath> {
        match self {
            Place::Nowhere => None,
            Place::At(path) => Some(path),
        }
    }

    /// What is kept beside it, by its extension: nothing beside nowhere.
    pub(crate) fn beside(&self, extension: &str) -> Option<HostPath> {
        self.at().map(|p| p.with_extension(extension))
    }
}

impl From<&str> for Place {
    fn from(text: &str) -> Place {
        Place::parse(text)
    }
}

impl From<String> for Place {
    fn from(text: String) -> Place {
        Place::parse(&text)
    }
}

impl From<HostPath> for Place {
    fn from(path: HostPath) -> Place {
        Place::At(path)
    }
}

/// As its path is, or `":memory:"`.
impl fmt::Debug for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Place::Nowhere => ":memory:".fmt(f),
            Place::At(path) => path.fmt(f),
        }
    }
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Place::Nowhere => f.write_str(":memory:"),
            Place::At(path) => path.fmt(f),
        }
    }
}

/// A whole path, as an example: one this machine takes.
pub(crate) const EXAMPLE: &str = if cfg!(windows) { r"D:\archive" } else { "/mnt/archive" };

/// `path` in its platform's form, whole: one that isn't is taken from the folder the server was
/// started in.
pub(crate) fn normalize(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    std::path::absolute(path).map_or_else(|_| path.to_path_buf(), |p| p.components().collect())
}

/// The whole path a person wrote, in its platform's form; none if it isn't whole here.
pub(crate) fn whole(text: &str) -> Option<PathBuf> {
    let given = Path::new(text.trim());
    given.is_absolute().then(|| normalize(given))
}

/// `path` with its links resolved, as `fs::canonicalize` gives it, but without Windows' verbatim
/// prefix (`\\?\D:\runs` is `D:\runs`, `\\?\UNC\nas\runs` is `\\nas\runs`): the form people
/// write, and the one every path here is compared in. Kept verbatim when the plain form would read
/// as another path (a name ending in a dot, or a device's, as `CON`).
pub(crate) fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let p = std::fs::canonicalize(path)?;
    let plain = match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        Some(rest) if rest.starts_with(r"UNC\") => PathBuf::from(format!(r"\\{}", &rest[4..])),
        _ => return Ok(p),
    };
    Ok(if normalize(&plain) == plain { plain } else { p })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_path_is_its_platforms_and_reads_as_it_does() {
        if cfg!(windows) {
            for (given, kept) in [
                ("D:/archive", r"D:\archive"),
                (r"D:\archive\", r"D:\archive"),
                ("D:/runs//2026/./june/../may", r"D:\runs\2026\may"),
                (r"  \\nas\runs/kept  ", r"\\nas\runs\kept"),
            ] {
                assert_eq!(whole(given), Some(PathBuf::from(kept)), "{given}");
            }
            for given in [r"\archive", "/mnt/archive", "D:archive", "archive", ""] {
                assert_eq!(whole(given), None, "{given}");
            }
        } else {
            for (given, kept) in [("/mnt/archive", "/mnt/archive"), ("/mnt/archive/", "/mnt/archive"), ("/mnt//runs/./2026", "/mnt/runs/2026"), ("/mnt/link/../runs", "/mnt/link/../runs"), (r"/mnt/a\b", r"/mnt/a\b")] {
                assert_eq!(whole(given), Some(PathBuf::from(kept)), "{given}");
            }
            for given in ["archive", "./archive", ""] {
                assert_eq!(whole(given), None, "{given}");
            }
        }
        assert!(whole(EXAMPLE).is_some(), "its example is whole here");
    }

    #[test]
    fn a_path_joined_in_parts_or_whole_reads_the_same() {
        let dir = std::env::temp_dir();
        assert_eq!(normalize(dir.join("values/packs")), normalize(dir.join("values").join("packs")));
        assert_eq!(normalize(dir.join("values/packs")).display().to_string(), dir.join("values").join("packs").display().to_string());
        // Not whole: from the folder it was started in.
        assert_eq!(normalize("kept"), std::env::current_dir().unwrap().join("kept"));
    }

    #[test]
    fn a_kept_path_stays_whole_and_its_platforms() {
        let dir = HostPath::new(std::env::temp_dir());
        let packs = dir.join("values/packs");
        assert_eq!(packs, HostPath::new(dir.join("values").join("packs")));
        assert_eq!(packs.to_string(), std::env::temp_dir().join("values").join("packs").display().to_string(), "its platform's separators");
        assert_eq!(dir.join("deployments.json").with_extension("logs"), dir.join("deployments.logs"));
        assert_eq!(packs.parent(), Some(dir.join("values")));
        // Kept as its text, and read back in its platform's form, however it was written there.
        let text = serde_json::to_string(&packs).unwrap();
        assert_eq!(serde_json::from_str::<HostPath>(&text).unwrap(), packs);
        let written = serde_json::to_string(&format!("{}/values/./packs/", std::env::temp_dir().display())).unwrap();
        assert_eq!(serde_json::from_str::<HostPath>(&written).unwrap(), packs);
        // Kept nowhere, or at a path; nothing beside nowhere.
        assert_eq!(Place::parse(":memory:"), Place::Nowhere);
        assert_eq!(Place::Nowhere.beside("logs"), None);
        let deployments = Place::from(dir.join("deployments.json"));
        assert_eq!(deployments.beside("logs"), Some(dir.join("deployments.logs")));
        assert_eq!(Place::parse(&dir.join("deployments.json").to_string()), deployments);
    }

    #[test]
    fn a_resolved_path_is_as_people_write_it() {
        let dir = std::env::temp_dir().join(format!("holon-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("runs")).unwrap();
        let real = canonical(&dir.join("runs/../runs")).unwrap();
        assert!(!real.to_string_lossy().starts_with(r"\\?\"), "{}", real.display());
        assert_eq!(real, canonical(&dir).unwrap().join("runs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
