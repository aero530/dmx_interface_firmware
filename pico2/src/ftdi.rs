//! FT232R emulation on the module's native USB, for `USB>DMX` mode.
//!
//! # Why this exists
//!
//! The Enttec DMX USB Pro is built on FTDI silicon, and software that discovers
//! it — QLC+ in particular — enumerates FTDI devices through D2XX or libftdi
//! rather than opening a serial port. A CDC-ACM port speaking the same protocol
//! is invisible to it, which was confirmed on the bench: the widget in
//! `usb_device.rs` works with xLights and OLA `usbpro` and is not seen by QLC+
//! at all. The carrier answers this with a real FT232RNL (U2) on its own USB-C
//! port, but U2 could not be sourced for the first build, so the module's own
//! USB has to look like FTDI silicon instead.
//!
//! **This is a stand-in, not the product path.** On a board with U2 fitted the
//! FTDI identity comes from that chip's EEPROM and this module is not used. It
//! also means the device claims FTDI's vendor ID, which is legitimate for the
//! real chip and is a decision to revisit if a unit ever ships without one.
//!
//! # What an FTDI device has to look like
//!
//! Not a VID/PID change. Four things together, and the driver needs all of them:
//!
//! 1. **Identity.** VID `0x0403`, PID `0x6001`, and `bcdDevice` = `0x0600` —
//!    the Linux `ftdi_sio` driver and D2XX both read `bcdDevice` to decide
//!    *which* FTDI chip this is, and `0x0600` means FT232R. The strings are
//!    Enttec's own, `ENTTEC` / `DMX USB PRO`, matching what the EEPROM recipe
//!    in `docs/ft232rnl-eeprom.md` programs into U2.
//! 2. **A vendor-class interface**, `0xFF/0xFF/0xFF`, with one bulk IN and one
//!    bulk OUT endpoint. Not CDC, and deliberately *not* composite: for PID
//!    `0x6001` the Windows INF matches `USB\VID_0403&PID_6001`, not the
//!    `&MI_00` form a composite device presents. That is why this build has no
//!    CDC console — see `usb_device.rs` for how one of the two is chosen.
//! 3. **Two status bytes in front of every IN packet.** Real silicon prefixes
//!    each packet with modem status and line status, and the host driver strips
//!    them before the application sees the data. Omit them and the first two
//!    bytes of every reply are eaten instead. This costs two bytes of each
//!    64-byte packet, so payload chunks are 62.
//! 4. **The vendor control requests**, below. The driver issues them while
//!    opening the port and gives up if they are not answered.
//!
//! # What is deliberately not implemented
//!
//! Baud rate, data format and flow control are accepted and ignored: there is
//! no UART behind this, the "wire" is USB in both directions, and the Enttec
//! protocol is self-framing. The EEPROM read request returns zeros — the
//! identity strings the host needs are in the USB descriptors, and a faithful
//! FT232R EEPROM image (layout, string offsets, checksum) would be a lot of
//! work for something only D2XX's `FT_EE_Read` looks at. If QLC+ ever fails to
//! see the device, that request is the first thing to make real.

use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{info, warn};
    } else {
        use defmt::{info, warn};
    }
}

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_futures::select::{Either, select};
use embassy_rp::peripherals;
use embassy_rp::usb::Driver;
use embassy_time::Timer;
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb::driver::{Direction, Endpoint, EndpointAddress, EndpointError, EndpointIn, EndpointOut};
use embassy_usb::{Builder, Handler};

use crate::channels::{DmxChannelTx, DmxFeedbackChannelRx};
use crate::enttec_protocol::{EnttecParser, MAX_PAYLOAD};
use crate::enttec_widget::{ChangeForwarder, MSG_MAX, handle_message};
use crate::event_router::DmxFeedbackEvent;
use crate::ui::InputMode;

/// FTDI's vendor ID and the FT232R product ID, as an Enttec Pro carries them.
pub const FTDI_VID: u16 = 0x0403;
pub const FTDI_PID: u16 = 0x6001;

/// `bcdDevice` for an FT232R. Drivers switch behaviour on this, so it is not
/// cosmetic: `0x0400` would make the host treat us as an FT232BM and compute
/// baud divisors differently.
const BCD_FT232R: u16 = 0x0600;

/// Full-speed bulk endpoints, and the 62 payload bytes left after the status
/// prefix.
const PACKET: u16 = 64;
const PAYLOAD_PER_PACKET: usize = PACKET as usize - 2;

/// Endpoint numbers, pinned to what an FT232R actually uses: **bulk IN on
/// `0x81`, bulk OUT on `0x02`**.
///
/// Letting the allocator choose gave `0x01`/`0x81`, which enumerated fine and
/// then failed to start with Code 10 - the Windows FTDI driver binds on the
/// VID/PID and expects the endpoint layout of the chip `bcdDevice` claims,
/// rather than reading it back from the descriptors.
const EP_IN: usize = 1;
const EP_OUT: usize = 2;

