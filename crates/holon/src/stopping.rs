//! Stopping `holon serve`: told to (ctrl+c, SIGTERM, or its console closing), the server starts
//! no more runs, finishes those it started, each to its log line, syncs the logs and the witness's
//! queue, and exits. Told twice, it exits at once.
//!
//! Its triggers ask `Deployment::takes_runs` before they start a run, as they ask whether its
//! deployment is paused, and its requests are answered 503 (`admitted`, `serve_flow`): the
//! server's call hub says it is stopping (`calls::Hub::stop`). A run holds the door open ([`run`])
//! from when it starts (`fire_with_state`) until its `FiredRun` drops, after its log line; so does
//! a flow's request, whole (`serve_flow`), and the writing of a log line (`log_run`, [`hold`]), so
//! the door is never left unheld between a run and its line. It closes as the last lets go, in
//! one step ([`Door::close_if_empty`]): a run a trigger let through before the server was told,
//! arriving after, waits at the door until the process exits, never started, so nothing of it is
//! half done. A run inside a held one (a flow's call) finds the door held, and is never shut out.

use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::{Duration, Instant};

/// What a request to a server stopping is answered, 503.
pub(crate) const REFUSED: &str = "the server is stopping: try again once it is back";

/// What holds the door open (the runs, the requests being answered, the lines being written),
/// with whether it is closed in the top bit; and the runs alone, to say how many go on.
pub(crate) struct Door {
    held: AtomicUsize,
    runs: AtomicUsize,
}

const CLOSED: usize = 1 << (usize::BITS - 1);

impl Door {
    pub(crate) const fn new() -> Door {
        Door { held: AtomicUsize::new(0), runs: AtomicUsize::new(0) }
    }

    /// Hold the door open (a run, if `run`) unless it is closed.
    fn enter(&'static self, run: bool) -> Option<Held> {
        self.held.fetch_update(SeqCst, SeqCst, |n| (n & CLOSED == 0).then_some(n + 1)).ok()?;
        if run {
            self.runs.fetch_add(1, SeqCst);
        }
        Some(Held { door: self, run })
    }

    /// How many runs are going on.
    pub(crate) fn runs(&self) -> usize {
        self.runs.load(SeqCst)
    }

    /// Close the door if nothing holds it: whether it is closed.
    fn close_if_empty(&self) -> bool {
        matches!(self.held.compare_exchange(0, CLOSED, SeqCst, SeqCst), Ok(_) | Err(CLOSED))
    }
}

/// The door held open while this lives.
pub(crate) struct Held {
    door: &'static Door,
    run: bool,
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.run {
            self.door.runs.fetch_sub(1, SeqCst);
        }
        self.door.held.fetch_sub(1, SeqCst);
    }
}

/// This server's door.
pub(crate) static DOOR: Door = Door::new();

/// Hold the door open for a run until what this answers drops. Once the door is closed, the
/// server is about to exit: this waits for it, and the run never starts.
pub(crate) fn run() -> Held {
    through(true)
}

/// The same for what isn't a run: a flow's request answered, a log line written.
pub(crate) fn hold() -> Held {
    through(false)
}

fn through(run: bool) -> Held {
    match DOOR.enter(run) {
        Some(held) => held,
        None => loop {
            std::thread::park();
        },
    }
}

/// How many times the server was told to stop: the second time, the handler exits at once.
static TOLD: AtomicUsize = AtomicUsize::new(0);

/// Have ctrl+c, SIGTERM and the console closing tell the server to stop.
pub(crate) fn install() {
    signals::install();
}

