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

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

thread_local! {
    /// The bytes this thread has allocated and not freed, since it began.
    static HELD: Cell<i64> = const { Cell::new(0) };
}

/// The bytes this thread holds, by the [`Counting`] allocator: what it has allocated and not
/// freed since it began. Zero, always, without the allocator installed. Only a difference of two
/// readings means anything: what the thread made and kept between them.
#[inline]
pub fn held() -> i64 {
    HELD.try_with(Cell::get).unwrap_or(0)
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
        count(-kept);
    }
    out
}

#[inline]
fn count(bytes: i64) {
    // `try_with`: a thread being torn down still frees, after its count is gone.
    let _ = HELD.try_with(|held| held.set(held.get().wrapping_add(bytes)));
}

/// A global allocator that counts, on each thread, the bytes it allocates and frees ([`held`]),
/// and leaves the allocating to `A`:
///
/// ```ignore
/// #[global_allocator]
/// static ALLOCATOR: holon_core::memory::Counting<std::alloc::System> = holon_core::memory::Counting(std::alloc::System);
/// ```
pub struct Counting<A>(pub A);

// SAFETY: every call is `A`'s, with the same arguments; counting reads and writes a thread-local
// `Cell` initialised as a constant, which neither allocates nor unwinds.
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
}
