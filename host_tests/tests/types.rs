//! Tests for the settings model (`common/src/ui/types.rs`): enum cycling,
//! EEPROM encoding size, and the virtual-LED / universe-offset math the
//! event router relies on.

use common::ui::{
    ArtNetAddr, BootStatus, EthernetIPMode, IncDec, InputMode, IpAddrMenu, LedGrouping, LedPower,
    MenuData, ModuleSettings, SmartLedColorMode, SmartLedDmxGroupSize, SmartLedPortMode,
    SmartLedSettings,
};
// The very constant the firmware's EEPROM driver writes with (`pico2/src/eeprom.rs`
// re-exports it), so this suite cannot drift from it.
use common::{SETTINGS_SIZE, SMARTLED_PORT_COUNT};

/// Headroom the worst case must leave inside the slot, so that adding a
/// settings field fails here rather than on a customer's EEPROM.
const REQUIRED_MARGIN: usize = 32;

/// Widen a four-port pattern to whatever `SMARTLED_PORT_COUNT` currently is.
///
/// The interesting cases all live in the first four ports; the rest are padded
/// so these tests do not have to be rewritten every time the port count moves.
/// Group sizes pad with 1, not 0 — a zero divisor would make
/// `virtual_leds_per_port` produce a NaN cast rather than a meaningful result.
fn widen(v: [u16; 4], pad: u16) -> [u16; SMARTLED_PORT_COUNT] {
    core::array::from_fn(|i| if i < 4 { v[i] } else { pad })
}

fn smart_led(leds: [u16; 4], groups: [u16; 4], color: SmartLedColorMode) -> SmartLedSettings {
    SmartLedSettings {
        leds_per_port: widen(leds, 0),
        color_mode: color,
        port_mode: SmartLedPortMode::Individual,
        // These fixtures exercise the stored group sizes, so the grouping mode
        // must be the one that uses them.
        grouping: LedGrouping::Groups,
        dmx_group_size: SmartLedDmxGroupSize(widen(groups, 1)),
    }
}

