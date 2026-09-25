//! Declarative settings-field metadata.
//!
//! Every editable (or displayable) setting is a [`FieldId`]. All behavior —
//! label, valid range, digit count, rendering, editing, console get/set — is
//! defined here once, so navigation and clamping are generic code and a new
//! setting is added in exactly one place. Both the on-device TFT UI and the
//! USB console protocol are driven from this table.

use alloc::format;
use alloc::string::String;

use defmt::Format;

use crate::ui::{
    LedPower,
    EthernetIPMode, IncDec, InputMode, IpAddrMenu, MenuData, ModuleSettings, SmartLedColorMode,
    LedGrouping, SmartLedPortMode, SmartLedSettings,
};
use crate::ui::fixtures::{FixtureEntry, FixtureMap, MAX_FIXTURES_PER_PORT};
use crate::{DMX_UNIVERSE_SIZE, MAX_LEDS_PER_PORT, SMARTLED_PORT_COUNT};

/// One setting shown in the UI / exposed on the console.
#[derive(Clone, Copy, PartialEq, Eq, Format, Debug)]
pub enum FieldId {
    DmxAddress,
    InputMode,
    /// DHCP or the static address below. Applies at the next boot.
    IpMode,
    /// The address in use (read-only; DHCP result or the static address).
    IpAddr,
    StaticIp,
    StaticPrefix,
    StaticGateway,
    ArtNetNet,
    ArtNetSubNet,
    ArtNetUniverse,
    /// First sACN universe (1-based, per E1.31).
    SacnUniverse,
    /// Read-only: how many universes the active mode binds from its base —
    /// the same number the Art-Net BindIndex replies advertise.
    UniversesBound,
    /// Read-only: DMX channels the current configuration uses.
    TotalChannels,
    PortMode,
    /// How a strip is carved into items: Individual / Groups / Fixtures.
    /// Applies to every port.
    Grouping,
    ColorMode,
    GroupSize(usize),
    /// One entry of one port's fixture list: `(port, slot)`.
    ///
    /// Unlike every other field this does **not** live in `MenuData` - the map
    /// is a separate store - so it is edited through the `fixture_*` methods
    /// that take a [`FixtureMap`], not through `get_u16`/`set_u16`.
    Fixture(usize, usize),
    LedsPerPort(usize),
    UniverseOffsets,
    /// Read-only: universes consumed vs. what the buffer covers, and whether
    /// the configuration is inside its byte budget.
    TotalUniverses,
    EthernetEnabled,
    LedPower,
    /// TFT backlight duty, 1..=255.
    Backlight,
}

/// Per-port field labels.
///
/// Kept to 14 characters so a label plus its value fits the 35-column grid of
/// the 320x172 TFT — values start at column 16 (`pico2/src/tft_ui.rs`). Extend
/// both tables if `SMARTLED_PORT_COUNT` grows past 8 — the `.min()` guard keeps
/// it safe but would repeat the last label.
/// Just the port. What the row *means* is set by `Grouping` at the top of the
/// same page - a size under `Groups`, a fixed 1 under `Individual`, a fixture
/// list under `Fixtures` - so naming it "Group Size" was right in only one of
/// the three.
const GROUP_LABELS: [&str; 8] = [
    "Port 1", "Port 2", "Port 3", "Port 4",
    "Port 5", "Port 6", "Port 7", "Port 8",
];
/// Console protocol keys, one per port. `dmx_console` discovers these by
/// iterating `all_fields`, so extending the tables is all that is needed for it
/// to drive the new ports.
const GROUP_KEYS: [&str; 8] = [
    "group_size_1", "group_size_2", "group_size_3", "group_size_4",
    "group_size_5", "group_size_6", "group_size_7", "group_size_8",
];
const LED_KEYS: [&str; 8] = [
    "leds_1", "leds_2", "leds_3", "leds_4", "leds_5", "leds_6", "leds_7", "leds_8",
];

const LED_LABELS: [&str; 8] = [
    "Port 1", "Port 2", "Port 3", "Port 4",
    "Port 5", "Port 6", "Port 7", "Port 8",
];

/// Most fields any one page may carry.
///
/// The 320x172 TFT is **35x11 characters** with `FONT_9X15` (see
/// `pico2/src/tft_ui.rs`), and one row goes to the page title / status line.
/// Exceeding this means fields fall off the bottom of the display with no
/// scroll to reach them — the menu is paged, not scrolling. Enforced by a test
/// in `host_tests`.
///
/// Nine, not ten: the bottom row is the button-hint footer (`pico2/src/tft_ui.rs`),
/// so a page has the title row, nine field rows, and the footer.
pub const MAX_FIELDS_PER_PAGE: usize = 9;

/// Column where field values start on the display; labels must fit before it.
pub const VALUE_COLUMN: u16 = 16;

/// Whether a page is in the main rotation or reached from another page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageKind {
    /// In the Up/Down/Esc ring.
    Main,
    /// Reached by selecting the port it belongs to, and skipped by the ring.
    ///
    /// Without this, the eight per-port fixture pages would sit in the Esc
    /// rotation and an operator pressing Esc from `System` would walk through
    /// eight tables before reaching `Main` again - and through all eight even in
    /// a mode that has no fixtures.
    Detail,
}

/// A page (tab) of the on-device menu.
pub struct Page {
    pub title: &'static str,
    pub fields: &'static [FieldId],
    pub kind: PageKind,
}

impl Page {
    /// Pages the Up/Down/Esc ring visits.
    pub fn in_ring(&self) -> bool {
        self.kind == PageKind::Main
    }
}

