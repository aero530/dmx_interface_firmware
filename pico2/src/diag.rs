//! Bring-up diagnostics readable without a debug probe.
//!
//! The firmware logs each init step over defmt, but a board flashed from a UF2
//! has no probe attached and no way to see that log. So the display and
//! expander paths also raise a bit here as they pass or fail each step, and the
//! USB console prints the set as `diag=...` in `info`. Flags are sticky for the
//! life of the boot: the point is to answer "how far did it get" from a serial
//! terminal.

use alloc::string::String;
use core::fmt::Write;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};

static FLAGS: AtomicU32 = AtomicU32::new(0);

pub const EXPANDER_OK: u32 = 1 << 0;
pub const EXPANDER_FAIL: u32 = 1 << 1;
pub const EXPANDER_RES_FAIL: u32 = 1 << 2;
pub const TFT_RES_RELEASED: u32 = 1 << 3;
pub const TFT_CS_ASSERTED: u32 = 1 << 13;
pub const TFT_INIT_OK: u32 = 1 << 4;
pub const TFT_INIT_FAIL: u32 = 1 << 5;
pub const TFT_SPLASH_DONE: u32 = 1 << 6;
pub const BACKLIGHT_OK: u32 = 1 << 7;
pub const BACKLIGHT_FAIL: u32 = 1 << 8;
pub const TERMINAL_OK: u32 = 1 << 9;
pub const TERMINAL_FAIL: u32 = 1 << 10;
pub const FIRST_FRAME: u32 = 1 << 11;
pub const DRAW_ERROR: u32 = 1 << 12;
pub const PANEL_TEST_DONE: u32 = 1 << 14;

/// The W6300 answered and reported the version the driver expects.
pub const ETH_OK: u32 = 1 << 15;
/// Nothing came back over SPI at all — the transaction itself errored.
pub const ETH_NO_REPLY: u32 = 1 << 16;
/// Something came back, but not a healthy W6300. `eth_ver=` carries the byte,
/// which is the whole diagnosis: see [`set_eth_version`].
pub const ETH_BAD_VERSION: u32 = 1 << 17;

/// The PHY negotiated a link at least once. Without this, `ip=0.0.0.0` means
/// no cable, no switch, or a dead magjack — not a DHCP problem.
pub const ETH_LINK_UP: u32 = 1 << 18;
/// The link came up and then dropped. Sticky, so an intermittent cable leaves
/// evidence even though the link is back by the time anyone runs `info`.
pub const ETH_LINK_DROPPED: u32 = 1 << 19;
/// The stack reached a configured address (DHCP lease or the static config).
pub const ETH_CONFIGURED: u32 = 1 << 20;

/// Version byte actually read from the W6300's `CIDR2`, or [`NO_VERSION`].
///
/// This one byte separates the cases that otherwise all read as "no chip":
///
/// * `0x11` — correct; the chip is fine and the fault is above the transport.
/// * `0x00` — reads come back all zeros. MISO is never being sampled: wrong
///   pin, the PIO not shifting in, or the chip holding its output low.
/// * `0xFF` — MISO is floating or stuck high; nothing is driving it.
/// * anything else — the bus works but the framing is off. A value that is a
///   bit-shift of `0x11` (`0x22`, `0x08`, `0x88`) means the sample edge or the
///   dummy phase is wrong, which on this part is clock-rate dependent — halve
///   `w6300::SPI_FREQ_HZ` and read it again.
static ETH_VERSION: AtomicU16 = AtomicU16::new(NO_VERSION);

/// Sentinel for "the version register was never read".
pub const NO_VERSION: u16 = 0xFFFF;

/// SPI clock the W6300 link actually runs at, in Hz; 0 until one is chosen.
/// Shown as `eth_hz=`.
static ETH_HZ: AtomicU32 = AtomicU32::new(0);

/// Highest clock the probe found working, in Hz — the measured ceiling, which
/// is deliberately above the operating rate (`w6300::OPERATING_MAX_HZ`). Shown
/// as `eth_max=`. Worth watching over a board's life: a ceiling that falls is
/// the transport degrading, and it degrades silently.
static ETH_MAX_HZ: AtomicU32 = AtomicU32::new(0);

const NAMES: [(u32, &str); 21] = [
    (EXPANDER_OK, "expander_ok"),
    (EXPANDER_FAIL, "EXPANDER_FAIL"),
    (EXPANDER_RES_FAIL, "EXPANDER_RES_FAIL"),
    (TFT_RES_RELEASED, "tft_res_released"),
    (TFT_CS_ASSERTED, "tft_cs_asserted"),
    (PANEL_TEST_DONE, "panel_test_done"),
    (TFT_INIT_OK, "tft_init_ok"),
    (TFT_INIT_FAIL, "TFT_INIT_FAIL"),
    (TFT_SPLASH_DONE, "tft_splash_done"),
    (BACKLIGHT_OK, "backlight_ok"),
    (BACKLIGHT_FAIL, "BACKLIGHT_FAIL"),
    (TERMINAL_OK, "terminal_ok"),
    (TERMINAL_FAIL, "TERMINAL_FAIL"),
    (FIRST_FRAME, "first_frame_drawn"),
    (DRAW_ERROR, "DRAW_ERROR"),
    (ETH_OK, "eth_ok"),
    (ETH_NO_REPLY, "ETH_NO_REPLY"),
    (ETH_BAD_VERSION, "ETH_BAD_VERSION"),
    (ETH_LINK_UP, "eth_link_up"),
    (ETH_LINK_DROPPED, "ETH_LINK_DROPPED"),
    (ETH_CONFIGURED, "eth_configured"),
];

pub fn set(bit: u32) {
    FLAGS.fetch_or(bit, Ordering::Relaxed);
}

/// Record the W6300 version byte, whatever it turned out to be.
pub fn set_eth_version(version: u8) {
    ETH_VERSION.store(version as u16, Ordering::Relaxed);
}

/// Record the SPI clock the link runs at.
pub fn set_eth_hz(hz: u32) {
    ETH_HZ.store(hz, Ordering::Relaxed);
}

/// Record the highest SPI clock that read the version register cleanly.
pub fn set_eth_max_hz(hz: u32) {
    ETH_MAX_HZ.store(hz, Ordering::Relaxed);
}

/// Space-separated names of every flag raised so far; failures in capitals.
pub fn text() -> String {
    let f = FLAGS.load(Ordering::Relaxed);
    let mut s = String::new();
    for (bit, name) in NAMES {
        if f & bit != 0 {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(name);
        }
    }
    if s.is_empty() {
        s.push_str("none");
    }
    // Appended rather than made a flag: the *value* is the diagnosis, not the
    // fact that a read happened.
    let version = ETH_VERSION.load(Ordering::Relaxed);
    if version != NO_VERSION {
        let _ = write!(s, " eth_ver={version:#04x}");
    }
    let hz = ETH_HZ.load(Ordering::Relaxed);
    if hz != 0 {
        let _ = write!(s, " eth_hz={}k", hz / 1000);
    }
    let max = ETH_MAX_HZ.load(Ordering::Relaxed);
    if max != 0 {
        let _ = write!(s, " eth_max={}k", max / 1000);
    }
    s
}
