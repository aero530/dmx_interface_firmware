//! W6300 Ethernet transport.
//!
//! The W6300 is a hardwired TCP/IP controller, but it is driven here in
//! **MACRAW** mode — raw Ethernet frames in and out — so smoltcp sits on top and
//! `embassy-net` presents the same API the STM32 build had against its built-in
//! MAC. That is what lets the Art-Net task in `common` port unchanged.
//!
//! # Why the link is PIO and not hardware SPI
//!
//! On the W6300-EVB-Pico2 the W6300's `SCLK` lands on **GP17**, which is a
//! `CSn` mux position on the RP2350, not `SCK`. Hardware SPI0 physically cannot
//! reach it. So the transport is a PIO SPI on PIO0 SM0 — mandatory even in
//! single-SPI mode, which is why the PIO budget reserves that state machine.
//!
//! # Single SPI now, QSPI later
//!
//! `embassy-net-wiznet` 0.3 drives the W6300 in single SPI. QSPI is embassy
//! PR #5809, still a draft. When it lands, only [`SpiBus`] below changes — the
//! `embassy-net` device above it, and everything above that, stays as it is.
//!
//! # Diagnosing a dead chip
//!
//! [`InitError`] distinguishes the two cases that used to look identical on the
//! Nucleo, where a dead PHY could only be found with a scope:
//!
//! * `SpiError` — the chip is not answering on the bus at all.
//! * `InvalidChipVersion` — it answers, but the version register is wrong.
//!
//! Both are logged distinctly. Since the Ethernet failure that prompted this
//! redesign was never diagnosed and now never can be, this is the board's own
//! account of what went wrong. See `docs/ARCHITECTURE.md` §10.

use defmt::*;
use embassy_net_wiznet::chip::W6300;
use embassy_net_wiznet::{Device, InitError, Runner, State};
use embassy_rp::gpio::{Input, Output};
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio_programs::spi::Spi as PioSpi;
use embassy_rp::spi::Async;
use embassy_time::Timer;

/// PIO SPI bus to the W6300 — PIO0 state machine 0.
pub type SpiBus = PioSpi<'static, PIO0, 0, Async>;

/// The `SpiDevice` the driver talks through: the PIO bus plus the software
/// chip select on GP16, with the header of every register access coalesced into
/// one blocking burst and DMA kept for the frame payload alone. See
/// `spi_coalesce.rs` for why that replaced `embedded_hal_bus`'s
/// `ExclusiveDevice`, and the measurement behind it. The W6300 is alone on this
/// bus, so exclusive ownership is exactly right.
pub type SpiDev = crate::spi_coalesce::CoalescingSpi;

/// Bus and chip select, kept apart until [`init`] has settled on a clock.
///
/// The driver takes ownership of a whole `SpiDevice`, and once the bus is
/// inside one there is no way back to `set_frequency` — so the clock has to be
/// chosen before the two are joined.
pub struct SpiParts {
    pub bus: SpiBus,
    pub cs: Output<'static>,
}

/// `CIDR2` minor chip ID a healthy W6300 reports. Mirrors the driver's own
/// constant, which is not public; `diag` shows it so a good board and a bad one
/// print the same shape of answer.
pub const CHIP_VERSION: u8 = 0x11;

/// Driver task handle. `INT` is GP15, `RST` is GP22.
pub type W6300Runner = Runner<'static, W6300, SpiDev, Input<'static>, Output<'static>>;

