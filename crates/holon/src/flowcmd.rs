//! `holon new`, `check`, `show`, `fmt` and `types`: flows as text files, on any machine, a screen
//! or not. What each says is holon-author's (commands.rs, types.rs), the same code the editor
//! compiles and prints flows with; this reads and writes the files.

use std::path::{Path, PathBuf};

use holon_author::commands::{self, Report};
use holon_author::types;

const NEW_USAGE: &str = "usage: holon new <file> [--sample NAME]

  Writes a new flow into <file>, to edit and deploy: a small starter, or one of the samples,
  each a working flow with comments saying what it shows and how to try it. A file already
  there is left as it is. Beside it, as holon types does: holon.d.ts, for an editor's
  TypeScript, and a tsconfig.json when there is none.

  --sample NAME            write that sample instead of the starter (they are listed below)

examples:
  holon new hello.holon.ts
  holon new shop.holon.ts --sample shop
  holon new stress.holon.ts --sample stress-test

samples:";

const CHECK_USAGE: &str = "usage: holon check <file>

  Compiles a flow without deploying it, and prints what it would be: its routes and clocks, its
  unit's hash, and what it may do, as a deploy would ask to approve it. Each mistake is printed
  instead, on stderr, with its line and column. Exits 1 when there is one. It needs no server.

examples:
  holon check shop.holon.ts
  holon check shop.holon.ts && holon deploy shop.holon.ts";

const SHOW_USAGE: &str = "usage: holon show <file>

  Prints an outline of a flow: its routes, and each node by its name in the text, with its
  params and what feeds it. It needs no server.

examples:
  holon show shop.holon.ts";

const FMT_USAGE: &str = "usage: holon fmt <file>

  Rewrites a flow in place as the samples and holon new are written: laid out Prettier's way,
  js() bodies included, its comments kept, var and == made const or let and ===. A flow that
  doesn't compile is left as it is, its mistakes printed. It needs no server.

examples:
  holon fmt shop.holon.ts
  holon fmt shop.holon.ts && git diff shop.holon.ts";

const TYPES_USAGE: &str = "usage: holon types [dir]

  Writes holon.d.ts into <dir>, the current folder when left out: the types of what a flow
  imports, so an editor's TypeScript completes and checks a .holon.ts file. A tsconfig.json is
  written beside it when there is none. A holon.d.ts that holon didn't write is left as it is.
  Run it again after an upgrade, as the leaves it lists are this holon's. It needs no server.

examples:
  holon types
  holon types flows";

/// The help of the flow command `cmd`.
fn usage(cmd: &str) -> String {
    match cmd {
        "new" => format!("{NEW_USAGE}\n{}", wrapped(&commands::sample_names(), 94, "  ")),
        "check" => CHECK_USAGE.into(),
        "show" => SHOW_USAGE.into(),
        "types" => TYPES_USAGE.into(),
        _ => FMT_USAGE.into(),
    }
}

/// `list` (names, comma-separated) in lines at most `width` wide, each starting with `indent`.
fn wrapped(list: &str, width: usize, indent: &str) -> String {
    let mut lines = vec![indent.to_string()];
    for word in list.split_whitespace() {
        let line = lines.last_mut().unwrap();
        if line.len() > indent.len() && line.len() + 1 + word.len() > width {
            lines.push(format!("{indent}{word}"));
        } else {
            if line.len() > indent.len() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines.join("\n")
}

/// Run the flow command `cmd` with `args`; answers the exit code.
pub(crate) fn run(cmd: &str, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage(cmd));
        return 0;
    }
    if cmd == "types" {
        return match args {
            [] => write_types(Path::new(""), true),
            [dir] if !dir.starts_with('-') => write_types(Path::new(dir), true),
            _ => {
                eprintln!("holon types: {}? (holon types --help says how)", args.join(" "));
                2
            }
        };
    }
    let Some(file) = args.first().filter(|a| !a.starts_with('-')) else {
        eprintln!("holon {cmd}: which file? (holon {cmd} --help says how)");
        return 2;
    };
    if cmd == "new" {
        return new(file, &args[1..]);
    }
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{file}: no such file");
            return 1;
        }
        Err(e) => {
            eprintln!("{file}: {e}");
            return 1;
        }
    };
    match cmd {
        "check" => print(commands::check(file, &source)),
        "show" => print(commands::show(file, &source)),
        _ => match commands::fmt(file, &source) {
            Ok(text) => write(file, &text, &format!("formatted {file}")),
            Err(report) => print(report),
        },
    }
}

