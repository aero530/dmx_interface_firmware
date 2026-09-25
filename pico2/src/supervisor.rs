//! Watchdog and health monitoring.
//!
//! A fixture-class lighting device has to recover from a fault on its own: a
//! panic that halts the core, or a task that wedges, must end in a reset rather
//! than strips frozen until someone finds the power switch.
//!
//! # Two cores, one watchdog
//!
//! The RP2350 watchdog is fed from core 0 only, but a core-1 fault (UI, buttons,
//! DMX, LED render) would otherwise go unnoticed while core 0 happily keeps
//! feeding. So core 1 has to check in: the button task sets [`CORE1_ALIVE`] on
//! every 20 ms poll, and the feeder here only feeds if that flag was set since
//! the last check. Either core stalling therefore ends in a reset within
//! [`WATCHDOG_TIMEOUT`].
//!
//! # Panics
//!
//! `panic-probe` prints the panic over defmt and then halts in a HardFault.
//! Nothing special is needed to turn that into a reset: the halted core stops
//! feeding (or stops checking in), and the watchdog fires. The boot flag in the
//! EEPROM records that the boot before was incomplete, and the reset reason is
//! logged at the next start (`main.rs`).
//!
//! # VSYS monitor
//!
//! The module exposes VSYS/3 on its internal ADC (GP29). Reading it costs
//! nothing and answers two bench questions at once: is the main supply actually
//! present (a board running from the module USB through D3/F18 sits around
//! 4.2 V and must not be asked to drive strips), and did a brown-out just happen.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// Logging goes wherever the build says: `defmt` over RTT by default, the
// `log` crate over USB with `--features usb`. Format strings therefore have
// to parse as both - no `{=type}` hints. See `usb_log.rs`.
use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{debug, info, warn};
    } else {
        use defmt::{debug, info, warn};
    }
}
use embassy_rp::adc::{Adc, Blocking, Channel};
use embassy_rp::watchdog::Watchdog;
use embassy_time::{Duration, Timer};

/// Set by core 1 on every button poll; cleared by the feeder when consumed.
pub static CORE1_ALIVE: AtomicBool = AtomicBool::new(false);

/// Last VSYS reading, published for the console.
///
/// The routine reading is `debug!`, so at the shipping `info` level - which is
/// all the USB logger carries - the only visible sign of this rail was the
/// warning. That is no use for *measuring* it, which Stage 2 asks for and the
/// threshold above now needs. 0 means no reading has been taken yet.
static VSYS_MV: AtomicU32 = AtomicU32::new(0);

/// Last VSYS reading in millivolts, or 0 before the first poll.
pub fn vsys_mv() -> u32 {
    VSYS_MV.load(Ordering::Relaxed)
}

/// Reset if neither core has checked in for this long. Long enough to cover a
/// full TFT redraw and an EEPROM page-write burst with margin; short enough
/// that a wedged unit is back within a few seconds.
pub const WATCHDOG_TIMEOUT: Duration = Duration::from_millis(4000);

/// How often core 1's check-in is examined and the watchdog fed.
const FEED_INTERVAL: Duration = Duration::from_millis(1000);

/// VSYS below this means the main supply is absent or sagging (a USB-only
/// bench feed lands around 4.2 V; the 5 V rail through D2 is ~4.6 V).
///
/// **This currently fires at idle, and that is the rail's fault rather than
/// the threshold's.** Measured on J26 alone, 2026-09-21, post-R6-P1:
/// **4.28 V with no LEDs, 3.90 V with two strips** - against a brownout at
/// VSYS ~= 3.6 V (see ARCHITECTURE section 7 item 3). Two strips leaves
/// 300 mV.
///
/// An earlier set appeared to show 4.80 V idle. That was taken with the module
/// USB connected: VSYS is diode-OR'd, so the module's own internal Schottky
/// held it up and hid the sag entirely. Any VSYS measurement is worthless
/// unless `pwr=` reads `ftdi_vbus=0 rpi_vbus=0`.
///
/// **Do not move this constant to silence the warning.** At these voltages the
/// board really is near its floor and the warning is correct. The rail is what
/// needs fixing - trim the J26 PSU to the >= 5.1 V review section 87 asks for
/// (it measured 4.97 V), then the harness gauge. Re-set this from a
/// measurement afterwards, not before.
const VSYS_LOW_MV: u32 = 4400;

const VSYS_POLL: Duration = Duration::from_secs(5);

/// Feed the watchdog while both cores are alive.
///
/// `wdt` must already be started (`main.rs` does that before spawning, so the
/// window between start and first feed is covered by the initial timeout).
#[embassy_executor::task]
pub async fn watchdog_task(mut wdt: Watchdog) -> ! {
    loop {
        Timer::after(FEED_INTERVAL).await;
        if CORE1_ALIVE.swap(false, Ordering::AcqRel) {
            wdt.feed(WATCHDOG_TIMEOUT);
        } else {
            // Not feeding is the action: say why the reset that follows happened.
            warn!("supervisor: core 1 has not checked in - withholding the watchdog feed");
        }
    }
}

/// Log VSYS, and warn once when it drops below [`VSYS_LOW_MV`].
#[embassy_executor::task]
pub async fn vsys_monitor(mut adc: Adc<'static, Blocking>, mut vsys: Channel<'static>) -> ! {
    let mut warned = false;
    loop {
        // 12-bit result against the 3.3 V ADC reference, through the module's
        // 1:3 divider.
        match adc.blocking_read(&mut vsys) {
            Ok(raw) => {
                let mv = raw as u32 * 3300 * 3 / 4096;
                VSYS_MV.store(mv, Ordering::Relaxed);
                if mv < VSYS_LOW_MV {
                    if !warned {
                        warned = true;
                        // Do not name a cause this task cannot distinguish.
                        // The old wording said "USB-only bench feed? do not
                        // expect strips to light", which is actively wrong when
                        // the strips are lit and are themselves what is pulling
                        // the rail down - the likeliest trigger on a real rig.
                        warn!(
                            "VSYS {} mV, below {} mV - rail sagging: heavy LED load, an undersized supply, or no main supply at all",
                            mv, VSYS_LOW_MV
                        );
                    }
                } else {
                    if warned {
                        info!("VSYS {} mV - supply restored", mv);
                    }
                    warned = false;
                    debug!("VSYS {} mV", mv);
                }
            }
            Err(_) => warn!("VSYS: ADC read failed"),
        }
        Timer::after(VSYS_POLL).await;
    }
}
