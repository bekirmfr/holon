//! The bytes a thread holds, counted as it allocates and frees: what holds a round to the memory
//! its host grants it (`max_memory`, [`Budgeted::with_memory`](crate::Budgeted::with_memory)).
//!
//! A program that installs [`Counting`] as its global allocator (the `holon` server, the editor's
//! wasm) has every allocation and free of a thread added to that thread's count, and [`held`]
//! reads it: one read of a thread-local, cheap enough to ask at every step of a round. Only
//! what a thread allocates and frees moves its count, so a value made on one thread and dropped
//! on another moves both counts the wrong way; a round runs on one thread and reads what its
//! count grew by while it ran, which counts what the round made and still holds, whatever came
//! before. Without the allocator installed the count stays at zero, and nothing is ever held
//! past a budget.
//!
//! What a round holds isn't the same on every build: a 32-bit build's values are smaller than a
//! 64-bit one's, and an allocator rounds sizes its own way. A round stopped for its memory is
//! therefore one a check can't run again to the same end (`holon verify` reads it as not run
//! again to its end, as it does one stopped for time); what is the same everywhere is that no
//! leaf joins a text past `MAX_TEXT` units, a list past `MAX_ITEMS` items or bytes past
//! `MAX_BYTES`, or reads more than `MAX_ITEMS` values out of one text or bytes, refused before
//! it is made.
//!
//! **The process's count.** A round's budget is its thread's, so a server running many rounds at
//! once held each to its `max_memory` and the whole to nothing: 448 rounds at once may each hold
//! 256 MiB (audit R16). The allocator also counts every thread's bytes into one [`process_held`],
//! each thread adding what it moved once it has moved [`FLUSH`] bytes either way, as a round ends
//! and as the thread ends, so the count costs an atomic add per 64 KiB allocated or freed and is
//! off by at most that much a thread. A host sets the most it may hold
//! ([`set_process_cap`]); a round [`Hold`]s its budget while it runs, and a step, or a leaf that
//! makes many values in one step (a parser), that finds the thread past its round's budget or the
//! process past its cap stops, and the round fails for it ([`stopped`]), where before one parse
//! could make hundreds of megabytes in a step before anything looked.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicI64, Ordering};

thread_local! {
    /// What this thread counts at every allocation and free: one thread-local, read once.
    static COUNT: Count = const { Count { held: Cell::new(0), unadded: Cell::new(BEGUN) } };
    /// The most this thread may hold while a round [`Hold`]s it: `i64::MAX` when none does.
    static AT_MOST: Cell<i64> = const { Cell::new(i64::MAX) };
    /// Whether a round holds this thread to its budget and the process's cap.
    static HOLDING: Cell<bool> = const { Cell::new(false) };
    /// Why a step or a leaf of the round holding this thread stopped for memory, if one did.
    static STOPPED: Cell<Option<Over>> = const { Cell::new(None) };
    /// Adds what the thread moved and hadn't added as it ends.
    static ENDING: Ending = const { Ending };
}

/// A thread's counts.
struct Count {
    /// The bytes this thread has allocated and not freed, since it began.
    held: Cell<i64>,
    /// What this thread allocated less what it freed since it last added to [`PROCESS`]; [`BEGUN`]
    /// past it until the thread's first count.
    unadded: Cell<i64>,
}

/// What a thread's `unadded` begins at: farther from zero than a thread ever moves, so the first
/// count takes the way a count past [`FLUSH`] does ([`add`]), which takes it off and arranges for
/// the thread's last moves to be added as it ends ([`ENDING`]). Every other count reads no more
/// than it did before the thread's first.
const BEGUN: i64 = i64::MIN / 4;

/// What adds a thread's last moves to the process's count as the thread ends ([`flush`]). Without
/// it a thread that ends adds nothing of what it moved since it last added: a thread a request
/// runs on frees the request another made and makes an answer another frees, under [`FLUSH`]
/// bytes each, and a server answering a million of them drifted by gigabytes, one way.
struct Ending;

impl Drop for Ending {
    fn drop(&mut self) {
        flush();
    }
}

/// Add what this thread moved and hadn't added to the process's count: as a round ends, and as
/// the thread does.
pub fn flush() {
    let _ = COUNT.try_with(|count| {
        // A thread that hasn't counted yet has nothing to add.
        if count.unadded.get() > BEGUN / 2 {
            let moved = count.unadded.replace(0);
            if moved != 0 {
                PROCESS.fetch_add(moved, Ordering::Relaxed);
            }
        }
    });
}