#[test]
fn menu_data_round_trips_through_bincode() {
    let original = MenuData {
        dmx_address: 300,
        input_mode: InputMode::ArtNetToDmx,
        ethernet_ip_mode: EthernetIPMode::Static,
        artnet_address: ArtNetAddr([12, 3, 4]),
        module: ModuleSettings::SmartLed(smart_led([170, 0, 999, 1], [1, 1, 3, 999], SmartLedColorMode::Rgbw)),
        ip_addr: IpAddrMenu::new(192, 168, 1, 50),
        ethernet_enabled: true,
        sacn_universe: 63999,
        backlight: 1,
        static_ip: IpAddrMenu::new(10, 20, 30, 40),
        static_prefix: 24,
        static_gateway: IpAddrMenu::new(10, 20, 30, 1),
        led_power: LedPower::UsbBrick,
    };

    let mut buffer = [0_u8; SETTINGS_SIZE];
    let length = bincode::encode_into_slice(original, &mut buffer, bincode::config::standard()).unwrap();
    let (decoded, _): (MenuData, usize) = bincode::decode_from_slice(&buffer[..length], bincode::config::standard()).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn fresh_unit_defaults_are_the_product_defaults() {
    // A blank EEPROM decodes to these: Ethernet on (it is a network node), a
    // non-zero LED count so attached strips show life, a visible backlight.
    let d = MenuData::default();
    assert!(d.ethernet_enabled);
    assert_eq!(d.sacn_universe, 1);
    assert_eq!(d.backlight, common::ui::DEFAULT_BACKLIGHT);
    let ModuleSettings::SmartLed(s) = d.module;
    assert_eq!(s.leds_per_port, [common::ui::DEFAULT_LEDS_PER_PORT; SMARTLED_PORT_COUNT]);
    assert!(s.leds_per_port[0] as usize <= common::MAX_LEDS_PER_PORT);
}

#[test]
fn worst_case_settings_fit_the_eeprom_slot() {
    // bincode varint encoding costs 3 bytes per u16 >= 251, so maximal legal
    // settings are the largest encoding. This is the regression test for the
    // bug where SETTINGS_SIZE = 32 silently dropped saves.
    let worst = MenuData {
        dmx_address: 512,
        input_mode: InputMode::UsbToDmx,
        ethernet_ip_mode: EthernetIPMode::Static,
        artnet_address: ArtNetAddr([127, 15, 15]),
        module: ModuleSettings::SmartLed(smart_led([999; 4], [999; 4], SmartLedColorMode::Rgbw)),
        ip_addr: IpAddrMenu::new(255, 255, 255, 255),
        ethernet_enabled: true,
        sacn_universe: 63999,
        backlight: 255,
        static_ip: IpAddrMenu::new(255, 255, 255, 255),
        static_prefix: 30,
        static_gateway: IpAddrMenu::new(255, 255, 255, 255),
        led_power: LedPower::UsbBrick,
    };

    let mut buffer = [0_u8; SETTINGS_SIZE];
    let length = bincode::encode_into_slice(worst, &mut buffer, bincode::config::standard())
        .expect("worst-case settings must fit SETTINGS_SIZE");

    // Measured worst case: 40 bytes at SMARTLED_PORT_COUNT = 4, and 64 bytes at
    // 8 ports — which landed exactly on the old 64-byte slot, passing a
    // `<= SETTINGS_SIZE` check with zero room to spare. Assert real margin so
    // the next field added, or the next port-count bump, fails here first.
    assert!(
        length + REQUIRED_MARGIN <= SETTINGS_SIZE,
        "worst-case settings encode to {length} bytes, leaving less than \
         {REQUIRED_MARGIN} bytes of the {SETTINGS_SIZE}-byte slot free; \
         raise common::SETTINGS_SIZE (and check it still ends inside the EEPROM)"
    );
    assert!(length > 32, "if this fails the old 32-byte slot would have been fine");
}

#[test]
fn default_settings_decode_from_blank_eeprom_data() {
    // A fresh EEPROM reads as 0xFF. The firmware never decodes it (the schema
    // byte gate catches a blank part first), but if it ever did, a clean
    // decode *failure* — not plausible garbage — is the contract this pins.
    let blank = [0xFF_u8; SETTINGS_SIZE];
    let result: Result<(MenuData, usize), _> = bincode::decode_from_slice(&blank, bincode::config::standard());
    assert!(result.is_err());
}

#[test]
fn input_mode_cycles_through_all_variants() {
    let mut mode = InputMode::Dmx;
    let forward = [
        InputMode::ArtNet,
        InputMode::ArtNetToDmx,
        InputMode::UsbToDmx,
        InputMode::Sacn,
        InputMode::Dmx,
    ];
    for expected in forward {
        mode = mode.increment(0);
        assert_eq!(mode, expected);
    }
    // Wraps backwards from the first to the last variant
    assert_eq!(InputMode::Dmx.decrement(0), InputMode::Sacn);
    assert_eq!(InputMode::Sacn.decrement(0), InputMode::UsbToDmx);
}

#[test]
fn two_state_enums_toggle_both_directions() {
    assert_eq!(EthernetIPMode::Dhcp.increment(0), EthernetIPMode::Static);
    assert_eq!(EthernetIPMode::Static.increment(0), EthernetIPMode::Dhcp);
    assert_eq!(EthernetIPMode::Dhcp.decrement(0), EthernetIPMode::Static);

    assert_eq!(SmartLedPortMode::Individual.increment(0), SmartLedPortMode::Mirror);
    assert_eq!(SmartLedPortMode::Mirror.increment(0), SmartLedPortMode::Individual);

    // Both colour modes are rendered by the output driver
    assert_eq!(SmartLedColorMode::Rgb.increment(0), SmartLedColorMode::Rgbw);
    assert_eq!(SmartLedColorMode::Rgbw.increment(0), SmartLedColorMode::Rgb);
    assert_eq!(SmartLedColorMode::Rgb.decrement(0), SmartLedColorMode::Rgbw);
    assert!(SmartLedColorMode::Rgbw.is_supported());
}

#[test]
fn is_dmx_output_matches_the_transmit_modes() {
    assert!(!InputMode::Dmx.is_dmx_output());
    assert!(!InputMode::ArtNet.is_dmx_output());
    assert!(!InputMode::Sacn.is_dmx_output());
    assert!(InputMode::ArtNetToDmx.is_dmx_output());
    assert!(InputMode::UsbToDmx.is_dmx_output());
}

#[test]
fn network_and_artnet_classification() {
    // Network modes store channel 1 at slot offset 0; Art-Net modes are the
    // ones the Net/Sub-Net/Universe fields apply to.
    for m in [InputMode::ArtNet, InputMode::ArtNetToDmx, InputMode::Sacn] {
        assert!(m.is_network(), "{m} is a network mode");
    }
    for m in [InputMode::Dmx, InputMode::UsbToDmx] {
        assert!(!m.is_network(), "{m} stores DMX-style");
    }
    assert!(InputMode::ArtNet.is_artnet() && InputMode::ArtNetToDmx.is_artnet());
    assert!(!InputMode::Sacn.is_artnet());
}

#[test]
fn mode_display_strings_match_console_parsing() {
    // The console accepts (lowercased) display strings; keep them in sync
    assert_eq!(format!("{}", InputMode::Dmx), "DMX");
    assert_eq!(format!("{}", InputMode::ArtNet), "ArtNet");
    assert_eq!(format!("{}", InputMode::ArtNetToDmx), "ArtNet>DMX");
    assert_eq!(format!("{}", InputMode::UsbToDmx), "USB>DMX");
    assert_eq!(format!("{}", InputMode::Sacn), "sACN");
}

#[test]
fn virtual_leds_round_up_per_group() {
    let s = smart_led([10, 9, 0, 999], [3, 3, 1, 999], SmartLedColorMode::Rgb);
    // ceil(10/3)=4, 9/3=3, no LEDs -> 0, ceil(999/999)=1; padded ports have no
    // LEDs so they contribute nothing.
    assert_eq!(s.virtual_leds_per_port(), widen([4, 3, 0, 1], 0));
}

#[test]
fn universe_offsets_accumulate_prior_ports() {
    // 170 RGB LEDs = 510 bytes -> exactly one universe per active port
    let s = smart_led([170, 170, 0, 0], [1, 1, 1, 1], SmartLedColorMode::Rgb);
    // Empty ports consume no universes, so the offset stays flat past port 1.
    assert_eq!(s.universe_offset(), widen([0, 1, 2, 2], 2));

    // 171 RGB LEDs = 513 bytes -> spills into a second universe
    let s = smart_led([171, 1, 0, 0], [1, 1, 1, 1], SmartLedColorMode::Rgb);
    assert_eq!(s.universe_offset(), widen([0, 2, 3, 3], 3));
}

#[test]
fn artnet_addr_sub_uni_indexes_one_full_net() {
    // Buffer index = sub-net (high nibble) : universe (low nibble)
    assert_eq!(ArtNetAddr([0, 0, 0]).sub_uni(), 0);
    assert_eq!(ArtNetAddr([7, 3, 5]).sub_uni(), 0x35);
    assert_eq!(ArtNetAddr([127, 15, 15]).sub_uni(), 255);
    // The net byte plays no part, and corrupt (out-of-range) stored values
    // are masked to 4 bits so the index can never exceed 255
    assert_eq!(ArtNetAddr([200, 255, 255]).sub_uni(), 255);
}

#[test]
fn artnet_addr_buffer_base_is_clamped_to_the_buffer() {
    // sub_uni() can reach 255, but DMX_BUFFER holds DMX_UNIVERSE_COUNT
    // universes. buffer_base() is what buffer indexing must use — the raw
    // value sliced the USB forwarder out of bounds (a panic) on a configured
    // sub-net past the buffer.
    use common::DMX_UNIVERSE_COUNT;
    assert_eq!(ArtNetAddr([0, 0, 0]).buffer_base(), 0);
    assert_eq!(ArtNetAddr([0, 3, 15]).buffer_base(), DMX_UNIVERSE_COUNT - 1);
    assert_eq!(ArtNetAddr([0, 15, 15]).buffer_base(), DMX_UNIVERSE_COUNT - 1);
}

#[test]
fn colour_decoding_carries_white_only_in_rgbw() {
    use smart_leds::White;
    let slots = [10u8, 20, 30, 40];
    let rgb = SmartLedColorMode::Rgb.color(&slots);
    assert_eq!((rgb.r, rgb.g, rgb.b, rgb.a), (10, 20, 30, White(0)));
    let rgbw = SmartLedColorMode::Rgbw.color(&slots);
    assert_eq!((rgbw.r, rgbw.g, rgbw.b, rgbw.a), (10, 20, 30, White(40)));
    // Short data (end of buffer) is black, never a panic
    assert_eq!(SmartLedColorMode::Rgbw.color(&slots[..2]), common::ui::BLACK);
    assert_eq!(SmartLedColorMode::Rgbw.color(&slots[..3]).a, White(0));
}

#[test]
fn static_defaults_follow_artnet_convention() {
    let d = MenuData::default();
    assert_eq!(d.static_ip.octets(), [2, 0, 0, 1]);
    assert_eq!(d.static_prefix, 8);
    assert!(d.static_gateway.is_unspecified());
    assert_eq!(d.ethernet_ip_mode, EthernetIPMode::Dhcp);
}

#[test]
fn boot_status_defaults_to_failed() {
    // The ethernet-lockout logic depends on unknown boot flags reading as
    // Failed (fail safe)
    assert_eq!(BootStatus::default(), BootStatus::Failed);
}

/// Every port set to the same value, for whole-board budget cases.
fn all(leds: u16, group: u16, color: SmartLedColorMode) -> SmartLedSettings {
    SmartLedSettings {
        leds_per_port: [leds; SMARTLED_PORT_COUNT],
        color_mode: color,
        port_mode: SmartLedPortMode::Individual,
        // These fixtures exercise the stored group sizes, so the grouping mode
        // must be the one that uses them.
        grouping: LedGrouping::Groups,
        dmx_group_size: SmartLedDmxGroupSize([group; SMARTLED_PORT_COUNT]),
    }
}

#[test]
fn rgb_600_is_the_budget() {
    let s = all(600, 1, SmartLedColorMode::Rgb);
    assert_eq!(s.bytes_per_port()[0], common::MAX_BYTES_PER_PORT);
    // Ports start on universe boundaries: ceil(1800/512) = 4 each, not 29 total.
    assert_eq!(s.universes_per_port()[0], 4);
    assert_eq!(s.total_universes(), 4 * SMARTLED_PORT_COUNT as u16);
    assert!(!s.over_budget());
}

#[test]
fn rgbw_450_matches_rgb_600_exactly() {
    // The claim the whole budget framing rests on: colour mode and LED count
    // are two ways of spending one budget, so RGBW needs a shorter string, not
    // a different design.
    let rgb = all(600, 1, SmartLedColorMode::Rgb);
    let rgbw = all(450, 1, SmartLedColorMode::Rgbw);
    assert_eq!(rgb.bytes_per_port(), rgbw.bytes_per_port());
    assert_eq!(rgb.universes_per_port(), rgbw.universes_per_port());
    assert_eq!(rgb.total_universes(), rgbw.total_universes());
    assert!(!rgbw.over_budget());
}

#[test]
fn rgbw_at_full_rgb_length_is_over_budget() {
    // 600 RGBW is 2400 B/port - a 24 ms frame, which is a 42 Hz ceiling and
    // already below the 44 Hz Art-Net rate.
    let s = all(600, 1, SmartLedColorMode::Rgbw);
    assert!(s.bytes_per_port()[0] > common::MAX_BYTES_PER_PORT);
    assert!(s.ports_over_budget());
    assert!(s.over_budget());
}

#[test]
fn grouping_reduces_the_budget_cost() {
    // Grouping collapses physical LEDs into virtual ones, so the wire cost
    // falls even though the string is the same length.
    let ungrouped = all(600, 1, SmartLedColorMode::Rgb);
    let grouped = all(600, 4, SmartLedColorMode::Rgb);
    assert_eq!(grouped.bytes_per_port()[0], ungrouped.bytes_per_port()[0] / 4);
    assert!(!grouped.over_budget());
}

#[test]
fn total_universes_stays_inside_the_buffer() {
    // DMX_BUFFER only covers DMX_UNIVERSE_COUNT universes; the Art-Net task
    // drops anything above it, so a configuration that needs more is flagged.
    let s = all(600, 1, SmartLedColorMode::Rgb);
    assert!(s.total_universes() as usize <= common::DMX_UNIVERSE_COUNT);
}

// ---------------------------------------------------------------------------
// `LedGrouping::Individual` is `Groups` with every size pinned to 1 — not a
// second layout that happens to agree. These tests are the reason it is routed
// through one accessor.
// ---------------------------------------------------------------------------

fn with_grouping(s: SmartLedSettings, grouping: LedGrouping, mirror: bool) -> SmartLedSettings {
    SmartLedSettings {
        grouping,
        port_mode: if mirror {
            SmartLedPortMode::Mirror
        } else {
            SmartLedPortMode::Individual
        },
        ..s
    }
}

#[test]
fn individual_is_exactly_groups_with_every_size_one() {
    // The regression this guards: an earlier design defined Mirror+Individual
    // as 1:1 per pixel. Size 1 is the shipping default and Mirror stretches
    // proportionally, so a 200-LED port mirroring a 100-LED port groups by 2 —
    // a 1:1 rule would have darkened half of it, one toggle from defaults.
    let mut leds = [150_u16; SMARTLED_PORT_COUNT];
    leds[1] = 300; // longer, so Mirror's proportional scaling is exercised
    leds[2] = 75; // shorter
    leds[3] = 0; // disabled

    for mirror in [false, true] {
        // Individual, with deliberately noisy stored sizes it must ignore.
        let noisy = SmartLedSettings {
            leds_per_port: leds,
            color_mode: SmartLedColorMode::Rgb,
            port_mode: SmartLedPortMode::Individual,
            grouping: LedGrouping::Groups,
            dmx_group_size: SmartLedDmxGroupSize([7; SMARTLED_PORT_COUNT]),
        };
        let individual = with_grouping(noisy, LedGrouping::Individual, mirror);

        // Groups, with the sizes Individual claims to mean.
        let ones = SmartLedSettings {
            dmx_group_size: SmartLedDmxGroupSize([1; SMARTLED_PORT_COUNT]),
            ..noisy
        };
        let groups = with_grouping(ones, LedGrouping::Groups, mirror);

        let what = if mirror { "Mirror" } else { "Individual ports" };
        assert_eq!(
            individual.virtual_leds_per_port(),
            groups.virtual_leds_per_port(),
            "{what}: virtual LEDs differ"
        );
        assert_eq!(individual.bytes_per_port(), groups.bytes_per_port(), "{what}: bytes differ");
        assert_eq!(
            individual.universes_per_port(),
            groups.universes_per_port(),
            "{what}: universes differ"
        );
        assert_eq!(
            individual.universe_offset(),
            groups.universe_offset(),
            "{what}: offsets differ"
        );
        assert_eq!(
            individual.bound_universes(),
            groups.bound_universes(),
            "{what}: bound universes differ — this is what ArtPollReply advertises"
        );
        assert_eq!(
            individual.mirror_group_count(),
            groups.mirror_group_count(),
            "{what}: mirror item count differs"
        );
        assert_eq!(
            individual.mirror_group_size(),
            groups.mirror_group_size(),
            "{what}: mirror per-port sizes differ"
        );
    }
}

#[test]
fn individual_ignores_the_stored_sizes_and_groups_does_not() {
    // Guards the other direction: that the two modes are genuinely distinct and
    // the test above is not passing because grouping is being ignored entirely.
    let base = all(120, 4, SmartLedColorMode::Rgb);
    let groups = with_grouping(base, LedGrouping::Groups, false);
    let individual = with_grouping(base, LedGrouping::Individual, false);

    assert_eq!(groups.effective_group_size(0), 4, "Groups uses the stored size");
    assert_eq!(individual.effective_group_size(0), 1, "Individual pins it to 1");
    assert_eq!(groups.virtual_leds_per_port()[0], 30, "120 LEDs grouped by 4");
    assert_eq!(individual.virtual_leds_per_port()[0], 120, "one item per pixel");
}

#[test]
fn fixtures_uses_its_own_default_rather_than_the_stored_group_sizes() {
    // An early version fell back to `Groups` when no map was stored. That was
    // rejected as hidden state: nothing on the panel told you whether a port
    // was unconfigured or deliberately set to mimic another mode. An
    // unconfigured port now shows and uses one `F001`, which is visible and
    // editable.
    let base = all(120, 4, SmartLedColorMode::Rgb);
    let groups = with_grouping(base, LedGrouping::Groups, false);
    let fixtures = with_grouping(base, LedGrouping::Fixtures, false);

    assert_eq!(groups.virtual_leds_per_port()[0], 30, "120 LEDs grouped by 4");
    assert_eq!(fixtures.virtual_leds_per_port()[0], 1, "one default run");
    // Per-port BYTES, not universes: both round up to one universe per port, so
    // the universe count cannot tell the two apart and asserting on it would
    // have proved nothing.
    assert_eq!(groups.bytes_per_port()[0], 90, "30 items, RGB");
    assert_eq!(fixtures.bytes_per_port()[0], 3, "one item, RGB");
}
