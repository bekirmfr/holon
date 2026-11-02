//! The shell line, at the bottom right: a command typed there runs in this terminal, in the
//! person's own shell, as if `holon editor` had stepped aside: the editor leaves the screen and
//! the command has the terminal (colours, ctrl+c). Then the prompt again, as the folder is now,
//! for the next command, until esc brings the editor back. `cd` changes the folder the next
//! commands, and the editor's `:open`, start in.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal;

use super::app::App;

/// How many commands ↑ goes back through.
const HISTORY: usize = 100;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Who the commands run as, and where: `user@host`, as a shell's prompt starts.
pub(crate) fn who() -> String {
    let user = var("USERNAME").or_else(|| var("USER")).unwrap_or_else(|| "you".into());
    let host = var("COMPUTERNAME").or_else(|| var("HOSTNAME")).or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()));
    match host {
        Some(h) => format!("{user}@{h}"),
        None => user,
    }
}

/// The folder commands start in, home as `~`; past `width`, its end, from a separator where one
/// fits (`…\worktrees\holon`).
pub(crate) fn place(width: usize) -> String {
    let dir = std::env::current_dir().unwrap_or_default();
    let home = home();
    let full = match dir.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => {
            let rest = rest.display().to_string();
            if rest.is_empty() { "~".to_string() } else { format!("~{}{rest}", std::path::MAIN_SEPARATOR) }
        }
        _ => dir.display().to_string(),
    };
    let n = full.chars().count();
    if n <= width {
        return full;
    }
    let keep = width.saturating_sub(1);
    let tail: String = full.chars().skip(n - keep).collect();
    // From the next separator, unless that leaves too little to say where.
    match tail.find(['\\', '/']) {
        Some(i) if tail.len() - i > keep / 2 => format!("…{}", &tail[i..]),
        _ => format!("…{tail}"),
    }
}

/// The prompt before the line, as the shell itself would write it: `user@host path>` on Windows,
/// `user@host:path$` elsewhere, with the path kept to `width`.
pub(crate) fn prompt(width: usize) -> String {
    let who = who();
    let room = width.saturating_sub(who.chars().count() + 2).max(12);
    if cfg!(windows) { format!("{who} {}>", place(room)) } else { format!("{who}:{}$", place(room)) }
}

impl App {
    pub(crate) fn open_shell(&mut self) {
        self.shell = Some(String::new());
        self.shell_back = None;
    }

    pub(crate) fn shell_key(&mut self, k: KeyEvent) {
        let Some(line) = self.shell.as_mut() else { return };
        match k.code {
            KeyCode::Esc => self.shell = None,
            KeyCode::Enter => {
                let line = self.shell.take().unwrap_or_default();
                self.shell_command(line.trim());
            }
            KeyCode::Backspace => {
                line.pop();
            }
            KeyCode::Char(c) => line.push(c),
            KeyCode::Up | KeyCode::Down => {
                let n = self.shell_history.len();
                if n == 0 {
                    return;
                }
                let at = match (k.code, self.shell_back) {
                    (KeyCode::Up, None) => Some(n - 1),
                    (KeyCode::Up, Some(i)) => Some(i.saturating_sub(1)),
                    (_, Some(i)) if i + 1 < n => Some(i + 1),
                    _ => None,
                };
                self.shell_back = at;
                *line = at.map(|i| self.shell_history[i].clone()).unwrap_or_default();
            }
            _ => {}
        }
    }

    /// A line entered: `cd` is done here, so it lasts; anything else runs with the terminal.
    pub(crate) fn shell_command(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        if self.shell_history.last().map(String::as_str) != Some(line) {
            self.shell_history.push(line.to_string());
            if self.shell_history.len() > HISTORY {
                self.shell_history.remove(0);
            }
        }
        match line.split_once(char::is_whitespace).map(|(c, a)| (c, a.trim())).unwrap_or((line, "")) {
            ("cd" | "chdir" | "Set-Location" | "sl", to) => {
                let to = to.trim_matches(['"', '\'']);
                let dir = match to {
                    "" | "~" => home(),
                    _ => to.strip_prefix("~/").or_else(|| to.strip_prefix("~\\")).map(|rest| home().join(rest)).unwrap_or_else(|| PathBuf::from(to)),
                };
                match std::env::set_current_dir(&dir) {
                    Ok(()) => self.say(format!("in {}", std::env::current_dir().unwrap_or(dir).display())),
                    Err(e) => self.fail(format!("cd {to}: {e}")),
                }
            }
            ("exit", "") => {}
            _ => self.run = Some(line.to_string()),
        }
    }

