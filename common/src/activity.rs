//! "Is DMX arriving?" - the one bit behind the panel's activity lamp.
//!
//! Every input path (wired DMX, Art-Net, sACN, the USB widget) posts a
//! `DmxEvent` to the router after filling `DMX_BUFFER`, and the render loop
//! marks here as it takes each one. The UI polls once a second and clears the
//! mark as it reads it, so the lamp says "at least one frame reached the
//! router in the last second", whichever input it came from.
//!
//! A flag and not a timestamp because `common` has no clock: the *reader*
//! supplies the period by how often it polls, and a bool needs no arithmetic
//! that could go wrong when the tick counter wraps.

use core::sync::atomic::{AtomicBool, Ordering};

static SEEN: AtomicBool = AtomicBool::new(false);

/// A frame arrived. Called from the render loop, once per DMX event.
pub fn mark() {
    SEEN.store(true, Ordering::Relaxed);
}

/// Whether anything arrived since the last call, clearing the mark.
pub fn take() -> bool {
    SEEN.swap(false, Ordering::Relaxed)
}
