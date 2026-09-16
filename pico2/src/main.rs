//! DMX interface firmware — W6300-EVB-Pico2 target (RP2350 + W6300).
//!
//! Replaces the STM32H563 Nucleo build. Everything target-agnostic lives in the
//! `common` crate and is shared with `host_tests/`; this crate is only the
//! peripheral setup, the PIO programs and the executor wiring.
//!
//! See `../docs/ARCHITECTURE.md` for why the firmware is shaped this way.
//!
//! # Core split
//!
//! * **Core 0** — W6300 + smoltcp, Art-Net / sACN, USB, the FT232RNL widget
//!   port, the event router, EEPROM, the watchdog feeder.
//! * **Core 1** — TFT UI, buttons, wired DMX, LED render.
//!
//! The split exists so network load cannot make the UI laggy: a console
//! broadcasting universes we do not care about still has to be ingested and
//! discarded, and that work stays off the core driving the display.

#![no_std]
#![no_main]

extern crate alloc;

// The ported sources (artnet, event_router) use `crate::`-rooted paths. Mirror
// nucleo's re-export shape so they compile without edits.
pub use common::constants::*;
pub use common::{channels, enttec_protocol, ui, DMX_BUFFER, LED_COLORS};

use defmt::*;
use defmt_rtt as _;
use embassy_executor::{Executor, Spawner};
use embassy_rp::adc::{self, Adc};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_net::StackResources;
use embassy_rp::i2c::{self, I2c, InterruptHandler as I2cInterruptHandler};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_rp::clocks::RoscRng;
use embassy_rp::dma::InterruptHandler as DmaInterruptHandler;
use embassy_rp::gpio::{Input, Pull};
use embassy_rp::peripherals::{
    DMA_CH0, DMA_CH1, DMA_CH10, DMA_CH11, DMA_CH2, DMA_CH3, DMA_CH4, DMA_CH5, DMA_CH6,
    DMA_CH7, DMA_CH8, DMA_CH9, I2C1, PIO0, PIO1, PIO2, UART0, USB,
};
use embassy_rp::uart::{self, BufferedInterruptHandler as UartInterruptHandler, BufferedUart};
use embassy_rp::pio_programs::spi::Spi as PioSpi;
use embassy_rp::spi::{Config as SpiConfig, Spi as HwSpi};
use embassy_rp::watchdog::{ResetReason, Watchdog};
use static_cell::StaticCell;
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_rp::multicore::{spawn_core1, Stack as CoreStack};
use embassy_time::Timer;
use panic_probe as _;

mod artnet;
mod buttons;
mod console_usb;
mod diag;
mod netstats;
mod spi_coalesce;
mod dmx;
mod dmx_pio;
mod eeprom;
mod enttec_uart;
mod enttec_widget;
mod event_router;
mod m24x02;
mod panel_test;
mod pca9633;
mod sacn_rx;
mod smart_led;
mod identity;
mod supervisor;
mod tca9555;
mod tft_ui;
mod usb_device;
mod w6300;
mod ws2812;
use smart_led::Ws2812Outputs;
use ws2812::{Ws2812, Ws2812Program};

/// Program metadata for `picotool info`.
#[unsafe(link_section = ".bi_entries")]
#[used]
pub static PICOTOOL_ENTRIES: [embassy_rp::binary_info::EntryAddr; 4] = [
    embassy_rp::binary_info::rp_program_name!(c"DMX Interface"),
    embassy_rp::binary_info::rp_program_description!(
        c"Art-Net / sACN / DMX to 8x WS2812, W6300-EVB-Pico2"
    ),
    embassy_rp::binary_info::rp_cargo_version!(),
    embassy_rp::binary_info::rp_program_build_attribute!(),
];

/// Heap for `common::ui::fields`, which formats menu strings, and for the
/// Ratatui cell buffers behind the TFT.
///
/// Sized from a measurement, not a guess: a `ratatui_core` `Cell` is **40
/// bytes**, and `Terminal` keeps two 35x11 buffers (current and previous), so
/// the display alone pins **30.8 KB** of heap for the life of the firmware. The
/// old 32 KB left under 2 KB for every `format!` in a redraw and for
/// mousefood's own allocations — an out-of-memory panic at the first frame and
/// a 4 s watchdog reset loop with a dark panel. 64 KB gives ~33 KB of working
/// room; RAM is not the constraint on this chip (~190 KB of statics in 520 KB).
use embedded_alloc::LlffHeap as Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