    /// The next command for the terminal, in a session: lines are read (`read`, with what was
    /// entered before) until one is; `cd` is done on the way, and says where it went or why it
    /// couldn't. None: esc, or `exit`, back to the editor.
    pub(crate) fn next_command(&mut self, mut read: impl FnMut(&[String]) -> Option<String>, mut say: impl FnMut(&str, bool)) -> Option<String> {
        loop {
            let line = read(&self.shell_history)?;
            let line = line.trim();
            if line == "exit" {
                return None;
            }
            self.note = None;
            self.shell_command(line);
            if let Some(run) = self.run.take() {
                return Some(run);
            }
            if let Some(n) = &self.note {
                say(&n.text, n.error);
            }
        }
    }

    /// Back from a command: what it said is in the terminal's scrollback; here, how it ended.
    pub(crate) fn after_shell(&mut self, line: &str, ended: &std::io::Result<ExitStatus>) {
        self.refresh();
        match ended {
            Ok(s) if s.success() => self.say(format!("ran {line}")),
            Ok(s) => self.fail(format!("{line}: exited {}", s.code().map(|c| c.to_string()).unwrap_or_else(|| "on a signal".into()))),
            Err(e) => self.fail(format!("{line}: {e}")),
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

/// `line` for the person's shell: PowerShell 7, else Windows PowerShell, else cmd on Windows;
/// `$SHELL` (else sh) elsewhere.
fn commands(line: &str) -> Vec<Command> {
    if cfg!(windows) {
        // Encoded, so no quote in the line is read twice.
        let utf16: Vec<u8> = line.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = super::base64(&utf16);
        let mut out = vec![];
        for shell in ["pwsh", "powershell"] {
            let mut c = Command::new(shell);
            c.args(["-NoLogo", "-EncodedCommand", &encoded]);
            out.push(c);
        }
        let mut c = Command::new("cmd");
        c.args(["/C", line]);
        out.push(c);
        out
    } else {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "sh".into());
        let mut c = Command::new(shell);
        c.args(["-c", line]);
        vec![c]
    }
}

/// The prompt as the terminal shows it, then what was typed after it.
pub(crate) fn echo(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "\x1b[1;32m{}\x1b[0m {line}", prompt(usize::MAX));
    let _ = out.flush();
}

/// Run `line` with this terminal, until it ends; ctrl+c meanwhile stops it, not the editor.
pub(crate) fn run(line: &str) -> std::io::Result<ExitStatus> {
    interrupts::ignore(true);
    let mut ended = Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no shell found"));
    for mut c in commands(line) {
        ended = c.status();
        if !matches!(&ended, Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
            break;
        }
    }
    interrupts::ignore(false);
    ended
}

/// How a command ended, when not well: under what it printed, before the next prompt.
pub(crate) fn ended(ended: &std::io::Result<ExitStatus>) {
    let how = match ended {
        Ok(s) if s.success() => return,
        Ok(s) => s.code().map(|c| format!("exited {c}")).unwrap_or_else(|| "stopped".into()),
        Err(e) => e.to_string(),
    };
    say(&format!("[{how}]"), true);
}

/// A line of the editor's own, between commands: dimmed, or red for what went wrong.
pub(crate) fn say(text: &str, bad: bool) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "\x1b[{}m{text}\x1b[0m", if bad { "31" } else { "2" });
    let _ = out.flush();
}

/// The prompt, as the folder is now, and a line typed after it: enter gives it, ↑ ↓ go through
/// `history`, ctrl+c clears it; esc (or ctrl+d on an empty line) gives None, back to the editor.
pub(crate) fn read_line(history: &[String]) -> Option<String> {
    let mut out = std::io::stdout();
    let lead = prompt(usize::MAX);
    let mut line = String::new();
    let mut back: Option<usize> = None;
    let draw = |out: &mut std::io::Stdout, line: &str| {
        let _ = write!(out, "\r\x1b[2K\x1b[1;32m{lead}\x1b[0m {line}\x1b[2m{}\x1b[0m", if line.is_empty() { "   (esc: back to holon editor)" } else { "" });
        if line.is_empty() {
            // The cursor where typing goes, before the hint.
            let _ = write!(out, "\r\x1b[{}C", lead.chars().count() + 1);
        }
        let _ = out.flush();
    };
    let _ = terminal::enable_raw_mode();
    draw(&mut out, &line);
    let answer = loop {
        let Ok(e) = event::read() else { break None };
        let Event::Key(k) = e else { continue };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => break None,
            KeyCode::Enter => break Some(line.clone()),
            KeyCode::Char('d') if ctrl && line.is_empty() => break None,
            KeyCode::Char('c') if ctrl => line.clear(),
            KeyCode::Char('u') if ctrl => line.clear(),
            KeyCode::Backspace => {
                line.pop();
            }
            KeyCode::Char(c) if !ctrl => line.push(c),
            KeyCode::Up | KeyCode::Down if !history.is_empty() => {
                let n = history.len();
                back = match (k.code, back) {
                    (KeyCode::Up, None) => Some(n - 1),
                    (KeyCode::Up, Some(i)) => Some(i.saturating_sub(1)),
                    (_, Some(i)) if i + 1 < n => Some(i + 1),
                    _ => None,
                };
                line = back.map(|i| history[i].clone()).unwrap_or_default();
            }
            _ => {}
        }
        draw(&mut out, &line);
    };
    // What was typed stays on screen, as a shell leaves it; esc leaves the prompt bare.
    if answer.is_none() {
        line.clear();
        draw(&mut out, "");
        let _ = write!(out, "\x1b[0K");
    }
    let _ = write!(out, "\r\n");
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
    answer
}