pub const PAGES: &[Page] = &[
    Page {
        title: "Main",
        fields: &[
            FieldId::DmxAddress,
            FieldId::InputMode,
            FieldId::ArtNetNet,
            FieldId::ArtNetSubNet,
            FieldId::ArtNetUniverse,
            FieldId::SacnUniverse,
            FieldId::UniversesBound,
            FieldId::TotalChannels,
        ],
        kind: PageKind::Main,
    },
    Page {
        title: "Network",
        fields: &[
            FieldId::IpMode,
            FieldId::IpAddr,
            FieldId::StaticIp,
            FieldId::StaticPrefix,
            FieldId::StaticGateway,
        ],
        kind: PageKind::Main,
    },
    Page {
        title: "LED",
        fields: &[
            FieldId::PortMode,
            FieldId::ColorMode,
            // Offsets before the total: the panel draws the universe budget
            // as a bar on the row under `TotalUniverses`, so the total sits
            // last with its bar beneath it rather than splitting the two
            // read-only rows.
            FieldId::UniverseOffsets,
            FieldId::TotalUniverses,
        ],
        kind: PageKind::Main,
    },
    Page {
        title: "LEDs",
        fields: &[
            FieldId::LedsPerPort(0),
            FieldId::LedsPerPort(1),
            FieldId::LedsPerPort(2),
            FieldId::LedsPerPort(3),
            FieldId::LedsPerPort(4),
            FieldId::LedsPerPort(5),
            FieldId::LedsPerPort(6),
            FieldId::LedsPerPort(7),
        ],
        kind: PageKind::Main,
    },
    Page {
        title: "Layout",
        fields: &[
            // First, because it decides what every row below it means.
            FieldId::Grouping,
            FieldId::GroupSize(0),
            FieldId::GroupSize(1),
            FieldId::GroupSize(2),
            FieldId::GroupSize(3),
            FieldId::GroupSize(4),
            FieldId::GroupSize(5),
            FieldId::GroupSize(6),
            FieldId::GroupSize(7),
        ],
        kind: PageKind::Main,
    },
    Page {
        title: "System",
        fields: &[FieldId::EthernetEnabled, FieldId::Backlight, FieldId::LedPower],
        kind: PageKind::Main,
    },
    // The per-port fixture tables. `Detail`, so the ring skips them; reached by
    // selecting a port's row on the Layout page.
    Page {
        title: FIXTURE_TITLES[0],
        fields: &[
            FieldId::Fixture(0, 0),
            FieldId::Fixture(0, 1),
            FieldId::Fixture(0, 2),
            FieldId::Fixture(0, 3),
            FieldId::Fixture(0, 4),
        ],
        kind: PageKind::Detail,
    },
    Page {
        title: FIXTURE_TITLES[1],
        fields: &[
            FieldId::Fixture(1, 0),
            FieldId::Fixture(1, 1),
            FieldId::Fixture(1, 2),
            FieldId::Fixture(1, 3),
            FieldId::Fixture(1, 4),
        ],
        kind: PageKind::Detail,
    },
    Page {
        title: FIXTURE_TITLES[2],
        fields: &[
            FieldId::Fixture(2, 0),
            FieldId::Fixture(2, 1),
            FieldId::Fixture(2, 2),
            FieldId::Fixture(2, 3),
            FieldId::Fixture(2, 4),
        ],
        kind: PageKind::Detail,
    },
    Page {
        title: FIXTURE_TITLES[3],
        fields: &[
            FieldId::Fixture(3, 0),
            FieldId::Fixture(3, 1),
            FieldId::Fixture(3, 2),
            FieldId::Fixture(3, 3),
            FieldId::Fixture(3, 4),
        ],
        kind: PageKind::Detail,
    },
    Page {
        title: FIXTURE_TITLES[4],
        fields: &[
            FieldId::Fixture(4, 0),
            FieldId::Fixture(4, 1),
            FieldId::Fixture(4, 2),
            FieldId::Fixture(4, 3),
            FieldId::Fixture(4, 4),
        ],
        kind: PageKind::Detail,
    },
    Page {
        title: FIXTURE_TITLES[5],
        fields: &[
            FieldId::Fixture(5, 0),
            FieldId::Fixture(5, 1),
            FieldId::Fixture(5, 2),
            FieldId::Fixture(5, 3),
            FieldId::Fixture(5, 4),
        ],
        kind: PageKind::Detail,
    },
    Page {
        title: FIXTURE_TITLES[6],
        fields: &[
            FieldId::Fixture(6, 0),
            FieldId::Fixture(6, 1),
            FieldId::Fixture(6, 2),
            FieldId::Fixture(6, 3),
            FieldId::Fixture(6, 4),
        ],
        kind: PageKind::Detail,
    },
    Page {
        title: FIXTURE_TITLES[7],
        fields: &[
            FieldId::Fixture(7, 0),
            FieldId::Fixture(7, 1),
            FieldId::Fixture(7, 2),
            FieldId::Fixture(7, 3),
            FieldId::Fixture(7, 4),
        ],
        kind: PageKind::Detail,
    },
];

/// Page titles for the fixture tables. A table rather than a format string
/// because `title` is `&'static str` - and because the old
/// `GroupSize(_) => "...P4"` wildcard silently mislabelled every port past the
/// third once the count grew.
const FIXTURE_TITLES: [&str; 8] = [
    "Fixtures P1",
    "Fixtures P2",
    "Fixtures P3",
    "Fixtures P4",
    "Fixtures P5",
    "Fixtures P6",
    "Fixtures P7",
    "Fixtures P8",
];

/// Row labels inside a fixture table.
const FIXTURE_LABELS: [&str; 5] = ["Fix 1", "Fix 2", "Fix 3", "Fix 4", "Fix 5"];

/// Console keys, `fixture_<port>_<slot>`, both 1-based to match the labels.
const FIXTURE_KEYS: [[&str; 5]; 8] = [
    ["fixture_1_1", "fixture_1_2", "fixture_1_3", "fixture_1_4", "fixture_1_5"],
    ["fixture_2_1", "fixture_2_2", "fixture_2_3", "fixture_2_4", "fixture_2_5"],
    ["fixture_3_1", "fixture_3_2", "fixture_3_3", "fixture_3_4", "fixture_3_5"],
    ["fixture_4_1", "fixture_4_2", "fixture_4_3", "fixture_4_4", "fixture_4_5"],
    ["fixture_5_1", "fixture_5_2", "fixture_5_3", "fixture_5_4", "fixture_5_5"],
    ["fixture_6_1", "fixture_6_2", "fixture_6_3", "fixture_6_4", "fixture_6_5"],
    ["fixture_7_1", "fixture_7_2", "fixture_7_3", "fixture_7_4", "fixture_7_5"],
    ["fixture_8_1", "fixture_8_2", "fixture_8_3", "fixture_8_4", "fixture_8_5"],
];

