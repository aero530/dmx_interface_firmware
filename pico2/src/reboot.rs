//! One way out: every software reboot goes through here.
//!
//! Two things ask the board to restart itself - the USB identity watcher when
//! the stored mode needs the other USB identity, and a firmware update, which
//! reboots into the RP2350 bootrom's BOOTSEL mode so the module shows up as a
//! USB drive and a UF2 can be copied onto it with no button pressed. The update
//! can be asked for from the panel (System page, `Firmware`) or by picotool
//! through the reset interface in `usb_device.rs`.
//!
//! Both go through [`request`] and the single [`reboot_task`], which applies
//! the two rules every reboot here has to follow:
//!
//! * **Wait for `boot: complete`.** `boot_task` writes `BootStatus::Failed` on
//!   the way in and `Success` at the end, and two consecutive incomplete boots
//!   trip the guard that disables Ethernet (`BOOT_FAIL_LIMIT` in `main.rs`).
//!   A reboot before the flag is written would count as one of those, so the
//!   task waits for it - bounded, because a board whose EEPROM is dead never
//!   gets there and an update must still be possible on it.
//! * **Settle first.** Give the log a moment to drain and the operator a
//!   moment to read the panel; a screen that goes black mid-keypress reads as
//!   a crash.
//!
//! The BOOTSEL reboot is the bootrom's own `reboot()` call, the same one the
//! button path ends in, so flashing is exactly as safe as holding BOOTSEL:
//! the running firmware never writes flash itself.

use core::sync::atomic::{AtomicBool, Ordering};

use common::events::{RebootRequest, UiEvent};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};

// Logging goes wherever the build says - see `usb_log.rs`.
use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{info, warn};
    } else {
        use defmt::{info, warn};
    }
}

/// Set by `boot_task` when **this** boot finishes.
///
/// `GlobalData::boot_status` cannot do this job: it is read from the EEPROM
/// early in `boot_task` and describes the *previous* boot, so on any healthy
/// board it already says `Success` while this boot is still starting. Guarding
/// on it looked right and guarded nothing.
static BOOT_COMPLETE: AtomicBool = AtomicBool::new(false);

/// Called by `boot_task` at each of its exits.
pub fn mark_boot_complete() {
    BOOT_COMPLETE.store(true, Ordering::Relaxed);
}

/// Whether this boot has reached `boot: complete`.
pub fn boot_complete() -> bool {
    BOOT_COMPLETE.load(Ordering::Relaxed)
}

/// How long to let the dust settle before resetting.
///
/// Not waiting for anything in particular: a mode change only reaches the
/// identity watcher after the EEPROM has confirmed the write, and an update
/// request has nothing to save. It gives the log a moment to drain and the
/// operator a moment to see the panel.
const REBOOT_SETTLE: Duration = Duration::from_millis(750);

/// How long to wait for `boot: complete` before rebooting anyway.
///
/// Boot normally completes within a second or two of power-on; the bound only
/// matters on a board whose EEPROM is not answering, where the flag is never
/// written and the reboot would otherwise never happen.
const BOOT_WAIT_LIMIT: Duration = Duration::from_secs(5);

/// The pending request. A `Signal` so a request made from either core, from a
/// USB control handler or from the UI task, lands without blocking; a second
/// request before the first is acted on simply replaces it, and by then the
/// board is restarting either way.
static REQUEST: Signal<CriticalSectionRawMutex, RebootRequest> = Signal::new();

/// Ask for a reboot. Returns at once; [`reboot_task`] does the rest.
pub fn request(req: RebootRequest) {
    REQUEST.signal(req);
}

/// Waits for a request, then reboots the board the way it asked.
#[embassy_executor::task]
pub async fn reboot_task() -> ! {
    let req = REQUEST.wait().await;
    info!("reboot: {:?} requested", req);
    // Tell the panel, so a picotool-triggered reboot shows the same screen the
    // menu path draws for itself. Best effort: the UI channel may be full.
    let _ = common::channels::CHANNEL_UI.try_send(UiEvent::Rebooting(req));

    let deadline = Instant::now() + BOOT_WAIT_LIMIT;
    while !boot_complete() && Instant::now() < deadline {
        Timer::after_millis(50).await;
    }
    if !boot_complete() {
        warn!(
            "reboot: boot not complete after {} ms - this will count as one incomplete boot",
            BOOT_WAIT_LIMIT.as_millis()
        );
    }
    Timer::after(REBOOT_SETTLE).await;

    match req {
        // No activity LED; the disable mask is whatever picotool asked for
        // (0 from the panel: mass storage and PICOBOOT both on).
        RebootRequest::Bootsel { disable_mask } => {
            embassy_rp::rom_data::reset_to_usb_boot(0, u32::from(disable_mask));
        }
        RebootRequest::Flash => cortex_m::peripheral::SCB::sys_reset(),
    }
    // The bootrom call does not return; this is only here to satisfy the
    // signature if it somehow did.
    loop {
        Timer::after_secs(1).await;
    }
}
