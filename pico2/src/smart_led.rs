//! WS2812 / SK6812 output — eight strings, one PIO state machine each.
//!
//! PIO does the 800 kHz one-wire timing directly and DMA feeds it, so the CPU
//! cost per frame is packing the pixels into words (`ws2812.rs`).
//!
//! # State machine allocation
//!
//! Fixed here so the other PIO users do not have to negotiate for slots:
//!
//! | Block | SM0 | SM1 | SM2 | SM3 |
//! |---|---|---|---|---|
//! | PIO0 | *W6300 SPI transport* | string 1 | string 2 | string 3 |
//! | PIO1 | string 4 | string 5 | string 6 | string 7 |
//! | PIO2 | *DMX in* | *DMX out* | string 8 | spare |
//!
//! All three blocks hold the same 4-instruction WS2812 program; the state
//! machines within a block share it, which is why eight strings fit at all.
//!
//! # Colour mode
//!
//! The colour buffer is always RGBW. Each frame is packed for the configured
//! mode — 24-bit GRB for WS2812-class strips, 32-bit GRBW for SK6812-class —
//! so switching modes on the menu needs no re-initialisation. Only the
//! configured LED count is transmitted, so a 100-LED string refreshes in a
//! sixth of the time a 600-LED one does.
//!
//! # Why the strings are joined rather than awaited in turn
//!
//! Each `write` is DMA-backed and yields until its transfer completes. Awaiting
//! them sequentially would make the frame time the *sum* of eight strings
//! instead of the longest one — 144 ms rather than 18 ms at 600 LEDs. They are
//! joined so all eight clock out concurrently.

use common::channels::SmartLedChannelRx;
use common::events::SmartLedEvent;
use common::ui::{SmartLedColorMode, BLACK};
use common::usb_power;
use common::{LED_COLORS, SMARTLED_PORT_COUNT};
// Logging goes wherever the build says: `defmt` over RTT by default, the
// `log` crate over USB with `--features usb`. Format strings therefore have
// to parse as both - no `{=type}` hints. See `usb_log.rs`.
use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{info};
    } else {
        use defmt::{info};
    }
}
use embassy_futures::join::{join, join4};
use embassy_rp::peripherals::{PIO0, PIO1, PIO2};
use smart_leds::{White, RGBW};

use crate::ws2812::{pack_grb, pack_grbw, Ws2812};

/// Number of physical WS2812 outputs on the carrier.
pub const STRINGS: usize = 8;

/// LEDs per string the word buffers are sized for — the byte budget
/// (1800 B/port = 600 RGB) shared with the menu's LED-count limit through
/// `common::MAX_LEDS_PER_PORT`, so a configured count can never exceed what is
/// transmitted. The buffers hold 600 *words*, so 600 RGBW pixels also fit
/// (2400 B — over the budget and flagged as such on the LED page, but output).
pub const MAX_LEDS: usize = common::MAX_LEDS_PER_PORT;

/// All eight outputs. The types differ per string (each names its own PIO block
/// and state machine index), so they cannot live in an array — hence the
/// explicit fields.
pub struct Ws2812Outputs {
    pub s1: Ws2812<'static, PIO0, 1>,
    pub s2: Ws2812<'static, PIO0, 2>,
    pub s3: Ws2812<'static, PIO0, 3>,
    pub s4: Ws2812<'static, PIO1, 0>,
    pub s5: Ws2812<'static, PIO1, 1>,
    pub s6: Ws2812<'static, PIO1, 2>,
    pub s7: Ws2812<'static, PIO1, 3>,
    pub s8: Ws2812<'static, PIO2, 2>,
}

impl Ws2812Outputs {
    /// Clock one packed frame out of every string concurrently.
    ///
    /// Returns once the slowest string has finished, so the caller can treat
    /// this as "the frame is on the wire".
    pub async fn render(&mut self, frames: [&[u32]; STRINGS]) {
        join(
            join4(
                self.s1.write(frames[0]),
                self.s2.write(frames[1]),
                self.s3.write(frames[2]),
                self.s4.write(frames[3]),
            ),
            join4(
                self.s5.write(frames[4]),
                self.s6.write(frames[5]),
                self.s7.write(frames[6]),
                self.s8.write(frames[7]),
            ),
        )
        .await;
    }
}

