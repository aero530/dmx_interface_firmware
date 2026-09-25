//! Wired DMX-512 task — receive, and transmit in the `*>DMX` modes.
//!
//! The RP2040 bridge is gone: DMX runs in-process on PIO2, which deletes the
//! I²C bridge protocol, its 30 ms poll, and the whole class of bugs the Rev 1
//! review found in that link (§1.1, §1.2). The PIO driver in `dmx_pio.rs` is the
//! `rp2040_dmx` one, unchanged — it derives its clock divider from
//! `clk_sys_freq()`, so it adapts from the RP2040's 125 MHz to the RP2350's
//! 150 MHz on its own.
//!
//! # Direction
//!
//! GP10 drives Q1's gate on the Rev 2 carrier. The U7 opto LED is biased on
//! from 3V3 (receive) whenever GP10 is low or undriven; GP10 high turns Q1 on,
//! the LED goes off, and R7 pulls the THVD1400's DE/RE high = transmit. So the
//! firmware convention is unchanged from Rev 1 — **high = transmit** — and
//! every undriven state (boot, reset, crash) is receive. Gate current is
//! negligible; default pad drive is fine.
//!
//! # Transmit
//!
//! In `ArtNet>DMX` the configured Art-Net universe is re-transmitted on the
//! XLR; in `USB>DMX` it is whatever the Enttec host last sent (universe 0,
//! DMX-style). Packets go out back-to-back at the PIO program's natural ~43/s;
//! the mode is re-checked between packets so a change on the menu takes effect
//! within one frame.

use common::channels::{DmxFeedbackChannelRx, CHANNEL_DMX};
use common::event_router::{DmxEvent, DmxFeedbackEvent, PacketAddress};
use common::artnet::PortAddress;
use common::ui::InputMode;
use common::{DMX_BUFFER, DMX_UNIVERSE_SIZE};
// Logging goes wherever the build says: `defmt` over RTT by default, the
// `log` crate over USB with `--features usb`. Format strings therefore have
// to parse as both - no `{=type}` hints. See `usb_log.rs`.
use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{info, warn};
    } else {
        use defmt::{info, warn};
    }
}
use embassy_rp::gpio::Output;
use embassy_rp::peripherals::PIO2;
use embassy_time::{with_timeout, Duration};

use crate::dmx_pio::{PioDmxRx, PioDmxTx, DMX_FRAME_SIZE};

/// How long without a frame before saying so. DMX refreshes at ~44 Hz, so a
/// second of silence means the cable is out or the console stopped.
const SILENCE: Duration = Duration::from_millis(1000);

/// Receive wired DMX into `DMX_BUFFER` (and notify the router), or transmit
/// from it, according to the operating mode.
///
/// Received frames are written at offset 0 with the start code at index 0,
/// matching what the STM32 build did — so universe 0's channels live at
/// `[1..513]` and the router's existing indexing is unchanged.
#[embassy_executor::task]
pub async fn dmx_task(
    mut rx: PioDmxRx<'static, PIO2, 0>,
    mut tx: PioDmxTx<'static, PIO2, 1>,
    mut de: Output<'static>,
    mut feedback: DmxFeedbackChannelRx,
) {
    de.set_low(); // Q1 off, opto LED on: RS-485 driver disabled, receiver enabled
    info!("DMX: receiving on PIO2 SM0");

    let mut frame = [0u8; DMX_FRAME_SIZE];
    let mut quiet = false;
    let mut input_mode = InputMode::default();
    let mut artnet_base = 0usize;
    let mut transmitting = false;

    loop {
        if let Some(DmxFeedbackEvent::Mode(new_mode, addr, _, _)) = feedback.try_changed() {
            input_mode = new_mode;
            artnet_base = addr.buffer_base();
        }

        let want_tx = input_mode.is_dmx_output();
        if want_tx != transmitting {
            transmitting = want_tx;
            if transmitting {
                de.set_high(); // Q1 on, opto LED off: DE/RE pulled high = transmit
                info!("DMX: transmitting on PIO2 SM1 ({})", input_mode);
            } else {
                de.set_low();
                info!("DMX: receiving on PIO2 SM0");
                quiet = false;
            }
        }

        if transmitting {
            {
                let buf = DMX_BUFFER.lock().await;
                match input_mode {
                    // The Enttec host's frame is already DMX-style: start code
                    // at 0, channels at 1..=512.
                    InputMode::UsbToDmx => frame.copy_from_slice(&buf[..DMX_FRAME_SIZE]),
                    // Network universes are stored 0-based; prepend the NULL
                    // start code.
                    _ => {
                        let start = artnet_base * DMX_UNIVERSE_SIZE;
                        frame[0] = 0x00;
                        frame[1..].copy_from_slice(&buf[start..start + DMX_UNIVERSE_SIZE]);
                    }
                }
            }
            // Returns once the last stop bits are on the wire, so looping is
            // a continuous ~43 packet/s stream.
            tx.write(&frame).await;
            continue;
        }

        match with_timeout(SILENCE, rx.read(&mut frame)).await {
            Ok(n) => {
                if quiet {
                    info!("DMX: signal restored");
                    quiet = false;
                }
                // Same gate every other input has: only the active source may
                // write the buffer. A console left plugged into the DMX port
                // while in an Art-Net mode must not stomp universe 0.
                if input_mode != InputMode::Dmx {
                    continue;
                }
                // Only NULL start code frames are dimmer data; RDM and other
                // alternate start codes must not be rendered as channels.
                if n == 0 || frame[0] != 0x00 {
                    continue;
                }
                {
                    let mut buf = DMX_BUFFER.lock().await;
                    buf[0..n].copy_from_slice(&frame[..n]);
                    // A short packet defines the rest of the universe as
                    // unchanged per DMX; here the simpler and safer contract
                    // (matching the USB input path) is: what the console
                    // doesn't send goes dark.
                    buf[n..DMX_FRAME_SIZE].fill(0);
                }
                // Wired DMX is always Port-Address 0:0:0. A full channel just
                // means the router has not drained the last frame yet, and the
                // newest frame is the one that matters, so dropping is correct.
                let _ = CHANNEL_DMX
                    .sender()
                    .try_send(DmxEvent::DmxPacket(PacketAddress::new(
                        PortAddress::new(0, 0, 0),
                        0,
                    )));
            }
            Err(_) => {
                if !quiet {
                    warn!("DMX: no data");
                    quiet = true;
                }
            }
        }
    }
}