/// Iterate every field, in page order (used by the console `get` command).
pub fn all_fields() -> impl Iterator<Item = FieldId> {
    PAGES.iter().flat_map(|page| page.fields.iter().copied())
}

/// SmartLED is the only module Rev 2 has, so these never fail — they stay
/// `Option` because every call site already handles `None` and the shape
/// leaves room for a second module type without touching them.
fn smart_led(data: &MenuData) -> Option<&SmartLedSettings> {
    let ModuleSettings::SmartLed(settings) = &data.module;
    Some(settings)
}

fn smart_led_mut(data: &mut MenuData) -> Option<&mut SmartLedSettings> {
    let ModuleSettings::SmartLed(settings) = &mut data.module;
    Some(settings)
}

/// Dotted quad, each octet zero-padded to three digits so the per-digit editor
/// has a fixed layout: `192.168.001.050`.
fn ip_text(ip: &IpAddrMenu) -> String {
    let b = ip.octets();
    format!("{:03}.{:03}.{:03}.{:03}", b[0], b[1], b[2], b[3])
}

fn parse_ip(text: &str) -> Result<IpAddrMenu, &'static str> {
    let mut b = [0u8; 4];
    let mut n = 0;
    for part in text.split('.') {
        if n == 4 {
            return Err("expected a.b.c.d");
        }
        b[n] = part.parse().map_err(|_| "octets must be 0-255")?;
        n += 1;
    }
    if n != 4 {
        return Err("expected a.b.c.d");
    }
    Ok(IpAddrMenu::new(b[0], b[1], b[2], b[3]))
}

/// One Up/Down on digit `digit` (0..=11) of a dotted quad: three digits per
/// octet, wrapping 0..=9 without carry; a result over 255 is rejected.
fn adjust_ip(ip: &mut IpAddrMenu, digit: u8, up: bool) {
    let mut b = ip.octets();
    let octet = (digit / 3) as usize;
    let pow = 10u16.pow((2 - digit % 3) as u32);
    let v = b[octet] as u16;
    let current = (v / pow) % 10;
    let new_digit = if up { (current + 1) % 10 } else { (current + 9) % 10 };
    let new_value = v - current * pow + new_digit * pow;
    if new_value <= 255 {
        b[octet] = new_value as u8;
        *ip = IpAddrMenu::new(b[0], b[1], b[2], b[3]);
    }
}

