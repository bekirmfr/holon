//! `holon.d.ts`, the types of what a flow imports for TypeScript's own tools, and the
//! `tsconfig.json` beside it (typescript-syntax L4 §13). `holon new` and `holon types` write both
//! beside a flow; the samples' folder keeps them as these write them, and the editor's tests run
//! TypeScript's checker over every sample and every flow of the manual with them.

use crate::{agent, LIMIT_NAMES};

/// The first line of a `holon.d.ts` Holon wrote: a file starting so is Holon's to write again.
pub const MARK: &str = "// holon.d.ts: the types of what a flow imports, for TypeScript's own tools";

/// The vocabulary and the import sources, by hand; read with `\n` line endings, however it was
/// checked out.
const VOCABULARY: &str = include_str!("holon.d.ts");

/// Beside it: no DOM and no Node, so `fetch`, `console` and `setTimeout` are unknown names, as
/// they are to Holon; strict but for `noImplicitAny`, as a function's parameters needn't have
/// types, and `strictNullChecks`, as absence goes through plain `.` in Holon.
pub const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "esnext",
    "lib": ["esnext"],
    "module": "esnext",
    "moduleResolution": "bundler",
    "types": [],
    "strict": true,
    "noImplicitAny": false,
    "strictNullChecks": false,
    "noEmit": true
  }
}
"#;

/// Leaves JavaScript gives no name to import by: `import { const }` doesn't parse.
const RESERVED: &[&str] = &["await", "const", "default"];

/// `holon.d.ts`: the vocabulary and the import sources, then the deploy settings' limits and
/// every leaf of the live catalog, each with its signature and what it does, as the manual's
/// page of leaves gives them.
pub fn types() -> String {
    let vocabulary = VOCABULARY.replace("\r\n", "\n");
    let mut out = vec![
        MARK.to_string(),
        "// (an editor's completion, hover and checks). `holon types` wrote it from its catalog, and".into(),
        "// writes it again after an upgrade: don't edit it by hand. Holon's compiler decides what a".into(),
        "// flow means; `holon check` says where the two disagree.".into(),
        String::new(),
        vocabulary.trim_end().to_string(),
        String::new(),
        "declare module \"holon\" {".into(),
        "  /** The deploy settings' limits a flow may set: a whole number in its unit, a `_ms` or `_seconds` one a duration too. */".into(),
        "  export interface Limits {".into(),
    ];
    for name in LIMIT_NAMES.iter().filter(|n| !n.starts_with("keep_") && !n.starts_with("module_")) {
        let ty = if name.ends_with("_ms") || name.ends_with("_seconds") { "number | Duration" } else { "number" };
        out.push(format!("    {name}?: {ty}"));
    }
    out.push("  }".into());
    // The vocabulary's own names win over a leaf's.
    let holon = vocabulary.split("declare module \"holon\" {").nth(1).and_then(|m| m.split("\n}\n").next()).unwrap_or("");
    for (name, sig, doc) in agent::leaves() {
        let declared = holon.contains(&format!("export function {name}(")) || holon.contains(&format!("export const {name}:"));
        if RESERVED.contains(&name.as_str()) || declared {
            continue;
        }
        out.extend([String::new(), "  /**".into(), format!("   * `{sig}`")]);
        if !doc.is_empty() {
            out.push("   *".into());
            out.extend(doc.replace("*/", "*\\/").lines().map(|l| format!("   * {l}").trim_end().to_string()));
        }
        out.extend(["   */".into(), format!("  export function {name}(...args: any[]): any")]);
    }
    out.extend(["}".into(), String::new()]);
    out.join("\n")
}

/// Whether a `holon.d.ts` already there is one Holon wrote, and so may be written again.
pub fn is_holons(text: &str) -> bool {
    text.starts_with(MARK)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn the_samples_folder_has_them_as_written() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples");
        for (file, text) in [("holon.d.ts", types()), ("tsconfig.json", TSCONFIG.to_string())] {
            let path = dir.join(file);
            let kept = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
            if kept != text && std::env::var_os("HOLON_BLESS").is_some() {
                std::fs::write(&path, &text).unwrap();
                continue;
            }
            assert!(kept == text, "{} is out of date: HOLON_BLESS=1 cargo test -p holon-author types writes it again", path.display());
        }
    }

    #[test]
    fn every_leaf_a_flow_imports_by_name_is_typed() {
        let typed = types();
        for (name, ..) in agent::leaves() {
            let there = typed.contains(&format!("export function {name}("));
            assert_eq!(there, !RESERVED.contains(&name.as_str()), "{name}");
        }
        assert!(is_holons(&typed));
    }
}