/// The bytes every thread holds, as each has added them ([`FLUSH`]).
static PROCESS: AtomicI64 = AtomicI64::new(0);

/// The most the process may hold for a round to go on ([`set_process_cap`]): none set, no limit.
static PROCESS_CAP: AtomicI64 = AtomicI64::new(i64::MAX);

/// What a thread moves its count by before it adds it to the process's: one atomic add per this
/// many bytes allocated or freed, and the process's count off by at most this much a thread.
pub const FLUSH: i64 = 64 << 10;

/// How many values a leaf that makes many in one step (a parser) makes between asking for
/// [`room`]: a thread-local read or two each time, and a few hundred kilobytes made between them.
pub const ASK_EVERY: usize = 4096;

/// What a round past its memory is past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Over {
    /// Its own budget, `max_memory`.
    Round,
    /// The process's cap: every round's together, and all else the process holds.
    Process,
}

/// The bytes the whole process holds, by the [`Counting`] allocator, within [`FLUSH`] a thread.
#[inline]
pub fn process_held() -> i64 {
    PROCESS.load(Ordering::Relaxed)
}

/// Hold the process to `bytes`: a round that finds it past them stops ([`room`]). `u64::MAX`: no
/// cap.
pub fn set_process_cap(bytes: u64) {
    PROCESS_CAP.store(i64::try_from(bytes).unwrap_or(i64::MAX), Ordering::Relaxed);
}

/// The process's cap ([`set_process_cap`]).
pub fn process_cap() -> u64 {
    u64::try_from(PROCESS_CAP.load(Ordering::Relaxed)).unwrap_or(0)
}

/// Whether the process holds more than its cap.
#[inline]
pub fn process_over() -> bool {
    process_held() > PROCESS_CAP.load(Ordering::Relaxed)
}

/// A round's hold on its thread while it runs: the most the thread may hold (its budget, as
/// [`Budgeted::with_memory`](crate::Budgeted::with_memory) reckons it), and the process's cap.
/// What it found when made comes back when it is dropped, so a round run inside another (a flow's
/// call) leaves the outer one held as it was.
pub struct Hold {
    before: (bool, i64, Option<Over>),
}

impl Hold {
    /// Hold this thread to `at_most` bytes ([`held`]) and the process to its cap.
    pub fn new(at_most: i64) -> Hold {
        let before = (HOLDING.with(Cell::get), AT_MOST.with(Cell::get), STOPPED.with(Cell::get));
        HOLDING.with(|h| h.set(true));
        AT_MOST.with(|m| m.set(at_most));
        STOPPED.with(|s| s.set(None));
        Hold { before }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let (holding, at_most, stopped) = self.before;
        HOLDING.with(|h| h.set(holding));
        AT_MOST.with(|m| m.set(at_most));
        STOPPED.with(|s| s.set(stopped));
        flush();
    }
}

/// Whether the round holding this thread may make more: not when the thread holds more than its
/// budget or the process more than its cap, and once not, never again in the round ([`stopped`]
/// says why). A leaf that makes many values in one step asks every so often, and stops, giving
/// nothing, when it may not; the round then fails at its next step or as it ends, never answering
/// with what the leaf left out. Always, when no round holds the thread.
#[inline]
pub fn room() -> bool {
    if !HOLDING.with(Cell::get) {
        return true;
    }
    if STOPPED.with(Cell::get).is_some() {
        return false;
    }
    let over = if held() > AT_MOST.with(Cell::get) {
        Over::Round
    } else if process_over() {
        Over::Process
    } else {
        return true;
    };
    STOPPED.with(|s| s.set(Some(over)));
    false
}

/// Why the round holding this thread stopped for memory, if it did ([`room`]).
#[inline]
pub fn stopped() -> Option<Over> {
    STOPPED.try_with(Cell::get).ok().flatten()
}

/// The bytes this thread holds, by the [`Counting`] allocator: what it has allocated and not
/// freed since it began. Zero, always, without the allocator installed. Only a difference of two
/// readings means anything: what the thread made and kept between them.
#[inline]
pub fn held() -> i64 {
    COUNT.try_with(|count| count.held.get()).unwrap_or(0)
}

