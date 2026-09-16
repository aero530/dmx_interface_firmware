//! Types used in the user interface
use core::net::Ipv4Addr;

use crate::{DMX_UNIVERSE_COUNT, DMX_UNIVERSE_SIZE, SMARTLED_PORT_COUNT};
use bincode::{Decode, Encode};
// use embassy_net::IpAddress;
use heapless::Vec;
// Supplies `f32::ceil` in no_std. On the host (host_tests) std provides it as
// an inherent method, so the import is unused there but still required for the
// embedded builds — hence the allow rather than a cfg.
#[allow(unused_imports)]
use micromath::F32Ext;

use smart_leds::{White, RGBW};

/// Values that can be incremented or decremented
pub trait IncDec {
    fn increment(&self, index: usize) -> Self;
    fn decrement(&self, index: usize) -> Self;
}

use defmt::Format;
use enum_ordinalize::Ordinalize;

/// Output module type (defines which kind of module is connected)
#[derive(Clone, Copy, Default, PartialEq, Format, Debug, Decode, Encode)]
pub enum BootStatus {
    #[default]
    Failed,
    Success,
}

/// Output module type (defines which kind of module is connected)
#[derive(Clone, Copy, Default, PartialEq, Format, Debug, Decode, Encode)]
pub enum ModuleType {
    #[default]
    Unknown,
    SmartLed,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Format, Debug, Decode, Encode)]
pub struct IpAddrMenu([u8; 4]);

impl IpAddrMenu {
    pub fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        IpAddrMenu([a, b, c, d])
    }

    pub fn octets(&self) -> [u8; 4] {
        self.0
    }

    /// `0.0.0.0` — "not set" (no gateway, no address).
    pub fn is_unspecified(&self) -> bool {
        self.0 == [0; 4]
    }
}

impl From<Ipv4Addr> for IpAddrMenu {
    fn from(value: Ipv4Addr) -> Self {
        let b = value.octets();
        IpAddrMenu(b)
    }
}

/// Data displayed / configured in the menu system.
///
/// Bincode-encoded into the EEPROM with no self-describing header, so the field
/// order is the on-EEPROM format: **append new fields at the end and bump the
/// schema version** (`pico2/src/eeprom.rs`). Schema 2 added `sacn_universe`
/// and `backlight`; schema 3 added the static-IP fields.
#[derive(Clone, Copy, PartialEq, Format, Debug, Decode, Encode)]
pub struct MenuData {
    pub dmx_address: u16,
    pub input_mode: InputMode,
    pub ethernet_ip_mode: EthernetIPMode,
    pub artnet_address: ArtNetAddr,
    pub module: ModuleSettings,
    pub ip_addr: IpAddrMenu,
    pub ethernet_enabled: bool,
    /// First sACN universe rendered (E1.31 universes are 1-based); the
    /// following `DMX_UNIVERSE_COUNT - 1` universes map to the next buffer
    /// slots.
    pub sacn_universe: u16,
    /// TFT backlight PWM duty (PCA9633 LED0), 1..=255. Never 0 from the menu —
    /// a dark panel with no way to see the menu is not a setting anyone wants.
    pub backlight: u8,
    /// Address used when `ethernet_ip_mode` is `Static`.
    pub static_ip: IpAddrMenu,
    /// Prefix length for the static address, 1..=30.
    pub static_prefix: u8,
    /// Gateway for the static address; `0.0.0.0` = none (Art-Net rigs rarely
    /// have one).
    pub static_gateway: IpAddrMenu,
    /// Where the strips get their power. `External` (the J26 supply) is the
    /// default and leaves the board's USB switch off, so a PC plugged into the
    /// FTDI port is never asked to run LEDs.
    pub led_power: LedPower,
}

/// Backlight duty a fresh unit boots with.
pub const DEFAULT_BACKLIGHT: u8 = 200;

/// PWM duty the *dimmest* menu setting produces.
///
/// The menu's backlight range is 1..=255 and used to be the PWM duty directly,
/// which put the bottom of the scale below anything useful: at the very bottom
/// the duty steps are a doubling apart, and level 1 (0.4 %) was not readable on
/// the Rev 2 panel while level 2 (0.8 %) was. Rather than move the menu's
/// minimum to 2 — leaving a setting nobody should pick — level 1 now *maps* to
/// that duty and the rest of the range is rescaled onto it.
pub const MIN_BACKLIGHT_DUTY: u8 = 2;

