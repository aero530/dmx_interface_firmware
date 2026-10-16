//! Enttec DMX USB Pro widget emulation over the module's native USB (CDC-ACM).
//!
//! Presents the board to a PC as a serial device speaking the Enttec DMX USB
//! Pro protocol, so serial-port-agnostic lighting software (xLights, Vixen,
//! OLA `usbpro`, scripts) can use it as a USB-DMX interface. The protocol
//! handling itself is shared with the FT232RNL port in `enttec_widget.rs`; this
//! file is only the CDC transport and the composite device.
//!
//! Software that discovers Enttec hardware through the FTDI driver (QLC+,
//! D2XX applications) cannot see a CDC port at all — that is what the
//! carrier's FT232RNL USB-C port (`enttec_uart.rs`) exists for, and, on a board
//! where that chip is not fitted, what `ftdi.rs` stands in for.
//!
//! # Two identities, chosen at boot
//!
//! This task waits for the stored input mode before it builds anything, then
//! enumerates as one of two devices:
//!
//! * **`USB>DMX` mode** — a single vendor interface imitating an FT232R, so
//!   QLC+ and other FTDI-discovering software can drive the widget
//!   (`ftdi::run`). There is no console in this mode: the FTDI driver binds to
//!   a non-composite device, so the CDC interfaces cannot come along.
//! * **every other mode** — the composite CDC device below: widget, console,
//!   and the logger with the `usb` feature.
//!
//! Switching between those two means re-enumerating, which is why changing to
//! or from `USB>DMX` needs a reboot. The wait is bounded: if the mode never
//! arrives (a dead settings EEPROM), it falls back to CDC, because a board with
//! a console is one that can be debugged.
//!
//! This task owns the USB peripheral and builds a composite device: interface 1
//! is always the Enttec widget, interface 2 is always the console line protocol
//! (see `console_usb.rs`), and with the `usb` logging feature enabled a third
//! CDC-ACM interface carries the `log` output. The host sees the serial ports
//! in that order. Last comes one vendor interface with no endpoints: picotool's
//! **reset interface** (class `0xFF`, subclass `0x00`, protocol `0x01`, the
//! pico-sdk `pico_stdio_usb` convention), so `picotool load -f -x pico2.uf2`
//! can reboot a running unit into BOOTSEL, flash it and restart it with no
//! button pressed. Windows binds WinUSB to it on its own because the device
//! carries an MS OS 2.0 descriptor saying so. The FTDI identity cannot have
//! it: it has to stay a single, non-composite interface.

use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{info, warn};
    } else {
        use defmt::{info, warn};
    }
}

use embassy_futures::select::{Either, select};
use crate::usb_toggle::{UsbClass, UsbDriver};
use core::sync::atomic::{AtomicU8, Ordering};

use common::events::RebootRequest;
use embassy_time::{Duration, Timer, with_timeout};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::control::{OutResponse, Recipient, Request, RequestType};
use embassy_usb::driver::EndpointError;
use embassy_usb::msos::{self, CompatibleIdFeatureDescriptor, PropertyData, RegistryPropertyFeatureDescriptor};
use embassy_usb::types::InterfaceNumber;
use embassy_usb::Handler;

use crate::channels::{DmxChannelTx, DmxFeedbackChannelRx, GlobalDataChannelRx, RouterChannelTx};
use crate::enttec_protocol::{EnttecParser, MAX_PAYLOAD};
use crate::enttec_widget::{handle_message, ChangeForwarder, MSG_MAX};
use crate::event_router::DmxFeedbackEvent;
use crate::ui::InputMode;

/// Send a framed message chunked into full-speed USB packets, with a ZLP when
/// the total lands on a packet boundary so the host flushes the transfer.
async fn send_bytes(class: &mut UsbClass<'_>, bytes: &[u8]) -> Result<(), EndpointError> {
    for chunk in bytes.chunks(64) {
        class.write_packet(chunk).await?;
    }
    if bytes.len().is_multiple_of(64) {
        class.write_packet(&[]).await?;
    }
    Ok(())
}

/// **Placeholder VID/PID.** Fine on the bench — lighting software opens the
/// serial port by protocol, not by VID — but a real pair must be allocated
/// before a unit leaves the building. Free options: <https://pid.codes> for an
/// open-source project, or a sub-PID under Raspberry Pi's VID `0x2E8A`, which
/// they issue for RP2350-based products. This is the only place to change it.
pub const USB_VID: u16 = 0xc0de;
pub const USB_PID: u16 = 0xdcaf;

