//! PIO-backed DMX-512 receiver and transmitter.
//!
//! Ports of the Pico-DMX `DmxInput` and `DmxOutput` PIO programs
//! (jostlowe/Pico-DMX, BSD-3-Clause) to embassy-rp.
//!
//! * RX: the state machine hunts for a valid BREAK (>= 88us of continuous
//!   low), waits for the Mark-After-Break, then shifts in one byte per DMX
//!   slot. Each byte is pushed to the RX FIFO and drained into a buffer by
//!   DMA.
//! * TX: the state machine asserts a 177us BREAK and a 16us MAB, then
//!   shifts out one byte per DMX slot (8N2) fed from the TX FIFO by DMA.

use embassy_rp::Peri;
use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::dma;
use embassy_rp::interrupt;
use embassy_rp::gpio::{Level, Pull};
use embassy_rp::pio::program::pio_asm;
use embassy_rp::pio::{
    Common, Config, Direction, FifoJoin, Instance, LoadedProgram, PioPin, ShiftDirection,
    StateMachine,
};
use embassy_time::Timer;
use fixed::traits::ToFixed;

/// One DMX packet as stored in the receive buffer: start code + 512 channels.
pub const DMX_FRAME_SIZE: usize = 513;

/// The state machine must run at exactly 1MHz for the program's bit timing.
const DMX_SM_FREQ: u32 = 1_000_000;

/// This struct represents a DMX RX program loaded into pio instruction memory.
pub struct PioDmxRxProgram<'d, PIO: Instance> {
    prg: LoadedProgram<'d, PIO>,
}

impl<'d, PIO: Instance> PioDmxRxProgram<'d, PIO> {
    /// Load the DMX RX program into the given pio instruction memory.
    pub fn new(common: &mut Common<'d, PIO>) -> Self {
        let prg = pio_asm!(
            ".define dmx_bit 4",               // At 250kbaud a single DMX bit is 4us
            "break_reset:",
            "    set x, 29",                   // Counter for break_loop below
            "break_loop:",                     // One iteration is 3us; 30 * 3us = 90us > the 88us minimum BREAK
            "    jmp pin break_reset",         // Restart the hunt if the line goes high during the BREAK
            "    jmp x-- break_loop   [1]",
            "    wait 1 pin 0",                // Stall until the line goes high for the Mark-After-Break (MAB)
            ".wrap_target",
            "    wait 0 pin 0",                // Stall until the start bit is asserted
            "    set x, 7             [dmx_bit]", // Preload bit counter, delay until halfway through the first bit
            "bitloop:",
            "    in pins, 1",                  // Shift data bit into ISR
            "    jmp x-- bitloop      [dmx_bit-2]", // Loop 8 times, one iteration per 4us bit
            "    wait 1 pin 0",                // Wait for the line to go high for the stop bits
            "    in NULL, 24",                 // Right-justify the byte at ISR[7:0] where the 8-bit DMA read expects it
            "    push",
            ".wrap",
        );

        let prg = common.load_program(&prg.program);
        Self { prg }
    }
}

/// PIO-backed DMX-512 receiver.
pub struct PioDmxRx<'d, PIO: Instance, const SM: usize> {
    sm: StateMachine<'d, PIO, SM>,
    dma: dma::Channel<'d>,
    origin: u8,
}

impl<'d, PIO: Instance, const SM: usize> PioDmxRx<'d, PIO, SM> {
    /// Configure a pio state machine to use the loaded DMX RX program.
    pub fn new<D: dma::ChannelInstance>(
        common: &mut Common<'d, PIO>,
        mut sm: StateMachine<'d, PIO, SM>,
        dma: Peri<'d, D>,
        irq: impl interrupt::typelevel::Binding<D::Interrupt, dma::InterruptHandler<D>> + 'd,
        rx_pin: Peri<'d, impl PioPin>,
        program: &PioDmxRxProgram<'d, PIO>,
    ) -> Self {
        let mut cfg = Config::default();
        cfg.use_program(&program.prg, &[]);

        let mut rx_pin = common.make_pio_pin(rx_pin);
        rx_pin.set_pull(Pull::Up); // DMX line idles high
        sm.set_pin_dirs(Direction::In, &[&rx_pin]);
        cfg.set_in_pins(&[&rx_pin]); // for WAIT, IN
        cfg.set_jmp_pin(&rx_pin); // for JMP

        // DMX sends each slot LSB-first; shift right and push each byte manually.
        cfg.shift_in.auto_fill = false;
        cfg.shift_in.direction = ShiftDirection::Right;
        cfg.shift_in.threshold = 32;

        // Deeper RX FIFO, we never transmit.
        cfg.fifo_join = FifoJoin::RxOnly;

        cfg.clock_divider = (clk_sys_freq() / DMX_SM_FREQ).to_fixed();

        sm.set_config(&cfg);

        // The state machine stays disabled until the first `read()` arms it.
        Self {
            sm,
            dma: dma::Channel::new(dma, irq),
            origin: program.prg.origin,
        }
    }

