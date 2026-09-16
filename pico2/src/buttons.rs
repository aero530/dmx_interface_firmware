//! Front-panel buttons, read through the TCA9555 — polled.
//!
//! Polled rather than interrupt-driven: the expander's `INT` line used to sit
//! on GP28, which the Rev 2 carrier now spends on the FT232RNL USB-DMX port
//! (GP28 is the only UART0 TX that was not already committed). At 50 Hz a poll
//! is one 3-byte I²C transaction (~80 µs at 400 kHz) every 20 ms, well under
//! 1 % of the bus, and it buys two things the interrupt never gave for free:
//! debounce by construction (a 20 ms sample spacing is longer than any tactile
//! switch bounces), and no interrupt latch that can fail to re-arm.
//!
//! The poll doubles as core 1's watchdog check-in (`supervisor.rs`).
//!
//! The press/release state machine and the emit-on-release behaviour are
//! deliberately identical to the STM32 build's `button_row_task`, so the router
//! and UI see exactly the events they saw before.

use core::sync::atomic::Ordering;

use common::channels::RouterChannelTx;
use common::event_router::RouterEvent;
use common::events::{KeyPadButton, KeyPadEvent};
use common::usb_power;
use defmt::*;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embedded_hal_async::i2c::I2c;

use crate::diag;
use crate::supervisor::CORE1_ALIVE;
use crate::tca9555::{self, Tca9555, BTN_DOWN, BTN_ESC, BTN_SELECT, BTN_UP};

/// Sample interval. A tap shorter than this can be missed, and nobody taps a
/// panel button in under 20 ms; anything longer starts to feel laggy.
const POLL: Duration = Duration::from_millis(20);

/// The carrier's 3V3 (and with it the expander) is enable-gated by the module's
/// own 3V3_OUT, so it comes up a few hundred microseconds after this core
/// starts running. Waiting this long before the first I²C transaction makes
/// the first-boot log deterministic instead of occasionally showing a NAK.
const CARRIER_SETTLE: Duration = Duration::from_millis(20);

/// Port 0 bit → logical button.
///
/// The names are the ones the event router already switches on:
/// `D` → Down, `Pound` → Up, `N0` → Select, `Star` → Esc. Keeping them means
/// the router's mapping needs no change.
const BUTTONS: [(u8, KeyPadButton); 4] = [
    (BTN_DOWN, KeyPadButton::D),
    (BTN_UP, KeyPadButton::Pound),
    (BTN_SELECT, KeyPadButton::N0),
    (BTN_ESC, KeyPadButton::Star),
];

/// Pressed → Held after one further sample; only `Released` leaves this task,
/// so the faster poll changes nothing the router or UI can see.
fn next(state: KeyPadEvent, pressed: bool) -> KeyPadEvent {
    match (state, pressed) {
        (KeyPadEvent::None, true) => KeyPadEvent::Pressed,
        (KeyPadEvent::Pressed, true) => KeyPadEvent::Held,
        (KeyPadEvent::Released, true) => KeyPadEvent::Pressed,
        (KeyPadEvent::Held, true) => KeyPadEvent::Held,
        (KeyPadEvent::Pressed, false) => KeyPadEvent::Released,
        (KeyPadEvent::Held, false) => KeyPadEvent::Released,
        (_, false) => KeyPadEvent::None,
    }
}

/// Drive the expander: configure it (which asserts the TFT's CS and holds its
/// RES), release the display reset (signalling `display_ready` so the UI task
/// can safely init the panel), then poll the buttons forever.
pub async fn run<I2C: I2c>(
    mut expander: Tca9555<I2C>,
    tx: RouterChannelTx,
    display_ready: &'static Signal<CriticalSectionRawMutex, ()>,
) -> ! {
    Timer::after(CARRIER_SETTLE).await;

    if expander.init().await.is_ok() {
        diag::set(diag::EXPANDER_OK);
    } else {
        // Not fatal on its own, but the panel is dead and the display will
        // never come out of reset, so say which of the I²C parts failed.
        diag::set(diag::EXPANDER_FAIL);
        error!("TCA9555: init failed - no panel, no display");
    }
    if expander.release_display_reset().await.is_err() {
        diag::set(diag::EXPANDER_RES_FAIL);
        error!("TCA9555: could not release the display reset");
    }
    // Then, as a separate transaction, assert CS: the panel must see the
    // chip-select fall *after* it leaves reset or its serial interface never
    // synchronises. See `tca9555::DISPLAY_CS`.
    if expander.select_display().await.is_ok() {
        diag::set(diag::TFT_CS_ASSERTED);
    } else {
        error!("TCA9555: could not assert the display chip select");
    }
    // Signalled even on failure so the UI task logs its init result instead of
    // waiting forever; if RES really is stuck low the expander error above is
    // the diagnostic.
    display_ready.signal(());

    let mut state = [KeyPadEvent::None; BUTTONS.len()];
    let mut power = UsbPowerControl::default();

    loop {
        Timer::after(POLL).await;
        // Core 1 is alive and scheduling — see `supervisor::watchdog_task`.
        CORE1_ALIVE.store(true, Ordering::Release);

        let ports = match expander.inputs().await {
            Ok(p) => p,
            Err(_) => {
                error!("TCA9555: read failed");
                continue;
            }
        };

        power.service(&mut expander, ports[0]).await;

        for (i, (mask, button)) in BUTTONS.iter().enumerate() {
            // Active low: a pressed button pulls its input to ground.
            let pressed = ports[0] & mask == 0;
            state[i] = next(state[i], pressed);

            if state[i] == KeyPadEvent::Released {
                if let Err(e) = tx.try_send(RouterEvent::ButtonArray((*button, KeyPadEvent::Released)))
                {
                    error!("button dropped, router channel full: {:?}", e);
                }
            }
        }
    }
}