/// Runs `f`, and leaves off this thread's count what it kept, but for `counted` bytes of what it
/// hands back: for what a process keeps for all its runs (a module compiled once, and each of its
/// functions translated as a call first reaches it), made in a round only because the round was
/// the first to want it, and what a call holds and frees within `f`. `counted` is how much of what
/// `f` hands back is the caller's, counted as anything it makes.
pub fn uncounted<T>(f: impl FnOnce() -> T, counted: impl FnOnce(&T) -> usize) -> T {
    let before = held();
    let out = f();
    let kept = held() - before - i64::try_from(counted(&out)).unwrap_or(i64::MAX);
    if kept > 0 {
        // Off the thread's count alone: the process does hold it.
        let _ = COUNT.try_with(|count| count.held.set(count.held.get().wrapping_sub(kept)));
    }
    out
}

#[cfg(test)]
thread_local! {
    /// The most this thread has held since a test set it: how far a leaf got before it stopped.
    static PEAK: Cell<i64> = const { Cell::new(0) };
}

#[inline]
fn count(bytes: i64) {
    // `try_with`: a thread being torn down still frees, after its count is gone.
    let _ = COUNT.try_with(|count| {
        let held = count.held.get().wrapping_add(bytes);
        count.held.set(held);
        #[cfg(test)]
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(held)));
        let moved = count.unadded.get().wrapping_add(bytes);
        if moved.unsigned_abs() < FLUSH as u64 {
            count.unadded.set(moved);
        } else {
            add(count, moved);
        }
    });
}

/// A count that takes `unadded` past [`FLUSH`] either way: added to the process's. The thread's
/// first count comes here too ([`BEGUN`]), and arranges for its last moves to be added as it ends.
#[cold]
fn add(count: &Count, moved: i64) {
    if moved < BEGUN / 2 {
        // `unadded` set first: arranging may allocate (a list of what runs as the thread ends),
        // and so count again, which then finds the thread begun and arranges nothing.
        count.unadded.set(moved - BEGUN);
        let _ = ENDING.try_with(|_| {});
        let moved = count.unadded.get();
        if moved.unsigned_abs() >= FLUSH as u64 {
            PROCESS.fetch_add(moved, Ordering::Relaxed);
            count.unadded.set(0);
        }
        return;
    }
    PROCESS.fetch_add(moved, Ordering::Relaxed);
    count.unadded.set(0);
}

/// A global allocator that counts, on each thread, the bytes it allocates and frees ([`held`]),
/// and leaves the allocating to `A`:
///
/// ```ignore
/// #[global_allocator]
/// static ALLOCATOR: holon_core::memory::Counting<std::alloc::System> = holon_core::memory::Counting(std::alloc::System);
/// ```
pub struct Counting<A>(pub A);