const HEAP_SIZE: usize = 64 * 1024;

bind_interrupts!(struct Irqs {
    I2C1_IRQ => I2cInterruptHandler<I2C1>;
    // FT232RNL USB-DMX port (buffered UART, no DMA channels).
    UART0_IRQ => UartInterruptHandler<UART0>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    PIO1_IRQ_0 => PioInterruptHandler<PIO1>;
    PIO2_IRQ_0 => PioInterruptHandler<PIO2>;
    // Every DMA channel shares DMA_IRQ_0, so each one used needs its own
    // handler bound to it. embassy-rp 0.10 made the DMA irq binding an explicit
    // argument to the PIO drivers.
    DMA_IRQ_0 => DmaInterruptHandler<DMA_CH0>, DmaInterruptHandler<DMA_CH1>,
                 DmaInterruptHandler<DMA_CH2>, DmaInterruptHandler<DMA_CH3>,
                 DmaInterruptHandler<DMA_CH4>, DmaInterruptHandler<DMA_CH5>,
                 DmaInterruptHandler<DMA_CH6>, DmaInterruptHandler<DMA_CH7>,
                 DmaInterruptHandler<DMA_CH8>, DmaInterruptHandler<DMA_CH9>,
                 DmaInterruptHandler<DMA_CH10>, DmaInterruptHandler<DMA_CH11>;
});

/// Pin allocation — see the Altium netlist (`dmx_interface_dev_board_v2/`).
///
/// GP15–22 are consumed by the W6300 inside the module and cannot be reused.
/// GP23/24/25/29 are not on the header at all: the module keeps four GPIOs
/// internal (buck MD/SYNC, VBUS sense, user LED, VSYS/3 ADC). Every remaining
/// GPIO is spoken for — the last spare (GP13) and the former expander INT
/// (GP28) became the FT232RNL UART.
pub mod pins {
    /// WS2812 data out, one per string, GP0–GP7 (port 1 = GP0).
    pub const WS2812: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

    pub const DMX_RX: u8 = 8;
    pub const DMX_TX: u8 = 9;
    /// Driver enable / receiver enable, tied together. Must stay on a real
    /// GPIO — it switches per frame against PIO output timing.
    pub const DMX_DE: u8 = 10;

    /// ST7789 TFT on SPI1. `RES` and `CS` are on the I/O expander (P10/P11):
    /// both are static after init, so neither costs a GPIO. `BL` is the
    /// PCA9633's LED0.
    pub const TFT_MOSI: u8 = 11; // SPI1 TX
    pub const TFT_DC: u8 = 12; // plain GPIO, toggles per transaction
    pub const TFT_SCK: u8 = 14; // SPI1 SCK

    /// FT232RNL USB-DMX port on UART0 — the only UART0 pair reachable.
    /// `FTDI_RX` (GP13, UART0 RX) listens to the FT232RNL's TXD; `FTDI_TX`
    /// (GP28, UART0 TX) drives its RXD. GP28 used to be the expander INT.
    pub const FTDI_RX: u8 = 13;
    pub const FTDI_TX: u8 = 28;

    /// Shared I²C1: TCA9555 expander at 0x20, M24C02 EEPROM at 0x56,
    /// PCA9633 backlight driver at 0x62. The expander's INT goes to a test
    /// point only — buttons are polled (see `buttons.rs`).
    pub const I2C_SDA: u8 = 26;
    pub const I2C_SCL: u8 = 27;
}


