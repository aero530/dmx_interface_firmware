//! sACN / E1.31 receiver.
//!
//! Runs alongside the Art-Net task and writes into the same `DMX_BUFFER`, so
//! the router and everything downstream are unchanged.
//!
//! # Why multicast is the point
//!
//! Art-Net's ingest problem is that broadcast traffic arrives whether it is
//! wanted or not — a console pushing 32 universes costs SPI bandwidth even if
//! only four are ours. sACN puts each universe in its own multicast group, so
//! joining only the ones in use lets an **IGMP-snooping switch drop the rest
//! upstream**. They never reach the W6300 at all. That is a better answer than
//! unicast Art-Net, which still has to arrive before it can be discarded.
//!
//! # Universe numbering
//!
//! sACN universes are 1-based and unrelated to Art-Net Port-Addresses, so the
//! node has its own `sacn_universe` setting: universe *base* lands in buffer
//! slot 0, *base + 1* in slot 1, and so on for the `DMX_UNIVERSE_COUNT` slots
//! the buffer holds. The router renders from slot 0 in `sACN` mode, so the
//! LED page's universe offsets read directly as "universes above the base" —
//! the same mental model as Art-Net, just with the base spelled as one number.
//!
//! Only the groups for that window are joined, and they are re-joined when the
//! base changes. Joining every possible group would defeat the purpose, since
//! the switch would then forward everything.

use common::channels::{DmxChannelTx, DmxFeedbackChannelRx, RouterChannelTx};
use common::event_router::{DmxEvent, DmxFeedbackEvent, PacketAddress};
use common::artnet::PortAddress;
use common::sacn::{multicast_group, parse, PORT};
use common::ui::InputMode;
use common::{DMX_BUFFER, DMX_UNIVERSE_COUNT, DMX_UNIVERSE_SIZE};
use defmt::*;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, Stack};

/// Universes joined above the base. Eight ports of 1800 B is 32 universes;
/// the buffer holds 64, so a window of 32 covers any legal configuration.
const JOIN_COUNT: u16 = 32;

fn group(universe: u16) -> IpAddress {
    let g = multicast_group(universe);
    IpAddress::v4(g[0], g[1], g[2], g[3])
}

/// Move the multicast window from `old` to `new`. Joining is what tells an
/// IGMP-snooping switch to start forwarding these groups — and, just as
/// importantly, leaving is what stops the old ones.
fn rejoin(stack: &Stack<'static>, old: Option<u16>, new: u16) {
    if let Some(old) = old {
        for u in old..old.saturating_add(JOIN_COUNT) {
            let _ = stack.leave_multicast_group(group(u));
        }
    }
    let mut joined = 0u16;
    for u in new..new.saturating_add(JOIN_COUNT) {
        if stack.join_multicast_group(group(u)).is_ok() {
            joined += 1;
        }
    }
    if joined < JOIN_COUNT {
        // smoltcp's group table is a compile-time size (see the smoltcp line
        // in Cargo.toml). Universes past the joined count still work on a
        // switch that floods multicast, and silently do not on one that snoops.
        warn!(
            "sACN: only {} of {} multicast groups joined - raise smoltcp's iface-max-multicast-group-count",
            joined, JOIN_COUNT
        );
    }
    info!("sACN: universes {}..{} - joined {} groups", new, new.saturating_add(JOIN_COUNT - 1), joined);
}

#[embassy_executor::task]
pub async fn sacn_task(
    stack: Stack<'static>,
    tx: DmxChannelTx,
    _tx_router: RouterChannelTx,
    mut rx: DmxFeedbackChannelRx,
) {
    stack.wait_config_up().await;

    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut rx_buf = [0u8; 1536];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_buf = [0u8; 64];
    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buf,
        &mut tx_meta,
        &mut tx_buf,
    );

    if socket.bind(PORT).is_err() {
        error!("sACN: could not bind port {}", PORT);
        return;
    }

    // The router has normally broadcast the mode before the network is up;
    // start from it rather than waiting for the next change.
    let mut input_mode = InputMode::default();
    let mut base: u16 = 1;
    if let Some(DmxFeedbackEvent::Mode(m, _, b, _)) = rx.try_get() {
        input_mode = m;
        base = b;
    }
    rejoin(&stack, None, base);
    let mut joined_base = base;

    let mut buf = [0u8; 1536];
    let mut warned_out_of_range = false;

    loop {
        if let Some(DmxFeedbackEvent::Mode(m, _, b, _)) = rx.try_changed() {
            input_mode = m;
            base = b;
            if base != joined_base {
                rejoin(&stack, Some(joined_base), base);
                joined_base = base;
                warned_out_of_range = false;
            }
        }

        let Ok((n, _from)) = socket.recv_from(&mut buf).await else {
            continue;
        };

        // Same guard as the Art-Net path: only store when sACN is the selected
        // source, or stray traffic overwrites the active input's data.
        if input_mode != InputMode::Sacn {
            continue;
        }

        let Some(packet) = parse(&buf[..n]) else {
            continue;
        };
        // Preview data is explicitly not for live output; a terminated stream
        // is an announcement, not channel values.
        if packet.preview || packet.terminated {
            continue;
        }

        // Rebase onto the buffer: universe `base` is slot 0. Anything below
        // the base or past the buffer is not ours to render — and the number
        // came off the wire, so it is bounds-checked, not trusted.
        let Some(slot) = packet.universe.checked_sub(base) else {
            continue;
        };
        if slot as usize >= DMX_UNIVERSE_COUNT {
            if !warned_out_of_range {
                warn!(
                    "sACN: ignoring universe {} - beyond the {} buffered above universe {}",
                    packet.universe, DMX_UNIVERSE_COUNT, base
                );
                warned_out_of_range = true;
            }
            continue;
        }

        let start = slot as usize * DMX_UNIVERSE_SIZE;
        let len = packet.values.len().min(DMX_UNIVERSE_SIZE);
        {
            let mut dmx_buffer = DMX_BUFFER.lock().await;
            dmx_buffer[start..start + len].copy_from_slice(&packet.values[..len]);
        }

        crate::netstats::sacn_stored();
        // Reported as a Port-Address with Net 0; the router skips the Net
        // filter in sACN mode.
        let _ = tx.try_send(DmxEvent::ArtNetPacket(PacketAddress::new(
            PortAddress::new(0, 0, slot as u8),
            packet.sequence,
        )));
    }
}
