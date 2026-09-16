//! Art-Net receiver
//!
//! Moved from the STM32 build unchanged apart from dropping a vestigial
//! `net_task`/`Device` alias that named the STM32 MAC. The task itself only
//! ever touched `embassy_net::Stack`, so it is transport-agnostic — which is
//! the whole reason MACRAW was chosen over the W6300's hardwired socket API.
//!
//! Get DMX data over ethernet

#![allow(unused)]
use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{error, info, warn, debug};
    } else {
        use defmt::{error, info, warn, debug};
    }
}
use embassy_futures::yield_now;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::Stack;

use crate::channels::{DmxChannelTx, DmxFeedbackChannelRx, RouterChannelTx};
use crate::event_router::{DmxEvent, DmxFeedbackEvent, PacketAddress, RouterEvent};
use crate::ui::{ArtNetAddr, InputMode};
use crate::{ARTNET_OEM, DMX_BUFFER, DMX_UNIVERSE_COUNT, DMX_UNIVERSE_SIZE};
use common::events::NetStatus;

// Parser lives in `common`; this module keeps the socket-facing task.
pub use common::artnet::tiny_artnet;
pub use tiny_artnet::{Art, PortAddress};

/// Pull DMX data from ArtNet via ethernet.
///
/// This only runs if the mode is set to ArtNet from the main menu.
#[embassy_executor::task]
pub async fn artnet_task(
    stack: Stack<'static>,
    // runner: Runner<'static, Ethernet<'static, ETH, GenericPhy>>,
    // spawner: Spawner,
    tx: DmxChannelTx,
    mut rx: DmxFeedbackChannelRx,
) {
    // Ensure DHCP configuration is up before trying to bind.
    //
    // This is the *only* task that may wait here: `embassy-net` keeps one
    // waker slot for stack-state changes, so a second waiter would displace
    // this one and neither side would report an error. Anything else that
    // needs to know about the link polls instead — see `w6300::net_watch_task`,
    // which also owns reporting the address.
    info!("Waiting for DHCP...");
    let _a = stack.wait_config_up().await;

    let cfg = stack.config_v4().unwrap();

    let local_addr = cfg.address.address();
    info!(" ");
    info!("IP address: {:?}", local_addr);
    info!(" ");

    // Lookup the mac address -- assumed to be the first 6 bytes of the micro UID
    // let uid = embassy_stm32::uid::uid();
    // let mac_address_bytes = [uid[0], uid[1], uid[2], uid[3], uid[4], uid[5]];
    let hw_addr = stack.hardware_address();
    let mut mac_address_bytes = [0; 6];
    match hw_addr {
        embassy_net::HardwareAddress::Ethernet(address) => {
            for (i, b) in address.as_bytes().iter().enumerate() {
                mac_address_bytes[i] = *b;
            }
        }
    }
    // let mac_address_bytes = [mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]];

    // Then we can use it!
    // 4 KB is enough here, and it was measured rather than assumed: raising it
    // to 32 KB on 2026-09-16 changed ingest by 0 %. The burst absorber is the
    // W6300's own RX memory, not this buffer - by the time packets reach the
    // socket they arrive at the SPI drain rate, one every ~1-2 ms, and this task
    // pulls them faster than that. See ARCHITECTURE.md section 11.
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];
    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut tx_meta = [PacketMetadata::EMPTY; 16];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buffer, &mut tx_meta, &mut tx_buffer);

    let port = tiny_artnet::PORT;
    socket.bind(port).unwrap();

    // One Ethernet frame is plenty: an ArtDmx packet is ~530 B + headers, and
    // smoltcp does not reassemble IP fragments, so nothing above the MTU can
    // arrive in one piece anyway. (This was 65_507 — the UDP maximum — which
    // spent 64 KB of RAM, an eighth of the chip, on impossible packets.)
    let mut buf = [0; 1536];
    let mut input_mode = InputMode::default();
    // Configured Art-Net address (net, sub-net, universe), advertised in ArtPollReply
    let mut artnet_addr = ArtNetAddr::default();
    // Universes the configuration binds from that address (`MenuData::bound_universes`)
    let mut bound: u16 = 1;

    // Latches so a controller blasting out-of-range universes, or traffic on
    // another Net, logs once rather than once per packet at 44 Hz.
    let mut warned_out_of_range = false;
    let mut warned_other_net = false;

    loop {
        // Try to update current mode
        if let Some(input_data) = rx.try_changed() {
            // info!("ArtNet - update mode to {}", input_data);
            match input_data {
                DmxFeedbackEvent::Mode(new_mode, new_addr, _sacn_base, new_bound) => {
                    input_mode = new_mode;
                    artnet_addr = new_addr;
                    bound = new_bound;
                    // A new address or mode is a new situation: let both
                    // one-shot warnings fire again if it is still wrong.
                    warned_other_net = false;
                    warned_out_of_range = false;
                }
            }
        }

        let (len, from_addr) = match socket.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) => {
                error!("ArtNet socket receive error {:?}", e);
                crate::netstats::artnet_error();
                continue;
            }
        };
        crate::netstats::artnet_rx();
        // Fix 4 (docs/ARCHITECTURE.md section 11): hand the core back before
        // touching the packet. When this task wakes, the W6300 driver has
        // usually just finished a payload DMA and its wake is queued behind us;
        // measured 2026-09-16, it waited 263 us per frame for the parse-lock-
        // copy path below to run to completion. Yielding here lets that queued
        // wake run first, splitting our per-packet work into two short pieces
        // instead of one long one. `buf`, `len` and `from_addr` are ours, so
        // nothing changes under us while we are away; the cost is one extra
        // poll per packet, a microsecond or two at 1300 packets a second.
        yield_now().await;
        // trace!("Ethernet {:?}", buf);

        match tiny_artnet::from_slice(&buf[..len]) {
            Ok(Art::Dmx(dmx)) => {
                // info!(
                //     "RX: ArtDMX - These packets contain data for one DMX512 universe Seq: {:?} physical: {:?} port_address: {:?} Data: {:?}...",
                //     dmx.sequence,
                //     dmx.physical,
                //     dmx.port_address,
                //     &dmx.data[0..10],
                // );

                // Only store data when ArtNet is the active input, otherwise stray
                // network packets overwrite the wired-DMX / USB / sACN data.
                if !input_mode.is_artnet() {
                    // Arrived, deliberately not rendered: a mode change is a
                    // reason to ignore traffic, not a reason to lose count of it.
                    crate::netstats::artnet_ignored();
                }
                if input_mode.is_artnet() {
                    // Filter on Net *before* touching the buffer. The buffer is
                    // indexed by SubUni only, so a packet on another Net with a
                    // matching sub-net:universe would land on top of ours.
                    if dmx.port_address.net != artnet_addr.0[0] {
                        if !warned_other_net {
                            warned_other_net = true;
                            warn!(
                                "ArtNet: ignoring traffic on Net {} (configured {})",
                                dmx.port_address.net, artnet_addr.0[0]
                            );
                        }
                        crate::netstats::artnet_ignored();
                        continue;
                    }
                    // Index by the packet's SubUni byte (sub-net:universe) so
                    // consecutive Port-Addresses map to consecutive buffer slots
                    // even when a multi-universe span crosses a sub-net boundary.
                    //
                    // SubUni is a full byte (0..=255) but the buffer covers
                    // DMX_UNIVERSE_COUNT of them, so this MUST be bounds-checked:
                    // indexing straight from the wire would let a remote packet
                    // panic the firmware. Out-of-range universes are simply not
                    // ours to render.
                    let sub_uni = dmx.port_address.sub_uni();
                    if sub_uni >= DMX_UNIVERSE_COUNT {
                        if !warned_out_of_range {
                            warn!(
                                "ArtNet: ignoring universe {} - beyond the {} buffered",
                                sub_uni, DMX_UNIVERSE_COUNT
                            );
                            warned_out_of_range = true;
                        }
                        crate::netstats::artnet_ignored();
                        continue;
                    }
                    let start = DMX_UNIVERSE_SIZE * sub_uni;
                    // The spec allows packets carrying fewer than 512 channels
                    let len = dmx.data.len().min(DMX_UNIVERSE_SIZE);
                    let mut dmx_buffer = DMX_BUFFER.lock().await;
                    dmx_buffer[start..start + len].copy_from_slice(&dmx.data[..len]);
                    drop(dmx_buffer);
                    // info!("{}", dmx_buffer[start..end]);

                    // if dmx.port_address.universe == 2 {
                    crate::netstats::artnet_stored();
                    let _ = tx.try_send(DmxEvent::ArtNetPacket(PacketAddress::new(dmx.port_address, dmx.sequence)));
                    // }
                }
                // match tx.try_send(DmxEvent::ArtNetPacket((dmx.port_address, dmx.sequence))) {
                //     Ok(_) => {}
                //     Err(_) => error!("Unable to send DMX Event packet"),
                // };

                // // package the dmx data into 513 bytes
                // let mut buf_out = [0_u8; DMX_BUFF_SIZE];
                // // dmx.data does not include the DMX start byte.
                // dmx.data.iter().enumerate().for_each(|(i, v)| buf_out[i + 1] = *v);

                // if !tx.is_empty() {
                //     info!("ArtNet DMX Buffer Full - Clearing DMX channel");
                //     tx.clear(); // clear any existing message on the channel
                // }
                // match tx.try_send(DmxEvent::ArtNetPacket((buf_out, dmx.port_address, dmx.sequence))) {
                //     Ok(_) => {}
                //     Err(_) => error!("Unable to send DMX Event packet"),
                // }
            }
            Ok(Art::Sync) => {
                debug!("RX: ArtSync - Use these to buffer DMX packets and then synchronize the rendering of multiple DMX universes.");
            }
            Ok(Art::Poll(_poll)) => {
                debug!("RX: ArtPoll - answering with one ArtPollReply per four bound universes");

                // Report the CURRENT address, not the one captured at boot —
                // a DHCP renewal can move it, and a reply with a stale IP
                // makes controllers unicast into the void.
                let ip_now = stack
                    .config_v4()
                    .map(|c| c.address.address())
                    .unwrap_or(local_addr);
                let ip_bytes = ip_now.octets();

                // Art-Net 4 BindIndex: one ArtPollReply can carry at most four
                // ports, all in one Sub-Net. A node binding more universes —
                // eight ports of 600 LEDs is 32 — sends several replies, each
                // with the next BindIndex and the same BindIp, and controllers
                // that auto-patch from ArtPollReply then bind every universe.
                //
                // The bound span starts at the configured Port-Address and runs
                // for `bound` universes (clamped to the buffer by the router),
                // so it can cross Sub-Net boundaries; a reply is cut at each
                // one. This is exactly the count the UI shows as "Univ Bound".
                let net = artnet_addr.0[0];
                let base = artnet_addr.buffer_base() as u16;
                let count = bound.max(1).min(256 - base);
                let mut universe = base;
                let mut bind_index: u8 = 1;
                while universe < base + count {
                    let sub = (universe >> 4) as u8;
                    let first = (universe & 0x0F) as u8;
                    let remaining = (base + count - universe) as usize;
                    let ports = remaining.min(16 - first as usize).min(4);

                    let mut swout = [0u8; 4];
                    let mut port_types = [0u8; 4];
                    let mut good_output = [0u8; 4];
                    for i in 0..ports {
                        swout[i] = first + i as u8;
                        port_types[i] = 0b1000_0000; // output port, DMX512
                        good_output[i] = 0b1000_0000; // data is being output
                    }

                    let poll_reply = tiny_artnet::PollReply {
                        ip_address: &ip_bytes,
                        port,
                        firmware_version: 0x0001,
                        oem: ARTNET_OEM,
                        short_name: "DMX LED Interface",
                        long_name: "DMX/Art-Net LED Interface",
                        net_switch: net,
                        sub_switch: sub,
                        swout: &swout,
                        mac_address: &mac_address_bytes,
                        num_ports: ports as u16,
                        port_types: &port_types,
                        good_output_a: &good_output,
                        bind_ip_address: &ip_bytes,
                        bind_index,
                        // Bit 3: 15-bit Port-Address (Art-Net 3/4) supported;
                        // bit 1: DHCP capable.
                        status2: 0b0000_1010,
                        ..Default::default()
                    };
                    let reply_message = poll_reply.ser();
                    if socket.send_to(&reply_message, from_addr).await.is_err() {
                        error!("Artnet Unable to send on socket.");
                        break;
                    }

                    universe += ports as u16;
                    bind_index = bind_index.saturating_add(1);
                }
                debug!(
                    "Sent {} ArtPollReply(s) for {} universes from {}:{} to {:?}",
                    bind_index - 1, count, net, base, from_addr.endpoint.addr
                );
            }
            Ok(Art::Command(command)) => {
                debug!("command {:?} - {:?}", command.esta_manufacturer_code, command.data);
            }
            Err(err) => {
                crate::netstats::artnet_malformed();
                // info!("Error: {:?}", err);

                match err {
                    tiny_artnet::Error::UnsupportedProtocolVersion(e) => {
                        error!("ArtNet Unsupported protocol version {:?}", e)
                    }
                    tiny_artnet::Error::UnsupportedOpCode(e) => {
                        error!("ArtNet Unsupported op code {:?}", e)
                    }
                    tiny_artnet::Error::ParseIncomplete(e) => {
                        error!("ArtNet parse incomplete {:?}", e)
                    }
                    tiny_artnet::Error::ParseFault(error_kind) => {
                        error!("ArtNet parse error {:?}", error_kind)
                    }
                    tiny_artnet::Error::ParseFailure => error!("ArtNet parse failure"),
                }
            }
        };
    }
}