fn new(file: &str, rest: &[String]) -> i32 {
    let sample = match rest {
        [] => None,
        [flag, name] if flag == "--sample" => Some(name.as_str()),
        _ => {
            eprintln!("holon new: {}? (holon new --help says how)", rest.join(" "));
            return 2;
        }
    };
    if Path::new(file).exists() {
        eprintln!("{file} exists: not overwritten");
        return 1;
    }
    match commands::new_flow(sample) {
        Ok(text) => match write(file, &text, &format!("wrote {file}")) {
            0 => write_types(Path::new(file).parent().unwrap_or(Path::new("")), false),
            code => code,
        },
        Err(e) => {
            eprintln!("holon new: {e}");
            2
        }
    }
}

/// `name` in `dir`, written as the user would: `holon.d.ts` in the current folder.
fn beside(dir: &Path, name: &str) -> PathBuf {
    if dir.as_os_str().is_empty() {
        PathBuf::from(name)
    } else {
        dir.join(name)
    }
}

/// Writes holon.d.ts into `dir`, and a tsconfig.json when there is none; says what it wrote, and
/// a holon.d.ts already as it would write it only when `say_current`.
fn write_types(dir: &Path, say_current: bool) -> i32 {
    let dts = beside(dir, "holon.d.ts");
    let text = types::types();
    let mut code = match std::fs::read_to_string(&dts) {
        Ok(kept) if kept.replace("\r\n", "\n") == text => {
            if say_current {
                println!("{} is up to date", dts.display());
            }
            0
        }
        Ok(kept) if !types::is_holons(&kept) => {
            eprintln!("{}: holon didn't write it, so it is left as it is", dts.display());
            1
        }
        _ => write(&dts.display().to_string(), &text, &format!("wrote {}", dts.display())),
    };
    let tsconfig = beside(dir, "tsconfig.json");
    if !tsconfig.exists() {
        code = code.max(write(&tsconfig.display().to_string(), types::TSCONFIG, &format!("wrote {}", tsconfig.display())));
    }
    code
}

fn write(file: &str, text: &str, done: &str) -> i32 {
    match std::fs::write(file, text) {
        Ok(()) => {
            println!("{done}");
            0
        }
        Err(e) => {
            eprintln!("{file}: {e}");
            1
        }
    }
}

fn print(report: Report) -> i32 {
    for line in &report.out {
        println!("{line}");
    }
    for line in &report.err {
        eprintln!("{line}");
    }
    report.code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_writes_holon_d_ts_and_a_tsconfig_once_and_leaves_what_it_didnt_write() {
        let dir = std::env::temp_dir().join(format!("holon-types-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(write_types(&dir, true), 0);
        assert_eq!(read("holon.d.ts"), types::types());
        assert_eq!(read("tsconfig.json"), types::TSCONFIG);
        // Again, as after an upgrade: an older holon.d.ts is written anew, a tsconfig kept.
        std::fs::write(dir.join("holon.d.ts"), format!("{}\n// older", types::MARK)).unwrap();
        std::fs::write(dir.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(write_types(&dir, true), 0);
        assert_eq!(read("holon.d.ts"), types::types());
        assert_eq!(read("tsconfig.json"), "{}");
        // One holon didn't write is left as it is.
        std::fs::write(dir.join("holon.d.ts"), "declare module \"holon\"").unwrap();
        assert_eq!(write_types(&dir, true), 1);
        assert_eq!(read("holon.d.ts"), "declare module \"holon\"");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
