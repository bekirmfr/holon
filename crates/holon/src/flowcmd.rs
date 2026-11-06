//! `holon new`, `check`, `show` and `fmt`: flows as text files, on any machine, a screen or not.
//! What each says is holon-author's (commands.rs), the same code the editor compiles and prints
//! flows with; this reads and writes the files.

use std::path::Path;

use holon_author::commands::{self, Report};

const NEW_USAGE: &str = "usage: holon new <file> [--sample NAME]

  Writes a new flow into <file>, to edit and deploy: a small starter, or one of the samples,
  each a working flow with comments saying what it shows and how to try it. A file already
  there is left as it is.

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

  Rewrites a flow in place as the samples and holon new are written: laid out as written,
  Prettier's way, its comments kept, var and == made const or let and ===, a js() body never
  touched. A flow that doesn't compile is left as it is, its mistakes printed. A flow in the
  older .holon form is printed in that form's layout, a name its body uses that its header
  doesn't declare declared for it. It needs no server.

examples:
  holon fmt shop.holon.ts
  holon fmt shop.holon.ts && git diff shop.holon.ts";

/// The help of the flow command `cmd`.
fn usage(cmd: &str) -> String {
    match cmd {
        "new" => format!("{NEW_USAGE}\n{}", wrapped(&commands::sample_names(), 94, "  ")),
        "check" => CHECK_USAGE.into(),
        "show" => SHOW_USAGE.into(),
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
        _ => match commands::fmt_declaring(file, &source) {
            Ok((text, declared)) if declared.is_empty() => write(file, &text, &format!("formatted {file}")),
            Ok((text, declared)) => write(file, &text, &format!("formatted {file}: declared {}", declared.join(", "))),
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
        Ok(text) => write(file, &text, &format!("wrote {file}")),
        Err(e) => {
            eprintln!("holon new: {e}");
            2
        }
    }
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
