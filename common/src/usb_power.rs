//! Shared state for the USB-brick LED power path (board U14 TPS2553-1).
//!
//! Three parties touch this and none of them can hold a lock across an await
//! on the other core, so it is a set of atomics rather than a mutex:
//!
//! - the **router** publishes the `LED Power` setting whenever settings load
//!   or change,
//! - the **button task** (core 1, owns the TCA9555) drives `USB_LED_EN` on P04
//!   and publishes what it reads back from P05/P06/P07,
//! - the **LED task** reads the mode plus the enable state to decide whether
//!   the brightness budget applies, and the **UI** reads it for the title row.
//!
//! Board reference: EN is active-high with a 100 k pull-down (R38), so the
//! path is off until firmware drives P04 high. FAULT (P07) is open-drain and
//! pulled up (R35): **low means the switch has latched off** after an
//! overcurrent or a reverse-voltage event, and it only re-arms when EN is
//! toggled.

use core::sync::atomic::{AtomicU8, AtomicU16, Ordering};

/// What the user asked for on the System page.
pub const MODE_EXTERNAL: u8 = 0;
/// Strips are powered from a 5 V brick on the FTDI USB-C connector.
pub const MODE_USB_BRICK: u8 = 1;

/// Selected `LED Power` mode. Published by the router.
static MODE: AtomicU8 = AtomicU8::new(MODE_EXTERNAL);

// Status bits, published by the button task.
/// P04 is currently driven high (the switch is enabled).
pub const ST_ENABLED: u8 = 1 << 0;
/// P05: VBUS present on the FTDI USB-C connector.
pub const ST_FTDI_VBUS: u8 = 1 << 1;
/// P06: VBUS present on the module's USB port.
pub const ST_RPI_VBUS: u8 = 1 << 2;
/// P07 low: the switch has latched off. Sticky until a successful re-arm.
pub const ST_FAULT: u8 = 1 << 3;

static STATUS: AtomicU8 = AtomicU8::new(0);

/// The scale factor the LED task last applied, published so the UI can say so.
///
/// Without this the panel can tell you the brick is the supply but not that the
/// frame is being held back, and dim strips look like a fault rather than the
/// budget working. Unity until the first frame is rendered.
static SCALE: AtomicU16 = AtomicU16::new(256);

/// Current-limit headroom, in milliamps, for the strips when they run from the
/// brick. The switch itself limits at ≈ 1.45 A (R37 = 18 k) and latches off;
/// the budget sits under that so normal content never trips it.
pub const BRICK_BUDGET_MA: u32 = 1_300;

/// The TPS2553-1 limit with R37 = 18 k is 1.45 A typical, **1.34 A minimum**.
/// The budget has to sit under the minimum or a worst-case part latches off on
/// content the firmware considered legal — so this is checked at compile time.
const _: () = assert!(BRICK_BUDGET_MA < 1_340, "brick budget must stay below the switch's 1.34 A minimum limit");

/// Milliamps a single WS2812 colour channel draws at full scale. 20 mA per
/// channel is the datasheet figure; an RGB pixel at full white is 60 mA.
pub const MA_PER_CHANNEL_FULL: u32 = 20;

pub fn set_mode(mode: u8) {
    MODE.store(mode, Ordering::Relaxed);
}

pub fn mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}

/// True when the user has selected the brick as the LED supply.
pub fn brick_selected() -> bool {
    mode() == MODE_USB_BRICK
}

pub fn set_status(bits: u8) {
    STATUS.store(bits, Ordering::Relaxed);
}

pub fn status() -> u8 {
    STATUS.load(Ordering::Relaxed)
}

/// One-line summary of the whole USB-power path, for the console.
///
/// Exists because until 2026-09-21 none of this was visible anywhere but the
/// title row and the `LED Power` note. When the note failed to appear on the
/// bench there was no way to tell a dead present-detect from a display bug,
/// which is the kind of blind spot that costs an afternoon.
///
/// `mode` is what the menu asks for; `en`/`ftdi`/`rpi`/`flt` are P04–P07 as the
/// button task last read them; `scale` is what the LED task last applied.
pub fn status_text(out: &mut heapless::String<80>) {
    use core::fmt::Write as _;
    let s = status();
    let bit = |b: u8| if s & b != 0 { 1 } else { 0 };
    let _ = write!(
        out,
        "mode={} en={} ftdi_vbus={} rpi_vbus={} fault={} scale={}%",
        if brick_selected() { "usb_brick" } else { "external" },
        bit(ST_ENABLED),
        bit(ST_FTDI_VBUS),
        bit(ST_RPI_VBUS),
        bit(ST_FAULT),
        scale_percent()
    );
}

/// Called by the LED task once per frame with whatever [`budget_scale`] gave it.
pub fn set_scale(scale: u16) {
    SCALE.store(scale, Ordering::Relaxed);
}

/// The last applied scale, 0..=256 where 256 is unity.
pub fn scale() -> u16 {
    SCALE.load(Ordering::Relaxed)
}

/// The last applied scale as whole percent, for display. 100 means the budget
/// is not biting.
///
/// Rounds **down**, so a frame held at 99.6 % reads 99 and not 100: the panel
/// should not claim full output while anything is being taken off.
pub fn scale_percent() -> u8 {
    (scale().min(256) as u32 * 100 / 256) as u8
}

/// True while the strips are actually being fed from the brick — the setting
/// alone is not enough, the switch has to be on and not latched off.
pub fn brick_powering() -> bool {
    let s = status();
    brick_selected() && (s & ST_ENABLED) != 0 && (s & ST_FAULT) == 0
}

/// Scale factor (0..=256, fixed point) to apply to a frame so its estimated
/// draw fits `BRICK_BUDGET_MA`. Returns 256 (unity) when the frame already
/// fits or when the brick is not the supply.
///
/// `channel_sum` is the sum of every colour byte in the frame; at 8 bits per
/// channel a full-scale channel contributes 255.
pub fn budget_scale(channel_sum: u32) -> u16 {
    if !brick_powering() {
        return 256;
    }
    scale_for_sum(channel_sum)
}

/// Estimated draw, in milliamps, for a frame whose colour bytes sum to
/// `channel_sum`.
pub fn estimate_ma(channel_sum: u32) -> u32 {
    channel_sum.saturating_mul(MA_PER_CHANNEL_FULL) / 255
}

/// The budget maths on its own, with no dependence on the shared state — this
/// is the part worth testing, and `budget_scale` is the thin state check on
/// top of it.
pub fn scale_for_sum(channel_sum: u32) -> u16 {
    let estimate = estimate_ma(channel_sum);
    if estimate <= BRICK_BUDGET_MA {
        return 256;
    }
    // Scale down proportionally; never below 1/256 so the output does not go
    // fully dark and leave the operator with no feedback at all.
    let scale = BRICK_BUDGET_MA.saturating_mul(256) / estimate.max(1);
    scale.clamp(1, 256) as u16
}