/// picotool's reset interface, as pico-sdk's `pico_stdio_usb` defines it:
/// a vendor interface picotool recognises by subclass and protocol, and two
/// class-type control requests to it.
const RESET_SUBCLASS: u8 = 0x00;
const RESET_PROTOCOL: u8 = 0x01;
/// Reboot into BOOTSEL. wValue bits 0-6 are the bootrom's interface-disable
/// mask; bit 8 and above name an activity LED, which this board ignores.
const RESET_REQUEST_BOOTSEL: u8 = 0x01;
/// Plain reboot into the application.
const RESET_REQUEST_FLASH: u8 = 0x02;
/// bMS_VendorCode for the MS OS 2.0 descriptor set (pico-sdk uses 1 too).
const MSOS_VENDOR_CODE: u8 = 0x01;
/// The device-interface GUID Windows files the reset interface under; the one
/// pico-sdk ships, so a PC that has met a Pico already knows it.
const RESET_INTERFACE_GUID: &str = "{bc7398c1-73cd-4cb7-98b8-913a8fca7bf6}";

/// Answers picotool on the reset interface.
///
/// Accepts the request (so the host sees a clean status stage) and hands the
/// reboot to `reboot.rs`, which waits for boot-complete and a short settle
/// before acting. pico-sdk reboots inside the request itself and picotool copes
/// with the resulting pipe error; finishing the transfer first is kinder.
struct ResetInterface {
    interface: InterfaceNumber,
}

impl Handler for ResetInterface {
    fn control_out(&mut self, req: Request, _data: &[u8]) -> Option<OutResponse> {
        if req.request_type != RequestType::Class
            || req.recipient != Recipient::Interface
            || req.index != u16::from(u8::from(self.interface))
        {
            return None;
        }
        match req.request {
            RESET_REQUEST_BOOTSEL => {
                let disable_mask = (req.value & 0x7F) as u8;
                info!("USB: picotool asked for BOOTSEL (disable mask {:#04x})", disable_mask);
                crate::reboot::request(RebootRequest::Bootsel { disable_mask });
                Some(OutResponse::Accepted)
            }
            RESET_REQUEST_FLASH => {
                info!("USB: picotool asked for a reboot");
                crate::reboot::request(RebootRequest::Flash);
                Some(OutResponse::Accepted)
            }
            other => {
                warn!("USB: unknown reset-interface request {:#04x}", other);
                Some(OutResponse::Rejected)
            }
        }
    }
}

/// How long to wait for the stored input mode before giving up and building
/// the CDC device.
///
/// The router broadcasts the mode as soon as the settings come back from the
/// EEPROM, which is well inside this. Waiting delays enumeration by that much -
/// harmless, a host simply sees the device attach a moment later - and the
/// fallback is deliberate: without settings there is nothing to put the board
/// into `USB>DMX` mode anyway, and a console is worth more than a guess.
const MODE_WAIT: Duration = Duration::from_secs(2);

/// USB device task. Picks an identity from the stored mode, then owns the
/// peripheral for the life of the board.
#[embassy_executor::task]
pub async fn usb_device_task(
    driver: UsbDriver,
    tx: DmxChannelTx,
    mut rx: DmxFeedbackChannelRx,
    router_tx: RouterChannelTx,
    global_rx: GlobalDataChannelRx,
    serial: &'static str,
) {
    // This consumes the first broadcast, so the mode and Art-Net base are
    // handed on rather than left for the transport to rediscover - otherwise
    // both would start on `InputMode::default()` and stay there until the next
    // settings change.
    let (mode, artnet_base) = match with_timeout(MODE_WAIT, rx.changed()).await {
        Ok(DmxFeedbackEvent::Mode(mode, addr, _, _)) => (mode, addr.buffer_base()),
        Err(_) => {
            warn!("USB: no input mode within {} ms - enumerating as CDC", MODE_WAIT.as_millis());
            (InputMode::default(), 0)
        }
    };

    let ftdi = mode == InputMode::UsbToDmx;
    // Published only now, once it is a fact. Until this store the watcher has
    // nothing to compare against and stays out of the way.
    USB_IDENTITY.store(if ftdi { IDENTITY_FTDI } else { IDENTITY_CDC }, Ordering::Relaxed);
    if ftdi {
        crate::ftdi::run(driver, tx, rx, serial, mode, artnet_base).await;
    } else {
        run_cdc(driver, tx, rx, router_tx, global_rx, serial, mode, artnet_base).await;
    }
}

/// Which identity [`usb_device_task`] built, for [`usb_identity_watch_task`] to
/// compare the live setting against.
///
/// Three states, not two. "Not yet chosen" has to be distinguishable from
/// "CDC": the watcher and the USB task learn the stored mode from the same
/// router broadcast and the order between them is not fixed, so a plain
/// `false` default let the watcher see a `USB>DMX` setting, compare it against
/// an identity that had not been decided, and reboot. Every boot. That was the
/// 2026-09-17 boot loop.
static USB_IDENTITY: AtomicU8 = AtomicU8::new(IDENTITY_UNCHOSEN);
const IDENTITY_UNCHOSEN: u8 = 0;
const IDENTITY_CDC: u8 = 1;
const IDENTITY_FTDI: u8 = 2;

