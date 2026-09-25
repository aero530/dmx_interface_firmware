//! Types used in the user interface
use core::net::Ipv4Addr;

use crate::ui::fixtures::FixtureMap;
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
    /// How a strip is carved into addressable items. Appended for schema 5;
    /// `MenuData` is bincode with no self-describing header, so new fields go
    /// on the end.
    pub grouping: LedGrouping,
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
            grouping: LedGrouping::default(),
        }
    }
}

/// The derived geometry of all eight ports: how a strip is carved up, how many
/// DMX channels that costs, and where each port sits in the buffer.
///
/// Borrows the settings **and** the fixture map because the two together decide
/// the answer. Build one and call several methods; they share work through the
/// same chain (`virtual_leds_per_port` → `bytes_per_port` → `universes_per_port`
/// → the rest), so a single figure computed here is consistent across all of
/// them by construction.
pub struct PortLayout<'a> {
    settings: &'a SmartLedSettings,
    fixtures: &'a FixtureMap,
}

impl SmartLedSettings {
    /// Geometry for these settings against a given fixture map.
    pub fn layout<'a>(&'a self, fixtures: &'a FixtureMap) -> PortLayout<'a> {
        PortLayout { settings: self, fixtures }
    }
}

impl PortLayout<'_> {
    /// The group size a port actually uses, after [`LedGrouping`] has had its
    /// say. **Every read of `dmx_group_size` should come through here.**
    ///
    /// `Individual` is not a separate layout - it is `Groups` with the size
    /// pinned to 1, which is also the shipping default. Routing it through one
    /// accessor is what makes that identity true by construction rather than by
    /// two code paths agreeing: the alternative, a bespoke 1:1 rule, silently
    /// changed `Mirror` (which stretches proportionally, so a 200-LED port
    /// mirroring a 100-LED port groups by 2) and would have darkened half of it
    /// one toggle from defaults.
    ///
    /// `Fixtures` has no single size; callers that care must consult the
    /// fixture map. This returns the stored `Groups` value for it so the
    /// universe maths keeps working until that lands.
    pub fn effective_group_size(&self, port: usize) -> u16 {
        match self.settings.grouping {
            LedGrouping::Individual => 1,
            LedGrouping::Groups | LedGrouping::Fixtures => {
                self.settings.dmx_group_size.0[port.min(SMARTLED_PORT_COUNT - 1)]
            }
        }
    }

    /// Addressable items each port carries — the quantity every byte and
    /// universe figure below is built on.
    ///
    /// **This is the one method fixtures change.** Everything else reaches them
    /// through it, which is why the whole chain had to move onto this type: a
    /// method on `SmartLedSettings` cannot see a map stored outside it, and
    /// would have gone on quietly returning the `Groups` answer.
    ///
    /// Integer `div_ceil` rather than the f32 `ceil()` this replaced: identical
    /// for every value in range, and it cannot produce the NaN a zero group size
    /// used to (`host_tests/tests/types.rs` documents that hazard).
    pub fn virtual_leds_per_port(&self) -> [u16; SMARTLED_PORT_COUNT] {
        core::array::from_fn(|port| self.items_on(port))
    }

    /// Where item `item` lands on `port`, as `(start, leds)` zero-based.
    ///
    /// The one place the two layouts converge: a uniform group is
    /// `(item * size, size)`, a fixture is its run from the map. Every caller
    /// that paints works in these terms and needs to know nothing else, which
    /// is what keeps the render one loop rather than a branch per mode.
    ///
    /// `None` when the port has no such item.
    pub fn item_span(&self, port: usize, item: usize) -> Option<(u16, u16)> {
        if self.settings.grouping == LedGrouping::Fixtures {
            // Gaps advance the cursor inside `spans` without yielding, so item
            // N is the Nth *lit* run wherever the gaps fall. A port with
            // nothing stored uses `FixtureMap::DEFAULT_ENTRY`, not its group
            // size - see that constant for why.
            return self.fixtures.spans(port).nth(item);
        }
        if item >= self.items_on(port) as usize {
            return None;
        }
        let size = self.effective_group_size(port).max(1);
        Some(((item as u16).saturating_mul(size), size))
    }

    /// Addressable items [`SmartLedPortMode::Mirror`] shows, set by port 1
    /// whatever the layout is.
    pub fn mirror_items(&self) -> u16 {
        if self.settings.grouping == LedGrouping::Fixtures {
            return self.items_on(0);
        }
        self.mirror_group_count()
    }

    /// Where item `item` lands on `port` under [`SmartLedPortMode::Mirror`].
    ///
    /// Every port shows the same items; what differs is the geometry each one
    /// applies to them.
    ///
    /// * **Fixtures** - the port's own list, so a mirrored rig can have
    ///   differently-built strips showing the same content.
    /// * **Groups / Individual** - the proportionally scaled size from
    ///   [`Self::mirror_group_size`], unchanged from before fixtures existed and
    ///   guarded by the exhaustive identical-ports test in
    ///   `host_tests/tests/mirror.rs`.
    pub fn mirror_item_span(&self, port: usize, item: usize) -> Option<(u16, u16)> {
        if self.settings.grouping == LedGrouping::Fixtures {
            return self.fixtures.spans(port).nth(item);
        }
        let size = self.mirror_group_size()[port.min(SMARTLED_PORT_COUNT - 1)];
        Some(((item as u16).saturating_mul(size), size))
    }

    /// Items on one port.
    fn items_on(&self, port: usize) -> u16 {
        if self.settings.grouping == LedGrouping::Fixtures {
            if self.settings.leds_per_port[port] == 0 {
                // A port with no LEDs costs no channels. Without this the
                // default entry would reserve a universe slot for a strip that
                // does not exist.
                return 0;
            }
            // Gaps consume LEDs but no channels, so only lit runs count.
            return self.fixtures.fixture_count(port);
        }
        self.settings.leds_per_port[port].div_ceil(self.effective_group_size(port).max(1))
    }

    /// Port 1's group size as Mirror can actually use it.
    ///
    /// A port cannot group more LEDs than it has, and a stored zero - only
    /// reachable from a corrupt EEPROM, since the menu floor is 1 - would
    /// divide by zero in both callers.
    fn mirror_first_size(&self) -> u16 {
        self.effective_group_size(0).min(self.settings.leds_per_port[0]).max(1)
    }

    /// How many addressable items [`SmartLedPortMode::Mirror`] shows, set by
    /// port 1.
    ///
    /// **Rounds down**, unlike [`Self::virtual_leds_per_port`], which rounds up
    /// so that no LED goes unaddressed. Mirror cannot round up: the ragged last
    /// group is a partial group on port 1 but a *full* one on every other port,
    /// so rounding up makes two identical strips render differently. 64 LEDs
    /// grouped by 3 is **21** items, not 22. Found on the bench 2026-09-17:
    /// with 22, a second identical 64-LED port computed `64 / 22 = 2` and lit
    /// 44 LEDs beside port 1's 64.
    ///
    /// The odd LED left on the end goes dark - the same rounding rule
    /// [`Self::mirror_group_size`] applies to every port.
    pub fn mirror_group_count(&self) -> u16 {
        (self.settings.leds_per_port[0] / self.mirror_first_size()).max(1)
    }

    /// Group size each port uses in [`SmartLedPortMode::Mirror`].
    ///
    /// Every port shows the same **addressable items**, so a port with twice
    /// port 1's LEDs groups by twice as much: the size scales with the length,
    /// `size_n = leds_n * size_1 / leds_1`. Port 1 with 24 LEDs grouped by 4 is
    /// six items, so a 12-LED port mirrors it grouped by 2 and a 48-LED port
    /// grouped by 8 - the same six items, each stretched to the strip it is on.
    ///
    /// **Scaled from port 1, not divided by the item count.** The two agree
    /// whenever port 1's group size divides its LED count, which is the case
    /// every worked example above is. Where they disagree, scaling is the one
    /// that keeps the mode's promise: 10 LEDs grouped by 4 is two items, and
    /// `leds / items` on a second identical port gives `10 / 2 = 5` against
    /// port 1's 4 - the same strip, configured the same way, rendered
    /// differently. Scaling returns port 1's own size for any port of port 1's
    /// length, by construction, which is what a mirror has to do.
    ///
    /// **Rounds down.** 50 LEDs mirroring 24-grouped-by-4 is 8.33, so the port
    /// groups by 8 and covers 48; the two LEDs on the end stay dark rather than
    /// making one group visibly longer than the rest. A port too short to fill
    /// even one item is floored at 1 instead of collapsing to zero - it shows
    /// as many of the leading items as it has LEDs.
    ///
    /// Port 1 keeps its configured size, clamped only by its own LED count, and
    /// in `Individual` mode none of this is used: the configured sizes stand.
    pub fn mirror_group_size(&self) -> [u16; SMARTLED_PORT_COUNT] {
        let first_size = self.mirror_first_size() as u32;
        let first_leds = self.settings.leds_per_port[0].max(1) as u32;
        // Seeded from the effective sizes so `Individual` mirrors as
        // size-1-everywhere would, rather than from the stored values it hides.
        let mut sizes = self.settings.dmx_group_size.0;
        for (port, size) in sizes.iter_mut().enumerate() {
            *size = self.effective_group_size(port);
        }
        sizes[0] = first_size as u16;
        for (port, size) in sizes.iter_mut().enumerate().skip(1) {
            // In u32: 600 LEDs scaled by a 600 group overflows u16 on the way.
            let scaled = self.settings.leds_per_port[port] as u32 * first_size / first_leds;
            *size = scaled.max(1) as u16;
        }
        sizes
    }

    /// Bytes each port puts on the wire per frame: virtual LEDs x colour width.
    ///
    /// This is the quantity the budget is expressed in — see
    /// [`crate::MAX_BYTES_PER_PORT`].
    pub fn bytes_per_port(&self) -> [u16; SMARTLED_PORT_COUNT] {
        self.virtual_leds_per_port()
            .iter()
            .map(|n| n.saturating_mul(self.settings.color_mode.addr_size() as u16))
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

    /// DMX channels the configuration actually uses.
    ///
    /// **Not the span it occupies.** Ports start on universe boundaries, so a
    /// configuration using 90 channels on each of eight ports reaches into
    /// eight universes while using 720 channels. This is the smaller number —
    /// what a console operator patches — and `Universes` beside it is the
    /// larger one.
    ///
    /// Under `Mirror` every port reads the same block, so it is counted once.
    pub fn total_channels(&self) -> u32 {
        let bytes = self.bytes_per_port();
        match self.settings.port_mode {
            SmartLedPortMode::Individual => bytes.iter().map(|b| *b as u32).sum(),
            SmartLedPortMode::Mirror => bytes[0] as u32,
        }
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
        match self.settings.port_mode {
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


/// Geometry against the **live** fixture map.
///
/// These landed reading `FixtureMap::EMPTY`, so that the Phase 2 refactor
/// changed no call sites. That was fine until fixtures existed and then it was
/// a bug: every one of these is on a path that must honour the map, and all six
/// were quietly reporting the layout of an empty one. It showed up as a channel
/// count of 6 where the rig used 9 — and, less visibly, as the wrong universe
/// count in the BindIndex ArtPollReply that controllers auto-patch from.
///
/// Reading the global here is the honest shape: at runtime there is exactly one
/// map, it is the single source of truth, and a caller with nothing else to say
/// should get it. A caller that needs to reason about a *hypothetical* map -
/// the tests, an edit in progress - builds a [`PortLayout`] explicitly.
impl SmartLedSettings {
    pub fn effective_group_size(&self, port: usize) -> u16 {
        let live = crate::buffers::fixture_map();
        self.layout(&live).effective_group_size(port)
    }
    pub fn virtual_leds_per_port(&self) -> [u16; SMARTLED_PORT_COUNT] {
        let live = crate::buffers::fixture_map();
        self.layout(&live).virtual_leds_per_port()
    }
    pub fn mirror_group_count(&self) -> u16 {
        let live = crate::buffers::fixture_map();
        self.layout(&live).mirror_group_count()
    }
    pub fn mirror_group_size(&self) -> [u16; SMARTLED_PORT_COUNT] {
        let live = crate::buffers::fixture_map();
        self.layout(&live).mirror_group_size()
    }
    pub fn bytes_per_port(&self) -> [u16; SMARTLED_PORT_COUNT] {
        let live = crate::buffers::fixture_map();
        self.layout(&live).bytes_per_port()
    }
    pub fn universes_per_port(&self) -> [u16; SMARTLED_PORT_COUNT] {
        let live = crate::buffers::fixture_map();
        self.layout(&live).universes_per_port()
    }
    pub fn total_universes(&self) -> u16 {
        let live = crate::buffers::fixture_map();
        self.layout(&live).total_universes()
    }
    pub fn total_channels(&self) -> u32 {
        let live = crate::buffers::fixture_map();
        self.layout(&live).total_channels()
    }
    pub fn ports_over_budget(&self) -> bool {
        let live = crate::buffers::fixture_map();
        self.layout(&live).ports_over_budget()
    }
    pub fn bound_universes(&self) -> u16 {
        let live = crate::buffers::fixture_map();
        self.layout(&live).bound_universes()
    }
    pub fn over_budget(&self) -> bool {
        let live = crate::buffers::fixture_map();
        self.layout(&live).over_budget()
    }
    pub fn universe_offset(&self) -> [u16; SMARTLED_PORT_COUNT] {
        let live = crate::buffers::fixture_map();
        self.layout(&live).universe_offset()
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

/// How a strip is carved into addressable items.
///
/// Orthogonal to [`SmartLedPortMode`], which decides whether ports *share* a
/// DMX block rather than how any one of them is divided. Both axes stay
/// independently useful: a fixture layout can be mirrored across ports.
///
/// `Groups` is the default so an existing board behaves exactly as it did -
/// with `dmx_group_size` defaulting to 1, `Groups` and `Individual` are the
/// same picture on a fresh unit.
#[derive(Default, Clone, Copy, PartialEq, Eq, Ordinalize, Format, Debug, Decode, Encode)]
pub enum LedGrouping {
    /// One DMX channel set per pixel. Exactly `Groups` with every size at 1.
    Individual,
    #[default]
    /// A uniform run length per port, from `dmx_group_size`.
    Groups,
    /// A per-port ordered list of runs, each a length and a fixture/gap flag.
    Fixtures,
}

impl IncDec for LedGrouping {
    fn increment(&self, _index: usize) -> Self {
        Self::from_ordinal(self.ordinal().saturating_add(1)).unwrap_or(LedGrouping::Individual)
    }

    fn decrement(&self, _index: usize) -> Self {
        Self::from_ordinal(self.ordinal().saturating_sub(1)).unwrap_or(LedGrouping::Fixtures)
    }
}

impl core::fmt::Display for LedGrouping {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            LedGrouping::Individual => write!(f, "Individual"),
            LedGrouping::Groups => write!(f, "Groups"),
            LedGrouping::Fixtures => write!(f, "Fixtures"),
        }
    }
}

/// Whether the eight ports share one DMX block.
///
/// Shown on the panel as `Mirror Ports: Off/On` - the variant names predate
/// [`LedGrouping`] and are kept because they are the on-EEPROM ordinals and the
/// console spellings. "Individual" here has never meant what it means there:
/// this is about ports, that is about pixels.
#[derive(Default, Clone, Copy, PartialEq, Eq, Ordinalize, Format, Debug, Decode, Encode)]
pub enum SmartLedPortMode {
    #[default]
    /// Every port gets its own DMX block.
    Individual,
    /// One block (port 1's) drives all eight.
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
        // Reads as the answer to "Mirror Ports?", which is what the label asks.
        // Saying "Individual" here would collide with `LedGrouping::Individual`
        // on the same page, meaning something else entirely.
        match self {
            SmartLedPortMode::Individual => write!(f, "Off"),
            SmartLedPortMode::Mirror => write!(f, "On"),
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