/// Wait until the server is told to stop.
pub(crate) fn wait_until_told() {
    while TOLD.load(SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Wait until nothing holds `door` open, then close it, saying how many runs are left every
/// `every`.
pub(crate) fn drain(door: &Door, every: Duration, say: impl Fn(usize)) {
    let mut said = Instant::now();
    while !door.close_if_empty() {
        if said.elapsed() >= every {
            say(door.runs());
            said = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
mod signals {
    use super::{SeqCst, TOLD};

    extern "C" fn told(_: libc::c_int) {
        if TOLD.fetch_add(1, SeqCst) > 0 {
            // SAFETY: `_exit` is safe in a signal handler; nothing is flushed.
            unsafe { libc::_exit(130) };
        }
    }

    pub(super) fn install() {
        let handler = told as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // SAFETY: a handler that only counts, or exits.
        unsafe {
            libc::signal(libc::SIGINT, handler);
            libc::signal(libc::SIGTERM, handler);
        }
    }
}

#[cfg(windows)]
mod signals {
    use super::{SeqCst, TOLD};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::TRUE;
    use windows_sys::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT};

    unsafe extern "system" fn told(event: u32) -> BOOL {
        let before = TOLD.fetch_add(1, SeqCst);
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT if before > 0 => std::process::exit(130),
            CTRL_C_EVENT | CTRL_BREAK_EVENT => TRUE,
            // Its console closing, a sign-out or a shutdown: the process ends as this returns, so
            // it doesn't, and the server exits once stopped, or when Windows ends it, a few
            // seconds on.
            _ => loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            },
        }
    }

    pub(super) fn install() {
        // SAFETY: a handler that only counts, waits or exits.
        unsafe { SetConsoleCtrlHandler(Some(told), TRUE) };
    }
}

#[cfg(not(any(windows, unix)))]
mod signals {
    pub(super) fn install() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    /// Guards: the door closes only when no run goes on, and a run either is counted before it
    /// closes, and waited for, or finds it closed and never starts.
    /// Why: the server exits once the door is closed; a run that starts after the drain saw none
    /// would be cut off half done (an outbound call made, its line never written).
    /// If it fails: a run in flight at the exit, or a nested run (a flow's call) shut out by its
    /// own outer run's door, which would hang the drain.
    #[test]
    fn the_door_closes_only_on_no_runs_and_a_run_is_counted_or_turned_away() {
        static DOOR: Door = Door::new();
        let request = DOOR.enter(false).expect("open");
        let outer = DOOR.enter(true).expect("open");
        assert!(!DOOR.close_if_empty(), "a run goes on");
        let inner = DOOR.enter(true).expect("a run inside a held door is never shut out");
        assert_eq!(DOOR.runs(), 2, "runs, not what else holds it");
        drop((inner, outer));
        assert_eq!(DOOR.runs(), 0);
        assert!(!DOOR.close_if_empty(), "the request that ran them still holds it");
        drop(request);
        assert!(DOOR.close_if_empty());
        assert!(DOOR.close_if_empty(), "closed stays closed");
        assert!(DOOR.enter(true).is_none() && DOOR.enter(false).is_none());

        // Runs racing the drain: each either got in before the door closed, and was waited for,
        // or found it closed. None ran past the close.
        for _ in 0..200 {
            static RACED: Door = Door::new();
            RACED.held.store(0, SeqCst);
            let after = Arc::new(AtomicUsize::new(0));
            let start = Arc::new(Barrier::new(9));
            let runners: Vec<_> = (0..8)
                .map(|_| {
                    let (after, start) = (Arc::clone(&after), Arc::clone(&start));
                    std::thread::spawn(move || {
                        start.wait();
                        if let Some(running) = RACED.enter(true) {
                            std::thread::yield_now();
                            if RACED.held.load(SeqCst) & CLOSED != 0 {
                                after.fetch_add(1, SeqCst);
                            }
                            drop(running);
                        }
                    })
                })
                .collect();
            start.wait();
            drain(&RACED, Duration::from_secs(60), |_| {});
            for r in runners {
                r.join().unwrap();
            }
            assert_eq!(after.load(SeqCst), 0, "a run went on after the door closed");
        }
    }

    /// Guards: the drain says how many runs are left while it waits, and returns once none is.
    /// Why: a stop that waits says on what, or it looks hung.
    /// If it fails: a stopping server silent while it waits, or one that never finishes.
    #[test]
    fn the_drain_waits_for_the_last_run_saying_how_many_are_left() {
        static DOOR: Door = Door::new();
        let running = DOOR.enter(true).unwrap();
        let said = Arc::new(std::sync::Mutex::new(vec![]));
        let heard = Arc::clone(&said);
        let drained = std::thread::spawn(move || drain(&DOOR, Duration::from_millis(20), |n| heard.lock().unwrap().push(n)));
        std::thread::sleep(Duration::from_millis(200));
        assert!(!drained.is_finished());
        drop(running);
        drained.join().unwrap();
        let said = said.lock().unwrap();
        assert!(!said.is_empty() && said.iter().all(|n| *n == 1), "{said:?}");
        assert!(DOOR.enter(true).is_none());
    }
}