/// Ask for a reboot when the stored mode needs a USB identity other than the
/// one this boot built.
///
/// USB identity is fixed at enumeration: a device cannot become an FT232R
/// while attached. Crossing into or out of `USB>DMX` therefore needs a restart,
/// and doing it automatically beats leaving someone to wonder why the port did
/// not change.
///
/// Three conditions guard it, and each exists because of a way this went wrong:
///
/// * **The USB identity must have been chosen.** Not merely defaulted - see
///   [`USB_IDENTITY`]. Comparing against a default is what caused a boot loop.
/// * **This boot must have completed**, tracked by `reboot::mark_boot_complete`
///   rather than by `GlobalData::boot_status`, which describes the *previous*
///   boot. `boot_task` writes `BootStatus::Failed` on the way in and `Success`
///   at the end, and two consecutive incomplete boots trip the guard that
///   disables Ethernet - so resetting mid-boot would take Ethernet out after
///   two mode changes. (`reboot_task` checks this too; checking it here as
///   well keeps a mode change during boot from even queueing a request.)
/// * **The setting must have reached the EEPROM.** The mode only arrives here
///   through `StoreSettings`, which the EEPROM task sends *after* a successful
///   write - so by the time this sees it, a reboot will read it back.
///
/// With those, a reboot loop is impossible: the only path to a reset is a
/// settings change observed after a completed boot, and a reboot makes the
/// identity match the setting. If the EEPROM write failed the board comes back
/// as it was, which also matches. Either way the next boot is quiet.
#[embassy_executor::task]
pub async fn usb_identity_watch_task(mut global_rx: GlobalDataChannelRx) -> ! {
    loop {
        let data = global_rx.changed().await;
        let identity = USB_IDENTITY.load(Ordering::Relaxed);
        if identity == IDENTITY_UNCHOSEN {
            // The USB task has not decided yet. Nothing to compare against.
            continue;
        }
        if !crate::reboot::boot_complete() {
            // Resetting now would be counted against the Ethernet lockout guard.
            continue;
        }
        let wants_ftdi = data.menu_settings.input_mode == InputMode::UsbToDmx;
        if wants_ftdi == (identity == IDENTITY_FTDI) {
            continue;
        }
        warn!(
            "USB: {} mode needs the {} identity - rebooting to re-enumerate",
            data.menu_settings.input_mode,
            if wants_ftdi { "FT232R" } else { "CDC" }
        );
        // `reboot_task` settles and resets; this loop keeps watching, and a
        // repeat request before the reset lands changes nothing.
        crate::reboot::request(RebootRequest::Flash);
    }
}

