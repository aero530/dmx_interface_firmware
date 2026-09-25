//! Event router to send commands between tasks

use crate::artnet::PortAddress;
use crate::events::{ButtonEvent, KeyPadButton, KeyPadEvent, NetStatus};
use crate::channels::*;
use crate::events::{EepromEvent, SmartLedEvent, UiEvent};
use crate::ui::{ArtNetAddr, BLACK, BootStatus, FieldId, InputMode, IpAddrMenu, MenuData, ModuleSettings, ModuleType, SmartLedPortMode};
use crate::{DMX_BUFFER, DMX_UNIVERSE_SIZE, LED_COLORS, SMARTLED_PORT_COUNT};
use core::net::Ipv4Addr;
use defmt::Format;
use smart_leds::RGBW;

use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{error, info, warn};
    } else {
        use defmt::{error, info, warn};
    }
}



// Supplies `f32::ceil` in no_std; std provides it inherently on the host, so
// the import is unused there but required for the embedded builds.
#[allow(unused_imports)]
use micromath::F32Ext;

/// Data stored for global use (primarily for logging / terminal display)
/// Where the configured "DMX Address" lands in `DMX_BUFFER`.
///
/// The two input families store a universe differently, and this is the one
/// place that difference is reconciled:
///
/// * **Wired DMX and USB** keep the frame DMX-style — the start code sits at
///   index 0, so channel N is at index N and a 1-based address indexes
///   directly.
/// * **Art-Net and sACN** carry no start code, so channel 1 is at slot offset
///   0 and the address has to shift down by one.
///
/// Without the shift the same configured address selected a different channel
/// depending on the input mode (the 2026-07 review logged this as N18).
/// `saturating_sub` keeps address 0 — which the UI does not allow, but an old
/// EEPROM or the console can still produce — pinned at the start of the slot.
/// Paint one port: blank whatever the layout does not cover, then fill each
/// item's run.
///
/// Split out of the render so the **blanking** can be tested against a dirty
/// buffer, which is the only way to catch a missing one: WS2812s latch, so a
/// pixel that is never written keeps its last colour, and a test that renders
/// into an already-black buffer passes whether or not this function blanks.
///
/// Blanking first, rather than painting and then tidying the leftovers, is what
/// makes one rule cover every case: a declared gap, the tail past the last item,
/// pixels freed by shrinking an item or dropping one, and a switch to a layout
/// that reaches less far than the one before it.
///
/// `span(item)` gives `(start, leds)` or `None`; `color(item)` is called only
/// for items that have somewhere to go.
/// The colour item `item` reads, from `base` bytes into the DMX buffer.
///
/// The bounds guards are the ones both render branches used to carry inline.
/// They matter because `base` is built from operator-set values (the DMX start
/// address, the Art-Net base, the per-port stride), so a configuration can point
/// past the end of the buffer and must clamp rather than panic.
fn item_color(
    dmx: &[u8],
    mode: crate::ui::SmartLedColorMode,
    base: usize,
    item: usize,
) -> RGBW<u8> {
    let width = mode.addr_size();
    let mut start = base + item * width;
    let mut end = start + width - 1;

    if end >= dmx.len() {
        warn!("DMX buffer end {} > buffer length {}.", end, dmx.len());
        end = dmx.len() - 1;
    }
    if start >= dmx.len() {
        warn!("DMX buffer start {} > buffer length {}.", start, dmx.len());
        start = dmx.len() - 1;
    }
    if start > end {
        warn!("DMX buffer start {} > buffer end {}.", start, end);
        start = end;
    }
    mode.color(&dmx[start..=end])
}

pub fn paint_port(
    out: &mut [RGBW<u8>],
    leds: usize,
    items: usize,
    span: impl Fn(usize) -> Option<(u16, u16)>,
    color: impl Fn(usize) -> RGBW<u8>,
) {
    // Beyond the configured count nothing is clocked out, so there is no point
    // blanking or painting it.
    let leds = leds.min(out.len());
    out[..leds].fill(BLACK);

    for item in 0..items {
        let Some((start, len)) = span(item) else {
            continue;
        };
        let c = color(item);
        for i in 0..len as usize {
            let place = start as usize + i;
            // A group that does not divide the LED count, or a fixture list
            // longer than the strip, stops here rather than past the end.
            if place >= leds {
                break;
            }
            out[place] = c;
        }
    }
}

pub fn buffer_index(dmx_address: u16, is_network: bool) -> usize {
    if is_network {
        (dmx_address as usize).saturating_sub(1)
    } else {
        dmx_address as usize
    }
}

