//! `holon new`, `check`, `show` and `fmt`: flows as text files, on any machine, a screen or not.
//! What each says is holon-author's (commands.rs), the same code the editor compiles and prints
//! flows with; this reads and writes the files.

use std::path::Path;

use holon_author::commands::{self, Report};

fn usage() -> String {
    format!("{}\n\nsamples for holon new --sample: {}", crate::USAGE, commands::sample_names())
}

/// Run the flow command `cmd` with `args`; answers the exit code.
pub(crate) fn run(cmd: &str, args: &[String]) -> i32 {
    let Some(file) = args.first().filter(|a| !a.starts_with('-')) else {
        eprintln!("holon {cmd}: which file?\n\n{}", usage());
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
            eprintln!("holon new: {}\n\n{}", rest.join(" "), usage());
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
