//! Shared data buffers.
//!
//! The two buffers every input writes into and every output renders from.
//! They are plain `embassy-sync` statics with no HAL involvement, so they live
//! here rather than beside the peripheral setup.
//!
//! # Why `CriticalSectionRawMutex`
//!
//! The STM32 build previously used `ThreadModeRawMutex`, which elides real
//! locking by assuming every accessor runs in thread mode on a single core.
//! Two things make that the wrong choice here:
//!
//! 1. It is `cortex-m`-only, so it cannot appear in a crate that also compiles
//!    for the host — which is how `host_tests` exercises this code.
//! 2. It is **unsound across two cores**. The RP2350 plan puts the network
//!    stack and router on core 0 and the LED render and UI on core 1 — a split
//!    that runs straight through both of these buffers.
//!
//! `CriticalSectionRawMutex` is correct on one core or two, and costs a masked
//! interrupt per lock rather than nothing. At the ~44 Hz these buffers turn
//! over, that is not measurable.
//!
//! Consumers must link a `critical-section` implementation — and on a
//! multi-core target it must be a CROSS-CORE one. The pico2 build gets the
//! hardware-spinlock implementation from embassy-rp's `critical-section-impl`
//! feature (single-core `cortex-m` impls would be unsound there); a
//! single-core target would use `cortex-m/critical-section-single-core`; the
//! host uses `critical-section/std`.

use core::cell::Cell;

use embassy_sync::blocking_mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use smart_leds::{White, RGBW};

use crate::ui::fixtures::FixtureMap;
use crate::{DMX_UNIVERSE_COUNT, DMX_UNIVERSE_SIZE, SMARTLED_NUM_LEDS_MAX, SMARTLED_PORT_COUNT};

/// Raw mutex backing the shared buffers. See the module docs.
pub type BufferRawMutex = CriticalSectionRawMutex;

pub type LedBuffer = Mutex<BufferRawMutex, [[RGBW<u8>; SMARTLED_NUM_LEDS_MAX]; SMARTLED_PORT_COUNT]>;

const OFF: RGBW<u8> = RGBW::<u8> { r: 0, g: 0, b: 0, a: White(0) };

/// Buffer of current LED colors. Always RGBW: in RGB mode the white byte is
/// zero and the output driver does not transmit it.
pub static LED_COLORS: LedBuffer = Mutex::new([[OFF; SMARTLED_NUM_LEDS_MAX]; SMARTLED_PORT_COUNT]);

pub type DmxBuffer = Mutex<BufferRawMutex, [u8; DMX_UNIVERSE_COUNT * DMX_UNIVERSE_SIZE]>;

/// Buffer to hold incoming data (DMX or ArtNet)
///
/// Data is stored flat, `DMX_UNIVERSE_COUNT` (64) universes of the configured
/// Art-Net net: location = sub_uni * 512 + channel offset, where sub_uni is the
/// Port-Address "SubUni" byte (sub-net in the high nibble, universe in the low
/// nibble), bounds-checked by the receive tasks. sACN is rebased so its
/// configured base universe is slot 0. Wired DMX / USB use offset 0 with the
/// start code at index 0 — see `event_router::buffer_index` for how the two
/// layouts are reconciled.
pub static DMX_BUFFER: DmxBuffer = Mutex::new([0_u8; DMX_UNIVERSE_COUNT * DMX_UNIVERSE_SIZE]);

/// The per-port fixture map.
///
/// A **blocking** mutex around a `Cell`, unlike the two buffers above, and
/// deliberately so. The render already holds `DMX_BUFFER` and `LED_COLORS`
/// across awaits; adding a third async lock would create an
/// `LED_COLORS -> DMX_BUFFER -> FIXTURE_MAP` ordering that nothing enforces and
/// that a future code path could take the other way round — a cross-core
/// deadlock at 44 Hz, found on a rig rather than on the bench.
///
/// This one is never held across an await. The render takes a **snapshot**
/// (`FIXTURE_MAP.lock(|m| m.get())`, 80 bytes) before it locks anything else,
/// which also buys frame atomicity for free: a layout edited mid-frame cannot
/// paint half of one arrangement and half of another. The UI writes it without
/// ever waiting on the render.
pub static FIXTURE_MAP: blocking_mutex::Mutex<BufferRawMutex, Cell<FixtureMap>> =
    blocking_mutex::Mutex::new(Cell::new(FixtureMap::EMPTY));

/// The current map, copied out. Cheap, and the only way the render should read
/// it — see the note on lock ordering above.
pub fn fixture_map() -> FixtureMap {
    FIXTURE_MAP.lock(|m| m.get())
}

/// Replace the map. The caller is responsible for persisting it.
pub fn set_fixture_map(map: FixtureMap) {
    FIXTURE_MAP.lock(|m| m.set(map));
}
