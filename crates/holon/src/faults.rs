//! Writes made to fail by name, for the tests that hold a write that fails to never being taken
//! as done (release audit R28): an event's line, a spooled mail or message. Per thread, so tests
//! side by side don't fail each other's writes. Nothing outside the tests: there each check is
//! `None`, inlined away.

#[cfg(test)]
thread_local! {
    /// The writes failing on this thread: each by name, and whether part of it lands first.
    static FAILING: std::cell::RefCell<Vec<(&'static str, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The writes named `what` fail on this thread from now until [`heal`]; with `partway`, part of
/// each lands first, as a disk filling mid-write leaves it.
#[cfg(test)]
pub(crate) fn fail(what: &'static str, partway: bool) {
    FAILING.with(|f| f.borrow_mut().push((what, partway)));
}

/// No write fails on this thread any more.
#[cfg(test)]
pub(crate) fn heal() {
    FAILING.with(|f| f.borrow_mut().clear());
}

/// Whether the write named `what` fails: `Some(partway)` if it does.
#[cfg(test)]
pub(crate) fn failing(what: &str) -> Option<bool> {
    FAILING.with(|f| f.borrow().iter().find(|(name, _)| *name == what).map(|(_, partway)| *partway))
}

#[cfg(not(test))]
#[inline(always)]
pub(crate) fn failing(_what: &str) -> Option<bool> {
    None
}

/// The error a failed write gives.
pub(crate) fn error(what: &str) -> std::io::Error {
    std::io::Error::other(format!("a write of {what} made to fail"))
}