/// SPI clock for the TFT.
///
/// Two separate ceilings, and the second is the one that bites:
///
/// * **Spec**: the ST7789's minimum serial *write* cycle is 66 ns — a 15 MHz
///   limit. (An earlier comment here claimed ~62 MHz, which is the *read*
///   figure misread, and the clock was set to 40 MHz.)
/// * **This board**: `DISPLAY.SCK` is measurably capacitive. Scoped on the
///   panel connector at 9.6 MHz it is not a square wave at all but a sine
///   swinging ~0.5 V to ~2.4 V, never reaching either rail, while MOSI beside
///   it is clean and full-swing — SCK is the only line toggling every bit, so
///   it loads first. A 58 % swing at a 52 ns half-period implies an RC time
///   constant near 20 ns on that net.
///
/// Note the Rev 1 board drove this same panel model at **100 MHz requested /
/// ~50 MHz actual** (`nucleo/src/main.rs`, SPI5) and worked, so the panel
/// itself is happy far above anything here — the 3 MHz figure elsewhere in
/// these notes was the WS2812 LED SPI, not the display. Clock rate is
/// therefore a signal-integrity workaround on this board, not a panel limit.
///
/// Sweep this if the waveform is not square: halve until SCK reaches the
/// rails, then leave margin. Cost is only the first full-screen redraw
/// (~110 KB); per-keypress redraws are a few hundred bytes.
const TFT_SPI_HZ: u32 = 5_000_000;

/// Incomplete boots in a row before the lockout guard skips Ethernet.
///
/// One incomplete boot is what a power blip during bring-up looks like, and
/// costing the network for that would leave an Art-Net install dark until the
/// *next* power cycle. Two in a row is the pattern of an Ethernet bring-up that
/// hangs — the failure the guard exists for.
const BOOT_FAIL_LIMIT: u8 = 2;

/// Shared I²C1 bus: the TCA9555 at 0x20, the M24C02 at 0x56, the PCA9633 at 0x62.
type I2cBus = Mutex<CriticalSectionRawMutex, I2c<'static, I2C1, i2c::Async>>;
type I2cDev = I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, I2C1, i2c::Async>>;

/// Display-reset handshake: signalled by the button task (which owns the
/// expander that drives the panel's RES and CS lines) and awaited by the UI
/// task before it sends the ST7789 init sequence.
static DISPLAY_READY: embassy_sync::signal::Signal<CriticalSectionRawMutex, ()> =
    embassy_sync::signal::Signal::new();

/// Concrete wrapper — embassy tasks cannot be generic, so the generic driver in
/// `buttons` is instantiated here.
#[embassy_executor::task]
async fn button_task(expander: tca9555::Tca9555<I2cDev>, tx: common::channels::RouterChannelTx) -> ! {
    buttons::run(expander, tx, &DISPLAY_READY).await
}

/// Heartbeat on the module's user LED, so a board that is alive but not
/// networking is distinguishable from one that is not running at all.
#[embassy_executor::task]
async fn heartbeat(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after_millis(50).await;
        led.set_low();
        Timer::after_millis(950).await;
    }
}

/// Core 1's stack. Task futures live in static task storage, not here, so this
/// only has to cover the executor poll loop and ordinary call depth — but that
/// depth includes a full Ratatui draw through mousefood and mipidsi, and unlike
/// core 0 (where flip-link turns an overflow into a HardFault) **nothing guards
/// this buffer**: an overflow silently corrupts whichever static sits below it.
/// 16 KB costs nothing here and doubles the margin.
static mut CORE1_STACK: CoreStack<16384> = CoreStack::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();