/// Clock rates the pre-flight probe tries, **fastest first**; the first one
/// that reads the version register correctly is the one the link runs at.
///
/// # Why this is measured at boot rather than fixed
///
/// This is the transport for every Art-Net universe, so the clock is the main
/// lever on the ingest budget and wants to be as high as it will go. But the
/// usable ceiling is a property of this board and module, not something that
/// can be read off a datasheet.
///
/// The PIO SPI samples MISO at the rising edge of SCK — the earliest legal
/// moment — and the W6300 presents each bit on the preceding falling edge. So a
/// read is correct only while the SCK half period exceeds the chip's output
/// delay plus whatever the sampling path adds. When it is not, the failure is
/// silent: every byte arrives shifted one bit right, because each sample
/// catches the *previous* bit. The first board read its version register as
/// `0x08` — exactly `0x11 >> 1` — and looked like a dead chip.
///
/// # What the measurements said
///
/// | Sampling path | Highest clean rate | Implied chip output delay |
/// |---|---|---|
/// | Through the input synchroniser | **15 MHz** (16 MHz failed) | 17.9-20.0 ns |
/// | Synchroniser bypassed | measured at boot, `eth_hz=` | - |
///
/// The RP2350 puts every PIO input through a two-flop synchroniser, so the
/// value latched is the pin as it was ~2 `clk_sys` cycles (13.3 ns at 150 MHz)
/// earlier. Against a 33 ns half period at 15 MHz that is most of the budget,
/// and it is pure lag on the read path - which is why `main.rs` bypasses it for
/// GP19. That moves the sample from roughly a quarter of the way into each bit
/// to the nominal half, and is worth far more than any rung on this ladder.
///
/// The ladder therefore runs well above the old ceiling to find where the real
/// one now is. If it still settles at 15 MHz the bypass did not take effect; if
/// it settles around 22-25 MHz that is the chip's own output delay, and the
/// next lever would be a custom PIO program that samples later still.
pub const PROBE_FREQS_HZ: [u32; 18] = [
    32_000_000, 30_000_000, 28_000_000, 26_000_000, 24_000_000, 22_000_000, 20_000_000,
    18_000_000, 16_000_000, 15_000_000, 14_000_000, 13_000_000, 12_000_000, 10_000_000,
    8_000_000, 6_000_000, 4_000_000, 2_000_000,
];
/// Fastest rate the link will actually **run** at, whatever the probe finds it
/// is *capable* of.
///
/// The probe measures a ceiling; this is the operating point, and they are
/// deliberately not the same number.
///
/// Measured on the first board (2026-09-15, synchroniser bypassed): 24 MHz
/// passed and 26 MHz failed. Working back from the sample point — half a bit
/// period, so 20.8 ns at 24 MHz and 19.2 ns at 26 — the W6300's output delay is
/// **19.2-20.0 ns**, which also agrees with the independent measurement taken
/// through the synchroniser (15 MHz pass, 16 MHz fail -> 17.9-20.0 ns).
///
/// Running at 24 MHz would therefore leave **under 2 ns** of margin. That is
/// not an operating point: output delay moves with temperature, supply and
/// part-to-part spread, and the failure is silent bit-shifted data rather than
/// anything that raises an error. At 20 MHz the sample lands 25 ns after the
/// chip presents the bit, for ~5 ns — a quarter of a bit — of margin.
///
/// Nothing is given up for it. Section 6 of `docs/ARCHITECTURE.md` draws the
/// bandwidth budget at 20 MHz, and above ~2400 B/port the WS2812 protocol is
/// the wall rather than the link, so the extra 4 MHz buys throughput the design
/// cannot spend. Raise this only with a measurement that says otherwise.
pub const OPERATING_MAX_HZ: u32 = 20_000_000;

/// Consecutive clean reads a rate must produce before it is accepted.
///
/// One read is not evidence. The failure here is a setup-time violation, and
/// right at the boundary it stops being deterministic — the synchroniser
/// resolves whichever way it happens to, so a marginal rate can return the
/// right byte once and the wrong one under load or at a different temperature.
/// Repeating the read is nearly free (a few microseconds each) and turns a
/// coin-flip into a reliable rejection.
const PROBE_READS: u32 = 16;

/// Fastest rate the probe will try. The *operating* rate is capped separately
/// by [`OPERATING_MAX_HZ`].
pub const SPI_FREQ_HZ: u32 = PROBE_FREQS_HZ[0];

/// Receive queue depth, in MACRAW frames, between the driver and smoltcp.
///
/// The burst is absorbed by the chip, not here: socket 0 has the W6300 whole
/// 16 KB (fix 1, vendored driver), about 28 full-size frames. This queue only
/// carries frames the driver has already pulled over SPI to the point where
/// `net_task` hands them to smoltcp.
///
/// 8 is the measured best, and 2 was tried (fix 5, 2026-09-16) on the theory
/// that `net_task` was processing several queued frames per poll and the
/// driver, woken by its payload DMA, sat behind that run. It was not: with 2
/// slots the driver post-DMA wait was unchanged (216 -> 211 us), so the queue
/// was already near one frame deep and the depth is not the lever. What 2 did
/// add was episodic starvation - the driver blocking for a free slot - which
/// widened frames-per-window from 30-31 to 27-33 and cost 4 % of throughput
/// (1346 -> 1289/s). Reverted to 8. 4 was never tested; with no benefit to
/// trade against there is nothing for it to buy. See docs/ARCHITECTURE.md
/// section 11.
pub const N_RX: usize = 8;
/// Transmit queue depth. Only ArtPollReply and DHCP go out, so this is small.
pub const N_TX: usize = 4;