/// Map a menu backlight level (1..=255) onto a PWM duty
/// ([`MIN_BACKLIGHT_DUTY`]..=255).
///
/// Linear, with both ends pinned: level 1 gives the dimmest *usable* duty and
/// level 255 gives full brightness. Above the bottom of the scale this is
/// within one count of the identity it replaced, so existing saved settings
/// look unchanged.
pub const fn backlight_duty(level: u8) -> u8 {
    if level <= 1 {
        return MIN_BACKLIGHT_DUTY;
    }
    let span = 255 - MIN_BACKLIGHT_DUTY as u32;
    (MIN_BACKLIGHT_DUTY as u32 + (level as u32 - 1) * span / 254) as u8
}

impl Default for MenuData {
    fn default() -> Self {
        Self {
            dmx_address: 1,
            input_mode: InputMode::Dmx,
            ethernet_ip_mode: EthernetIPMode::Dhcp,
            artnet_address: ArtNetAddr::default(),
            module: ModuleSettings::default(),
            ip_addr: IpAddrMenu::new(0, 0, 0, 0),
            // On by default: this is an Art-Net/sACN node first. The
            // boot-lockout guard (two consecutive incomplete boots) is what
            // protects against an Ethernet bring-up that hangs, not this flag.
            ethernet_enabled: true,
            sacn_universe: 1,
            backlight: DEFAULT_BACKLIGHT,
            // Art-Net convention: 2.x.x.x/8, no gateway.
            static_ip: IpAddrMenu::new(2, 0, 0, 1),
            static_prefix: 8,
            static_gateway: IpAddrMenu::new(0, 0, 0, 0),
            led_power: LedPower::External,
        }
    }
}

impl MenuData {
    /// Universes this configuration binds in the active mode — what the UI
    /// shows as "Univ Bound" and what the Art-Net task advertises through
    /// BindIndex ArtPollReplies. Clamped to what the buffer holds above the
    /// configured base; wired DMX and USB are always exactly one universe.
    pub fn bound_universes(&self) -> u16 {
        let ModuleSettings::SmartLed(s) = &self.module;
        let wanted = s.bound_universes().max(1);
        match self.input_mode {
            InputMode::Dmx | InputMode::UsbToDmx => 1,
            InputMode::ArtNet | InputMode::ArtNetToDmx => {
                let room = (DMX_UNIVERSE_COUNT - self.artnet_address.buffer_base()) as u16;
                wanted.min(room)
            }
            InputMode::Sacn => wanted.min(DMX_UNIVERSE_COUNT as u16),
        }
    }
}

/// Module settings types
#[derive(Clone, Copy, PartialEq, Format, Debug, Decode, Encode)]
pub enum ModuleSettings {
    SmartLed(SmartLedSettings),
}

impl Default for ModuleSettings {
    fn default() -> Self {
        ModuleSettings::SmartLed(SmartLedSettings::default())
    }
}

/// Smart LED module settings
#[derive(Clone, Copy, PartialEq, Format, Debug, Decode, Encode)]
pub struct SmartLedSettings {
    pub leds_per_port: [u16; SMARTLED_PORT_COUNT],
    pub color_mode: SmartLedColorMode,
    pub port_mode: SmartLedPortMode,
    pub dmx_group_size: SmartLedDmxGroupSize,
}

/// LEDs per port a fresh unit assumes. Non-zero so a first power-up with strips
/// attached shows life instead of eight dark ports until every count is typed
/// in; short enough (one 5 m strip at 30/m) to be safe on any real string.
pub const DEFAULT_LEDS_PER_PORT: u16 = 150;

impl Default for SmartLedSettings {
    fn default() -> Self {
        Self {
            leds_per_port: [DEFAULT_LEDS_PER_PORT; SMARTLED_PORT_COUNT],
            color_mode: SmartLedColorMode::default(),
            port_mode: SmartLedPortMode::default(),
            dmx_group_size: SmartLedDmxGroupSize::default(),
        }
    }
}

impl SmartLedSettings {
    /// Return the number of virtual LEDs on each port.
    /// The number of virtual LEDs is calcuated based the number of physical LEDs
    /// and the grouping size for each port.
    pub fn virtual_leds_per_port(&self) -> [u16; SMARTLED_PORT_COUNT] {
        self.leds_per_port
            .iter()
            .zip(self.dmx_group_size.0.iter())
            .map(|(led_count, grouping)| (*led_count as f32 / *grouping as f32).ceil() as u16)
            .collect::<Vec<u16, SMARTLED_PORT_COUNT>>()
            .as_slice()
            .try_into()
            .unwrap_or_default()
    }

