//! Signal handling: SIGPIPE ignored, SIGINT stops the run.
//!
//! The interrupt handler performs one atomic store, the same
//! async-signal-safe shape as the wrk handler.

use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupt(_signal: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Installs the handlers in the wrk order.
pub fn install() {
    // SAFETY: the handlers are plain functions and the interrupt
    // handler only performs an atomic store, matching the C flow
    // where SIGPIPE is ignored before threads spawn.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        let handler = interrupt as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
    }
}

/// Whether SIGINT arrived.
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}
