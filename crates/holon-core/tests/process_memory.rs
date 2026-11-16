//! The process's memory count (holon_core::memory) against what its threads move: a test binary
//! of its own, so no other test's memory moves the count as it is read.

use holon_core::memory::{process_held, FLUSH};

#[global_allocator]
static ALLOCATOR: holon_core::memory::Counting<std::alloc::System> = holon_core::memory::Counting(std::alloc::System);

/// Guards: the process's count holds what every thread holds, threads that end included: a
/// thousand threads that each make 40 KiB (less than [`FLUSH`], so none adds it as it goes) and
/// end, the 40 KiB freed on a thread that goes on, leave the count where it began, within a few
/// [`FLUSH`]es.
/// Why: a thread adds its moves to the process's count a [`FLUSH`] at a time, and one that ends
/// added nothing of what it moved since it last did: a thread a request runs on (the control plane
/// starts one for each) makes an answer another frees, and a server answering a million of them
/// drifted by gigabytes, one way, until its cap (`--max-memory`) held to nothing or to too little
/// (audit R16).
/// If it fails: a thread's last moves aren't added as it ends (`Ending` in memory.rs isn't arranged
/// for each thread, or doesn't add), or are added twice.
#[test]
fn a_thread_that_ends_leaves_the_process_count_as_its_moves_do() {
    let before = process_held();
    let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
    let freeing = std::thread::spawn(move || {
        let mut freed = 0usize;
        for made in receiver {
            freed += made.len();
            drop(made);
        }
        freed
    });
    for _ in 0..1000 {
        let sender = sender.clone();
        std::thread::spawn(move || sender.send(vec![7u8; 40 << 10]).unwrap()).join().unwrap();
    }
    drop(sender);
    assert_eq!(freeing.join().unwrap(), 1000 * (40 << 10));
    let drift = process_held() - before;
    assert!(drift.abs() < 16 * FLUSH, "a thousand threads that ended left the process's count {drift} bytes from where it began");
}