    /// Bytes each port puts on the wire per frame: virtual LEDs x colour width.
    ///
    /// This is the quantity the budget is expressed in — see
    /// [`crate::MAX_BYTES_PER_PORT`].
    pub fn bytes_per_port(&self) -> [u16; SMARTLED_PORT_COUNT] {
        self.virtual_leds_per_port()
            .iter()
            .map(|n| n.saturating_mul(self.color_mode.addr_size() as u16))
            .collect::<Vec<u16, SMARTLED_PORT_COUNT>>()
            .as_slice()
            .try_into()
            .unwrap_or_default()
    }

    /// Universes each port consumes.
    ///
    /// Ports start on universe boundaries, so this rounds up per port rather
    /// than dividing the total — which is why eight ports of 1800 B come to 32
    /// universes and not 29.
    pub fn universes_per_port(&self) -> [u16; SMARTLED_PORT_COUNT] {
        self.bytes_per_port()
            .iter()
            .map(|bytes| bytes.div_ceil(DMX_UNIVERSE_SIZE as u16))
            .collect::<Vec<u16, SMARTLED_PORT_COUNT>>()
            .as_slice()
            .try_into()
            .unwrap_or_default()
    }

    /// Universes consumed across every port.
    pub fn total_universes(&self) -> u16 {
        self.universes_per_port().iter().sum()
    }

    /// Ports whose frame exceeds [`crate::MAX_BYTES_PER_PORT`].
    ///
    /// Over budget does not mean broken — it means the strip cannot be clocked
    /// out inside the Art-Net frame interval, so the refresh rate silently
    /// drops. Surfacing it beats letting someone wonder why 44 Hz became 30.
    pub fn ports_over_budget(&self) -> bool {
        self.bytes_per_port()
            .iter()
            .any(|b| *b > crate::MAX_BYTES_PER_PORT)
    }

    /// Universes the configuration consumes from its base: every port's block
    /// in `Individual` mode, one shared block (port 1's) in `Mirror` mode.
    pub fn bound_universes(&self) -> u16 {
        match self.port_mode {
            SmartLedPortMode::Individual => self.total_universes(),
            SmartLedPortMode::Mirror => self.universes_per_port()[0],
        }
    }

    /// True if the configuration cannot be held or delivered as asked: either a
    /// port is over its byte budget, or the total exceeds what `DMX_BUFFER`
    /// covers.
    pub fn over_budget(&self) -> bool {
        self.ports_over_budget()
            || self.total_universes() as usize > crate::DMX_UNIVERSE_COUNT
    }

    pub fn universe_offset(&self) -> [u16; SMARTLED_PORT_COUNT] {
        let universe_count = self.universes_per_port();

        universe_count
            .iter()
            .enumerate()
            .map(|(i, _c)| universe_count[0..i].iter().sum())
            .collect::<Vec<u16, SMARTLED_PORT_COUNT>>()
            .as_slice()
            .try_into()
            .unwrap_or_default()
    }
}
/// Impl increment and decrement for u8
impl IncDec for u8 {
    fn increment(&self, _index: usize) -> u8 {
        if *self == 255 {
            0
        } else {
            self + 1
        }
    }

    fn decrement(&self, _index: usize) -> u8 {
        if *self == 0 {
            255
        } else {
            self - 1
        }
    }
}

/// Operating mode: where DMX data comes from and where it goes.
///
/// The first two modes receive data to drive the local LED outputs; the
/// `*ToDmx` modes additionally (ArtNet) or exclusively (USB) turn the wired
/// DMX port around and transmit (PIO2 SM1 on Rev 2 — the RP2040 bridge is
/// gone).
///
/// New variants must be appended so bincode-encoded EEPROM settings from
/// older firmware keep decoding to the same modes.
#[derive(Default, Clone, Copy, PartialEq, Eq, Ordinalize, Format, Debug, Decode, Encode)]
pub enum InputMode {
    #[default]
    /// Wired DMX in -> LEDs
    Dmx,
    /// Art-Net in -> LEDs
    ArtNet,
    /// Art-Net in -> LEDs + wired DMX out (Art-Net node)
    ArtNetToDmx,
    /// Enttec-protocol USB in -> LEDs + wired DMX out
    UsbToDmx,
    /// sACN / E1.31 multicast in -> LEDs. Universes are addressed from
    /// `MenuData::sacn_universe`, not the Art-Net fields.
    Sacn,
}

impl InputMode {
    #[allow(unused)]
    pub fn dmx_addr_limit(&self) -> usize {
        // Every mode uses a 512 byte universe
        DMX_UNIVERSE_SIZE
    }