/// Shared state for the driver. Must outlive the stack, so it is a `StaticCell`
/// in `main`.
pub type W6300State = State<N_RX, N_TX>;

/// Read `CIDR2` (common block, `0x0004`) with no driver in the way.
///
/// Byte-for-byte the frame `embassy-net-wiznet` uses: block select, then the
/// 16-bit address big-endian, then one dummy byte, then the data phase.
async fn read_chip_version(parts: &mut SpiParts) -> u8 {
    const COMMON_BLOCK: u8 = 0x00;
    let mut version = [0u8; 1];
    parts.cs.set_low();
    let framed = parts.bus.write(&[COMMON_BLOCK, 0x00, 0x04, 0x00]).await.is_ok()
        && parts.bus.read(&mut version).await.is_ok();
    parts.cs.set_high();
    if framed {
        version[0]
    } else {
        0
    }
}

/// Test one rate. `true` only if every read of the version register is clean.
async fn rate_is_clean(parts: &mut SpiParts, freq: u32) -> (bool, u8) {
    parts.bus.set_frequency(freq);
    let mut seen = 0u8;
    for _ in 0..PROBE_READS {
        seen = read_chip_version(parts).await;
        if seen != CHIP_VERSION {
            return (false, seen);
        }
    }
    (true, seen)
}

/// Measure the ceiling, then settle on an operating point below it.
///
/// Returns the operating rate, having left the bus set to it. `None` means no
/// rate worked at all, and the last value read is left in `diag` — at 2 MHz
/// there is no timing margin left to blame, so a wrong byte there points at the
/// module, the wiring or the frame itself rather than at the clock.
async fn tune_clock(parts: &mut SpiParts, reset: &mut Output<'static>) -> Option<u32> {
    // The driver resets the chip itself, but that happens after this runs.
    reset.set_low();
    Timer::after_millis(1).await;
    reset.set_high();
    Timer::after_millis(100).await;

    let mut seen = 0u8;
    for (n, freq) in PROBE_FREQS_HZ.iter().copied().enumerate() {
        let (clean, last) = rate_is_clean(parts, freq).await;
        seen = last;
        crate::diag::set_eth_version(seen);
        if !clean {
            debug!("W6300: {} Hz -> version {=u8:#04x}, want {=u8:#04x}", freq, seen, CHIP_VERSION);
            continue;
        }

        // The ceiling. Reported, but not necessarily run at: see
        // `OPERATING_MAX_HZ` for why the two are kept apart.
        crate::diag::set_eth_max_hz(freq);
        info!("W6300: ceiling {} Hz", freq);

        let operating = PROBE_FREQS_HZ[n..].iter().copied().find(|f| *f <= OPERATING_MAX_HZ);
        let Some(operating) = operating else {
            // Every rung at or below the ceiling is still above the cap, which
            // can only happen if the ladder has no rung under it. Run at the
            // ceiling rather than refusing to bring the link up, and say so.
            warn!("W6300: no rung at or below {} Hz; running at the ceiling", OPERATING_MAX_HZ);
            crate::diag::set_eth_hz(freq);
            return Some(freq);
        };

        if operating != freq {
            info!("W6300: derating {} -> {} Hz for timing margin", freq, operating);
        }
        // Re-verify at the rate actually used. It sits below a rate that just
        // passed, so this should never fail — but the operating point is the
        // one that matters, and confirming it costs microseconds.
        let (ok, last) = rate_is_clean(parts, operating).await;
        seen = last;
        crate::diag::set_eth_version(seen);
        if ok {
            crate::diag::set_eth_hz(operating);
            return Some(operating);
        }
        error!("W6300: {} Hz passed but {} Hz did not - not monotonic, bailing", freq, operating);
        return None;
    }
    error!(
        "W6300: no clock from {} down to {} Hz reads the version register; last was {=u8:#04x}",
        PROBE_FREQS_HZ[0],
        PROBE_FREQS_HZ[PROBE_FREQS_HZ.len() - 1],
        seen
    );
    None
}

