//! `holon editor`: the editor in a terminal, on one screen. The flows on the server and what waits
//! for a person, a flow's status, runs and history, and its text in tabs, checked as it is typed,
//! with what it may do beside it; deployed from here as the person signed in, who approves what
//! it asks for by deploying it. With no server, it still edits, checks and formats files.

mod app;
mod client;
mod text;
mod ui;

#[cfg(test)]
mod tests;

use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::App;

const USAGE: &str = "usage: holon editor [file.holon | slug] [--server PORT|URL]

  The editor in this terminal: the flows on a server and what waits for you, each flow's
  status, runs and history, and flows' text in tabs, checked as you type. Deploying from
  here asks you to approve what a flow may do, as the browser's editor does.

  A file opens in a tab (a new one is started); a deployment's name opens its page.
  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL>, with HOLON_TOKEN.
  Without a server, files still open, check and format.

  ? in the editor lists the keys.";

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
    let result = event_loop(&mut terminal, &mut app);
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
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                // Windows reports releases too.
                Event::Key(k) if k.kind != KeyEventKind::Release => app.key(k),
                Event::Paste(text) => app.paste(&text),
                _ => {}
            }
        }
        app.tick();
    }
    Ok(())
}