/// The composite CDC device: Enttec widget + console (+ logger).
#[allow(clippy::too_many_arguments)]
async fn run_cdc(
    driver: UsbDriver,
    tx: DmxChannelTx,
    mut rx: DmxFeedbackChannelRx,
    router_tx: RouterChannelTx,
    global_rx: GlobalDataChannelRx,
    serial: &'static str,
    mode: InputMode,
    artnet_base: usize,
) {
    let mut config = embassy_usb::Config::new(USB_VID, USB_PID);
    config.manufacturer = Some("EQUUS");
    config.product = Some("DMX USB Pro compatible");
    // Unique per unit (OTP chip ID) so two boxes on one PC keep separate
    // COM-port bindings.
    config.serial_number = Some(serial);
    config.max_packet_size_0 = 64;
    // `embassy_usb::Config::new` defaults to 100 mA, which this board does not
    // honour: the RP2350 and the W6300 together are about 200 mA before the TFT
    // backlight, and `LedPower::Usb` puts a strip on top of that. A descriptor
    // that under-declares is a bug whatever the host does with it - most PCs
    // ignore bMaxPower, but a bus-powered hub enforcing its budget is entitled
    // to cut the port. The FTDI identity keeps its own 90 mA on purpose, to
    // match real FT232R silicon; see `ftdi.rs`.
    config.max_power = 500;
    // Composite device with Interface Association Descriptors: without them
    // Windows will not bind its CDC driver to each serial function separately.
    // Verified on the Rev 1 bench firmware (`rp2040_dmx`, retired), which used
    // the same layout.
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;
    // Bumped from the 0x0010 default when the reset interface and its MS OS
    // descriptor arrived (2026-10-16). Windows remembers, per VID/PID/bcdDevice,
    // whether a device answered the MS OS descriptor query; a PC that met the
    // old firmware would never ask again and never bind WinUSB. bcdDevice is
    // not part of the composite child instance IDs, so COM port numbers keep.
    config.device_release = 0x0020;

    let mut config_descriptor = [0; 320];
    let mut bos_descriptor = [0; 256];
    // The MS OS 2.0 set: header, configuration subset, one function subset
    // with a compatible ID and a registry property - about 180 bytes.
    let mut msos_descriptor = [0; 256];
    let mut control_buf = [0; 64];
    let mut state = State::new();
    // Declared alongside the other USB state so they outlive everything
    // sharing the builder's lifetime.
    let mut console_state = State::new();
    #[cfg(feature = "usb")]
    let mut logger_state = State::new();
    let mut reset = ResetInterface { interface: InterfaceNumber(0) };

    let mut builder = embassy_usb::Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );
    // Before any function, so the per-function MS OS features below have a
    // set to land in. Windows 8.1 is the first version that reads MS OS 2.0.
    builder.msos_descriptor(msos::windows_version::WIN8_1, MSOS_VENDOR_CODE);

    let mut class = CdcAcmClass::new(&mut builder, &mut state, 64);

    // Second CDC-ACM interface: console line protocol.
    let mut console_class = CdcAcmClass::new(&mut builder, &mut console_state, 64);

    // Third CDC-ACM interface carrying the `log` output.
    #[cfg(feature = "usb")]
    let logger_class = CdcAcmClass::new(&mut builder, &mut logger_state, 64);

    // Last, so the CDC interface numbers - and with them the `MI_xx` child IDs
    // Windows binds COM ports to - stay exactly what they were. picotool's
    // reset interface: no endpoints, just the two control requests.
    {
        let mut function = builder.function(0xFF, RESET_SUBCLASS, RESET_PROTOCOL);
        // What makes Windows load WinUSB for this interface unprompted.
        function.msos_feature(CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        function.msos_feature(RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            PropertyData::RegMultiSz(&[RESET_INTERFACE_GUID]),
        ));
        let mut interface = function.interface();
        reset.interface = interface.interface_number();
        let _alt = interface.alt_setting(0xFF, RESET_SUBCLASS, RESET_PROTOCOL, None);
    }
    builder.handler(&mut reset);

    let mut usb = builder.build();

    let protocol = async {
        let mut parser = EnttecParser::new();
        let mut forwarder = ChangeForwarder::new();
        let mut packet = [0_u8; 64];
        let mut payload = [0_u8; MAX_PAYLOAD];
        let mut msg = [0_u8; MSG_MAX];
        let mut input_mode = mode;
        forwarder.set_artnet_base(artnet_base);

        loop {
            class.wait_connection().await;
            info!("Enttec: USB host connected");

            'connected: loop {
                if let Some(DmxFeedbackEvent::Mode(new_mode, artnet_addr, _, _)) = rx.try_changed() {
                    input_mode = new_mode;
                    forwarder.set_artnet_base(artnet_addr.buffer_base());
                }

                match select(class.read_packet(&mut packet), Timer::after_millis(30)).await {
                    Either::First(Ok(n)) => {
                        for &byte in &packet[..n] {
                            if parser.feed(byte) {
                                // The payload is copied out so `parser` isn't
                                // borrowed across the await point below.
                                let length = parser.payload().len();
                                payload[..length].copy_from_slice(parser.payload());
                                if let Some(len) = handle_message(parser.label(), &payload[..length], input_mode, &tx, &mut msg).await {
                                    if send_bytes(&mut class, &msg[..len]).await.is_err() {
                                        break 'connected;
                                    }
                                }
                            }
                        }
                    }
                    Either::First(Err(_)) => break 'connected,
                    Either::Second(()) => {
                        // Forward received DMX to the host (label 5) on change.
                        if let Some(len) = forwarder.poll(input_mode, &mut msg).await {
                            if send_bytes(&mut class, &msg[..len]).await.is_err() {
                                break 'connected;
                            }
                        }
                    }
                }
            }
            info!("Enttec: USB host disconnected");
        }
    };

    let console = crate::console_usb::run(&mut console_class, router_tx, global_rx);

    cfg_if! {
        if #[cfg(feature = "usb")] {
            // `crate::usb_log`, not `embassy-usb-logger`: the sink was
            // installed in `main`, and this only drains once the host opens
            // the port. See that module for why that matters.
            let logger = crate::usb_log::run(logger_class);
            embassy_futures::join::join4(usb.run(), protocol, console, logger).await;
        } else {
            embassy_futures::join::join3(usb.run(), protocol, console).await;
        }
    }
}
