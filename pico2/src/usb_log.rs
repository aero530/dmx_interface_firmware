//! `log` output over a CDC-ACM interface, held back until the host opens it.
//!
//! Only built with the `usb` feature, which swaps `defmt` for `log` across the
//! whole firmware - every file picks its macros through `cfg_if`, so the two
//! builds carry the same 135 statements to different places: RTT by default,
//! this interface with `--features usb`. Nothing is split between them, which
//! is a change from 2026-09-17: before it, only eight files had the shim and
//! the USB port carried 53 statements, none of them from the boot sequence.
//! Connecting to it on an idle board showed an empty terminal, correctly.
//!
//! It replaces `embassy-usb-logger`, for one reason:
//!
//! that crate starts writing as soon as `wait_connection()` returns, and
//! `wait_connection` means **the endpoint is enabled** - SET_CONFIGURATION -
//! not that anyone is listening. So the boot log is pushed into the IN endpoint
//! within milliseconds of enumeration, where it sits armed until a host asks
//! for it. Two consequences, both bad:
//!
//! * The device's data toggle advances before the first open. When the host
//!   then opens the port it resets *its* toggle, and the two disagree - see
//!   [`crate::usb_toggle`] for why that drops the connection.
//! * The boot log, which is the part worth having, is spent on a port nobody
//!   has open. Whatever arrives before the endpoint fills is gone.
//!
//! Here the sink is installed at boot, so the log accumulates in a pipe from the
//! first line of `main` rather than being thrown at an endpoint nobody is
//! reading. `write_packet` blocks once the endpoint is full, so the backlog
//! stays in the pipe and streams out when a host finally opens the port.
//!
//! **Not gated on DTR.** It was, briefly, on 2026-09-17 - holding the drain
//! until an application raised DTR meant nothing at all reached the endpoint
//! before the port was opened, which sidestepped the data-toggle bug in
//! [`crate::usb_toggle`]. It also meant total silence on any terminal that does
//! not raise DTR, which is how it was found: a port that opened cleanly and
//! never printed. The gate was only ever a workaround, and `usb_toggle` fixes
//! the toggle at its source for every endpoint, so the gate is gone and this
//! drains whenever the endpoint will take bytes.
//!
//! Lines are dropped, never blocked on: `log` is called from tasks that must
//! not stall behind a USB host, so a full pipe loses the newest line and the
//! firmware carries on. [`BUF`] is sized to hold a boot's worth.

use core::fmt::Write as _;

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::pipe::Pipe;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::Driver;
use log::{LevelFilter, Metadata, Record};

/// Bytes of log held while no host has the port open.
///
/// A boot at `info` is a few hundred bytes; this leaves room for the Ethernet
/// probe sweep, which is the chattiest part and the part most worth reading.
const BUF: usize = 4096;

/// One CDC packet.
const PACKET: usize = 64;

/// `CriticalSectionRawMutex` because `log` is called from tasks on core 0 while
/// the drain runs in the USB task; the pipe is the only thing between them.
static PIPE: Pipe<CriticalSectionRawMutex, BUF> = Pipe::new();

static LOGGER: PipeLogger = PipeLogger;

struct PipeLogger;

impl log::Log for PipeLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        // `set_max_level` has already filtered by the time this is called.
        true
    }

    fn log(&self, record: &Record) {
        // CRLF: these land in terminal emulators that do not translate.
        let _ = write!(Sink, "{:<5} {}\r\n", record.level(), record.args());
    }

    fn flush(&self) {}
}

/// Writes into [`PIPE`], dropping whatever does not fit.
struct Sink;

impl core::fmt::Write for Sink {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        // A `Pipe` cannot write across its wraparound in one call, so a short
        // write is normal rather than an error - the second call takes the
        // rest. If that one is short too the buffer really is full, and the
        // remainder is dropped.
        if let Ok(n) = PIPE.try_write(b) {
            if n < b.len() {
                let _ = PIPE.try_write(&b[n..]);
            }
        }
        Ok(())
    }
}

/// Install the sink. Call once, as early in `main` as possible - every `log`
/// call before this one goes nowhere.
pub fn init(level: LevelFilter) {
    // SAFETY: called once, from `main`, before any executor starts, so no other
    // thread can be inside `log`.
    unsafe {
        let _ = log::set_logger_racy(&LOGGER).map(|()| log::set_max_level_racy(level));
    }
}

/// Drain the pipe to the host for as long as it keeps the port open.
///
/// Never returns; join it with the rest of the USB futures.
pub async fn run<'d, D: Driver<'d>>(class: CdcAcmClass<'d, D>) {
    let (mut sender, mut receiver) = class.split();

    let drain = async {
        let mut buf = [0_u8; PACKET];
        loop {
            // Endpoint enabled, i.e. the device is configured. Whether anyone
            // is listening is not knowable without DTR, and DTR is not
            // dependable - see the module docs.
            sender.wait_connection().await;

            loop {
                let n = PIPE.read(&mut buf).await;
                if sender.write_packet(&buf[..n]).await.is_err() {
                    break;
                }
                // A full packet needs a zero-length one behind it, or the host
                // waits for a short packet that never comes.
                if n == PACKET && sender.write_packet(&[]).await.is_err() {
                    break;
                }
            }
        }
    };

    // Nothing is expected from the host, but an unread OUT endpoint NAKs
    // forever and some terminals treat that as the port being wedged.
    let discard = async {
        let mut sink = [0_u8; PACKET];
        loop {
            receiver.wait_connection().await;
            while receiver.read_packet(&mut sink).await.is_ok() {}
        }
    };

    join(drain, discard).await;
}