// FTDI vendor request codes (bRequest), from the published `ftdi_sio` protocol.
const REQ_RESET: u8 = 0x00;
const REQ_MODEM_CTRL: u8 = 0x01;
const REQ_SET_FLOW_CTRL: u8 = 0x02;
const REQ_SET_BAUD_RATE: u8 = 0x03;
const REQ_SET_DATA: u8 = 0x04;
const REQ_GET_MODEM_STATUS: u8 = 0x05;
const REQ_SET_EVENT_CHAR: u8 = 0x06;
const REQ_SET_ERROR_CHAR: u8 = 0x07;
const REQ_SET_LATENCY_TIMER: u8 = 0x09;
const REQ_GET_LATENCY_TIMER: u8 = 0x0A;
const REQ_SET_BITMODE: u8 = 0x0B;
const REQ_READ_PINS: u8 = 0x0C;
const REQ_READ_EEPROM: u8 = 0x90;

/// The two bytes that lead every IN packet.
///
/// Byte 0 is modem status: the low nibble reads back as 1 on real silicon, and
/// the handshake lines we do not have stay clear. Byte 1 is line status, where
/// `0x60` is THRE | TEMT — "transmitter empty", which is always true here
/// because there is no UART to be busy.
const STATUS: [u8; 2] = [0x01, 0x60];

/// Latency timer in milliseconds, as last set by the host.
///
/// Real silicon sends a status-only packet this often when it has nothing else
/// to say, and drivers use that as the read heartbeat. Kept in an atomic rather
/// than in [`FtdiControl`] because the control handler is owned by the USB
/// builder for its lifetime while the transport loop needs to read it.
static LATENCY_MS: AtomicU8 = AtomicU8::new(16);

/// Answers the vendor control requests a host makes while opening the port.
struct FtdiControl;

impl Handler for FtdiControl {
    fn control_out(&mut self, req: Request, _data: &[u8]) -> Option<OutResponse> {
        if req.request_type != RequestType::Vendor || req.recipient != Recipient::Device {
            return None;
        }
        match req.request {
            // The latency timer is the one setting worth keeping: it is how
            // often the host expects to hear from us when there is no data.
            REQ_SET_LATENCY_TIMER => {
                LATENCY_MS.store((req.value as u8).max(1), Ordering::Relaxed);
                Some(OutResponse::Accepted)
            }
            // Accepted and ignored. There is no UART behind this interface, so
            // baud, framing and flow control have nothing to configure - but
            // refusing them would make the driver abandon the open.
            REQ_RESET | REQ_MODEM_CTRL | REQ_SET_FLOW_CTRL | REQ_SET_BAUD_RATE | REQ_SET_DATA
            | REQ_SET_EVENT_CHAR | REQ_SET_ERROR_CHAR | REQ_SET_BITMODE => {
                Some(OutResponse::Accepted)
            }
            other => {
                warn!("FTDI: unhandled vendor OUT request {=u8:#04x}", other);
                // Still accept: an unknown request is far more likely to be one
                // this emulation has not met than a real error, and a rejection
                // aborts the host's open.
                Some(OutResponse::Accepted)
            }
        }
    }

    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        if req.request_type != RequestType::Vendor || req.recipient != Recipient::Device {
            return None;
        }
        match req.request {
            REQ_GET_MODEM_STATUS => {
                buf[..2].copy_from_slice(&STATUS);
                Some(InResponse::Accepted(&buf[..2]))
            }
            REQ_GET_LATENCY_TIMER => {
                buf[0] = LATENCY_MS.load(Ordering::Relaxed);
                Some(InResponse::Accepted(&buf[..1]))
            }
            // No bit-bang pins are wired to anything.
            REQ_READ_PINS => {
                buf[0] = 0;
                Some(InResponse::Accepted(&buf[..1]))
            }
            // See the module docs: the identity the host needs is in the string
            // descriptors, so a blank EEPROM word is enough to satisfy the read.
            REQ_READ_EEPROM => {
                buf[..2].copy_from_slice(&[0, 0]);
                Some(InResponse::Accepted(&buf[..2]))
            }
            other => {
                warn!("FTDI: unhandled vendor IN request {=u8:#04x}", other);
                Some(InResponse::Rejected)
            }
        }
    }
}

/// Write a framed Enttec message as FTDI packets: 62 payload bytes at a time,
/// each behind the two status bytes.
///
/// An empty `bytes` sends the status-only packet that stands in for the
/// hardware's idle heartbeat.
async fn send<E: EndpointIn>(ep: &mut E, bytes: &[u8]) -> Result<(), EndpointError> {
    if bytes.is_empty() {
        return ep.write(&STATUS).await;
    }
    let mut packet = [0u8; PACKET as usize];
    packet[..2].copy_from_slice(&STATUS);
    let mut ended_full = false;
    for chunk in bytes.chunks(PAYLOAD_PER_PACKET) {
        packet[2..2 + chunk.len()].copy_from_slice(chunk);
        ep.write(&packet[..2 + chunk.len()]).await?;
        ended_full = chunk.len() == PAYLOAD_PER_PACKET;
    }
    // A full-size packet leaves the host waiting for more; a short one ends the
    // transfer. The heartbeat would do this at the next latency tick anyway,
    // but not soon enough for a host that is waiting on the reply it just asked
    // for.
    if ended_full {
        ep.write(&STATUS).await?;
    }
    Ok(())
}