/// Drives `USB_LED_EN` and watches `FAULT` on the TPS2553-1.
///
/// Polled from the same 50 Hz loop as the buttons, because the expander is the
/// only way to reach any of these lines and one I²C read already has them.
///
/// The switch latches off on an overcurrent or a reverse-voltage event (the
/// `-1` suffix part) and only re-arms when EN is toggled, so a fault here is
/// deliberately sticky: we retry a few times, spaced out, and then stay off
/// until the operator changes something. Hammering EN against a real short is
/// how you cook the switch.
#[derive(Default)]
struct UsbPowerControl {
    enabled: bool,
    /// Polls left before the next re-arm attempt.
    cooldown: u16,
    /// Re-arm attempts used since the path last ran cleanly.
    retries: u8,
    /// Latched: too many failed re-arms, stay off.
    given_up: bool,
    /// Last mode observed, so a setting change clears the latch.
    last_selected: bool,
}


/// Polls between re-arm attempts (50 Hz poll → ~2 s).
const FAULT_COOLDOWN_POLLS: u16 = 100;
/// Re-arm attempts before giving up until the setting is changed.
const FAULT_MAX_RETRIES: u8 = 3;

impl UsbPowerControl {
    async fn service<I2C: I2c>(&mut self, expander: &mut Tca9555<I2C>, port0: u8) {
        let selected = usb_power::brick_selected();
        let ftdi_vbus = port0 & tca9555::VBUS_FTDI_DET != 0;
        let rpi_vbus = port0 & tca9555::VBUS_RPI_DET != 0;
        // Open drain, pulled up: low is the fault.
        let fault_pin = port0 & tca9555::USB_LED_FLT == 0;

        // A change of mode is the operator saying "try again".
        if selected != self.last_selected {
            self.last_selected = selected;
            self.given_up = false;
            self.retries = 0;
            self.cooldown = 0;
        }

        // Only close the switch when the user asked for it *and* something is
        // actually plugged into the FTDI connector. Without VBUS there is
        // nothing to switch and FAULT would read as a fault.
        let want = selected && ftdi_vbus && !self.given_up;

        if self.enabled && !ftdi_vbus {
            // The brick was pulled. With IN gone the switch may well report
            // FAULT too, but that is not an overcurrent and must not spend a
            // retry — three unplugs would otherwise latch the path off until
            // the setting is toggled. Just drop EN; a replug re-enables.
            info!("USB LED power: brick removed, disabling the brick path");
            let _ = expander.set(0, tca9555::USB_LED_EN, false).await;
            self.enabled = false;
            self.cooldown = 0;
        } else if self.enabled && fault_pin {
            // Latched off. Drop EN so the part can re-arm, then wait.
            warn!("USB LED power: FAULT - switch latched off, backing off");
            let _ = expander.set(0, tca9555::USB_LED_EN, false).await;
            self.enabled = false;
            self.cooldown = FAULT_COOLDOWN_POLLS;
            self.retries = self.retries.saturating_add(1);
            if self.retries >= FAULT_MAX_RETRIES {
                error!(
                    "USB LED power: {} faults - staying off until LED Power is changed",
                    FAULT_MAX_RETRIES
                );
                self.given_up = true;
            }
        } else if want && !self.enabled {
            if self.cooldown > 0 {
                self.cooldown -= 1;
            } else {
                info!("USB LED power: enabling the brick path");
                if expander.set(0, tca9555::USB_LED_EN, true).await.is_ok() {
                    self.enabled = true;
                }
            }
        } else if !want && self.enabled {
            info!("USB LED power: disabling the brick path");
            if expander.set(0, tca9555::USB_LED_EN, false).await.is_ok() {
                self.enabled = false;
            }
        } else if self.enabled && !fault_pin {
            // A clean poll while conducting clears the retry budget.
            self.retries = 0;
        }

        let mut bits = 0;
        if self.enabled {
            bits |= usb_power::ST_ENABLED;
        }
        if ftdi_vbus {
            bits |= usb_power::ST_FTDI_VBUS;
        }
        if rpi_vbus {
            bits |= usb_power::ST_RPI_VBUS;
        }
        // Sticky through the back-off, not just the one poll that saw the
        // pin: the title row should read `PWR flt` for the whole ~2 s the path
        // is off, or the operator sees the strips drop with no explanation.
        if self.given_up || self.cooldown > 0 || (self.enabled && fault_pin) {
            bits |= usb_power::ST_FAULT;
        }
        usb_power::set_status(bits);
    }
}
