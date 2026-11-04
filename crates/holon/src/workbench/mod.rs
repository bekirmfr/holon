//! `holon editor`: the editor in a terminal, on one screen. The flows on the server and what waits
//! for a person, a flow's status, runs and history, and its text in tabs, checked as it is typed,
//! with what it may do beside it; deployed from here as the person signed in, who approves what
//! it asks for by deploying it. With no server, it still edits, checks and formats files.

mod admin;
mod app;
mod client;
mod diff;
mod flow;
mod graph;
mod palette;
mod review;
mod shell;
mod text;
mod ui;

#[cfg(test)]
mod tests;

use std::io::Write;
use std::time::Duration;

use ratatui::crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyEventKind};
use ratatui::crossterm::execute;

use app::App;

const USAGE: &str = "usage: holon editor [file.holon | slug] [--server PORT|URL]

  The editor in this terminal: the flows on a server and what waits for you, each flow's
  status, runs and history, and flows' text in tabs, checked as you type. Deploying from here
  asks you to approve what a flow may do, as the browser's editor does. ? in the editor lists
  its keys.

  file.holon               open that file in a tab (started afresh if it isn't there)
  slug                     open that deployment's page
  --server PORT|URL        which holon server to ask (see below); without a server, files still
                           open, check and format

examples:
  holon editor
  holon editor shop.holon
  holon editor shop --server 7421

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

pub(crate) fn run(args: &[String]) -> i32 {
    let mut server: Option<String> = None;
    let mut open: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--server" => match it.next() {
                Some(s) => server = Some(s.clone()),
                None => {
                    eprintln!("{USAGE}");
                    return 2;
                }
            },
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return 0;
            }
            _ if open.is_none() && !a.starts_with('-') => open = Some(a.clone()),
            _ => {
                eprintln!("holon editor: no option {a:?}\n\n{USAGE}");
                return 2;
            }
        }
    }
    let mut app = App::new(server);
    app.connect();
    if let Some(o) = &open {
        app.open_arg(o);
    }
    let mut terminal = ratatui::init();
    // The mouse clicks and scrolls; a paste arrives whole, as one edit (shift+drag still selects
    // the terminal's own text in most terminals).
    let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let result = event_loop(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("holon editor: {e}");
            1
        }
    }
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    while !app.quit {
        terminal.draw(|f| ui::draw(f, app))?;
        if let Some(text) = app.to_clipboard.take() {
            // OSC 52: the terminal puts it on the system's clipboard (where it allows that).
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
            let _ = out.flush();
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                // Windows reports releases too.
                Event::Key(k) if k.kind != KeyEventKind::Release => app.key(k),
                Event::Paste(text) => app.paste(&text),
                Event::Mouse(m) => app.mouse(m),
                _ => {}
            }
        }
        if let Some(line) = app.run.take() {
            hand_over(terminal, app, &line)?;
        }
        app.tick();
    }
    Ok(())
}

/// Step aside for a command from the shell line: the terminal as it was before the editor, the
/// command in it; then the prompt again, as the folder is now, for the next, until esc brings
/// the editor back.
fn hand_over(terminal: &mut ratatui::DefaultTerminal, app: &mut App, line: &str) -> std::io::Result<()> {
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    shell::echo(line);
    let mut line = Some(line.to_string());
    while let Some(l) = line.take() {
        let ended = shell::run(&l);
        shell::ended(&ended);
        app.after_shell(&l, &ended);
        line = app.next_command(shell::read_line, shell::say);
    }
    *terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    terminal.clear()?;
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}
