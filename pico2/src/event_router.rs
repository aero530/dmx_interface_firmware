//! Executor glue for the event router.
//!
//! The router itself — `Router` and all its routing logic — is target-agnostic
//! and lives in `common::event_router`. Only the embassy task wrapper is here,
//! because `#[embassy_executor::task]` ties it to this crate's executor.
//!
//! # Rendering on a timer, not per packet
//!
//! The router's DMX handler rebuilds the whole LED map: every port, every
//! virtual LED, holding `LED_COLORS` and `DMX_BUFFER`, then wakes the WS2812
//! task on core 1 to push all eight strings. Doing that once per *packet* was
//! the Art-Net ingest ceiling, measured 2026-09-16: with the render running a
//! frame cost 1.227 ms against 0.608 ms without, and the W6300 driver's wait
//! after each payload DMA went from 43 µs to 434 µs — it was queued behind a
//! rebuild every time. 866 packets/s in Art-Net mode against 1342 in DMX mode,
//! the difference being 0.62 ms of render per packet (`docs/ARCHITECTURE.md`
//! §11).
//!
//! The strips do not need 1408 rebuilds a second; at 44 Hz input they need 44.
//! So a DMX event no longer renders. It is kept as the latest pending event and
//! a ticker renders at most once per [`RENDER_PERIOD`] if anything arrived.
//! That is safe to coalesce because the rebuild reads the *current*
//! `DMX_BUFFER`, which the Art-Net and DMX tasks have already filled from every
//! packet — the event only says "something changed". Other-Net traffic never
//! reaches here (the Art-Net task filters before storing), so keeping only the
//! latest event loses nothing.
//!
//! Trailing edge on purpose: a 32-universe burst lands in ~1.5 ms and is
//! rendered whole on the next tick, rather than 32 partial renders. The cost is
//! up to one period of added latency from first packet to LEDs, which at 16 ms
//! is under one Art-Net frame.
//!
//! This lives here and not in `common` because it is executor policy, not
//! routing logic: `common` stays free of `embassy_time`.

pub use common::event_router::*;

use embassy_futures::select::{Either3, select3};
use embassy_time::{Duration, Ticker};

/// How often the LED map may be rebuilt.
///
/// 16 ms is 62.5 Hz: above the 44 Hz Art-Net/sACN input rate, so every input
/// frame is rendered (a tick shorter than the input period never skips one),
/// and above the 56 Hz ceiling a 600-LED WS2812 string can be clocked at, so
/// the strips are never starved. Shorter buys latency at the price of more
/// rebuilds under a flood; at this period a saturated node spends ~4 % of core
/// 0 rendering instead of ~50 %.
pub const RENDER_PERIOD: Duration = Duration::from_millis(16);

#[embassy_executor::task]
pub async fn event_router(mut router: Router) {
    let mut ticker = Ticker::every(RENDER_PERIOD);
    // The most recent DMX event since the last render, if any. `DmxEvent` is
    // not `Clone`, and does not need to be: it is moved in and taken out.
    let mut pending: Option<DmxEvent> = None;
    loop {
        // Event-driven: wake on whichever channel has data, or on the tick.
        match select3(router.channel.receive(), router.channel_dmx.receive(), ticker.next()).await {
            Either3::First(message) => router.process_router_event(message).await,
            // Note the arrival; the render waits for the tick.
            Either3::Second(message) => pending = Some(message),
            Either3::Third(()) => {
                if let Some(message) = pending.take() {
                    router.process_dmx_event(message).await;
                }
            }
        }
    }
}
