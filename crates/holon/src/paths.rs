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

use std::path::{Path, PathBuf};

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
    fn a_resolved_path_is_as_people_write_it() {
        let dir = std::env::temp_dir().join(format!("holon-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("runs")).unwrap();
        let real = canonical(&dir.join("runs/../runs")).unwrap();
        assert!(!real.to_string_lossy().starts_with(r"\\?\"), "{}", real.display());
        assert_eq!(real, canonical(&dir).unwrap().join("runs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
