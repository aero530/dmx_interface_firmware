//! Ingest counters, so the throughput measurement is a measurement.
//!
//! A sender reports what it *sent*. Without the other half, "flood it and see
//! if anything looks wrong" is the whole test — which finds a link that has
//! fallen over, and misses one quietly dropping a few percent of frames. These
//! counters are the node's own account of what arrived, and the `stats` console
//! command turns them into a rate.
//!
//! Deliberately plain `AtomicU32` with `Relaxed` ordering: they are counters
//! read by a human, not a protocol, and the receive path runs at up to a few
//! thousand packets a second on core 0. Nothing here may cost more than an
//! increment. At 44 Hz × 32 universes a `u32` wraps after about 35 days of
//! continuous flooding, which is well past the point of usefulness.

use core::sync::atomic::{AtomicU32, Ordering};

/// Datagrams taken off the Art-Net socket, whatever they turned out to be.
static ARTNET_RX: AtomicU32 = AtomicU32::new(0);
/// ArtDmx packets whose data reached `DMX_BUFFER`.
static ARTNET_STORED: AtomicU32 = AtomicU32::new(0);
/// Parsed fine, deliberately not rendered: another Net, a universe past the
/// buffer, or a mode that is not listening to Art-Net.
static ARTNET_IGNORED: AtomicU32 = AtomicU32::new(0);
/// Did not parse as Art-Net at all.
static ARTNET_MALFORMED: AtomicU32 = AtomicU32::new(0);
/// `recv_from` itself failed. The one counter that should stay at zero.
static ARTNET_ERRORS: AtomicU32 = AtomicU32::new(0);
/// sACN packets whose data reached `DMX_BUFFER`.
static SACN_STORED: AtomicU32 = AtomicU32::new(0);

macro_rules! counter {
    ($bump:ident, $get:ident, $cell:ident) => {
        pub fn $bump() {
            $cell.fetch_add(1, Ordering::Relaxed);
        }
        pub fn $get() -> u32 {
            $cell.load(Ordering::Relaxed)
        }
    };
}

counter!(artnet_rx, artnet_rx_count, ARTNET_RX);
counter!(artnet_stored, artnet_stored_count, ARTNET_STORED);
counter!(artnet_ignored, artnet_ignored_count, ARTNET_IGNORED);
counter!(artnet_malformed, artnet_malformed_count, ARTNET_MALFORMED);
counter!(artnet_error, artnet_error_count, ARTNET_ERRORS);
counter!(sacn_stored, sacn_stored_count, SACN_STORED);

/// Every counter at once, for the console.
pub struct Counts {
    pub rx: u32,
    pub stored: u32,
    pub ignored: u32,
    pub malformed: u32,
    pub errors: u32,
    pub sacn: u32,
}

pub fn counts() -> Counts {
    Counts {
        rx: artnet_rx_count(),
        stored: artnet_stored_count(),
        ignored: artnet_ignored_count(),
        malformed: artnet_malformed_count(),
        errors: artnet_error_count(),
        sacn: sacn_stored_count(),
    }
}