    /// The wired DMX port transmits in this mode (GP10 high, RS-485 driver enabled)
    pub fn is_dmx_output(&self) -> bool {
        matches!(self, InputMode::ArtNetToDmx | InputMode::UsbToDmx)
    }

    /// Data arrives over Ethernet and is stored network-style (channel 1 at
    /// slot offset 0, no start code) — as opposed to the wired-DMX/USB layout
    /// with the start code at index 0.
    pub fn is_network(&self) -> bool {
        matches!(self, InputMode::ArtNet | InputMode::ArtNetToDmx | InputMode::Sacn)
    }

    /// Art-Net is the active source (the Net/Sub-Net/Universe fields apply).
    pub fn is_artnet(&self) -> bool {
        matches!(self, InputMode::ArtNet | InputMode::ArtNetToDmx)
    }
}
impl IncDec for InputMode {
    fn increment(&self, _index: usize) -> Self {
        let next = self.ordinal().saturating_add(1);
        Self::from_ordinal(next).unwrap_or(InputMode::Dmx)
    }

    fn decrement(&self, _index: usize) -> Self {
        if *self == InputMode::Dmx {
            return InputMode::Sacn;
        }
        Self::from_ordinal(self.ordinal() - 1).unwrap_or(InputMode::Dmx)
    }
}

impl core::fmt::Display for InputMode {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            InputMode::Dmx => write!(f, "DMX"),
            InputMode::ArtNet => write!(f, "ArtNet"),
            InputMode::ArtNetToDmx => write!(f, "ArtNet>DMX"),
            InputMode::UsbToDmx => write!(f, "USB>DMX"),
            InputMode::Sacn => write!(f, "sACN"),
        }
    }
}

/// Supply the WS2812 strings run from.
///
/// `UsbBrick` closes the board's TPS2553-1 switch (TCA9555 P04) so a 5 V brick
/// on the FTDI USB-C connector feeds `V_LED`. It is deliberately not the
/// default: the same connector takes a PC, and a host port cannot run strips.
/// The firmware also holds the frame inside a current budget in this mode —
/// see `common::usb_power`.
#[derive(Default, Clone, Copy, PartialEq, Eq, Ordinalize, Format, Debug, Decode, Encode)]
pub enum LedPower {
    #[default]
    External,
    UsbBrick,
}

impl IncDec for LedPower {
    fn increment(&self, _index: usize) -> Self {
        let next = self.ordinal().saturating_add(1);
        Self::from_ordinal(next).unwrap_or(LedPower::External)
    }

    fn decrement(&self, _index: usize) -> Self {
        let prev = self.ordinal().saturating_sub(1);
        Self::from_ordinal(prev).unwrap_or(LedPower::UsbBrick)
    }
}

impl core::fmt::Display for LedPower {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            LedPower::External => write!(f, "External"),
            LedPower::UsbBrick => write!(f, "USB brick"),
        }
    }
}

/// Ethernet IP mode
#[derive(Default, Clone, Copy, PartialEq, Eq, Ordinalize, Format, Debug, Decode, Encode)]
pub enum EthernetIPMode {
    #[default]
    Dhcp,
    Static,
}

impl IncDec for EthernetIPMode {
    fn increment(&self, _index: usize) -> Self {
        let next = self.ordinal().saturating_add(1);
        Self::from_ordinal(next).unwrap_or(EthernetIPMode::Dhcp)
    }

    fn decrement(&self, _index: usize) -> Self {
        let prev = self.ordinal().saturating_sub(1);
        Self::from_ordinal(prev).unwrap_or(EthernetIPMode::Static)
    }
}

impl core::fmt::Display for EthernetIPMode {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            EthernetIPMode::Dhcp => write!(f, "DHCP"),
            EthernetIPMode::Static => write!(f, "Static"),
        }
    }
}

/// Smart LED group address type
#[derive(Default, Clone, Copy, PartialEq, Eq, Ordinalize, Format, Debug, Decode, Encode)]
pub enum SmartLedColorMode {
    #[default]
    Rgb,
    Rgbw,
}

impl SmartLedColorMode {
    pub fn addr_size(&self) -> usize {
        match self {
            SmartLedColorMode::Rgb => 3,
            SmartLedColorMode::Rgbw => 4,
        }
    }

    /// Decode one virtual LED's DMX slots into a colour. The buffer is always
    /// RGBW; in RGB mode the white channel is simply zero (and never sent).
    pub fn color(&self, data: &[u8]) -> RGBW::<u8> {
        let w = match self {
            SmartLedColorMode::Rgb => 0,
            SmartLedColorMode::Rgbw => data.get(3).copied().unwrap_or(0),
        };
        if data.len() >= 3 {
            RGBW::<u8> { r: data[0], g: data[1], b: data[2], a: White(w) }
        } else {
            BLACK
        }
    }
}

