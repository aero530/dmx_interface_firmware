//! Bare-metal ST7789 panel test — no mipidsi, no mousefood, no Ratatui.
//!
//! Runs once at boot, before the display stack is built, and exists to answer a
//! single question the rest of the firmware cannot: **does this panel respond to
//! anything at all?**
//!
//! Rev 2's blank-panel investigation (2026-09-15) eliminated every variable
//! reachable through the driver stack — pin order confirmed against the module
//! silk, 3V3 and GND present at the module, SCK/MOSI reaching the rails, a
//! textbook init decoded off the panel's own connector, a correct address
//! window, exactly 55 040 correct pixels per fill, CS held high through reset,
//! and clock rate swept 40 → 1 MHz. Two panels from the same batch behaved
//! identically, and a third from that batch had worked on the Rev 1 board.
//!
//! What remained untested was the init *content*. mipidsi's ST7789 model sends
//! only `SLPOUT`, `MADCTL`, `INVON`, `COLMOD`, `NORON`, `DISPON` and leaves
//! porch, gate, **VCOM**, power and gamma at their power-on defaults. Most
//! modules are happy with that. A panel whose factory VCOM default is wrong
//! drives the glass with no usable contrast — backlight lit, image invisible —
//! which is indistinguishable from "not receiving data" without a scope on the
//! panel itself.
//!
//! So this writes the full register set every mainstream ST7789 driver sends,
//! with datasheet-generous delays, then fills the controller's **entire**
//! 240x320 frame memory — ignoring display size, offset and rotation, so no
//! geometry mistake can hide the result.
//!
//! Outcome:
//!
//! * **Colour appears** — the panel is alive and mipidsi's minimal init is
//!   insufficient for it. Fold the missing registers in and keep the driver.
//! * **Still blank** — the panel is not responding to a complete, correctly
//!   timed, fully specified init at a clock it has already proven it can take.
//!   Firmware is exhausted; the fault is the module or the board.
//!
//! Set [`ENABLED`] to `false` once the panel is working; it costs ~2 s of boot.

use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI1;
use embassy_rp::spi::{Blocking, Spi};
use embassy_time::Timer;

/// Run the test at boot. Off by default: the display works as of 2026-09-15
/// (the fault was mipidsi's `SWRESET`, see `tft_ui::NoReset`). Kept as a bench
/// tool — flip to `true` to put a known-good full-frame fill on the panel
/// without any of the driver stack in the way.
pub const ENABLED: bool = false;

/// The controller's full frame memory, not the glass. Filling all of it means
/// no offset or rotation error can put the result somewhere invisible.
const RAM_W: u16 = 240;
const RAM_H: u16 = 320;

const SWRESET: u8 = 0x01;
const SLPOUT: u8 = 0x11;
const INVON: u8 = 0x21;
const DISPON: u8 = 0x29;
const CASET: u8 = 0x2A;
const RASET: u8 = 0x2B;
const RAMWR: u8 = 0x2C;
const MADCTL: u8 = 0x36;
const COLMOD: u8 = 0x3A;

/// Write one command byte (DC low).
fn cmd(spi: &mut Spi<'static, SPI1, Blocking>, dc: &mut Output<'static>, c: u8) {
    dc.set_low();
    let _ = spi.blocking_write(&[c]);
}

/// Write parameter bytes (DC high).
fn data(spi: &mut Spi<'static, SPI1, Blocking>, dc: &mut Output<'static>, d: &[u8]) {
    dc.set_high();
    let _ = spi.blocking_write(d);
}

fn cmd_data(spi: &mut Spi<'static, SPI1, Blocking>, dc: &mut Output<'static>, c: u8, d: &[u8]) {
    cmd(spi, dc, c);
    if !d.is_empty() {
        data(spi, dc, d);
    }
}

/// Full init, then fill the whole frame memory with `colour` (RGB565).
pub async fn raw_init_and_fill(
    spi: &mut Spi<'static, SPI1, Blocking>,
    dc: &mut Output<'static>,
    colour: u16,
) {
    // --- reset and wake ---------------------------------------------------
    // The panel has already had a hardware RES pulse from the expander; this
    // software reset is belt and braces, with the datasheet's full 120 ms.
    cmd(spi, dc, SWRESET);
    Timer::after_millis(150).await;
    cmd(spi, dc, SLPOUT);
    // Datasheet: 120 ms minimum out of sleep. mipidsi allows 10 ms, which is
    // the one timing in the stack that was out of spec.
    Timer::after_millis(255).await;

    // --- pixel format and orientation -------------------------------------
    cmd_data(spi, dc, COLMOD, &[0x55]); // 16 bit/px, RGB565
    Timer::after_millis(10).await;
    // Native portrait, no rotation: the simplest possible mapping, so a wrong
    // MADCTL cannot be the reason nothing appears.
    cmd_data(spi, dc, MADCTL, &[0x00]);

    // --- the registers mipidsi leaves at their defaults --------------------
    cmd_data(spi, dc, 0xB2, &[0x0C, 0x0C, 0x00, 0x33, 0x33]); // PORCTRL
    cmd_data(spi, dc, 0xB7, &[0x35]); // GCTRL  gate voltages
    cmd_data(spi, dc, 0xBB, &[0x19]); // VCOMS  0.725 V  <-- the usual culprit
    cmd_data(spi, dc, 0xC0, &[0x2C]); // LCMCTRL
    cmd_data(spi, dc, 0xC2, &[0x01]); // VDVVRHEN
    cmd_data(spi, dc, 0xC3, &[0x12]); // VRHS   4.45 V
    cmd_data(spi, dc, 0xC4, &[0x20]); // VDVS   0 V
    cmd_data(spi, dc, 0xC6, &[0x0F]); // FRCTRL2  60 Hz
    cmd_data(spi, dc, 0xD0, &[0xA4, 0xA1]); // PWCTRL1
    cmd_data(
        spi, dc, 0xE0, // PVGAMCTRL
        &[0xD0, 0x04, 0x0D, 0x11, 0x13, 0x2B, 0x3F, 0x54, 0x4C, 0x18, 0x0D, 0x0B, 0x1F, 0x23],
    );
    cmd_data(
        spi, dc, 0xE1, // NVGAMCTRL
        &[0xD0, 0x04, 0x0C, 0x11, 0x13, 0x2C, 0x3F, 0x44, 0x51, 0x2F, 0x1F, 0x1F, 0x20, 0x23],
    );

    // IPS panels on this controller need inversion on; without it a correct
    // image renders as its own negative, not as nothing, so this is not the
    // fault being chased — but match the main driver anyway.
    cmd(spi, dc, INVON);
    Timer::after_millis(10).await;
    cmd(spi, dc, DISPON);
    Timer::after_millis(120).await;

    // --- fill every addressable pixel -------------------------------------
    cmd_data(spi, dc, CASET, &[0, 0, ((RAM_W - 1) >> 8) as u8, ((RAM_W - 1) & 0xFF) as u8]);
    cmd_data(spi, dc, RASET, &[0, 0, ((RAM_H - 1) >> 8) as u8, ((RAM_H - 1) & 0xFF) as u8]);
    cmd(spi, dc, RAMWR);
    dc.set_high();
    // One row at a time: a 480-byte burst keeps the blocking write short
    // enough not to upset the executor, and 320 of them is the whole screen.
    let row = [[(colour >> 8) as u8, (colour & 0xFF) as u8]; RAM_W as usize].concat();
    for _ in 0..RAM_H {
        let _ = spi.blocking_write(&row);
    }
}