#[derive(Debug, PartialEq, Copy, Clone, Default)]
pub struct GlobalData {
    /// Current menu settings
    pub menu_settings: MenuData,
    /// Detected module type
    pub module_type: Option<ModuleType>,
    /// Board MAC address - only defined if ETH is enabled
    pub mac: Option<[u8; 6]>,
    /// Monitor boot process
    pub boot_status: Option<BootStatus>,
    /// Network state for the display and the console.
    pub net_status: NetStatus,
}

/// Indicate which channel should be used to return data
#[derive(Format, Debug)]
pub enum ReturnChannel {
    Main,
    #[allow(unused)]
    Ui,
}

/// Events the router watches for.  These trigger the router to pass along an event to another object.
#[derive(Format, Debug)]
pub enum RouterEvent {
    #[allow(unused)]
    UsbCommand(u8),

    ButtonArray((KeyPadButton, KeyPadEvent)),
    Button(ButtonEvent),
    WriteSettingsToEeprom(MenuData),
    /// Apply a single field from `source` to the router's authoritative
    /// settings and persist. Used by the USB console so a `set` can't
    /// clobber fields edited elsewhere (e.g. on the TFT) in the meantime.
    WriteFieldToEeprom(FieldId, MenuData),
    /// Store settings in global data
    StoreSettings(Option<MenuData>),
    /// Store module type in global data
    StoreModuleType(Option<ModuleType>),
    /// Store MAC address in global data
    StoreMacAddress(Option<[u8; 6]>),
    /// Store boot status in global data
    StoreBootStatus(Option<BootStatus>),
    /// Store IP address in global data
    #[allow(unused)]
    StoreIpAddr(Option<Ipv4Addr>),
    /// Network bring-up progress (boot task / Art-Net task -> display, console)
    StoreNetStatus(NetStatus),
    /// Get module type from global data
    GetModuleType(ReturnChannel),
    /// Get MAC address from global data
    #[allow(unused)]
    GetMacAddress(ReturnChannel),
    /// Get settings from global data
    GetSettings(ReturnChannel),
    /// Get status of previous boot
    GetBootStatus(ReturnChannel),
}

/// Data packet address and sequence info either passed through from ArtNet or set to default for DMX packets
#[derive(Format)]
pub struct PacketAddress {
    /// packet address info either passed through from ArtNet or set to default for DMX packets
    port: PortAddress,
    /// ArtNet packet sequence address - can be used to ensure packet order
    sequence: u8,
}

impl PacketAddress {
    pub fn new(port: PortAddress, sequence: u8) -> Self {
        Self { port, sequence }
    }
}

/// Send data from DMX or ArtNet task to event router
#[derive(Format)]
pub enum DmxEvent {
    /// New data came from wired DMX
    #[allow(unused)]
    DmxPacket(PacketAddress),
    /// New data came from ethernet / ArtNet
    #[allow(unused)]
    ArtNetPacket(PacketAddress),
}

/// Send data back to the DMX / ArtNet / sACN / USB tasks
#[derive(Copy, Clone, Debug, Format)]
pub enum DmxFeedbackEvent {
    /// Current operating mode, configured Art-Net address (net, sub-net,
    /// universe), the first sACN universe, and how many universes the
    /// configuration binds from its base (`MenuData::bound_universes`).
    Mode(InputMode, ArtNetAddr, u16, u16),
}

/// Send data back to the main task
#[derive(Format)]
#[allow(clippy::enum_variant_names)]
pub enum MainEvent {
    ReturnModuleType(Option<ModuleType>),
    ReturnSettings(MenuData),
    ReturnMacAddress(Option<[u8; 6]>),
    ReturnBootStatus(Option<BootStatus>),
}

/// Main communication interface between application tasks
pub struct Router {
    /// Listen for event router tasks
    pub channel: RouterChannelRx,
    pub channel_dmx: DmxChannelRx,
    pub channel_dmx_feedback: DmxFeedbackChannelTx,

    /// Channel to send LED events

    /// Channel to send Smart Led events
    pub channel_smart_led: SmartLedChannelTx,

    /// Channel to send UI events
    pub channel_ui: UiChannelTx,

    /// Channel to send data to the eeprom (write to the eeprom)
    pub channel_eeprom: EepromChannelTx,

    /// Channel to send data back to the main function (used during boot process)
    pub channel_main: MainChannelTx,

    // Global data store
    pub data: GlobalData,

    /// One-shot flag so "output blocked" is reported once per boot, not per frame
    reported_output_blocked: bool,
    /// One-shot flag for a configured Art-Net base beyond the buffered universes
    warned_base_clamp: bool,
    /// One-shot flag for Art-Net traffic on a different Net (was a per-packet
    /// warning — a log storm at 44 Hz on any site with more than one Net)
    warned_net_mismatch: bool,
}