impl FieldId {
    /// Label shown on the menu (at most `VALUE_COLUMN - 1` characters).
    pub fn label(&self) -> &'static str {
        match self {
            FieldId::DmxAddress => "DMX Address",
            FieldId::InputMode => "Mode",
            // Applies at the next boot; the status line shows what the
            // network is doing right now.
            FieldId::IpMode => "IP Mode (boot)",
            FieldId::IpAddr => "IP",
            FieldId::StaticIp => "Static IP",
            FieldId::StaticPrefix => "Prefix /bits",
            FieldId::StaticGateway => "Gateway",
            FieldId::ArtNetNet => "ArtNet Net",
            FieldId::ArtNetSubNet => "ArtNet Sub-Net",
            FieldId::ArtNetUniverse => "ArtNet Univ",
            FieldId::SacnUniverse => "sACN Universe",
            FieldId::UniversesBound => "Univ Bound",
            FieldId::TotalChannels => "DMX Channels",
            FieldId::PortMode => "Mirror Ports",
            FieldId::Grouping => "Grouping",
            FieldId::ColorMode => "Color Mode",
            // Table lookup rather than a match with a wildcard: the old
            // `GroupSize(_) => "...P4"` arm silently mislabelled every port past
            // the third once the count grew.
            FieldId::GroupSize(i) => GROUP_LABELS[(*i).min(GROUP_LABELS.len() - 1)],
            FieldId::Fixture(_, slot) => FIXTURE_LABELS[(*slot).min(FIXTURE_LABELS.len() - 1)],
            FieldId::LedsPerPort(i) => LED_LABELS[(*i).min(LED_LABELS.len() - 1)],
            FieldId::UniverseOffsets => "Univ Offsets",
            FieldId::TotalUniverses => "Universes",
            FieldId::EthernetEnabled => "Ethernet(boot)",
            FieldId::LedPower => "LED Power",
            FieldId::Backlight => "Backlight",
        }
    }

    /// Key used by the USB console protocol (`get` / `set <key> <value>`).
    pub fn key(&self) -> &'static str {
        match self {
            FieldId::DmxAddress => "dmx_address",
            FieldId::InputMode => "input_mode",
            FieldId::IpMode => "ip_mode",
            FieldId::IpAddr => "ip",
            FieldId::StaticIp => "static_ip",
            FieldId::StaticPrefix => "static_prefix",
            FieldId::StaticGateway => "static_gateway",
            FieldId::ArtNetNet => "artnet_net",
            FieldId::ArtNetSubNet => "artnet_subnet",
            FieldId::ArtNetUniverse => "artnet_universe",
            FieldId::SacnUniverse => "sacn_universe",
            FieldId::UniversesBound => "universes_bound",
            FieldId::TotalChannels => "dmx_channels",
            FieldId::PortMode => "port_mode",
            FieldId::Grouping => "grouping",
            FieldId::ColorMode => "color_mode",
            // Table lookup, not a wildcard: the old arms gave every port past
            // the third the key `group_size_4` / `leds_4`, so the console could
            // neither address the extra ports nor unambiguously address port 4.
            FieldId::GroupSize(i) => GROUP_KEYS[(*i).min(GROUP_KEYS.len() - 1)],
            FieldId::Fixture(port, slot) => {
                FIXTURE_KEYS[(*port).min(FIXTURE_KEYS.len() - 1)]
                    [(*slot).min(FIXTURE_LABELS.len() - 1)]
            }
            FieldId::LedsPerPort(i) => LED_KEYS[(*i).min(LED_KEYS.len() - 1)],
            FieldId::UniverseOffsets => "universe_offsets",
            FieldId::TotalUniverses => "total_universes",
            FieldId::EthernetEnabled => "ethernet_enabled",
            FieldId::LedPower => "led_power",
            FieldId::Backlight => "backlight",
        }
    }

    pub fn editable(&self) -> bool {
        !matches!(
            self,
            FieldId::IpAddr
            | FieldId::UniverseOffsets
            | FieldId::TotalUniverses
            | FieldId::UniversesBound
            | FieldId::TotalChannels
        )
    }

    /// Number of digit sub-positions for per-digit editing; 0 for values
    /// edited as a whole (enums, bool).
    pub fn digits(&self) -> u8 {
        match self {
            FieldId::DmxAddress | FieldId::ArtNetNet => 3,
            FieldId::GroupSize(_) | FieldId::LedsPerPort(_) => 3,
            // Kind plus three length digits: `F010`.
            FieldId::Fixture(..) => FIXTURE_VALUE_WIDTH as u8,
            FieldId::ArtNetSubNet | FieldId::ArtNetUniverse => 2,
            FieldId::SacnUniverse => 5,
            FieldId::Backlight => 3,
            FieldId::StaticPrefix => 2,
            // Four octets of three digits; the dots are not digits.
            FieldId::StaticIp | FieldId::StaticGateway => 12,
            _ => 0,
        }
    }

    /// Characters the rendered value occupies for a digit-edited field — the
    /// digits plus, for dotted quads, the three dots.
    pub fn display_width(&self) -> usize {
        match self {
            FieldId::StaticIp | FieldId::StaticGateway => 15,
            FieldId::Fixture(..) => FIXTURE_VALUE_WIDTH,
            other => other.digits() as usize,
        }
    }

    /// Whether digit position `digit` renders as a number.
    ///
    /// Every field answered "yes" until fixtures: their digit 0 is the
    /// Fixture/Gap kind and renders `F` or `G`. The cursor still lands on it and
    /// Up/Down still act on it - it is simply not a numeral, and the test that
    /// asserts the cursor sits on a digit has to know the difference.
    pub fn digit_is_numeric(&self, digit: u8) -> bool {
        !matches!(self, FieldId::Fixture(..)) || digit > 0
    }

    /// Display column (relative to the value start) of digit `digit`, so the
    /// edit cursor lands on the digit and not on a dot.
    pub fn cursor_column(&self, digit: u8) -> u16 {
        match self {
            FieldId::StaticIp | FieldId::StaticGateway => (digit + digit / 3) as u16,
            _ => digit as u16,
        }
    }

    fn is_ip(&self) -> bool {
        matches!(self, FieldId::StaticIp | FieldId::StaticGateway)
    }

    fn ip_field<'a>(&self, data: &'a mut MenuData) -> Option<&'a mut IpAddrMenu> {
        match self {
            FieldId::StaticIp => Some(&mut data.static_ip),
            FieldId::StaticGateway => Some(&mut data.static_gateway),
            _ => None,
        }
    }

    /// Valid range for numeric fields.
    fn range(&self) -> (u16, u16) {
        match self {
            FieldId::DmxAddress => (1, DMX_UNIVERSE_SIZE as u16),
            FieldId::ArtNetNet => (0, 127),
            // Art-Net allows sub-net 0..=15, but DMX_BUFFER holds
            // DMX_UNIVERSE_COUNT (64) universes = sub-nets 0..=3. A base
            // beyond that cannot be buffered, so don't let it be configured.
            FieldId::ArtNetSubNet => (0, (crate::DMX_UNIVERSE_COUNT / 16 - 1) as u16),
            FieldId::ArtNetUniverse => (0, 15),
            // E1.31 universes are 1..=63999.
            FieldId::SacnUniverse => (1, 63999),
            // Capped at what the output driver transmits, so a configured
            // count is never silently truncated on the wire.
            FieldId::GroupSize(_) => (1, MAX_LEDS_PER_PORT as u16),
            FieldId::LedsPerPort(_) => (0, MAX_LEDS_PER_PORT as u16),
            FieldId::Backlight => (1, 255),
            FieldId::StaticPrefix => (1, 30),
            _ => (0, 0),
        }
    }

    /// Numeric value of the field, if it is numeric.
    fn get_u16(&self, data: &MenuData) -> Option<u16> {
        match self {
            FieldId::DmxAddress => Some(data.dmx_address),
            FieldId::ArtNetNet => Some(data.artnet_address.0[0] as u16),
            FieldId::ArtNetSubNet => Some(data.artnet_address.0[1] as u16),
            FieldId::ArtNetUniverse => Some(data.artnet_address.0[2] as u16),
            FieldId::SacnUniverse => Some(data.sacn_universe),
            FieldId::GroupSize(i) => smart_led(data).map(|s| s.dmx_group_size.0[*i]),
            FieldId::LedsPerPort(i) => smart_led(data).map(|s| s.leds_per_port[*i]),
            FieldId::Backlight => Some(data.backlight as u16),
            FieldId::StaticPrefix => Some(data.static_prefix as u16),
            _ => None,
        }
    }

    fn set_u16(&self, data: &mut MenuData, value: u16) {
        match self {
            FieldId::DmxAddress => data.dmx_address = value,
            FieldId::ArtNetNet => data.artnet_address.0[0] = value as u8,
            FieldId::ArtNetSubNet => data.artnet_address.0[1] = value as u8,
            FieldId::ArtNetUniverse => data.artnet_address.0[2] = value as u8,
            FieldId::SacnUniverse => data.sacn_universe = value,
            FieldId::GroupSize(i) => {
                if let Some(s) = smart_led_mut(data) {
                    s.dmx_group_size.0[*i] = value;
                }
            }
            FieldId::LedsPerPort(i) => {
                if let Some(s) = smart_led_mut(data) {
                    s.leds_per_port[*i] = value;
                }
            }
            FieldId::Backlight => data.backlight = value.min(255) as u8,
            FieldId::StaticPrefix => data.static_prefix = value.min(30) as u8,
            _ => {}
        }
    }

    /// The value actually in force, when the current mode computes this field
    /// instead of using what is stored. `None` means the stored value stands.
    ///
    /// Only `Group Size` on ports 2..8 in [`SmartLedPortMode::Mirror`], today.
    /// Mirror gives every port the same number of addressable items, so those
    /// group sizes fall out of port 1's grouping and each port's own LED count
    /// - see [`SmartLedSettings::mirror_group_size`].
    ///
    /// The stored value is deliberately still returned by [`Self::display`] and
    /// still editable: it is what `Individual` mode goes back to. Hiding it
    /// would mean the only way to set a port up for `Individual` was to leave
    /// `Mirror` first. So the UI shows both - the stored one dimmed, this one
    /// beside it - rather than lying about which is used or throwing the other
    /// away.
    pub fn derived(&self, data: &MenuData) -> Option<String> {
        match self {
            // **Mirror is tested first, and the order is the point.** Under
            // Mirror every port's size is scaled from port 1's whatever the
            // grouping is - a 300-LED port mirroring a 150-LED one groups by 2
            // even under `Individual`. Checking `Individual` first reported 1
            // for every port and hid that entirely.
            FieldId::GroupSize(i)
                if *i > 0
                    && smart_led(data)
                        .is_some_and(|s| s.port_mode == SmartLedPortMode::Mirror) =>
            {
                let settings = smart_led(data)?;
                // Same zero padding as `display`, so the two line up.
                Some(format!(
                    "{:0width$}",
                    settings.mirror_group_size()[*i],
                    width = self.digits() as usize
                ))
            }
            // Not mirrored: `Individual` pins every port to 1, port 1 included.
            FieldId::GroupSize(i)
                if smart_led(data).is_some_and(|s| s.grouping == LedGrouping::Individual) =>
            {
                let settings = smart_led(data)?;
                Some(format!(
                    "{:0width$}",
                    settings.effective_group_size(*i),
                    width = self.digits() as usize
                ))
            }
            _ => None,
        }
    }

    /// Value rendered as text. Numeric fields are zero-padded to their digit
    /// count so the per-digit edit cursor lines up.
    pub fn display(&self, data: &MenuData) -> String {
        if let Some(v) = self.get_u16(data) {
            return format!("{:0width$}", v, width = self.digits() as usize);
        }
        match self {
            FieldId::InputMode => format!("{}", data.input_mode),
            FieldId::IpMode => format!("{}", data.ethernet_ip_mode),
            FieldId::IpAddr => {
                let b = data.ip_addr.octets();
                format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
            }
            FieldId::StaticIp => ip_text(&data.static_ip),
            FieldId::StaticGateway => ip_text(&data.static_gateway),
            // "<count> @ <base>" in the active mode's own addressing, so what
            // the console operator sees here matches what they patch.
            FieldId::UniversesBound => {
                let n = data.bound_universes();
                match data.input_mode {
                    InputMode::Dmx => String::from("1 (wired DMX)"),
                    InputMode::UsbToDmx => String::from("1 (USB)"),
                    InputMode::ArtNet | InputMode::ArtNetToDmx => {
                        let a = data.artnet_address.0;
                        format!("{} @ {}:{}:{}", n, a[0], a[1], a[2])
                    }
                    InputMode::Sacn => format!("{} @ {}", n, data.sacn_universe),
                }
            }
            // The fixture map is not in `MenuData`, so this cannot render one.
            // Callers that show fixtures dispatch on `fixture_slot()` and use
            // `fixture_display(&map)`; this placeholder is what a caller that
            // forgot will see, and it is four characters wide like the real
            // thing so a layout bug shows up as blanks rather than as a shifted
            // column.
            FieldId::Fixture(..) => String::from("----"),
            FieldId::PortMode => smart_led(data).map_or(String::from("-"), |s| format!("{}", s.port_mode)),
            FieldId::Grouping => smart_led(data).map_or(String::from("-"), |s| format!("{}", s.grouping)),
            FieldId::ColorMode => smart_led(data).map_or(String::from("-"), |s| format!("{}", s.color_mode)),
            // Every port, not the first four — the old fixed format silently
            // hid half of them once the port count doubled.
            FieldId::UniverseOffsets => smart_led(data).map_or(String::from("-"), |s| {
                let mut out = String::new();
                for (i, o) in s.universe_offset().iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    out.push_str(&format!("{o}"));
                }
                out
            }),
            // Over budget is not an error, it is a silently lower refresh rate,
            // so say so rather than letting someone wonder why 44 Hz became 30.
            // Channels used, and the strip length they drive. Two numbers
            // because a channel count alone does not say whether it is a lot:
            // `1530 / 4800 px` is immediately legible where `1530` is not.
            FieldId::TotalChannels => smart_led(data).map_or(String::from("-"), |s| {
                let leds: u32 = s.leds_per_port.iter().map(|n| *n as u32).sum();
                format!("{} / {} px", s.total_channels(), leds)
            }),
            FieldId::TotalUniverses => smart_led(data).map_or(String::from("-"), |s| {
                if s.over_budget() {
                    format!("{} OVER", s.total_universes())
                } else {
                    format!("{}/{}", s.total_universes(), crate::DMX_UNIVERSE_COUNT)
                }
            }),
            FieldId::EthernetEnabled => String::from(if data.ethernet_enabled { "On" } else { "Off" }),
            FieldId::LedPower => String::from(match data.led_power {
                LedPower::External => "External",
                LedPower::UsbBrick => "USB brick",
            }),
            _ => String::new(),
        }
    }

    /// Apply one Up/Down press while editing. For numeric fields `digit`
    /// selects the digit (0 = most significant); the digit wraps 0..=9
    /// without carrying, and a result outside the valid range is rejected.
    pub fn adjust(&self, data: &mut MenuData, digit: u8, up: bool) {
        if self.is_ip() {
            if let Some(ip) = self.ip_field(data) {
                adjust_ip(ip, digit, up);
            }
            return;
        }
        if let Some(v) = self.get_u16(data) {
            let (min, max) = self.range();
            let pow = 10u16.pow((self.digits() - 1 - digit) as u32);
            let current = (v / pow) % 10;
            let new_digit = if up { (current + 1) % 10 } else { (current + 9) % 10 };
            // Widened so a 5-digit field's ten-thousands digit cannot overflow
            // u16 on the way to being rejected.
            let new_value = v as u32 - current as u32 * pow as u32 + new_digit as u32 * pow as u32;
            if new_value >= min as u32 && new_value <= max as u32 {
                self.set_u16(data, new_value as u16);
            }
            return;
        }
        match self {
            FieldId::InputMode => {
                data.input_mode = if up { data.input_mode.increment(0) } else { data.input_mode.decrement(0) };
            }
            FieldId::IpMode => {
                data.ethernet_ip_mode = if up { data.ethernet_ip_mode.increment(0) } else { data.ethernet_ip_mode.decrement(0) };
            }
            FieldId::Grouping => {
                if let Some(s) = smart_led_mut(data) {
                    s.grouping = if up { s.grouping.increment(0) } else { s.grouping.decrement(0) };
                }
            }
            FieldId::PortMode => {
                if let Some(s) = smart_led_mut(data) {
                    s.port_mode = if up { s.port_mode.increment(0) } else { s.port_mode.decrement(0) };
                }
            }
            FieldId::ColorMode => {
                if let Some(s) = smart_led_mut(data) {
                    s.color_mode = if up { s.color_mode.increment(0) } else { s.color_mode.decrement(0) };
                }
            }
            FieldId::EthernetEnabled => data.ethernet_enabled = !data.ethernet_enabled,
            FieldId::LedPower => data.led_power = data.led_power.increment(0),
            _ => {}
        }
    }

    /// Copy this field's value from one settings struct to another. Used to
    /// merge a committed edit without clobbering fields that changed
    /// elsewhere (e.g. an IP update from DHCP) during the edit.
    pub fn transfer(&self, src: &MenuData, dst: &mut MenuData) {
        if let Some(v) = self.get_u16(src) {
            self.set_u16(dst, v);
            return;
        }
        match self {
            FieldId::InputMode => dst.input_mode = src.input_mode,
            FieldId::IpMode => dst.ethernet_ip_mode = src.ethernet_ip_mode,
            FieldId::StaticIp => dst.static_ip = src.static_ip,
            FieldId::StaticGateway => dst.static_gateway = src.static_gateway,
            FieldId::PortMode => {
                if let (Some(s), Some(d)) = (smart_led(src), smart_led_mut(dst)) {
                    d.port_mode = s.port_mode;
                }
            }
            FieldId::Grouping => {
                if let (Some(s), Some(d)) = (smart_led(src), smart_led_mut(dst)) {
                    d.grouping = s.grouping;
                }
            }
            FieldId::ColorMode => {
                if let (Some(s), Some(d)) = (smart_led(src), smart_led_mut(dst)) {
                    d.color_mode = s.color_mode;
                }
            }
            FieldId::EthernetEnabled => dst.ethernet_enabled = src.ethernet_enabled,
            FieldId::LedPower => dst.led_power = src.led_power,
            _ => {}
        }
    }

    /// Parse and apply a console-supplied value.
    pub fn set_from_str(&self, data: &mut MenuData, value: &str) -> Result<(), &'static str> {
        if !self.editable() {
            return Err("read-only field");
        }

        if self.is_ip() {
            let ip = parse_ip(value)?;
            if let Some(field) = self.ip_field(data) {
                *field = ip;
            }
            return Ok(());
        }

        if self.fixture_slot().is_some() {
            // The console dispatches these to `fixture_set_from_str` before it
            // gets here; reaching this arm means a caller did not, and a silent
            // success would look like the value took.
            return Err("fixture entries need the fixture map");
        }
        if self.digits() > 0 {
            let v: u16 = value.parse().map_err(|_| "not a number")?;
            let (min, max) = self.range();
            if v < min || v > max {
                return Err("value out of range");
            }
            self.set_u16(data, v);
            return Ok(());
        }

        let lower = value.to_ascii_lowercase();
        match self {
            FieldId::InputMode => {
                data.input_mode = match lower.as_str() {
                    "dmx" => InputMode::Dmx,
                    "artnet" => InputMode::ArtNet,
                    "artnet>dmx" => InputMode::ArtNetToDmx,
                    "usb>dmx" => InputMode::UsbToDmx,
                    "sacn" => InputMode::Sacn,
                    _ => return Err("expected dmx|artnet|artnet>dmx|usb>dmx|sacn"),
                };
            }
            FieldId::IpMode => {
                data.ethernet_ip_mode = match lower.as_str() {
                    "dhcp" => EthernetIPMode::Dhcp,
                    "static" => EthernetIPMode::Static,
                    _ => return Err("expected dhcp|static"),
                };
            }
            FieldId::PortMode => {
                // `off`/`on` match what the panel now shows; `individual`/
                // `mirror` are what existing scripts send. Both accepted - the
                // label changed, the stored ordinals did not.
                let mode = match lower.as_str() {
                    "off" | "individual" => SmartLedPortMode::Individual,
                    "on" | "mirror" => SmartLedPortMode::Mirror,
                    _ => return Err("expected off|on"),
                };
                smart_led_mut(data).ok_or("module is not SmartLED")?.port_mode = mode;
            }
            FieldId::Grouping => {
                let mode = match lower.as_str() {
                    "individual" => LedGrouping::Individual,
                    "groups" => LedGrouping::Groups,
                    "fixtures" => LedGrouping::Fixtures,
                    _ => return Err("expected individual|groups|fixtures"),
                };
                smart_led_mut(data).ok_or("module is not SmartLED")?.grouping = mode;
            }
            FieldId::ColorMode => {
                let mode = match lower.as_str() {
                    "rgb" => SmartLedColorMode::Rgb,
                    "rgbw" => SmartLedColorMode::Rgbw,
                    _ => return Err("expected rgb|rgbw"),
                };
                if !mode.is_supported() {
                    return Err("colour mode not supported by this firmware");
                }
                smart_led_mut(data).ok_or("module is not SmartLED")?.color_mode = mode;
            }
            FieldId::EthernetEnabled => {
                data.ethernet_enabled = match lower.as_str() {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    _ => return Err("expected on|off"),
                };
            }
            FieldId::LedPower => {
                data.led_power = match lower.as_str() {
                    "external" | "ext" | "j26" => LedPower::External,
                    "usb" | "brick" | "usb_brick" | "usbbrick" => LedPower::UsbBrick,
                    _ => return Err("expected external|usb"),
                };
            }
            _ => return Err("cannot set this field"),
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fixture entries.
//
// These need a `FixtureMap`, which `MenuData` does not carry — the map is a
// separate store for the reasons in `ui::fixtures`. So they are a parallel set
// of accessors rather than more arms on `get_u16`/`set_u16`, and the callers
// that edit fixtures (the TFT and the console) dispatch on `fixture_slot()`.
//
// The value is ONE token — `F010`, `G005` — because `console_usb.rs` keeps only
// the second whitespace-delimited token of `set`. `"Fix 010"` would round-trip
// as type-set, length-silently-lost.
// ---------------------------------------------------------------------------

/// Characters an entry renders in: kind plus three digits.
pub const FIXTURE_VALUE_WIDTH: usize = 4;

impl FieldId {
    /// `(port, slot)` if this field is a fixture entry.
    pub fn fixture_slot(&self) -> Option<(usize, usize)> {
        match self {
            FieldId::Fixture(port, slot) => Some((*port, *slot)),
            _ => None,
        }
    }

    /// `F010` for a lit run of ten, `G005` for a five-LED gap, `----` for an
    /// empty slot.
    pub fn fixture_display(&self, map: &FixtureMap) -> String {
        let Some((port, slot)) = self.fixture_slot() else {
            return String::new();
        };
        // `effective_entry`, not the raw slot: a port with nothing stored uses
        // `FixtureMap::DEFAULT_ENTRY`, and a table showing `----` while the
        // strip lights a pixel would be the hidden state this design removes.
        let entry = map.effective_entry(
            port.min(SMARTLED_PORT_COUNT - 1),
            slot.min(MAX_FIXTURES_PER_PORT - 1),
        );
        if entry.is_none() {
            // Four characters either way, so the column does not jump as
            // entries are added and removed.
            return String::from("----");
        }
        format!("{}{:03}", if entry.is_gap() { 'G' } else { 'F' }, entry.leds())
    }

    /// The span this entry covers, `001-010`, for the blue column beside the
    /// value. `None` for a gap or an empty slot — neither has DMX channels, and
    /// showing a range for one would suggest it does.
    pub fn fixture_span(&self, map: &FixtureMap) -> Option<String> {
        let (port, slot) = self.fixture_slot()?;
        let port = port.min(SMARTLED_PORT_COUNT - 1);
        // Walk the list rather than asking `spans`, because that skips gaps and
        // this needs to know where THIS slot sits, lit or not.
        let mut at = 0_u16;
        for (i, entry) in map.effective_entries(port).enumerate() {
            if i == slot {
                if entry.is_gap() {
                    return None;
                }
                return Some(format!("{:03}-{:03}", at + 1, at + entry.leds()));
            }
            at = at.saturating_add(entry.leds());
        }
        None
    }

    /// Largest length this entry may take without the port overrunning its
    /// strip.
    ///
    /// `current + remaining`, not `remaining`: bounding by what is left over
    /// would mean an entry already at the limit could never be *lowered*,
    /// because raising is rejected and the check does not know which way the
    /// edit went. Lowering `LEDs Port n` below the total would freeze every
    /// entry on that port permanently.
    pub fn fixture_max(&self, map: &FixtureMap, port_leds: u16) -> u16 {
        let Some((port, slot)) = self.fixture_slot() else {
            return 0;
        };
        let port = port.min(SMARTLED_PORT_COUNT - 1);
        let current = map.0[port][slot.min(MAX_FIXTURES_PER_PORT - 1)].leds();
        let others = map.used(port).saturating_sub(current);
        port_leds.saturating_sub(others).min(MAX_LEDS_PER_PORT as u16)
    }

    /// One Up/Down press on a fixture entry.
    ///
    /// Digit 0 is the kind — Up/Down both toggle it, as they do for every other
    /// two-state field. Digits 1..=3 are the length, edited per digit and
    /// wrapping without carry, exactly like `LEDs Port n`. A length that would
    /// overrun the strip is rejected rather than clamped, which is what
    /// `range()` does everywhere else.
    pub fn fixture_adjust(&self, map: &mut FixtureMap, port_leds: u16, digit: u8, up: bool) {
        let Some((port, slot)) = self.fixture_slot() else {
            return;
        };
        let port = port.min(SMARTLED_PORT_COUNT - 1);
        let slot = slot.min(MAX_FIXTURES_PER_PORT - 1);
        let entry = map.0[port][slot];
        let max = self.fixture_max(map, port_leds);

        if digit == 0 {
            // Toggling an empty slot **creates** an entry rather than toggling
            // nothing, because this is the only control the row has. It starts
            // as `F001`: a fixture, because that is what a person opening an
            // empty table is trying to make, and one LED because it is the
            // smallest thing that is unambiguously there.
            //
            // The obvious reading of "toggle" got this wrong - an empty slot is
            // not a gap, so toggling it produced `G001` and the first thing you
            // could create was the one kind you did not want.
            if entry.is_none() {
                if max > 0 {
                    map.0[port][slot] = FixtureEntry::fixture(1);
                }
                return;
            }
            map.0[port][slot] = if entry.is_gap() {
                FixtureEntry::fixture(entry.leds())
            } else {
                FixtureEntry::gap(entry.leds())
            };
            return;
        }

        let pow = 10_u16.pow(3 - digit as u32);
        let current = (entry.leds() / pow) % 10;
        let next = if up { (current + 1) % 10 } else { (current + 9) % 10 };
        let value = entry.leds() + next * pow - current * pow;
        if value > max {
            return;
        }
        map.0[port][slot] = if entry.is_gap() {
            FixtureEntry::gap(value)
        } else {
            FixtureEntry::fixture(value)
        };
    }

    /// Parse `F010`, `G005`, `----` or a bare number (which means a fixture).
    pub fn fixture_set_from_str(
        &self,
        map: &mut FixtureMap,
        port_leds: u16,
        value: &str,
    ) -> Result<(), &'static str> {
        let Some((port, slot)) = self.fixture_slot() else {
            return Err("not a fixture field");
        };
        let port = port.min(SMARTLED_PORT_COUNT - 1);
        let slot = slot.min(MAX_FIXTURES_PER_PORT - 1);
        let max = self.fixture_max(map, port_leds);

        let lower = value.to_ascii_lowercase();
        if lower == "----" || lower == "none" || lower == "off" {
            map.0[port][slot] = FixtureEntry::NONE;
            return Ok(());
        }

        let (gap, digits) = match lower.as_bytes().first() {
            Some(b'f') => (false, &lower[1..]),
            Some(b'g') => (true, &lower[1..]),
            _ => (false, lower.as_str()),
        };
        let leds: u16 = digits.parse().map_err(|_| "expected F<len>, G<len> or ----")?;
        if leds > max {
            return Err("longer than the strip has room for");
        }
        map.0[port][slot] = if gap {
            FixtureEntry::gap(leds)
        } else {
            FixtureEntry::fixture(leds)
        };
        Ok(())
    }
}