/// Boot orchestration on core 0 — the ladder the STM32 build ran from `main`.
///
/// 1. Load persisted state **through the EEPROM task**, so the results flow
///    to the router — which forwards settings to the UI and broadcasts the
///    operating mode to every input task. (Without this the router never sees
///    `BootStatus::Success` and deliberately renders nothing.)
/// 2. Count consecutive incomplete boots (EEPROM `0xA0`) and arm the boot flag
///    (write `Failed`): if this boot wedges before the `Success` write at the
///    end, the NEXT boot sees `Failed`. Two of those in a row and the network
///    is skipped — the lockout guard, softened from the STM32 build's one.
/// 3. Bring the network up, gated on the guard and on the `ethernet_enabled`
///    setting, reporting progress to the display.
/// 4. Record the successful boot, retried and confirmed via the router. If the
///    EEPROM will not confirm it, the local result is authoritative: a flaky
///    settings chip must not turn a working light into a dark one.
#[embassy_executor::task]
async fn boot_task(
    spawner: Spawner,
    mut ee: m24x02::M24x02<I2cDev>,
    spi_parts: w6300::SpiParts,
    int: Input<'static>,
    reset: Output<'static>,
) {
    use common::event_router::{MainEvent, ReturnChannel, RouterEvent};
    use common::events::{EepromEvent, NetStatus};
    use common::ui::BootStatus;
    use embassy_time::{with_timeout, Duration};

    // The carrier's 3V3 is enable-gated by the module's 3V3_OUT; let it settle
    // before the first EEPROM transaction.
    Timer::after_millis(20).await;

    channels::CHANNEL_EEPROM.send(EepromEvent::ReadBootStatus).await;
    Timer::after_millis(50).await;
    channels::CHANNEL_EEPROM.send(EepromEvent::ReadModuleType).await;
    Timer::after_millis(50).await;
    channels::CHANNEL_EEPROM.send(EepromEvent::ReadSettings).await;
    Timer::after_millis(50).await;

    let _ = channels::CHANNEL.try_send(RouterEvent::GetBootStatus(ReturnChannel::Main));
    let prev_boot = match with_timeout(Duration::from_millis(250), channels::CHANNEL_MAIN.receiver().receive()).await {
        Ok(MainEvent::ReturnBootStatus(Some(s))) => s,
        _ => BootStatus::Failed,
    };
    let _ = channels::CHANNEL.try_send(RouterEvent::GetSettings(ReturnChannel::Main));
    let settings = match with_timeout(Duration::from_millis(250), channels::CHANNEL_MAIN.receiver().receive()).await {
        Ok(MainEvent::ReturnSettings(s)) => s,
        _ => Default::default(),
    };
    info!("boot: previous boot {:?}", prev_boot);

    // Consecutive-failure counter. A blank byte (0xFF) counts as zero.
    let fails = if prev_boot == BootStatus::Success {
        0
    } else {
        let prev = ee.read_byte(eeprom::BOOT_FAIL_COUNT_ADDR).await.unwrap_or(0);
        if prev == 0xFF { 1 } else { prev.saturating_add(1) }
    };
    if fails > 0 {
        let _ = ee.write_byte_wait(eeprom::BOOT_FAIL_COUNT_ADDR, fails).await;
        warn!("boot: {} consecutive incomplete boot(s)", fails);
    }

    channels::CHANNEL_EEPROM.send(EepromEvent::WriteBootStatus(BootStatus::Failed)).await;
    Timer::after_millis(50).await;

    let net_status = if fails >= BOOT_FAIL_LIMIT {
        warn!("boot: Ethernet skipped this boot (lockout guard after {} incomplete boots)", fails);
        NetStatus::Guard
    } else if !settings.ethernet_enabled {
        info!("boot: Ethernet disabled in settings");
        NetStatus::Off
    } else {
        net_bringup(spawner, &mut ee, &settings, spi_parts, int, reset).await
    };
    let _ = channels::CHANNEL.try_send(RouterEvent::StoreNetStatus(net_status));

    // DMX -> LED rendering is gated on the router seeing Success; retry the
    // write and confirm it round-tripped (the EEPROM task echoes a successful
    // write as StoreBootStatus).
    for attempt in 1..=3u8 {
        channels::CHANNEL_EEPROM.send(EepromEvent::WriteBootStatus(BootStatus::Success)).await;
        Timer::after_millis(50).await;
        let _ = channels::CHANNEL.try_send(RouterEvent::GetBootStatus(ReturnChannel::Main));
        if let Ok(MainEvent::ReturnBootStatus(Some(BootStatus::Success))) =
            with_timeout(Duration::from_millis(250), channels::CHANNEL_MAIN.receiver().receive()).await
        {
            if fails > 0 {
                let _ = ee.write_byte_wait(eeprom::BOOT_FAIL_COUNT_ADDR, 0).await;
            }
            info!("boot: complete");
            return;
        }
        error!("boot: success flag write attempt {} not confirmed", attempt);
    }
    // The EEPROM is not answering, but this boot did complete. Treat the local
    // result as authoritative so output is not blocked by a dead settings chip;
    // the unwritten flag means the next boot reads Failed, which is the
    // documented guard behaviour.
    error!("boot: flag could not be stored - EEPROM unreachable? Continuing with output enabled.");
    let _ = channels::CHANNEL.try_send(RouterEvent::StoreBootStatus(Some(BootStatus::Success)));
}

