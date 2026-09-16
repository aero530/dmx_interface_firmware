//! An `SpiDevice` for the W6300 that spends round trips only where they pay.
//!
//! # Why this exists
//!
//! `embassy-net-wiznet` reads a MACRAW frame with seven register accesses and
//! one payload read, and its W6300 `bus_read`/`bus_write` express every access
//! as **four** `Operation`s: block-select, 16-bit address, dummy, data. Behind
//! `embedded_hal_bus::spi::ExclusiveDevice` over the PIO SPI, each `Operation`
//! became its own DMA transfer with an `await` — a DMA IRQ, an executor wake
//! and a task resume to move one or two bytes. Measured on the bus 2026-09-16
//! (`docs/ARCHITECTURE.md` §11): **~32 round trips per frame** at 4.6 µs each
//! when core 0 is quiet and up to 165 µs when it is not. That was most of the
//! register phase, 0.26–0.50 ms of each frame, and every one of those waits was
//! an opportunity to be pre-empted by smoltcp or the render.
//!
//! The bytes themselves are trivial: a 6-byte register access is 2.4 µs of
//! wire at 20 MHz — shorter than one round trip. So this device:
//!
//! 1. **coalesces the leading `Write`s** of a transaction into one stack buffer
//!    and sends it with a single *blocking* write (the header, plus the data for
//!    register writes);
//! 2. uses the PIO `Spi`'s **blocking** transfers for any operation of
//!    [`BLOCKING_MAX`] bytes or fewer — spinning for two microseconds beats
//!    yielding for five;
//! 3. keeps **DMA** for anything longer, which on the W6300 is exactly the
//!    frame payload and nothing else.
//!
//! A frame's ~32 round trips become ~2. Expected register phase 0.285 → ~0.05 ms
//! and per-frame ~0.78 → ~0.53 ms, which is the difference between a drain that
//! sits at the edge of a 22.7 ms window (see §11 on fix 2) and one with room.
//!
//! # What it is not
//!
//! Not a general shared-bus device: it assumes exclusive ownership of the bus,
//! which the W6300 has (nothing else is on PIO0 SM0). Not a place for
//! `DelayNs`: the driver never issues one; it is honoured for completeness.

use embassy_rp::gpio::Output;
use embassy_rp::pio_programs::spi::Error as PioSpiError;
use embassy_time::Timer;
use embedded_hal::spi::{ErrorType, Operation};
use embedded_hal_async::spi::SpiDevice;

use crate::w6300::SpiBus;

/// Longest operation done by spinning rather than by DMA.
///
/// Every W6300 register access is 5–6 bytes; the payload is hundreds. Anything
/// in between does not occur, so the exact threshold barely matters — 16 keeps
/// the coalescing buffer small and the spin bounded at ~6.4 µs.
pub const BLOCKING_MAX: usize = 16;

/// The W6300's PIO SPI bus plus its chip select, as one `SpiDevice`.
pub struct CoalescingSpi {
    bus: SpiBus,
    cs: Output<'static>,
}

impl CoalescingSpi {
    /// Take the bus and CS. CS is driven high here so the device starts idle.
    pub fn new(bus: SpiBus, mut cs: Output<'static>) -> Self {
        cs.set_high();
        Self { bus, cs }
    }

    /// Send whatever `Write`s have been coalesced so far, in one blocking burst.
    fn flush_header(&mut self, header: &mut [u8; BLOCKING_MAX], len: &mut usize) -> Result<(), PioSpiError> {
        if *len > 0 {
            self.bus.blocking_write(&header[..*len])?;
            *len = 0;
        }
        Ok(())
    }
}

impl ErrorType for CoalescingSpi {
    type Error = PioSpiError;
}

impl SpiDevice<u8> for CoalescingSpi {
    async fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), Self::Error> {
        let mut header = [0u8; BLOCKING_MAX];
        let mut hlen = 0usize;

        self.cs.set_low();
        // Run the operations, but never let an error skip the CS release.
        let result: Result<(), PioSpiError> = async {
            for op in operations.iter_mut() {
                match op {
                    // Consecutive writes that fit are gathered into one burst.
                    Operation::Write(data) if hlen + data.len() <= BLOCKING_MAX => {
                        header[hlen..hlen + data.len()].copy_from_slice(data);
                        hlen += data.len();
                    }
                    Operation::Write(data) => {
                        self.flush_header(&mut header, &mut hlen)?;
                        if data.len() <= BLOCKING_MAX {
                            self.bus.blocking_write(data)?;
                        } else {
                            self.bus.write(data).await?;
                        }
                    }
                    Operation::Read(buf) => {
                        self.flush_header(&mut header, &mut hlen)?;
                        if buf.len() <= BLOCKING_MAX {
                            self.bus.blocking_read(buf)?;
                        } else {
                            self.bus.read(buf).await?;
                        }
                    }
                    Operation::Transfer(read, write) => {
                        self.flush_header(&mut header, &mut hlen)?;
                        if read.len().max(write.len()) <= BLOCKING_MAX {
                            self.bus.blocking_transfer(read, write)?;
                        } else {
                            self.bus.transfer(read, write).await?;
                        }
                    }
                    Operation::TransferInPlace(buf) => {
                        self.flush_header(&mut header, &mut hlen)?;
                        if buf.len() <= BLOCKING_MAX {
                            self.bus.blocking_transfer_in_place(buf)?;
                        } else {
                            self.bus.transfer_in_place(buf).await?;
                        }
                    }
                    Operation::DelayNs(ns) => {
                        self.flush_header(&mut header, &mut hlen)?;
                        Timer::after_nanos(*ns as u64).await;
                    }
                }
            }
            // A transaction that is nothing but short writes ends here.
            self.flush_header(&mut header, &mut hlen)?;
            self.bus.flush()
        }
        .await;
        self.cs.set_high();
        result
    }
}