/// Fixture entries the Layout page lists before it starts counting.
///
/// Three `Fxxx ` groups plus a `+N` tail is 17 characters — exactly the value
/// column. Four would be 19 and would run off the glass.
const LAYOUT_SUMMARY_SHOWN: usize = 3;

impl FieldId {
    /// What a port's row on the **Layout** page shows under
    /// [`LedGrouping::Fixtures`]: its lit runs, `F008 F003`.
    ///
    /// **Gaps are left out.** They are visible in the port's own table, where
    /// the running total makes them meaningful; here they would be noise
    /// between the things that actually answer to DMX channels.
    ///
    /// `None` only when this is not a port row. A port with nothing stored
    /// summarises as its default, `F001`, because that is what it is doing —
    /// there is no longer a silent fallback to the group size.
    pub fn fixture_summary(&self, map: &FixtureMap) -> Option<String> {
        let FieldId::GroupSize(port) = self else {
            return None;
        };
        let port = (*port).min(SMARTLED_PORT_COUNT - 1);

        let total = map.fixture_count(port) as usize;
        let mut out = String::new();
        for (i, entry) in map
            .effective_entries(port)
            .filter(|e| !e.is_gap())
            .take(LAYOUT_SUMMARY_SHOWN)
            .enumerate()
        {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&format!("F{:03}", entry.leds()));
        }
        if total > LAYOUT_SUMMARY_SHOWN {
            out.push_str(&format!(" +{}", total - LAYOUT_SUMMARY_SHOWN));
        }
        Some(out)
    }

    /// LEDs configured on the port this row belongs to, for rows that have one.
    pub fn row_port_leds(&self, data: &MenuData) -> Option<u16> {
        let port = match self {
            FieldId::GroupSize(port) | FieldId::LedsPerPort(port) => *port,
            FieldId::Fixture(port, _) => *port,
            _ => return None,
        };
        let settings = smart_led(data)?;
        Some(settings.leds_per_port[port.min(SMARTLED_PORT_COUNT - 1)])
    }

    /// True when this row belongs to a port with **no LEDs**, so nothing it
    /// says can take effect.
    ///
    /// `LEDs Port n` itself is excluded - that is the row you would use to give
    /// the port some, and greying out the only way back would be a trap.
    pub fn port_unused(&self, data: &MenuData) -> bool {
        !matches!(self, FieldId::LedsPerPort(_)) && self.row_port_leds(data) == Some(0)
    }

    /// The **only** value a port row shows, when the mode decides it outright
    /// rather than merely overriding a stored number.
    ///
    /// The grey-stored / blue-computed pair exists for `Mirror`, where both
    /// numbers carry information: what the port would use on its own, and what
    /// mirroring makes it use. Two other cases have no such tension and showing
    /// a pair is just noise -
    ///
    /// * **`Individual`, not mirrored** - every port is 1. Rendering `004 001`
    ///   puts a number on screen that means nothing and cannot be edited.
    /// * **`Fixtures` with a list** - the runs *are* the value.
    ///
    /// `None` leaves the row to `display()` plus [`Self::derived`] as before.
    pub fn layout_row_value(&self, data: &MenuData, map: &FixtureMap) -> Option<String> {
        let FieldId::GroupSize(i) = self else {
            return None;
        };
        if self.port_unused(data) {
            // Ahead of the mode: a group size or a fixture list on a port with
            // no LEDs is configuration that cannot do anything, and saying so
            // beats showing a number that will never be used.
            return Some(String::from("has 0 LEDs"));
        }
        let settings = smart_led(data)?;
        match settings.grouping {
            LedGrouping::Fixtures => self.fixture_summary(map),
            LedGrouping::Individual if settings.port_mode != SmartLedPortMode::Mirror => {
                Some(format!(
                    "{:0width$}",
                    settings.effective_group_size(*i),
                    width = self.digits() as usize
                ))
            }
            _ => None,
        }
    }

    /// Whether this field may be edited **in the current mode**.
    ///
    /// [`Self::editable`] is a static property of the field; this is the one
    /// the panel asks. A port's group size is stored in every mode and settable
    /// from the console in every mode — it is what `Groups` goes back to — but
    /// on the panel it is only a value worth editing when `Groups` is what is
    /// running. Under `Individual` it is pinned to 1 whatever it says, and
    /// under `Fixtures` the row is a doorway into that port's table.
    pub fn editable_now(&self, data: &MenuData) -> bool {
        if !self.editable() || self.port_unused(data) {
            return false;
        }
        match self {
            FieldId::GroupSize(_) => {
                smart_led(data).is_none_or(|s| s.grouping == LedGrouping::Groups)
            }
            _ => true,
        }
    }
}