/// An off pixel.
pub const BLACK: RGBW<u8> = RGBW::<u8> { r: 0, g: 0, b: 0, a: White(0) };

impl SmartLedColorMode {
    /// Colour modes the output driver renders. Both: the PIO driver packs 24-bit
    /// GRB or 32-bit GRBW per frame from the same RGBW colour buffer
    /// (`pico2/src/ws2812.rs`). Kept as a list so a future mode can be staged
    /// behind it before it is offered.
    pub const SUPPORTED: &'static [SmartLedColorMode] = &[SmartLedColorMode::Rgb, SmartLedColorMode::Rgbw];

    pub fn is_supported(&self) -> bool {
        Self::SUPPORTED.contains(self)
    }
}

impl IncDec for SmartLedColorMode {
    fn increment(&self, _index: usize) -> Self {
        let i = Self::SUPPORTED.iter().position(|m| m == self).unwrap_or(0);
        Self::SUPPORTED[(i + 1) % Self::SUPPORTED.len()]
    }

    fn decrement(&self, _index: usize) -> Self {
        let i = Self::SUPPORTED.iter().position(|m| m == self).unwrap_or(0);
        Self::SUPPORTED[(i + Self::SUPPORTED.len() - 1) % Self::SUPPORTED.len()]
    }
}

impl core::fmt::Display for SmartLedColorMode {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            SmartLedColorMode::Rgb => write!(f, "RGB"),
            SmartLedColorMode::Rgbw => write!(f, "RGBW"),
        }
    }
}

/// Smart LED port mode
#[derive(Default, Clone, Copy, PartialEq, Eq, Ordinalize, Format, Debug, Decode, Encode)]
pub enum SmartLedPortMode {
    #[default]
    /// Individual - DMX size = SUM(#leds/port) / DmxGroupSize * color width
    Individual,
    /// Mirror - DMX size = MAX(#leds/port) / DmxGroupSize * color width
    Mirror,
}

impl IncDec for SmartLedPortMode {
    fn increment(&self, _index: usize) -> Self {
        let next = self.ordinal().saturating_add(1);
        match Self::from_ordinal(next) {
            Some(n) => n,
            None => SmartLedPortMode::Individual,
        }
    }

    fn decrement(&self, _index: usize) -> Self {
        let prev = self.ordinal().saturating_sub(1);
        match Self::from_ordinal(prev) {
            Some(n) => n,
            None => SmartLedPortMode::Mirror,
        }
    }
}

impl core::fmt::Display for SmartLedPortMode {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            SmartLedPortMode::Individual => write!(f, "Individual"),
            SmartLedPortMode::Mirror => write!(f, "Mirror"),
        }
    }
}

// LED DMX Group Size (1 to #PHYLEDs)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Format, Decode, Encode)]
pub struct SmartLedDmxGroupSize(pub [u16; SMARTLED_PORT_COUNT]);

impl Default for SmartLedDmxGroupSize {
    fn default() -> Self {
        Self([1; SMARTLED_PORT_COUNT])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Format, Decode, Encode)]
pub struct ArtNetAddr(pub [u8; 3]);

impl ArtNetAddr {
    /// The Art-Net "SubUni" byte of the configured Port-Address: Sub-Net in
    /// the high nibble, Universe in the low nibble (0..=255). This is the
    /// base index of the configured address within the flat one-net
    /// `DMX_BUFFER` (buffer offset = `sub_uni() * DMX_UNIVERSE_SIZE`).
    /// Nibbles are masked to 4 bits so corrupt stored settings can't index
    /// past the buffer.
    pub fn sub_uni(&self) -> usize {
        (((self.0[1] & 0x0F) as usize) << 4) | ((self.0[2] & 0x0F) as usize)
    }

    /// [`Self::sub_uni`] clamped to the last universe `DMX_BUFFER` actually
    /// holds. `sub_uni()` can reach 255 (sub-net 15:15) but the buffer covers
    /// [`crate::DMX_UNIVERSE_COUNT`] universes — indexing the buffer with the
    /// raw value would slice out of bounds (a panic in the USB forwarder, a
    /// per-frame warning storm in the router). Use THIS for buffer indexing
    /// and `sub_uni()` only for on-the-wire Art-Net values.
    pub fn buffer_base(&self) -> usize {
        self.sub_uni().min(crate::DMX_UNIVERSE_COUNT - 1)
    }
}