impl Router {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channel: RouterChannelRx,
        channel_dmx: DmxChannelRx,
        channel_dmx_feedback: DmxFeedbackChannelTx,
        // channel_led: LedChannelTx,
        channel_smart_led: SmartLedChannelTx,
        channel_ui: UiChannelTx,
        channel_eeprom: EepromChannelTx,
        channel_main: MainChannelTx,
    ) -> Self {
        Self {
            channel,
            channel_dmx,
            channel_dmx_feedback,
            // channel_led,
            // channel_pwm,
            channel_smart_led,
            channel_ui,
            channel_eeprom,
            channel_main,
            data: GlobalData::default(),
            reported_output_blocked: false,
            warned_base_clamp: false,
            warned_net_mismatch: false,
        }
    }

    /// Broadcast the current mode and addressing to every input task.
    /// Mirror the `LED Power` setting into `usb_power` so the button task
    /// (core 1, the only owner of the expander) can act on it and the UI can
    /// show it. Called from every path that assigns `menu_settings`.
    fn publish_led_power(&self) {
        crate::usb_power::set_mode(match self.data.menu_settings.led_power {
            crate::ui::LedPower::External => crate::usb_power::MODE_EXTERNAL,
            crate::ui::LedPower::UsbBrick => crate::usb_power::MODE_USB_BRICK,
        });
    }

    fn broadcast_mode(&self) {
        let s = &self.data.menu_settings;
        self.channel_dmx_feedback.send(DmxFeedbackEvent::Mode(
            s.input_mode,
            s.artnet_address,
            s.sacn_universe,
            s.bound_universes(),
        ));
    }

    /// Main event router
    pub async fn process_router_event(&mut self, event: RouterEvent) {
        match event {
            RouterEvent::UsbCommand(input) => {
                info!("USB command {:#?}", input);
            }
            RouterEvent::Button(evt) => {
                info!("Button event {:#?}", evt);
            }
            RouterEvent::ButtonArray((btn, evt)) => {
                info!("Button array event {:#?} {:#?}", btn, evt);
                match btn {
                    KeyPadButton::D => {
                        let _ = self.channel_ui.try_send(UiEvent::Down);
                    }
                    KeyPadButton::N0 => {
                        let _ = self.channel_ui.try_send(UiEvent::Select);
                    }
                    KeyPadButton::Star => {
                        let _ = self.channel_ui.try_send(UiEvent::Esc);
                    }
                    KeyPadButton::Pound => {
                        let _ = self.channel_ui.try_send(UiEvent::Up);
                    }
                    _ => {}
                }
            }
            RouterEvent::WriteSettingsToEeprom(menu_data) => {
                self.data.menu_settings = menu_data;
                self.publish_led_power();
                info!("Store settings in eeprom {:#?}", menu_data);
                let _ = self.channel_eeprom.try_send(EepromEvent::WriteSettings(menu_data));
            }
            RouterEvent::WriteFieldToEeprom(field, source) => {
                // Merge only the named field, exactly like a TFT commit does
                field.transfer(&source, &mut self.data.menu_settings);
                self.publish_led_power();
                info!("Store single field in eeprom {:#?}", self.data.menu_settings);
                let _ = self.channel_eeprom.try_send(EepromEvent::WriteSettings(self.data.menu_settings));
            }
            RouterEvent::StoreSettings(menu_data) => {
                if let Some(x) = menu_data {
                    self.data.menu_settings = x;
                    self.publish_led_power();
                    info!("Update settings on display {:#?}", x);
                    let _ = self.channel_ui.try_send(UiEvent::Load(x));
                    self.broadcast_mode();
                }
                CHANNEL_LOG.sender().send(self.data);
            }
            RouterEvent::StoreNetStatus(status) => {
                self.data.net_status = status;
                let _ = self.channel_ui.try_send(UiEvent::Net(status));
                CHANNEL_LOG.sender().send(self.data);
            }
            RouterEvent::StoreBootStatus(status) => {
                self.data.boot_status = status;
                CHANNEL_LOG.sender().send(self.data);
            }
            RouterEvent::StoreModuleType(module_type) => {
                self.data.module_type = module_type;
                CHANNEL_LOG.sender().send(self.data);
            }
            RouterEvent::StoreMacAddress(mac) => {
                self.data.mac = mac;
                CHANNEL_LOG.sender().send(self.data);
            }
            RouterEvent::StoreIpAddr(data) => {
                if let Some(addr) = data {
                    self.data.menu_settings.ip_addr = addr.into();
                    self.data.net_status = NetStatus::Up(addr.octets());
                } else {
                    self.data.menu_settings.ip_addr = IpAddrMenu::default();
                    self.data.net_status = NetStatus::Dhcp;
                }
                let _ = self.channel_ui.try_send(UiEvent::Load(self.data.menu_settings));
                let _ = self.channel_ui.try_send(UiEvent::Net(self.data.net_status));
                CHANNEL_LOG.sender().send(self.data);
            }
            RouterEvent::GetModuleType(return_channel) => {
                match return_channel {
                    ReturnChannel::Main => {
                        info!("get module type {:#?}", self.data.module_type);
                        let _ = self.channel_main.try_send(MainEvent::ReturnModuleType(self.data.module_type));
                    }
                    ReturnChannel::Ui => {} //self.channel_ui.try_send(MainEvent::ReturnModuleType(self.data.module_type)),
                }
            }
            RouterEvent::GetMacAddress(return_channel) => {
                match self.data.mac {
                    Some(mac) => info!("Router event get MAC {:#?}", mac),
                    None => info!("Router event did not get MAC."),
                }
                // info!("Router event get mac {:#X}", self.data.mac);
                match return_channel {
                    ReturnChannel::Main => {
                        let _ = self.channel_main.try_send(MainEvent::ReturnMacAddress(self.data.mac));
                    }
                    ReturnChannel::Ui => {} //self.channel_ui.try_send(MainEvent::ReturnMacAddress(self.data.mac)),
                }
            }
            RouterEvent::GetSettings(return_channel) => match return_channel {
                ReturnChannel::Main => {
                    let _ = self.channel_main.try_send(MainEvent::ReturnSettings(self.data.menu_settings));
                }
                ReturnChannel::Ui => {
                    let _ = self.channel_ui.try_send(UiEvent::Load(self.data.menu_settings));
                }
            },
            RouterEvent::GetBootStatus(return_channel) => {
                 match return_channel {
                    ReturnChannel::Main => {
                        info!("get boot status {:#?}", self.data.boot_status);
                        let _ = self.channel_main.try_send(MainEvent::ReturnBootStatus(self.data.boot_status));
                    }
                    ReturnChannel::Ui => {} //self.channel_ui.try_send(MainEvent::ReturnModuleType(self.data.module_type)),
                }
            },
        }
    }

    /// Update LED color in memory and apply to physical LEDs
    pub async fn process_dmx_event(&mut self, event: DmxEvent) {
        if self.data.boot_status == Some(BootStatus::Success) {
            let input_mode = self.data.menu_settings.input_mode;
            // Check that the incoming ArtNet packet is for us. Only the Net is
            // filtered (and only when Art-Net is the source — sACN packets
            // arrive as ArtNetPacket events with a zero Net): the buffer holds
            // one full net indexed by the packet's SubUni byte, so
            // multi-universe port spans can cross a sub-net boundary (the
            // configured sub-net:universe is the render base).
            let _packet_addr = match event {
                DmxEvent::DmxPacket(d) => d,
                DmxEvent::ArtNetPacket(packet_addr) => {
                    if input_mode.is_artnet() && packet_addr.port.net != self.data.menu_settings.artnet_address.0[0] {
                        if !self.warned_net_mismatch {
                            self.warned_net_mismatch = true;
                            warn!(
                                "ArtNet: ignoring traffic on Net {} (configured {})",
                                packet_addr.port.net, self.data.menu_settings.artnet_address.0[0]
                            );
                        }
                        return;
                    }
                    packet_addr
                }
            };

            // Lock global led color buffer
            let mut colors = LED_COLORS.lock().await;

            match self.data.menu_settings.module {
                ModuleSettings::SmartLed(smart_led_settings) => {

                    // One 80-byte snapshot for the whole frame, taken before
                    // the buffer locks below. Two reasons: the lock ordering
                    // stays `FIXTURE_MAP` (blocking, released immediately) then
                    // the async buffers, so no cross-core cycle exists; and a
                    // layout edited mid-frame cannot paint half of one
                    // arrangement and half of another.
                    let fixtures = crate::buffers::fixture_map();
                    let layout = smart_led_settings.layout(&fixtures);
                    // Lock global dmx data buffer
                    let dmx_buffer = DMX_BUFFER.lock().await;

                    let dmx_address_index = buffer_index(
                        self.data.menu_settings.dmx_address,
                        input_mode.is_network(),
                    );

                    // Base of the configured universe in the flat buffer.
                    // Art-Net: the configured sub-net:universe, clamped to what
                    // the buffer holds (buffer_base) — the UI limits the
                    // sub-net, but the value can also arrive from an old EEPROM
                    // or the console. sACN: the receive task already rebases
                    // universes onto slot 0.
                    let network_base = if input_mode.is_artnet() {
                        let addr = &self.data.menu_settings.artnet_address;
                        if addr.sub_uni() != addr.buffer_base() && !self.warned_base_clamp {
                            self.warned_base_clamp = true;
                            warn!(
                                "Art-Net base sub_uni {} beyond the buffered universes - clamped to {}",
                                addr.sub_uni(), addr.buffer_base()
                            );
                        }
                        addr.buffer_base() * DMX_UNIVERSE_SIZE
                    } else {
                        0
                    };

                    match smart_led_settings.port_mode {
                        SmartLedPortMode::Individual => {
                            // Offset index to account for many universes stored flat in dmx_buffer.  This value is always 0 for DMX but can vary for ArtNet data.
                            // let mut port_u_offset = match self.data.menu_settings.input_mode {
                            //     InputMode::Dmx => 0, // DMX can only handle one universe
                            //     InputMode::ArtNet => self.data.menu_settings.artnet_address.0[2] as usize,
                            // };

                            // Byte offset to place virtual leds at the right buffer location for each port
                            // (DMX-style layouts store their single universe at offset 0)
                            let mut port_virtual_led_offset = if input_mode.is_network() { network_base } else { 0 };

                            for (port_index, num_virtual_leds) in layout.virtual_leds_per_port().iter().enumerate() {
                                let base = dmx_address_index + port_virtual_led_offset;
                                // Blanks first, then paints each item's run —
                                // see `paint_port` for why that order is the
                                // whole gap/tail/shrink story in one rule.
                                paint_port(
                                    &mut colors[port_index],
                                    smart_led_settings.leds_per_port[port_index] as usize,
                                    *num_virtual_leds as usize,
                                    |item| layout.item_span(port_index, item),
                                    |item| item_color(&dmx_buffer[..], smart_led_settings.color_mode, base, item),
                                );

                                port_virtual_led_offset += if input_mode.is_network() {
                                    // Each port starts on a universe boundary: offset by the
                                    // universes this port uses times the size of a universe
                                    (*num_virtual_leds as f32 * smart_led_settings.color_mode.addr_size() as f32 / DMX_UNIVERSE_SIZE as f32).ceil() as usize * DMX_UNIVERSE_SIZE
                                } else {
                                    // Wired/USB: ports pack back-to-back in the one universe
                                    *num_virtual_leds as usize * smart_led_settings.color_mode.addr_size()
                                };
                            }
                        }
                        SmartLedPortMode::Mirror => {
                            // Offset into the flat one-net buffer; always 0 for DMX, the
                            // configured base for the network modes (same as Individual mode)
                            let universe_offset = if input_mode.is_network() { network_base } else { 0 };
                            let base = dmx_address_index + universe_offset;
                            // Port 1 sets how many addressable items there are.
                            let groups = layout.mirror_items() as usize;

                            // One block of DMX, eight ports reading it. Each
                            // applies its own geometry: the proportionally
                            // scaled size under `Groups`, its own run list under
                            // `Fixtures` - see `mirror_item_span`.
                            //
                            // Through the same `paint_port` as the other branch,
                            // so the blanking rule is written once. The tail-only
                            // pass that used to run after this loop is gone with
                            // it: that one could not reach a gap in the middle of
                            // a strip, only the end.
                            for port_index in 0..SMARTLED_PORT_COUNT {
                                paint_port(
                                    &mut colors[port_index],
                                    smart_led_settings.leds_per_port[port_index] as usize,
                                    groups,
                                    |item| layout.mirror_item_span(port_index, item),
                                    |item| item_color(&dmx_buffer[..], smart_led_settings.color_mode, base, item),
                                );
                            }
                        }
                    }

                    let _ = self.channel_smart_led.try_send(SmartLedEvent::UpdateLEDs {
                        counts: smart_led_settings.leds_per_port,
                        color_mode: smart_led_settings.color_mode,
                    });
                }
            }
        } else if !self.reported_output_blocked {
            // Without a readable module EEPROM the boot flag never reaches
            // Success and incoming DMX is deliberately not rendered (the
            // module EEPROM defines the output module). Say so once so a
            // bench user can tell why the LEDs are dark.
            self.reported_output_blocked = true;
            error!("DMX data ignored: boot incomplete or module EEPROM unavailable (boot status {:?})", self.data.boot_status);
        }
    }
}