/// Network bring-up: W6300 reset, MAC from the EEPROM, DHCP or the configured
/// static address, then the Art-Net and sACN receivers. Returns the status to
/// show on the display.
async fn net_bringup(
    spawner: Spawner,
    ee: &mut m24x02::M24x02<I2cDev>,
    settings: &common::ui::MenuData,
    spi_parts: w6300::SpiParts,
    int: Input<'static>,
    reset: Output<'static>,
) -> common::events::NetStatus {
    use common::event_router::RouterEvent;
    use common::events::NetStatus;
    use common::ui::EthernetIPMode;
    use embassy_net::{Ipv4Address, Ipv4Cidr, StaticConfigV4};

    let mac = eeprom::read_mac(ee).await.unwrap_or_else(|| {
        warn!("using fallback MAC - program the EEPROM (console: mac xx:xx:xx:xx:xx:xx) before shipping");
        identity::mac_fallback()
    });
    let _ = channels::CHANNEL.try_send(RouterEvent::StoreMacAddress(Some(mac)));

    static W6300_STATE: StaticCell<w6300::W6300State> = StaticCell::new();
    let Some((device, eth_runner)) =
        w6300::init(mac, W6300_STATE.init(w6300::W6300State::new()), spi_parts, int, reset).await
    else {
        // Deliberately not fatal: DMX, USB and the LEDs all still work without a
        // network, and the boot-flag / lockout logic depends on reaching this
        // point. The cause has already been logged specifically.
        warn!("continuing without Ethernet");
        return NetStatus::NoChip;
    };

    spawner.spawn(unwrap!(w6300::w6300_task(eth_runner)));

    // Static addressing per the Network page; `0.0.0.0` gateway means none
    // (an Art-Net rig on 2.x.x.x/8 has no router to speak of).
    let net_config = if settings.ethernet_ip_mode == EthernetIPMode::Static {
        let ip = settings.static_ip.octets();
        let gw = settings.static_gateway.octets();
        info!(
            "net: static {}.{}.{}.{}/{} gateway {}.{}.{}.{}",
            ip[0], ip[1], ip[2], ip[3], settings.static_prefix, gw[0], gw[1], gw[2], gw[3]
        );
        embassy_net::Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]), settings.static_prefix),
            gateway: (!settings.static_gateway.is_unspecified())
                .then(|| Ipv4Address::new(gw[0], gw[1], gw[2], gw[3])),
            dns_servers: heapless::Vec::new(),
        })
    } else {
        embassy_net::Config::dhcpv4(Default::default())
    };

    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let (stack, net_runner) = embassy_net::new(
        device,
        net_config,
        RESOURCES.init(StackResources::new()),
        RoscRng.next_u64(),
    );
    spawner.spawn(unwrap!(w6300::net_task(net_runner)));
    // Owns the reported address, and says *why* one never appeared.
    spawner.spawn(unwrap!(w6300::net_watch_task(stack)));
    spawner.spawn(unwrap!(artnet::artnet_task(
        stack,
        channels::CHANNEL_DMX.sender(),
        unwrap!(channels::CHANNEL_DMX_FEEDBACK.receiver()),
    )));
    spawner.spawn(unwrap!(sacn_rx::sacn_task(
        stack,
        channels::CHANNEL_DMX.sender(),
        channels::CHANNEL.sender(),
        unwrap!(channels::CHANNEL_DMX_FEEDBACK.receiver()),
    )));
    // The Art-Net task reports the address once the stack is configured
    // (immediately for static, after the lease for DHCP).
    NetStatus::Dhcp
}

