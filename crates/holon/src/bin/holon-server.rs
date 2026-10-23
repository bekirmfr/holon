//! `holon-server`: the name releases before 0.1.0-rc.4 installed, kept so the scripts and services
//! that start it keep working. It runs `holon` from beside it: `holon-server [options]` is
//! `holon serve [options]`, and `version` and `help` pass through. `update` and `uninstall` are
//! `holon`'s own to run: an installer can't replace this program while it waits on them.

use std::process::{Command, exit};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(String::as_str);
    if let Some(cmd @ ("update" | "uninstall")) = first {
        eprintln!("holon-server is now holon: run `holon {cmd}`");
        exit(2);
    }
    let name = if cfg!(windows) { "holon.exe" } else { "holon" };
    let Some(holon) = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join(name))).filter(|p| p.is_file()) else {
        eprintln!("holon-server is now `holon serve`, and holon isn't beside it: reinstall holon");
        exit(1);
    };
    let mut cmd = Command::new(&holon);
    if !matches!(first, Some("version" | "--version" | "-V" | "help" | "--help" | "-h")) {
        cmd.arg("serve");
    }
    cmd.args(&args);
    // On Unix it becomes holon (a service manager's signals reach the server itself).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = cmd.exec();
        eprintln!("holon-server: could not run {}: {e}", holon.display());
        exit(1);
    }
    #[cfg(not(unix))]
    match cmd.status() {
        Ok(status) => exit(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("holon-server: could not run {}: {e}", holon.display());
            exit(1);
        }
    }
}