/// Bring the chip up and return the `embassy-net` device plus its runner.
///
/// Probes for a usable clock first (see [`PROBE_FREQS_HZ`]), then hands the bus
/// to the driver. Logs the failure mode rather than just returning it, because
/// *which* way this fails is the diagnostic signal.
pub async fn init(
    mac_addr: [u8; 6],
    state: &'static mut W6300State,
    mut parts: SpiParts,
    int: Input<'static>,
    mut reset: Output<'static>,
) -> Option<(Device<'static>, W6300Runner)> {
    if tune_clock(&mut parts, &mut reset).await.is_none() {
        crate::diag::set(crate::diag::ETH_BAD_VERSION);
        return None;
    }

    let spi_dev = crate::spi_coalesce::CoalescingSpi::new(parts.bus, parts.cs);

    match embassy_net_wiznet::new(mac_addr, state, spi_dev, int, reset).await {
        Ok((device, runner)) => {
            info!("W6300: up, MAC {:02x}", mac_addr);
            crate::diag::set(crate::diag::ETH_OK);
            crate::diag::set_eth_version(CHIP_VERSION);
            Some((device, runner))
        }
        Err(InitError::SpiError(_)) => {
            // Nothing answered on the bus. Chip dead, unpowered, or the PIO SPI
            // is misconfigured. Distinct from "link down" on purpose.
            error!("W6300: ETH_CHIP_NOT_RESPONDING - no reply over SPI");
            crate::diag::set(crate::diag::ETH_NO_REPLY);
            None
        }
        Err(InitError::InvalidChipVersion { expected, actual }) => {
            // It answered, so the bus works, but it is not a healthy W6300.
            // The byte itself is the diagnosis, so it goes where a board with
            // no probe attached can be asked for it: `info` on the USB console.
            error!(
                "W6300: ETH_CHIP_BAD_VERSION - expected {=u8:#04x}, got {=u8:#04x}",
                expected, actual
            );
            crate::diag::set(crate::diag::ETH_BAD_VERSION);
            crate::diag::set_eth_version(actual);
            None
        }
    }
}

/// Pump the W6300 driver. Must run for the stack to move any traffic.
#[embassy_executor::task]
pub async fn w6300_task(runner: W6300Runner) -> ! {
    runner.run().await
}

/// Keep the reported network state in step with the stack.
///
/// # Why this polls instead of awaiting
///
/// `embassy-net`'s `wait_link_up`, `wait_config_up` and friends all register
/// into a **single-slot** `WakerRegistration` (`Inner::state_waker`). A second
/// waiter silently displaces the first, and the displaced task is then never
/// woken again. The Art-Net task legitimately waits there for its address, so a
/// diagnostic task waiting too is a coin flip over which one ever wakes — and
/// the losing side hangs forever. That is not theoretical: it is what ate
/// `eth_configured` on the first board while Art-Net got its lease.
///
/// Polling touches only `is_link_up` and `config_v4`, which take the lock and
/// read. At 250 ms it costs nothing and cannot displace anyone.
///
/// # Why this owns the reported address
///
/// The address is **derived** here rather than latched once by whoever happened
/// to see it first. That gives a single writer for network status — no ordering
/// race between the boot sequence and the Art-Net task over who reports last —
/// and it means a DHCP renewal onto a different address, or a lease lost with
/// the cable, updates the title row and `info` instead of showing the
/// boot-time value until the next reboot.
#[embassy_executor::task]
pub async fn net_watch_task(stack: embassy_net::Stack<'static>) -> ! {
    use common::channels;
    use common::event_router::RouterEvent;

    let router = channels::CHANNEL.sender();
    let mut link = false;
    let mut addr = None;
    loop {
        let up = stack.is_link_up();
        if up {
            crate::diag::set(crate::diag::ETH_LINK_UP);
        } else if link {
            crate::diag::set(crate::diag::ETH_LINK_DROPPED);
            warn!("W6300: link down");
        }
        link = up;

        let now = stack.config_v4().map(|c| c.address.address());
        if now != addr {
            match now {
                Some(ip) => {
                    crate::diag::set(crate::diag::ETH_CONFIGURED);
                    info!("net: address {:?}", ip);
                    let _ = router.try_send(RouterEvent::StoreIpAddr(Some(ip)));
                }
                None => {
                    info!("net: address lost");
                    let _ = router.try_send(RouterEvent::StoreIpAddr(None));
                }
            }
            addr = now;
        }
        Timer::after_millis(250).await;
    }
}

/// Pump the `embassy-net` stack.
#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, Device<'static>>) -> ! {
    runner.run().await
}