    /// Receive one DMX packet into `buf`; returns the number of slots
    /// received **including** the start code at `buf[0]`.
    ///
    /// The state machine is restarted at the BREAK detector before the
    /// transfer, so the data is always aligned to the start of a packet.
    ///
    /// DMX permits packets of 1..=512 channels and many consoles send fewer
    /// than 512, so a fixed 513-byte read would never complete on them (and
    /// the BREAK of the *next* packet would be shifted in as zero bytes,
    /// corrupting the frame). Instead the full-size DMA is raced against a
    /// stall detector: when the transfer makes no progress for two
    /// consecutive 4 ms samples after at least one byte arrived, the packet
    /// is over — at 250 kbaud a slot takes 44 us, so 4 ms of silence is far
    /// past any inter-slot MARK a real console emits (the spec allows up to
    /// 1 s; that pathological case is treated as end-of-packet here).
    ///
    /// Cancelling the returned future (e.g. racing it against a timeout)
    /// aborts the in-flight DMA transfer and leaves the receiver safe: the
    /// next `read()` re-arms from scratch.
    pub async fn read(&mut self, buf: &mut [u8; DMX_FRAME_SIZE]) -> usize {
        // Re-arm: reset the state machine to the BREAK detector so the byte
        // stream is aligned to a packet boundary, and drop anything stale
        // still sitting in the FIFO. This mirrors what the Pico-DMX C
        // library does in its DMA completion interrupt.
        self.sm.set_enable(false);
        self.sm.clear_fifos();
        self.sm.restart();
        unsafe { self.sm.exec_jmp(self.origin) };
        self.sm.set_enable(true);

        // The register block is Copy — grab it before `dma` is mutably
        // borrowed by the transfer so the stall detector can watch
        // TRANS_COUNT (remaining transfers) from outside.
        let regs = self.dma.regs();

        // Each RX FIFO word holds one DMX slot in its low byte; drain up to
        // one full packet with 8-bit DMA reads.
        let mut xfer = core::pin::pin!(self.sm.rx().dma_pull(&mut self.dma, buf, false));
        let mut last_remaining = DMX_FRAME_SIZE as u32;
        loop {
            match embassy_futures::select::select(&mut xfer, Timer::after_millis(4)).await {
                embassy_futures::select::Either::First(()) => return DMX_FRAME_SIZE,
                embassy_futures::select::Either::Second(()) => {
                    let remaining = regs.trans_count().read().count();
                    let received = DMX_FRAME_SIZE - remaining as usize;
                    if received > 0 && remaining == last_remaining {
                        // No progress for a full sample period: end of packet.
                        // Returning drops the pinned transfer, which aborts
                        // the DMA channel before the caller sees the frame.
                        return received;
                    }
                    last_remaining = remaining;
                }
            }
        }
    }
}

/// This struct represents a DMX TX program loaded into pio instruction memory.
pub struct PioDmxTxProgram<'d, PIO: Instance> {
    prg: LoadedProgram<'d, PIO>,
}

impl<'d, PIO: Instance> PioDmxTxProgram<'d, PIO> {
    /// Load the DMX TX program into the given pio instruction memory.
    pub fn new(common: &mut Common<'d, PIO>) -> Self {
        let prg = pio_asm!(
            ".side_set 1 opt",
            // Assert break condition
            // BREAK starts on this instruction, so it is 1 + 22*8 = 177us, not the
            // 176 the loop alone accounts for. Measured 177us on GP9, 2026-09-17.
            "    set x, 21          side 0",    // Preload bit counter, start the BREAK
            "breakloop:",                       // This loop will run 22 times
            "    jmp x-- breakloop  [7]",       // Each loop iteration is 8 cycles
            // Assert start condition
            "    nop                side 1 [7]", // Assert MAB. 8 cycles nop and 8 cycles stop-bits = 16us
            // Send data frame
            ".wrap_target",
            "    pull               side 1 [7]", // Assert 2 stop bits, or stall with line in idle state
            "    set x, 7           side 0 [3]", // Preload bit counter, assert start bit for 4 clocks
            "bitloop:",                          // This loop will run 8 times (8n1 UART)
            "    out pins, 1",                   // Shift 1 bit from OSR to the first OUT pin
            "    jmp x-- bitloop    [2]",        // Each loop iteration is 4 cycles
            ".wrap",
        );

        let prg = common.load_program(&prg.program);
        Self { prg }
    }
}

