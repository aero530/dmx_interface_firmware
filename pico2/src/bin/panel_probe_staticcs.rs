//! Panel probe variant B — the Rev 1 path, but with CS held statically low.
//!
//! Flash to a **W6300-EVB-Pico2 out of its socket**, on a breadboard, with the
//! panel wired straight to the module. Nothing from the carrier is involved: no
//! TCA9555, no PCA9633, no J4, no board traces.
//!
//! # What this isolates
//!
//! The working probe changed *two* things at once against the failing Rev 2
//! configuration: CS framing, and the reset path. This build keeps the reset
//! path from the working probe (`.reset_pin()`, so mipidsi pulses RES and sends
//! no `SWRESET`) and reverts **only** CS to static-low, exactly as the Rev 2
//! firmware's `NoCs` does.
//!
//! * **Colours appear** — CS framing is irrelevant; the reset path was the
//!   fault. Rev 2 is fixable in firmware, keeping CS on the expander.
//! * **Blank** — the panel genuinely requires CS to frame each transaction, and
//!   Rev 2's CS-on-the-I²C-expander cannot do that. Board-level problem.
//!
//! # Why the base of this is a copy rather than something hand-rolled
//!
//! The Rev 1 (`nucleo`) firmware drove this panel model successfully, so the
//! right move is to reproduce *its* configuration exactly rather than invent an
//! init sequence. Two things it does that neither the Rev 2 firmware nor the
//! first version of this probe did:
//!
//! 1. **CS is owned by `ExclusiveDevice`, so it toggles around every
//!    transaction** — low before, high after. Rev 2 puts CS on the I²C expander
//!    and holds it low for the life of the session; the first probe asserted it
//!    once too. The ST7789 re-initialises its serial interface state on CS
//!    high, so per-transaction framing is materially different from static-low,
//!    and static-low is the configuration that does not work here.
//! 2. **`.reset_pin(reset)` hands RES to mipidsi**, which pulses it itself and
//!    then issues *no* `SWRESET`. Rev 2 pulses RES from the expander long
//!    before init and then also sends `SWRESET`.
//!
//! Everything else — `display_size`, `display_offset`, `Rotation::Deg90`,
//! `ColorInversion::Inverted`, the 512-byte interface buffer — is copied from
//! `nucleo/src/ui/mod.rs` unchanged.
//!
//! # Wiring (module pin → panel silk)
//!
//! | Module | Panel |
//! |---|---|
//! | GND | GND |
//! | 3V3(OUT), pin 36 | VDD |
//! | GP14 | SCL |
//! | GP11 | SDA |
//! | GP13 | RES |
//! | GP12 | DC |
//! | GP10 | CS |
//! | 3V3(OUT) | BL (tie high — full brightness, no PWM here) |
//!
//! The module LED (GP25) blinks at 1 Hz throughout, so a dark panel with a
//! blinking LED means "running, panel not responding" rather than "did not
//! boot". Build: `cargo uf2 --bin panel_probe` from the repo root.

#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::SPI1;
use embassy_rp::spi::{Blocking, Config as SpiConfig, Spi};
use embassy_time::{Delay, Timer};
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::Builder;
use panic_probe as _;
use static_cell::StaticCell;

/// A chip select that does nothing, so `ExclusiveDevice` cannot toggle the
/// real line. Copied from `pico2::tft_ui::NoCs`.
pub struct NoCs;

impl embedded_hal::digital::ErrorType for NoCs {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::OutputPin for NoCs {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Rev 1 asked for 100 MHz (resolving to ~50 MHz actual) and worked, so the
/// panel is not the limit. Kept low here because Rev 2's SCK net is measurably
/// capacitive and this test is about the *protocol*, not signal integrity.
const SPI_HZ: u32 = 4_000_000;

/// Copied from `common::constants` / `nucleo`: 1.47" 172x320 glass centred in
/// the controller's 240-wide frame memory.
const DISPLAY_WIDTH: u16 = 172;
const DISPLAY_HEIGHT: u16 = 320;
const DISPLAY_OFFSET: u16 = 34;

/// mipidsi batches pixel data through this. 512 B, as in `nucleo`.
static DI_BUFFER: StaticCell<[u8; 512]> = StaticCell::new();

#[embassy_executor::task]
async fn heartbeat(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after_millis(50).await;
        led.set_low();
        Timer::after_millis(950).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("panel probe B: Rev 1 reset path, CS held STATIC LOW");

    spawner.spawn(unwrap!(heartbeat(Output::new(p.PIN_25, Level::Low))));

    let mut cfg = SpiConfig::default();
    cfg.frequency = SPI_HZ;
    let spi: Spi<'static, SPI1, Blocking> =
        Spi::new_blocking_txonly(p.SPI1, p.PIN_14, p.PIN_11, cfg);

    // Initial levels copied from `nucleo/src/main.rs`, except CS: held low by
    // a plain output for the whole session and hidden from ExclusiveDevice
    // behind a no-op pin — precisely what the Rev 2 firmware does.
    let mut cs_static = Output::new(p.PIN_10, Level::High);
    let dc = Output::new(p.PIN_12, Level::High);
    let reset = Output::new(p.PIN_13, Level::High);

    // CS asserted once, then left low for good. RES still belongs to mipidsi.
    Timer::after_millis(10).await;
    cs_static.set_low();
    Timer::after_millis(10).await;
    let spi_device = match ExclusiveDevice::new_no_delay(spi, NoCs) {
        Ok(d) => d,
        Err(_) => {
            error!("panel probe B: could not build the SPI device");
            return;
        }
    };
    let di = SpiInterface::new(spi_device, dc, DI_BUFFER.init([0u8; 512]));

    let mut display = match Builder::new(ST7789, di)
        .reset_pin(reset)
        .display_size(DISPLAY_WIDTH, DISPLAY_HEIGHT)
        .display_offset(DISPLAY_OFFSET, 0)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .invert_colors(ColorInversion::Inverted)
        .init(&mut Delay)
    {
        Ok(d) => {
            info!("panel probe B: mipidsi init returned Ok");
            d
        }
        Err(_) => {
            error!("panel probe B: mipidsi init failed");
            return;
        }
    };

    // Cycle solid colours forever. The SPI bus is write-only, so a clean init
    // proves nothing on its own — only the glass can answer.
    let mut n = 0usize;
    loop {
        let (name, colour) = match n % 4 {
            0 => ("red", Rgb565::RED),
            1 => ("green", Rgb565::GREEN),
            2 => ("blue", Rgb565::BLUE),
            _ => ("white", Rgb565::WHITE),
        };
        let _ = display.clear(colour);
        info!("panel probe B: filled {}", name);
        Timer::after_millis(1200).await;
        n += 1;
    }
}
