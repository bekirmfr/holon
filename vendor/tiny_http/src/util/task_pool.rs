use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// Manages a collection of threads.
///
/// A new thread is created every time all the existing threads are full.
/// Any idle thread will automatically die after a few seconds.
pub struct TaskPool {
    sharing: Arc<Sharing>,
}

struct Sharing {
    // list of the tasks to be done by worker threads
    todo: Mutex<VecDeque<Box<dyn FnMut() + Send>>>,

    // condvar that will be notified whenever a task is added to `todo`
    condvar: Condvar,

    // number of total worker threads running
    active_tasks: AtomicUsize,

    // number of idle worker threads
    waiting_tasks: AtomicUsize,
}

/// Minimum number of active threads.
static MIN_THREADS: usize = 4;

struct Registration<'a> {
    nb: &'a AtomicUsize,
}

impl<'a> Registration<'a> {
    fn new(nb: &'a AtomicUsize) -> Registration<'a> {
        nb.fetch_add(1, Ordering::Release);
        Registration { nb }
    }
}

impl<'a> Drop for Registration<'a> {
    fn drop(&mut self) {
        self.nb.fetch_sub(1, Ordering::Release);
    }
}

impl TaskPool {
    pub fn new() -> TaskPool {
        let pool = TaskPool {
            sharing: Arc::new(Sharing {
                todo: Mutex::new(VecDeque::new()),
                condvar: Condvar::new(),
                active_tasks: AtomicUsize::new(0),
                waiting_tasks: AtomicUsize::new(0),
            }),
        };

        for _ in 0..MIN_THREADS {
            pool.add_thread(None)
        }

        pool
    }

    /// Executes a function in a thread.
    /// If no thread is available, spawns a new one.
    pub fn spawn(&self, code: Box<dyn FnMut() + Send>) {
        let mut queue = self.sharing.todo.lock().unwrap();

        // Holon (PATCHED.md): a woken thread is counted as waiting until it holds the lock again,
        // so `waiting_tasks` alone counts threads already told of a queued task: queued past
        // them, a task waited for a thread to finish a connection, which can be never. Queue only
        // while the waiting threads outnumber the tasks already queued.
        if self.sharing.waiting_tasks.load(Ordering::Acquire) <= queue.len() {
            self.add_thread(Some(code));
        } else {
            queue.push_back(code);
            self.sharing.condvar.notify_one();
        }
    }

    fn add_thread(&self, initial_fn: Option<Box<dyn FnMut() + Send>>) {
        let sharing = self.sharing.clone();

        thread::spawn(move || {
            let sharing = sharing;
            let _active_guard = Registration::new(&sharing.active_tasks);

            if let Some(mut f) = initial_fn {
                f();
            }

            loop {
                let mut task: Box<dyn FnMut() + Send> = {
                    let mut todo = sharing.todo.lock().unwrap();

                    let task;
                    loop {
                        if let Some(poped_task) = todo.pop_front() {
                            task = poped_task;
                            break;
                        }
                        let _waiting_guard = Registration::new(&sharing.waiting_tasks);

                        let received =
                            if sharing.active_tasks.load(Ordering::Acquire) <= MIN_THREADS {
                                todo = sharing.condvar.wait(todo).unwrap();
                                true
                            } else {
                                let (new_lock, waitres) = sharing
                                    .condvar
                                    .wait_timeout(todo, Duration::from_millis(5000))
                                    .unwrap();
                                todo = new_lock;
                                !waitres.timed_out()
                            };

                        if !received && todo.is_empty() {
                            return;
                        }
                    }

                    task
                };

                task();
            }
        });
    }
}

impl Drop for TaskPool {
    fn drop(&mut self) {
        self.sharing
            .active_tasks
            .store(999_999_999, Ordering::Release);
        self.sharing.condvar.notify_all();
    }
}

// Holon (PATCHED.md): what the tests below and the server's read of the pool.
impl TaskPool {
    /// The threads the pool has now, busy or idle.
    #[cfg(test)]
    pub(crate) fn threads(&self) -> usize {
        self.sharing.active_tasks.load(Ordering::Acquire)
    }
}

// Holon (PATCHED.md): the accept thread spawns a task per connection, back to back when they come
// in together, and each task holds its thread while its connection lives. Before the fix, a burst
// larger than the idle threads queued tasks for threads already told of others: those started
// only once some connection closed (a page's reads left without an answer).
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    /// What a test here waits for, it waits this long for: a guard against a hang, not a measure
    /// (as Holon's own tests, `EVENTUALLY` in crates/holon/src/tests.rs, whose timing sweep reads
    /// this module). Without the fix, a task told to a busy thread never starts here: no hold ends
    /// while the test waits.
    const EVENTUALLY: Duration = Duration::from_secs(60);

    fn until(what: &str, done: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while !done() {
            if start.elapsed() > EVENTUALLY {
                eprintln!("not within {:?}: {}", EVENTUALLY, what);
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
        true
    }

    /// Bursts with no gap between spawns, onto a pool whose threads are idle (its first four,
    /// or those connections just left), each task held like a kept-alive connection: every task
    /// starts while the others still hold their threads.
    #[test]
    fn every_task_of_a_burst_starts_while_the_others_hold_their_threads() {
        // (connections that came and went just before, connections in the burst)
        for &(warm, burst) in &[(0, 6), (8, 13)] {
            for trial in 0..40 {
                let pool = TaskPool::new();
                if warm > 0 {
                    let release = Arc::new(AtomicBool::new(false));
                    for _ in 0..warm {
                        let release = release.clone();
                        pool.spawn(Box::new(move || {
                            while !release.load(Ordering::Acquire) {
                                thread::sleep(Duration::from_millis(1));
                            }
                        }));
                    }
                    release.store(true, Ordering::Release);
                }
                let idle = warm.max(MIN_THREADS);
                assert!(
                    until("the pool's threads are idle", || {
                        pool.sharing.waiting_tasks.load(Ordering::Acquire) >= idle
                    }),
                    "warm {} trial {}: the pool's threads never went idle",
                    warm,
                    trial
                );
                let started = Arc::new(AtomicUsize::new(0));
                let hold = Arc::new(AtomicBool::new(false));
                for _ in 0..burst {
                    let started = started.clone();
                    let hold = hold.clone();
                    pool.spawn(Box::new(move || {
                        started.fetch_add(1, Ordering::AcqRel);
                        while !hold.load(Ordering::Acquire) {
                            thread::sleep(Duration::from_millis(1));
                        }
                    }));
                }
                let all = until("every task starts", || {
                    started.load(Ordering::Acquire) == burst
                });
                let n = started.load(Ordering::Acquire);
                hold.store(true, Ordering::Release);
                assert!(
                    all,
                    "warm {} trial {}: {} of {} tasks never started while the others held their threads",
                    warm,
                    trial,
                    burst - n,
                    burst
                );
            }
        }
    }
}