/// Own the USB peripheral and present an FT232R carrying the Enttec widget.
///
/// Never returns: `usb.run()` and the protocol loop are joined for the life of
/// the board, exactly as the CDC path in `usb_device.rs` does.
pub async fn run(
    driver: Driver<'static, peripherals::USB>,
    tx: DmxChannelTx,
    mut rx: DmxFeedbackChannelRx,
    serial: &'static str,
    mode: InputMode,
    artnet_base: usize,
) {
    let mut config = embassy_usb::Config::new(FTDI_VID, FTDI_PID);
    config.manufacturer = Some("ENTTEC");
    config.product = Some("DMX USB PRO");
    config.serial_number = Some(serial);
    config.device_release = BCD_FT232R;
    // A plain USB 2.0 device with one vendor interface: class at the interface,
    // no IADs, and no BOS descriptor to make it look like something newer than
    // the silicon it is imitating.
    config.bcd_usb = embassy_usb::UsbVersion::Two;
    config.device_class = 0x00;
    config.device_sub_class = 0x00;
    config.device_protocol = 0x00;
    config.composite_with_iads = false;
    config.max_packet_size_0 = 64;
    // Bus-powered with remote wakeup and 90 mA, matching an FT232R's
    // bmAttributes/bMaxPower. Cosmetic next to the endpoint addresses, but this
    // descriptor is being compared against real silicon by a driver that was
    // written for real silicon.
    config.self_powered = false;
    config.supports_remote_wakeup = true;
    config.max_power = 90;

    let mut config_descriptor = [0; 128];
    let mut bos_descriptor = [0; 32];
    let mut control_buf = [0; 64];
    let mut control = FtdiControl;

    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut [],
        &mut control_buf,
    );
    builder.handler(&mut control);

    let (mut read_ep, mut write_ep) = {
        let mut function = builder.function(0xFF, 0xFF, 0xFF);
        let mut interface = function.interface();
        let mut alt = interface.alt_setting(0xFF, 0xFF, 0xFF, None);
        // IN first, so the descriptor lists the endpoints in the same order
        // real silicon does.
        let write_ep = alt.endpoint_bulk_in(
            Some(EndpointAddress::from_parts(EP_IN, Direction::In)),
            PACKET,
        );
        let read_ep = alt.endpoint_bulk_out(
            Some(EndpointAddress::from_parts(EP_OUT, Direction::Out)),
            PACKET,
        );
        (read_ep, write_ep)
    };

    let mut usb = builder.build();
    info!("FTDI: emulating FT232R {=u16:#06x}:{=u16:#06x} as ENTTEC / DMX USB PRO", FTDI_VID, FTDI_PID);

    let protocol = async {
        let mut parser = EnttecParser::new();
        let mut forwarder = ChangeForwarder::new();
        let mut packet = [0_u8; PACKET as usize];
        let mut payload = [0_u8; MAX_PAYLOAD];
        let mut msg = [0_u8; MSG_MAX];
        // Handed in by the caller, which consumed the first mode broadcast to
        // decide this device should be an FT232R at all.
        let mut input_mode = mode;
        forwarder.set_artnet_base(artnet_base);

        loop {
            write_ep.wait_enabled().await;
            info!("FTDI: host opened the port");

            'connected: loop {
                if let Some(DmxFeedbackEvent::Mode(new_mode, artnet_addr, _, _)) = rx.try_changed()
                {
                    input_mode = new_mode;
                    forwarder.set_artnet_base(artnet_addr.buffer_base());
                }

                let heartbeat = Timer::after_millis(LATENCY_MS.load(Ordering::Relaxed) as u64);
                match select(read_ep.read(&mut packet), heartbeat).await {
                    Either::First(Ok(n)) => {
                        for &byte in &packet[..n] {
                            if parser.feed(byte) {
                                // Copied out so `parser` is not borrowed across
                                // the await below.
                                let length = parser.payload().len();
                                payload[..length].copy_from_slice(parser.payload());
                                if let Some(len) = handle_message(
                                    parser.label(),
                                    &payload[..length],
                                    input_mode,
                                    &tx,
                                    &mut msg,
                                )
                                .await
                                {
                                    if send(&mut write_ep, &msg[..len]).await.is_err() {
                                        break 'connected;
                                    }
                                }
                            }
                        }
                    }
                    Either::First(Err(_)) => break 'connected,
                    Either::Second(()) => {
                        // Either a "Received DMX" message to forward, or the
                        // idle heartbeat real silicon would send.
                        let out = forwarder.poll(input_mode, &mut msg).await;
                        let result = match out {
                            Some(len) => send(&mut write_ep, &msg[..len]).await,
                            None => send(&mut write_ep, &[]).await,
                        };
                        if result.is_err() {
                            break 'connected;
                        }
                    }
                }
            }
            info!("FTDI: host closed the port");
        }
    };

    embassy_futures::join::join(usb.run(), protocol).await;
}
