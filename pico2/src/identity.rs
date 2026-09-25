//! Per-unit identity from the RP2350's factory chip ID.
//!
//! Two things must differ between units that are otherwise byte-identical:
//!
//! - the **USB serial-number string** — a host that sees two devices with the
//!   same serial treats them as one, and Windows reuses the COM-port binding,
//!   so two boxes on one PC would fight over a port;
//! - the **MAC address used until the EEPROM has one programmed** — two
//!   unprogrammed boards on one bench would otherwise collide.
//!
//! Both derive from the 64-bit chip ID burned into OTP at the factory. It is
//! unique per die and needs no provisioning step, which is the point: a unit
//! straight off the assembly line is already distinguishable.

// Logging goes wherever the build says: `defmt` over RTT by default, the
// `log` crate over USB with `--features usb`. Format strings therefore have
// to parse as both - no `{=type}` hints. See `usb_log.rs`.
use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{warn};
    } else {
        use defmt::{warn};
    }
}
use embassy_rp::otp;
use static_cell::StaticCell;

/// 64-bit factory chip ID. Zero only if the OTP read fails — logged, so the
/// derived identities degrade to a fixed value rather than to garbage.
pub fn chip_id() -> u64 {
    match otp::get_chipid() {
        Ok(id) => id,
        Err(_) => {
            warn!("OTP chip ID read failed - unit identity falls back to zero");
            0
        }
    }
}

static SERIAL: StaticCell<[u8; 16]> = StaticCell::new();

/// USB serial-number string: the chip ID as 16 upper-case hex digits.
///
/// Call exactly once — the buffer lives for the life of the firmware and a
/// second call would panic in `StaticCell::init`.
pub fn usb_serial() -> &'static str {
    let id = chip_id();
    let buf: &'static mut [u8; 16] = SERIAL.init([b'0'; 16]);
    for (i, b) in buf.iter_mut().enumerate() {
        let nibble = ((id >> (60 - 4 * i)) & 0xF) as u8;
        *b = if nibble < 10 { b'0' + nibble } else { b'A' + nibble - 10 };
    }
    // Only ASCII hex digits were written, so this cannot fail.
    core::str::from_utf8(&buf[..]).unwrap_or("0000000000000000")
}

/// MAC used until the EEPROM has one programmed.
///
/// Locally-administered prefix (bit 1 of the first octet set) so it is valid
/// on a private network, plus the low 24 bits of the chip ID so unprogrammed
/// boards do not collide. Boot still prefers the EEPROM copy at 0x02–0x07;
/// program a real address with the console's `mac` command.
pub fn mac_fallback() -> [u8; 6] {
    let id = chip_id();
    [0x02, 0x44, 0x4D, (id >> 16) as u8, (id >> 8) as u8, id as u8]
}

/// A MAC as `aa:bb:cc:dd:ee:ff`, for log lines that must read the same under
/// `defmt` and `log`.
///
/// `defmt` renders `{:02x}` over a `[u8; 6]` element by element; `core::fmt`
/// has no such impl and will not compile. Rendering to a string first is what
/// lets one format string serve both backends - see `usb_log.rs`.
pub fn mac_text(mac: &[u8; 6]) -> heapless::String<17> {
    use core::fmt::Write as _;
    let mut out = heapless::String::new();
    for (i, b) in mac.iter().enumerate() {
        // The capacity is exact, so this cannot truncate; ignore the result
        // rather than panicking inside a log call.
        let _ = write!(out, "{}{:02x}", if i == 0 { "" } else { ":" }, b);
    }
    out
}
