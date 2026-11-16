//! What the whole server is held to, read from the machine it runs on: the memory it may hold
//! (`--max-memory`, by default a share of the machine's or its container's) and the connections
//! it holds open at once (what its open-file limit leaves room for). A round is held to its own
//! grant and a deployment to its share of the runs; these hold all of them together, where before
//! nothing did: 448 rounds at once could each hold 256 MiB, and a server out of file handles spun
//! in every accept loop (audit R16).

/// The memory the server holds itself to when the operator says nothing: half of what the machine
/// has, or three quarters of what the container it runs in may hold, whichever is less (a
/// container holds nothing else; a machine does, and what the allocator doesn't count, a module's
/// code and the threads' stacks, is the rest). None where neither can be read.
pub(crate) fn default_max_memory() -> Option<u64> {
    let machine = physical_memory().map(|m| m / 2);
    let container = container_memory().map(|m| m / 4 * 3);
    machine.into_iter().chain(container).min()
}

/// The connections the server holds open when nothing else is said: [`CONNECTIONS`], or fewer
/// where the open-file limit leaves less room, [`RESERVED_FILES`] kept back for the server's own
/// files, stores and outbound calls. On unix the limit is first raised as far as the system lets it
/// (as Go's runtime does), since a soft limit of 1,024 (most Linux) or 256 (macOS) is a default
/// for programs that don't hold many connections.
pub(crate) fn max_connections_total() -> usize {
    match open_files() {
        Some(files) => CONNECTIONS.min(files.saturating_sub(RESERVED_FILES).max(files / 2)).max(1),
        None => CONNECTIONS,
    }
}

/// The least `--max-memory` may be: a server held to less fails its runs before they begin, its
/// caches holding more than that.
pub(crate) const LEAST_MAX_MEMORY: u64 = 64 << 20;

/// The most connections the server holds open at once by default, every listener's together:
/// four of the 1,024 a listener holds.
pub(crate) const CONNECTIONS: usize = 4096;

/// The open files kept back from connections for the server's own: its stores, logs, folders and
/// outbound calls.
pub(crate) const RESERVED_FILES: usize = 256;

/// The machine's memory, in bytes.
#[cfg(windows)]
fn physical_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed MEMORYSTATUSEX is valid, and the call reads only its length.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a MEMORYSTATUSEX with its length set, as the call asks.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys).filter(|&m| m > 0)
}

/// The machine's memory, in bytes.
#[cfg(unix)]
fn physical_memory() -> Option<u64> {
    // SAFETY: sysconf reads a constant of the system.
    let (pages, size) = unsafe { (libc::sysconf(libc::_SC_PHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
    u64::try_from(pages).ok()?.checked_mul(u64::try_from(size).ok()?).filter(|&m| m > 0)
}

#[cfg(not(any(windows, unix)))]
fn physical_memory() -> Option<u64> {
    None
}

/// What the container the server runs in may hold, if it is held to anything: its cgroup's
/// `memory.max` (cgroup v2), or `memory.limit_in_bytes` (v1, where no limit reads as a number near
/// the largest there is).
#[cfg(target_os = "linux")]
fn container_memory() -> Option<u64> {
    let read = |path: &str| std::fs::read_to_string(path).ok().and_then(|t| t.trim().parse::<u64>().ok());
    read("/sys/fs/cgroup/memory.max").or_else(|| read("/sys/fs/cgroup/memory/memory.limit_in_bytes")).filter(|&m| m > 0 && m < 1 << 60)
}

#[cfg(not(target_os = "linux"))]
fn container_memory() -> Option<u64> {
    None
}

/// The files the server may hold open, raised first to what the system lets it: at most 65,536,
/// and on macOS, which refuses more than its own most for a process, 10,240 if that is refused.
/// None where nothing limits them (Windows holds sockets to memory, not a count).
#[cfg(unix)]
fn open_files() -> Option<usize> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes the limit it reads into `limit`.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    for want in [65_536, 10_240] {
        let want = limit.rlim_max.min(want);
        if limit.rlim_cur >= want {
            break;
        }
        let raised = libc::rlimit { rlim_cur: want, rlim_max: limit.rlim_max };
        // SAFETY: setrlimit reads `raised`; the soft limit is raised within the hard one.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
            limit.rlim_cur = want;
            break;
        }
    }
    Some(usize::try_from(limit.rlim_cur).unwrap_or(usize::MAX))
}

#[cfg(not(unix))]
fn open_files() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards: the server's default bounds come out as numbers a server can run within: memory
    /// of some, at most half the machine's, and connections from 1 to [`CONNECTIONS`], fewer
    /// than the open-file limit where there is one.
    /// Why: with no cap read, `--max-memory`'s default holds nothing; with one of zero, every run
    /// fails; a connection total past the open-file limit is the accept spin the total is for.
    /// If it fails: a platform's reading of its memory or its file limit is wrong, or the shares
    /// are taken the wrong way round.
    #[test]
    fn the_default_bounds_are_ones_a_server_runs_within() {
        let memory = default_max_memory().expect("the machine's memory is read");
        let machine = physical_memory().expect("the machine's memory is read");
        assert!(memory > 0 && memory <= machine / 2, "{memory} of {machine}");
        let total = max_connections_total();
        assert!((1..=CONNECTIONS).contains(&total), "{total}");
        if let Some(files) = open_files() {
            assert!(total < files, "{total} connections, {files} files");
        }
    }
}