/// PIO-backed DMX-512 transmitter — driven by `dmx.rs` in the `ArtNet>DMX`
/// and `USB>DMX` modes.
pub struct PioDmxTx<'d, PIO: Instance, const SM: usize> {
    sm: StateMachine<'d, PIO, SM>,
    dma: dma::Channel<'d>,
    origin: u8,
}

impl<'d, PIO: Instance, const SM: usize> PioDmxTx<'d, PIO, SM> {
    /// Configure a pio state machine to use the loaded DMX TX program.
    pub fn new<D: dma::ChannelInstance>(
        common: &mut Common<'d, PIO>,
        mut sm: StateMachine<'d, PIO, SM>,
        dma: Peri<'d, D>,
        irq: impl interrupt::typelevel::Binding<D::Interrupt, dma::InterruptHandler<D>> + 'd,
        tx_pin: Peri<'d, impl PioPin>,
        program: &PioDmxTxProgram<'d, PIO>,
    ) -> Self {
        let mut cfg = Config::default();
        let mut tx_pin = common.make_pio_pin(tx_pin);
        // The TX opto LED needs ~8 mA of sink (220R from 3V3 through the
        // TLP2368 LED); the RP2350 pad default is 4 mA, which was already
        // marginal on Rev 1's RP2040 at 330R. See docs/ARCHITECTURE.md.
        tx_pin.set_drive_strength(embassy_rp::gpio::Drive::_12mA);
        // The BREAK/MAB/start/stop bits are driven by the side-set on the same pin
        cfg.use_program(&program.prg, &[&tx_pin]);
        cfg.set_out_pins(&[&tx_pin]);
        sm.set_pins(Level::High, &[&tx_pin]); // DMX line idles high
        sm.set_pin_dirs(Direction::Out, &[&tx_pin]);

        // DMX sends each slot LSB-first; each pull takes one byte from the FIFO.
        cfg.shift_out.auto_fill = false;
        cfg.shift_out.direction = ShiftDirection::Right;
        cfg.shift_out.threshold = 32;

        // Deeper TX FIFO, we never receive on this state machine.
        cfg.fifo_join = FifoJoin::TxOnly;

        cfg.clock_divider = (clk_sys_freq() / DMX_SM_FREQ).to_fixed();

        sm.set_config(&cfg);

        // The state machine stays disabled until the first `write()` arms it.
        Self {
            sm,
            dma: dma::Channel::new(dma, irq),
            origin: program.prg.origin,
        }
    }

    /// Transmit one complete DMX packet: BREAK, MAB, then all 513 slots
    /// (`frame[0]` = start code). Returns once the final stop bits are on
    /// the wire, so back-to-back calls produce a continuous ~43 packet/s
    /// DMX stream. Cancelling the returned future aborts the in-flight DMA
    /// transfer; the state machine drains what it already has and idles high.
    pub async fn write(&mut self, frame: &[u8; DMX_FRAME_SIZE]) {
        // Re-arm: reset the state machine to the BREAK generator, dropping
        // anything stale in the FIFO. This mirrors what the Pico-DMX C
        // library does at the start of DmxOutput::write().
        self.sm.set_enable(false);
        self.sm.clear_fifos();
        self.sm.restart();
        unsafe { self.sm.exec_jmp(self.origin) };
        let _ = self.sm.tx().stalled(); // reading clears the stale TXSTALL flag
        self.sm.set_enable(true);

        // The DMA writes one byte per 32-bit FIFO word (byte lanes are
        // replicated on the bus; `out pins, 1` shifts the low 8 bits).
        self.sm.tx().dma_push(&mut self.dma, frame, false).await;

        // DMA completion only means the FIFO was filled. Wait until the
        // state machine has drained it and stalled on `pull` with the line
        // idle (equivalent of the C library's DmxOutput::busy()).
        while !(self.sm.tx().empty() && self.sm.tx().stalled()) {
            Timer::after_micros(100).await;
        }
        // The stalled `pull` is what asserts the last byte's stop bits; give
        // them their full 8us before the caller can start the next BREAK.
        Timer::after_micros(10).await;
    }
}