// SAFETY: every call is `A`'s, with the same arguments; counting reads and writes thread-local
// `Cell`s initialised as constants and adds to an atomic, which neither allocates nor unwinds, and
// at a thread's first count arranges for its count to be added as it ends ([`Ending`]), which may
// allocate once, after `A` has answered, and counts that as any allocation (the thread begun by
// then, it arranges nothing again).
unsafe impl<A: GlobalAlloc> GlobalAlloc for Counting<A> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as the caller's.
        let p = unsafe { self.0.alloc(layout) };
        if !p.is_null() {
            count(layout.size() as i64);
        }
        p
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as the caller's.
        let p = unsafe { self.0.alloc_zeroed(layout) };
        if !p.is_null() {
            count(layout.size() as i64);
        }
        p
    }
    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as the caller's.
        unsafe { self.0.dealloc(ptr, layout) };
        count(-(layout.size() as i64));
    }
    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as the caller's.
        let p = unsafe { self.0.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            count(new_size as i64 - layout.size() as i64);
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards: [`uncounted`] leaves off the thread's count what `f` kept for the process, and
    /// counts what it hands back as the caller's: a call that keeps 4 MiB in a cache and answers
    /// 1 MiB moves the count by the 1 MiB alone; one that keeps nothing moves it by its answer;
    /// one that frees what came before it moves it down, as the free is.
    /// Why: a round's first js() call compiles its module and translates its functions, which the
    /// process keeps for every run after; counted as the round's, a call well within its grant
    /// failed the round for memory it would never hold again.
    /// If it fails: `uncounted` takes off more than `f` kept (the answer goes uncounted, and a
    /// round may keep it past its grant), or less (the process's caches count against a round).
    #[test]
    fn uncounted_leaves_off_what_a_call_keeps_but_counts_what_it_answers() {
        const MIB: usize = 1 << 20;
        let mut cache: Vec<Vec<u8>> = Vec::new();
        let before = held();
        let answer = uncounted(
            || {
                cache.push(vec![1; 4 * MIB]);
                vec![2u8; MIB]
            },
            Vec::capacity,
        );
        assert_eq!(held() - before, MIB as i64, "the answer alone is counted");
        let before = held();
        let again = uncounted(|| vec![3u8; MIB], Vec::capacity);
        assert_eq!(held() - before, MIB as i64, "an answer with nothing kept is counted whole");
        let before = held();
        uncounted(|| drop(answer), |()| 0);
        assert_eq!(held() - before, -(MIB as i64), "a free in it is a free");
        drop((cache, again));
    }

    /// Guards: a round is held to its budget and the process to its cap by every leaf that makes
    /// many values in one step, inside the leaf: `json_parse`, `csv_parse` (both headers) and
    /// `xml_parse` of a text that makes megabytes, under a [`Hold`], give nothing and say why once
    /// past the round's budget (`Over::Round`) or the process's cap (`Over::Process`), having
    /// made at most a few [`ASK_EVERY`]s of values past it, stay stopped, and give it all when
    /// neither is passed or no round holds the thread; a hold let go leaves the thread as it was.
    /// (tests/process_memory.rs guards the process's count itself.)
    /// Why: a parse made 80 times its text in one step, hundreds of megabytes before a step looked,
    /// and a server's rounds together were held to nothing (audit R16).
    /// If it fails: a parser doesn't ask [`room`] as it makes values, or `room` doesn't read the
    /// round's budget or the process's cap, or doesn't stay stopped once stopped.
    #[test]
    fn each_parser_stops_inside_the_leaf_past_the_round_or_the_process() {
        // A text for each parser that makes megabytes of values: a record each line, a field each.
        let json = format!("[{}]", vec![r#"{"name":"a","n":1}"#; 60_000].join(","));
        let csv = format!("name,n\n{}", "a,1\n".repeat(120_000));
        let xml = format!("<r>{}</r>", "<i n=\"1\">a</i>".repeat(60_000));
        let parsers: [(&str, &dyn Fn() -> Option<crate::Value>); 4] = [
            ("json_parse", &|| crate::json::parse(&json)),
            ("csv_parse first_row", &|| crate::csv::parse(&csv, "first_row", ",")),
            ("csv_parse none", &|| crate::csv::parse(&csv, "none", ",")),
            ("xml_parse", &|| crate::xml::parse(&xml)),
        ];
        const MIB: i64 = 1 << 20;
        for (what, parse) in parsers {
            let before = held();
            let whole = parse();
            let made = held() - before;
            assert!(whole.is_some() && made > 8 * MIB, "{what}: no round holds the thread, so it is made whole ({made} bytes)");
            drop(whole);
            // Held to its round's budget: a megabyte.
            let hold = Hold::new(held() + MIB);
            let before = held();
            PEAK.with(|p| p.set(before));
            assert_eq!(parse(), None, "{what}: past the round's budget, nothing");
            let peak = PEAK.with(Cell::get) - before;
            assert_eq!(stopped(), Some(Over::Round), "{what}: and says why");
            assert!(!room(), "{what}: and stays stopped");
            drop(hold);
            assert_eq!(stopped(), None, "{what}: a hold let go leaves the thread as it found it");
            assert!(peak < 8 * MIB, "{what}: it stops as it makes, not once it is done ({peak} bytes)");
            // Within both: made whole.
            let hold = Hold::new(i64::MAX);
            assert!(parse().is_some() && stopped().is_none(), "{what}: within the round's budget and the process's cap, made whole");
            // Past the process's cap: nothing, and the process said. No other test holds a round
            // to it: a parse beside this one, unheld, goes on.
            set_process_cap(0);
            let before = held();
            PEAK.with(|p| p.set(before));
            let parsed = parse();
            let peak = PEAK.with(Cell::get) - before;
            set_process_cap(u64::MAX);
            assert_eq!((parsed, stopped()), (None, Some(Over::Process)), "{what}: past the process's cap, nothing");
            assert!(peak < 8 * MIB, "{what}: it stops as it makes, not once it is done ({peak} bytes)");
            drop(hold);
        }
    }
}
