//! What starts a flow's runs when files change (docs/L4s/named-folders-L4.md §11, slice 5): each
//! `file_arrival` leaf names a folder, whether its runs are for files new or changed there
//! (`on: new`) or removed (`on: deleted`), and which names count (`matching`, a glob). The host
//! watches by these, read from the deployed graph as its capabilities are.

use std::collections::BTreeSet;

use crate::{dependencies, Resolver, Unit, UnitBody};

/// One `on file` line, as a `file_arrival` leaf says it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileTrigger {
    pub folder: String,
    /// For files removed from the folder, not files new or changed there.
    pub deleted: bool,
    /// The glob a file's name (relative to the folder) must match; empty for every file.
    pub matching: String,
}

impl FileTrigger {
    /// Whether a change of kind `deleted` to the file `name` starts a run.
    pub fn starts(&self, deleted: bool, name: &str) -> bool {
        self.deleted == deleted && (self.matching.is_empty() || glob_matches(&self.matching, name))
    }
}

/// Every `file_arrival` in `unit`'s closure, once each, in order.
pub fn file_triggers(unit: &Unit, resolver: &impl Resolver) -> Vec<FileTrigger> {
    let deps = dependencies(unit, resolver);
    let mut found = BTreeSet::new();
    for u in std::iter::once(unit).chain(deps.values().copied()) {
        let UnitBody::Primitive { name } = &u.body else { continue };
        if name != "file_arrival" {
            continue;
        }
        let param = |k: &str| u.params.iter().find(|p| p.name() == k).and_then(|p| p.value()).unwrap_or("").to_string();
        let folder = param("folder");
        if crate::kind::capabilities_of(name, &u.params).is_none() {
            continue;
        }
        found.insert(FileTrigger { folder, deleted: param("on") == "deleted", matching: param("matching") });
    }
    found.into_iter().collect()
}

/// Whether `name` (a file's path in its folder, `/` between folders) matches `glob`: `*` is any
/// run of characters and `?` any one, neither crossing a `/`; `**/` is any number of folders
/// (none included); letters match either case.
pub fn glob_matches(glob: &str, name: &str) -> bool {
    let (g, n): (Vec<char>, Vec<char>) = (glob.to_lowercase().chars().collect(), name.to_lowercase().chars().collect());
    matches_at(&g, &n)
}

fn matches_at(g: &[char], n: &[char]) -> bool {
    match g {
        [] => n.is_empty(),
        ['*', '*', '/', rest @ ..] => {
            // No folder, or one more folder and the same again.
            matches_at(rest, n) || n.iter().position(|c| *c == '/').is_some_and(|i| matches_at(g, &n[i + 1..]))
        }
        ['*', '*'] => true,
        ['*', rest @ ..] => (0..=n.len()).take_while(|i| *i == 0 || n[i - 1] != '/').any(|i| matches_at(rest, &n[i..])),
        ['?', rest @ ..] => n.first().is_some_and(|c| *c != '/') && matches_at(rest, &n[1..]),
        [c, rest @ ..] => n.first() == Some(c) && matches_at(rest, &n[1..]),
    }
}

/// Whether `glob` is one [`glob_matches`] reads: not empty, at most 200 characters, no `\`
/// (names use `/`), and `**` only as a whole folder (`**/`, or at the end).
pub fn is_glob(glob: &str) -> bool {
    if glob.is_empty() || glob.len() > 200 || glob.contains('\\') {
        return false;
    }
    let mut rest = glob;
    while let Some(i) = rest.find("**") {
        let before_ok = i == 0 || rest.as_bytes()[i - 1] == b'/';
        let after = &rest[i + 2..];
        if !before_ok || !(after.is_empty() || after.starts_with('/')) {
            return false;
        }
        rest = after;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glob_matches_names_in_a_folder_and_below_it() {
        assert!(glob_matches("*.csv", "sales.csv"));
        assert!(glob_matches("*.csv", "SALES.CSV"), "either case");
        assert!(!glob_matches("*.csv", "2026/sales.csv"), "* stays in its folder");
        assert!(glob_matches("**/*.csv", "sales.csv"), "**/ is any number of folders, none included");
        assert!(glob_matches("**/*.csv", "2026/10/sales.csv"));
        assert!(glob_matches("2026/*/a?.txt", "2026/10/ab.txt"));
        assert!(!glob_matches("2026/*/a?.txt", "2026/10/a.txt"));
        assert!(!glob_matches("a?", "a/"), "? isn't a /");
        assert!(glob_matches("in/**", "in/x/y.bin"));
        assert!(!glob_matches("*.csv", "sales.csv.tmp"));
        assert!(is_glob("**/*.csv") && is_glob("in/**") && is_glob("*.json"));
        assert!(!is_glob("") && !is_glob("a**") && !is_glob("**a") && !is_glob(r"a\b.csv"));
    }

    #[test]
    fn a_trigger_starts_runs_for_its_kind_of_change_and_its_names() {
        let new = FileTrigger { folder: "inbox".into(), deleted: false, matching: "*.csv".into() };
        assert!(new.starts(false, "a.csv") && !new.starts(true, "a.csv") && !new.starts(false, "a.txt"));
        let gone = FileTrigger { folder: "inbox".into(), deleted: true, matching: String::new() };
        assert!(gone.starts(true, "any/thing") && !gone.starts(false, "a.csv"));
    }
}