/// While a command runs, ctrl+c reaches it and the editor carries on. A handler, not "ignore":
/// a command started meanwhile doesn't inherit a handler, where it would inherit ignoring it.
#[cfg(windows)]
mod interrupts {
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::TRUE;
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    unsafe extern "system" fn handled(_: u32) -> BOOL {
        TRUE
    }

    pub(super) fn ignore(on: bool) {
        // SAFETY: a handler that only answers that it handled the event.
        unsafe { SetConsoleCtrlHandler(Some(handled), on as BOOL) };
    }
}

#[cfg(unix)]
mod interrupts {
    extern "C" fn handled(_: libc::c_int) {}

    pub(super) fn ignore(on: bool) {
        let handler = if on { handled as extern "C" fn(libc::c_int) as libc::sighandler_t } else { libc::SIG_DFL };
        // SAFETY: an empty handler, or the default, for SIGINT and SIGQUIT.
        unsafe {
            libc::signal(libc::SIGINT, handler);
            libc::signal(libc::SIGQUIT, handler);
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod interrupts {
    pub(super) fn ignore(_: bool) {}
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::workbench::app::App;

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn the_prompt_says_who_and_where_and_keeps_the_end_of_a_long_path() {
        let full = super::prompt(usize::MAX);
        assert!(full.starts_with(&super::who()), "{full}");
        let mark = if cfg!(windows) { '>' } else { '$' };
        assert!(full.ends_with(&format!("holon{mark}")), "the tests run in the crate's folder: {full}");
        let short = super::place(14);
        assert!(short.chars().count() <= 14 && short.starts_with('…') && short.ends_with("holon"), "{short}");
    }

    #[test]
    fn the_shell_line_runs_a_command_keeps_cd_and_recalls_what_ran() {
        let mut app = App::new(None);
        press(&mut app, KeyCode::Char('!'));
        assert_eq!(app.shell.as_deref(), Some(""));
        for c in "holon --version".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.shell.is_none());
        assert_eq!(app.run.take().as_deref(), Some("holon --version"), "for the event loop, which hands it the terminal");

        // `cd` is done in the editor, so it lasts (not tried here: the tests share one folder);
        // a folder that isn't there says so, and changes nothing.
        let here = std::env::current_dir().unwrap();
        app.shell_command("cd no-such-folder-here");
        assert!(app.note.as_ref().is_some_and(|n| n.error && n.text.starts_with("cd no-such-folder-here")));
        assert_eq!(std::env::current_dir().unwrap(), here);
        assert!(app.run.is_none());

        // ↑ goes back through what was entered, newest first.
        app.open_shell();
        press(&mut app, KeyCode::Up);
        assert_eq!(app.shell.as_deref(), Some("cd no-such-folder-here"));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.shell.as_deref(), Some("holon --version"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.shell.as_deref(), Some(""));
        press(&mut app, KeyCode::Esc);
        assert!(app.shell.is_none());
    }

    /// After a command, the session reads lines until one is for the terminal: `cd` is done on
    /// the way and says how it went; esc or `exit` ends it.
    #[test]
    fn a_session_takes_lines_until_a_command_and_esc_or_exit_ends_it() {
        let mut app = App::new(None);
        let mut lines = vec![Some("".to_string()), Some("cd no-such-folder-here".into()), Some("holon --version".into())].into_iter();
        let mut said = vec![];
        let next = app.next_command(|_| lines.next().flatten(), |t, bad| said.push((t.to_string(), bad)));
        assert_eq!(next.as_deref(), Some("holon --version"));
        assert!(said.len() == 1 && said[0].1 && said[0].0.starts_with("cd no-such-folder-here"), "{said:?}");
        assert!(app.run.is_none(), "the session runs it, not the event loop");
        assert_eq!(app.shell_history.last().map(String::as_str), Some("holon --version"));
        let mut seen = vec![];
        assert_eq!(app.next_command(|h| { seen = h.to_vec(); Some("exit".into()) }, |_, _| {}), None);
        assert!(seen.contains(&"cd no-such-folder-here".to_string()), "↑ has what was entered: {seen:?}");
        assert_eq!(app.next_command(|_| None, |_, _| {}), None, "esc");
    }
}