/// Render `LED_COLORS` to the strings whenever the router says it changed.
///
/// The frame is **packed into a local word buffer before rendering**, not
/// written straight from `LED_COLORS`. Holding that lock across the render
/// would block the router for the whole 18 ms a frame takes to clock out, and
/// the router is on the other core feeding Art-Net. Packing costs a few hundred
/// microseconds at 150 MHz.
///
/// Pixels past a port's configured count are not sent at all (the strip holds
/// its last latched value for those, which is why boot blanks every string).
#[embassy_executor::task]
pub async fn smart_led_task(mut out: Ws2812Outputs, rx: SmartLedChannelRx) {
    // Boot: every string dark, whatever type is attached. 600 zero words is
    // 800 RGB pixels or 600 RGBW pixels of black.
    let mut words = [[0u32; MAX_LEDS]; STRINGS];
    {
        let (a, b) = words.split_at(4);
        out.render([&a[0], &a[1], &a[2], &a[3], &b[0], &b[1], &b[2], &b[3]]).await;
    }
    info!("WS2812: {} strings ready, {} LEDs max each", STRINGS, MAX_LEDS);

    let mut pixels = [BLACK; MAX_LEDS];
    let mut lens = [0usize; STRINGS];
    // Staged copy of the frame, so the budget scaling never writes back into
    // the shared buffer (the router owns that, and a scaled value must not
    // become the new source of truth for the next frame).
    let mut staged = [[BLACK; MAX_LEDS]; STRINGS];
    let mut counts_used = [0usize; STRINGS];

    loop {
        let SmartLedEvent::UpdateLEDs { counts, color_mode } = rx.receive().await;
        let mut channel_sum: u32 = 0;

        {
            let colors = LED_COLORS.lock().await;
            for i in 0..STRINGS {
                // `counts` is sized by SMARTLED_PORT_COUNT (8, matching the
                // eight physical strings).
                let n = if i < SMARTLED_PORT_COUNT {
                    (counts[i] as usize).min(MAX_LEDS).min(colors[i].len())
                } else {
                    0
                };
                pixels[..n].copy_from_slice(&colors[i][..n]);
                counts_used[i] = n;
                channel_sum += frame_channel_sum(color_mode, &pixels[..n]);
                staged[i][..n].copy_from_slice(&pixels[..n]);
            }
        }

        // Brick mode only: hold the frame inside the switch's current budget
        // so normal content cannot latch it off. Unity (256) otherwise, and the
        // whole block costs one multiply per channel only when it bites.
        let scale = usb_power::budget_scale(channel_sum);
        // Publish it for the title row: dim strips with no explanation read as
        // a fault, and this is the difference between "broken" and "working".
        usb_power::set_scale(scale);
        for i in 0..STRINGS {
            let n = counts_used[i];
            if scale != 256 {
                for px in staged[i][..n].iter_mut() {
                    *px = scale_pixel(*px, scale);
                }
            }
            lens[i] = pack(color_mode, &staged[i][..n], &mut words[i]);
        }

        let (a, b) = words.split_at(4);
        out.render([
            &a[0][..lens[0]], &a[1][..lens[1]], &a[2][..lens[2]], &a[3][..lens[3]],
            &b[0][..lens[4]], &b[1][..lens[5]], &b[2][..lens[6]], &b[3][..lens[7]],
        ])
        .await;
    }
}

/// Pack one port's pixels for the wire; returns the word count.
fn pack(mode: SmartLedColorMode, pixels: &[RGBW<u8>], out: &mut [u32]) -> usize {
    match mode {
        SmartLedColorMode::Rgb => pack_grb(pixels, out),
        SmartLedColorMode::Rgbw => pack_grbw(pixels, out),
    }
}

/// Sum of every colour byte the wire will carry for one port — the input to
/// the current estimate in `common::usb_power`. White only counts in RGBW,
/// because in RGB mode it is never transmitted.
fn frame_channel_sum(mode: SmartLedColorMode, pixels: &[RGBW<u8>]) -> u32 {
    let mut sum: u32 = 0;
    for p in pixels {
        sum += p.r as u32 + p.g as u32 + p.b as u32;
        if mode == SmartLedColorMode::Rgbw {
            sum += p.a.0 as u32;
        }
    }
    sum
}

/// Scale one pixel by a 0..=256 fixed-point factor.
fn scale_pixel(p: RGBW<u8>, scale: u16) -> RGBW<u8> {
    let s = scale as u32;
    let ch = |v: u8| ((v as u32 * s) / 256) as u8;
    RGBW::<u8> {
        r: ch(p.r),
        g: ch(p.g),
        b: ch(p.b),
        a: White(ch(p.a.0)),
    }
}