#[cortex_m_rt::entry]
fn main() -> ! {
    // Before anything can allocate.
    {
        use core::mem::MaybeUninit;
        static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { HEAP.init(core::ptr::addr_of_mut!(HEAP_MEM) as usize, HEAP_SIZE) }
    }

    let p = embassy_rp::init(Default::default());
    info!("DMX interface starting on RP2350");

    // ---- Watchdog ---------------------------------------------------------
    // Started first so everything below is covered; fed by the supervisor
    // task once the executors run. Paused while a debugger halts the core.
    let mut wdt = Watchdog::new(p.WATCHDOG);
    match wdt.reset_reason() {
        Some(ResetReason::TimedOut) => warn!("previous reset: WATCHDOG TIMEOUT - a core stopped checking in"),
        Some(ResetReason::Forced) => warn!("previous reset: forced by software"),
        None => {}
    }
    wdt.pause_on_debug(true);
    wdt.start(supervisor::WATCHDOG_TIMEOUT);

    // Everything is constructed here, on core 0, and then moved into whichever
    // executor owns it. Peripherals cannot be split after the cores start.

    // ---- I2C1: EEPROM + expander + backlight, shared across both cores ----
    // The mutex is CriticalSectionRawMutex precisely because core 0 (EEPROM)
    // and core 1 (expander, backlight) both reach this bus.
    let mut i2c_cfg = i2c::Config::default();
    i2c_cfg.frequency = 400_000;
    let i2c = I2c::new_async(p.I2C1, p.PIN_27, p.PIN_26, Irqs, i2c_cfg);
    static I2C_BUS: StaticCell<I2cBus> = StaticCell::new();
    let i2c_bus = I2C_BUS.init(Mutex::new(i2c));

    let eeprom_dev = m24x02::M24x02::new(I2cDevice::new(i2c_bus), eeprom::ADDR);
    let expander = tca9555::Tca9555::new(I2cDevice::new(i2c_bus), tca9555::ADDR);
    let backlight = pca9633::Pca9633::new(I2cDevice::new(i2c_bus), pca9633::ADDR);

    // ---- UART0: FT232RNL USB-DMX port -------------------------------------
    // Buffered (interrupt-driven ring buffers) rather than DMA: the Enttec
    // stream is byte-framed, reads must return whatever has arrived, and the
    // baud gets changed at runtime while hunting. 8N2 so both 8N1 and 8N2 hosts
    // read our replies cleanly; the initial baud is the first hunt candidate.
    static UART_TX_BUF: StaticCell<[u8; 1024]> = StaticCell::new();
    static UART_RX_BUF: StaticCell<[u8; 2048]> = StaticCell::new();
    let mut uart_cfg = uart::Config::default();
    uart_cfg.baudrate = enttec_uart::BAUD_CANDIDATES[0];
    uart_cfg.stop_bits = uart::StopBits::STOP2;
    let ftdi_uart = BufferedUart::new(
        p.UART0,
        p.PIN_28, // TX -> FT232RNL RXD
        p.PIN_13, // RX <- FT232RNL TXD
        Irqs,
        UART_TX_BUF.init([0; 1024]),
        UART_RX_BUF.init([0; 2048]),
        uart_cfg,
    );

    // ---- PIO -------------------------------------------------------------
    // PIO0 SM0 is the W6300 transport; PIO2 SM0/SM1 are wired DMX. Everything
    // else is WS2812. See smart_led.rs for the allocation table.
    let Pio { common: mut pio0, sm0: p0sm0, sm1: p0sm1, sm2: p0sm2, sm3: p0sm3, .. } =
        Pio::new(p.PIO0, Irqs);
    let Pio { common: mut pio1, sm0: p1sm0, sm1: p1sm1, sm2: p1sm2, sm3: p1sm3, .. } =
        Pio::new(p.PIO1, Irqs);
    let Pio { common: mut pio2, sm0: p2sm0, sm1: p2sm1, sm2: p2sm2, .. } =
        Pio::new(p.PIO2, Irqs);

    // One copy of the 4-instruction WS2812 program per block, shared by that
    // block's state machines. The driver packs 24- or 32-bit pixels per frame,
    // so RGB/RGBW is a menu setting, not a build option.
    let ws_p0 = Ws2812Program::new(&mut pio0);
    let ws_p1 = Ws2812Program::new(&mut pio1);
    let ws_p2 = Ws2812Program::new(&mut pio2);

    let outputs = Ws2812Outputs {
        s1: Ws2812::new(&mut pio0, p0sm1, p.DMA_CH0, Irqs, p.PIN_0, &ws_p0),
        s2: Ws2812::new(&mut pio0, p0sm2, p.DMA_CH1, Irqs, p.PIN_1, &ws_p0),
        s3: Ws2812::new(&mut pio0, p0sm3, p.DMA_CH2, Irqs, p.PIN_2, &ws_p0),
        s4: Ws2812::new(&mut pio1, p1sm0, p.DMA_CH3, Irqs, p.PIN_3, &ws_p1),
        s5: Ws2812::new(&mut pio1, p1sm1, p.DMA_CH4, Irqs, p.PIN_4, &ws_p1),
        s6: Ws2812::new(&mut pio1, p1sm2, p.DMA_CH5, Irqs, p.PIN_5, &ws_p1),
        s7: Ws2812::new(&mut pio1, p1sm3, p.DMA_CH6, Irqs, p.PIN_6, &ws_p1),
        s8: Ws2812::new(&mut pio2, p2sm2, p.DMA_CH7, Irqs, p.PIN_7, &ws_p2),
    };

    // ---- Wired DMX on PIO2 ------------------------------------------------
    let dmx_rx_prog = dmx_pio::PioDmxRxProgram::new(&mut pio2);
    let dmx_rx = dmx_pio::PioDmxRx::new(&mut pio2, p2sm0, p.DMA_CH10, Irqs, p.PIN_8, &dmx_rx_prog);
    let dmx_tx_prog = dmx_pio::PioDmxTxProgram::new(&mut pio2);
    let dmx_tx = dmx_pio::PioDmxTx::new(&mut pio2, p2sm1, p.DMA_CH11, Irqs, p.PIN_9, &dmx_tx_prog);

    // ---- W6300 transport on PIO0 SM0 -------------------------------------
    // SCLK is on GP17, a CSn mux position, so hardware SPI0 cannot reach it.
    let mut spi_cfg = SpiConfig::default();
    spi_cfg.frequency = w6300::SPI_FREQ_HZ;
    let spi_bus = PioSpi::new(
        &mut pio0, p0sm0, p.PIN_17, // SCLK
        p.PIN_18,                   // IO0 / MOSI
        p.PIN_19,                   // IO1 / MISO
        p.DMA_CH8, p.DMA_CH9, Irqs, spi_cfg,
    );
    // Sample MISO without the two-flop input synchroniser. See
    // `w6300::PROBE_FREQS_HZ`: those two `clk_sys` cycles (~13 ns) are pure
    // lag on the read path and were costing most of the usable clock rate.
    //
    // Bypassing is safe *here* specifically because MISO is not an
    // asynchronous input: the W6300 clocks it out from the SCK this very state
    // machine generates, so its transitions are causally tied to our own clock
    // rather than free-running. The sample then lands mid-bit, roughly 25 ns
    // clear of the transition on either side at 20 MHz, which is nowhere near
    // a setup window. Only GP19 is bypassed — the WS2812 machines sharing PIO0
    // are output-only and must keep their defaults.
    pio0.set_input_sync_bypass(1 << 19, 1 << 19);
    // Bus and CS stay apart: `w6300::init` probes for a usable clock before it
    // joins them into the `SpiDevice` the driver takes ownership of.
    let spi_parts = w6300::SpiParts { bus: spi_bus, cs: Output::new(p.PIN_16, Level::High) };

    // ---- TFT on SPI1 --------------------------------------------------------
    // Blocking (mipidsi). CS and RES are static lines on the expander; the UI
    // task builds the display once the button task has released them.
    let mut tft_cfg = SpiConfig::default();
    tft_cfg.frequency = TFT_SPI_HZ;
    let tft_spi = HwSpi::new_blocking_txonly(p.SPI1, p.PIN_14, p.PIN_11, tft_cfg);
    let tft_dc = Output::new(p.PIN_12, Level::Low);

    // ---- VSYS monitor (module ADC3 = VSYS/3) ------------------------------
    let adc = Adc::new_blocking(p.ADC, adc::Config::default());
    let vsys = adc::Channel::new_pin(p.PIN_29, Pull::None);

    // ---- Core 1: UI, buttons, DMX, LED render ----------------------------
    // Everything here is periodic or human-paced. Keeping it off core 0 is what
    // stops a console broadcasting universes we do not want from making the
    // panel laggy — the claim the whole split exists to make good on.
    spawn_core1(
        p.CORE1,
        unsafe { &mut *core::ptr::addr_of_mut!(CORE1_STACK) },
        move || {
            let executor1 = EXECUTOR1.init(Executor::new());
            executor1.run(|s| {
                s.spawn(unwrap!(button_task(expander, channels::CHANNEL.sender())));
                s.spawn(unwrap!(dmx::dmx_task(
                    dmx_rx,
                    dmx_tx,
                    Output::new(p.PIN_10, Level::Low), // DE//RE via Q1, low = receive
                    unwrap!(channels::CHANNEL_DMX_FEEDBACK.receiver()),
                )));
                s.spawn(unwrap!(smart_led::smart_led_task(
                    outputs,
                    channels::CHANNEL_SMART_LED.receiver(),
                )));
                s.spawn(unwrap!(tft_ui::ui_task(
                    tft_spi,
                    tft_dc,
                    backlight,
                    channels::CHANNEL_UI.receiver(),
                    channels::CHANNEL.sender(),
                    &DISPLAY_READY,
                )));
            });
        },
    );

    // ---- Core 0: network, USB, EEPROM, the event router, the watchdog -----
    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|s| {
        s.spawn(unwrap!(heartbeat(Output::new(p.PIN_25, Level::Low))));
        s.spawn(unwrap!(supervisor::watchdog_task(wdt)));
        s.spawn(unwrap!(supervisor::vsys_monitor(adc, vsys)));

        // The hub: routes settings, button and EEPROM events, and maps
        // DMX_BUFFER into LED_COLORS on every packet before pinging the LED
        // task on core 1.
        s.spawn(unwrap!(event_router::event_router(event_router::Router::new(
            channels::CHANNEL.receiver(),
            channels::CHANNEL_DMX.receiver(),
            channels::CHANNEL_DMX_FEEDBACK.sender(),
            channels::CHANNEL_SMART_LED.sender(),
            channels::CHANNEL_UI.sender(),
            channels::CHANNEL_EEPROM.sender(),
            channels::CHANNEL_MAIN.sender(),
        ))));

        // USB composite device on the module's USB-C: the Enttec DMX USB Pro
        // widget plus the console line protocol `dmx_console` talks to. An
        // input like the network, so it sits alongside it rather than
        // competing with the UI on core 1.
        s.spawn(unwrap!(usb_device::usb_device_task(
            UsbDriver::new(p.USB, Irqs),
            channels::CHANNEL_DMX.sender(),
            unwrap!(channels::CHANNEL_DMX_FEEDBACK.receiver()),
            channels::CHANNEL.sender(),
            unwrap!(channels::CHANNEL_LOG.receiver()),
            // Per-unit serial from the OTP chip ID - see `identity.rs`.
            identity::usb_serial(),
        )));

        // The same Enttec widget on the carrier's FT232RNL USB-C port — the one
        // FTDI-stack software (QLC+) recognises as a genuine Pro. Another input,
        // so it lives on core 0 with USB and the network.
        s.spawn(unwrap!(enttec_uart::enttec_uart_task(
            ftdi_uart,
            channels::CHANNEL_DMX.sender(),
            unwrap!(channels::CHANNEL_DMX_FEEDBACK.receiver()),
        )));

        // Settings persistence. Shares I2C1 with the expander on core 1 — which
        // is what the CriticalSectionRawMutex on that bus is for.
        s.spawn(unwrap!(eeprom::eeprom_task(
            I2cDevice::new(i2c_bus),
            eeprom::ADDR,
            channels::CHANNEL_EEPROM.receiver(),
            channels::CHANNEL.sender(),
        )));

        s.spawn(unwrap!(boot_task(
            s,
            eeprom_dev,
            spi_parts,
            Input::new(p.PIN_15, Pull::Up), // W6300 INT
            Output::new(p.PIN_22, Level::High), // W6300 RSTn
        )));
    });

}
