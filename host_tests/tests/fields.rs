//! Tests for the settings-field metadata table (`common/src/ui/fields.rs`).

use common::ui::{
    all_fields, backlight_duty, FieldId, InputMode, LedPower, MenuData, ModuleSettings,
    MIN_BACKLIGHT_DUTY, PAGES,
};

/// Mirrors `pico2::tft_ui`: 35 columns, less one at each side for the panel's
/// rounded corners. Kept here as a plain number because the grid constants
/// live in the target crate, which this one cannot depend on.
const USABLE_COLUMNS: usize = 35 - 2;

fn defaults() -> MenuData {
    MenuData::default()
}

fn group_sizes(data: &MenuData) -> [u16; common::SMARTLED_PORT_COUNT] {
    let ModuleSettings::SmartLed(s) = &data.module;
    s.dmx_group_size.0
}

#[test]
fn pages_have_expected_shape() {
    let titles: Vec<&str> = PAGES.iter().map(|p| p.title).collect();
    assert_eq!(titles, ["Main", "Network", "LED", "Groups", "LEDs", "System"]);
    // Every page fits the 11-row TFT layout (the title/status row takes one)
    for page in PAGES {
        assert!(
            page.fields.len() <= common::ui::fields::MAX_FIELDS_PER_PAGE,
            "page {} has too many rows",
            page.title
        );
    }
}

#[test]
fn console_keys_are_unique() {
    let keys: Vec<&str> = all_fields().map(|f| f.key()).collect();
    let mut deduped = keys.clone();
    deduped.sort_unstable();
    deduped.dedup();
    assert_eq!(keys.len(), deduped.len(), "duplicate console key");
}

#[test]
fn numeric_fields_expose_values_on_default_settings() {
    let data = defaults();
    for field in all_fields().filter(|f| f.digits() > 0) {
        // display() zero-pads numerics to the digit count used by the editor
        // (plus the dots of a dotted quad), so the cursor always lands on a
        // rendered character
        assert_eq!(
            field.display(&data).len(),
            field.display_width(),
            "field {} display width mismatch",
            field.key()
        );
        for d in 0..field.digits() {
            let col = field.cursor_column(d) as usize;
            let ch = field.display(&data).chars().nth(col).expect("cursor inside the value");
            assert!(ch.is_ascii_digit(), "field {} digit {d} cursor sits on {ch:?}", field.key());
        }
    }
}

#[test]
fn dmx_address_display_is_zero_padded() {
    let data = defaults(); // dmx_address = 1
    assert_eq!(FieldId::DmxAddress.display(&data), "001");
}

#[test]
fn digit_editing_wraps_without_carry() {
    let mut data = defaults();
    FieldId::DmxAddress.set_from_str(&mut data, "199").unwrap();
    // Incrementing the ones digit of 199 wraps 9 -> 0 without carrying: 190
    FieldId::DmxAddress.adjust(&mut data, 2, true);
    assert_eq!(FieldId::DmxAddress.display(&data), "190");
}

#[test]
fn dmx_address_cannot_leave_valid_range() {
    let mut data = defaults();

    // 1 is the minimum: decrementing the ones digit (-> 0) must be rejected
    FieldId::DmxAddress.adjust(&mut data, 2, false);
    assert_eq!(data.dmx_address, 1);

    // 512 is the maximum: +100 and +1 must both be rejected
    FieldId::DmxAddress.set_from_str(&mut data, "512").unwrap();
    FieldId::DmxAddress.adjust(&mut data, 0, true); // hundreds: 612
    assert_eq!(data.dmx_address, 512);
    FieldId::DmxAddress.adjust(&mut data, 2, true); // ones: 513
    assert_eq!(data.dmx_address, 512);
}

#[test]
fn group_size_cannot_reach_zero() {
    let mut data = defaults(); // group sizes default to 1
    FieldId::GroupSize(0).adjust(&mut data, 2, false); // ones digit down -> 0
    assert_eq!(group_sizes(&data)[0], 1, "group size 0 must be rejected");
}

#[test]
fn artnet_fields_respect_wire_format_limits() {
    let mut data = defaults();

    FieldId::ArtNetNet.set_from_str(&mut data, "127").unwrap();
    FieldId::ArtNetNet.adjust(&mut data, 0, true); // 227 > 127
    assert_eq!(data.artnet_address.0[0], 127);

    assert!(FieldId::ArtNetSubNet.set_from_str(&mut data, "16").is_err());
    assert!(FieldId::ArtNetUniverse.set_from_str(&mut data, "16").is_err());
    FieldId::ArtNetUniverse.set_from_str(&mut data, "15").unwrap();
    assert_eq!(data.artnet_address.0[2], 15);

    // The sub-net is capped below the Art-Net wire maximum: DMX_BUFFER holds
    // DMX_UNIVERSE_COUNT (64) universes = sub-nets 0..=3, and a base past
    // that cannot be buffered — so it must not be configurable.
    let max_subnet = (common::DMX_UNIVERSE_COUNT / 16 - 1) as u16;
    assert!(FieldId::ArtNetSubNet
        .set_from_str(&mut data, "3")
        .is_ok_and(|_| data.artnet_address.0[1] == 3));
    assert!(FieldId::ArtNetSubNet.set_from_str(&mut data, "4").is_err());
    assert_eq!(max_subnet, 3);
}

#[test]
fn enum_fields_cycle_with_up_down() {
    let mut data = defaults();
    assert_eq!(data.input_mode, InputMode::Dmx);

    for expected in [
        InputMode::ArtNet,
        InputMode::ArtNetToDmx,
        InputMode::UsbToDmx,
        InputMode::Sacn,
        InputMode::Dmx,
    ] {
        FieldId::InputMode.adjust(&mut data, 0, true);
        assert_eq!(data.input_mode, expected);
    }
    // and wrap the other way from the first variant
    FieldId::InputMode.adjust(&mut data, 0, false);
    assert_eq!(data.input_mode, InputMode::Sacn);
}

#[test]
fn network_page_offers_static_addressing() {
    for f in [FieldId::IpMode, FieldId::StaticIp, FieldId::StaticPrefix, FieldId::StaticGateway] {
        assert!(all_fields().any(|x| x == f), "{f:?} missing from the menu");
    }
    let mut data = defaults();
    // Defaults follow the Art-Net convention
    assert_eq!(FieldId::StaticIp.display(&data), "002.000.000.001");
    assert_eq!(FieldId::StaticPrefix.display(&data), "08");
    assert_eq!(FieldId::StaticGateway.display(&data), "000.000.000.000");

    // Console form is a plain dotted quad, any padding
    FieldId::StaticIp.set_from_str(&mut data, "192.168.1.50").unwrap();
    assert_eq!(data.static_ip.octets(), [192, 168, 1, 50]);
    assert!(FieldId::StaticIp.set_from_str(&mut data, "192.168.1").is_err());
    assert!(FieldId::StaticIp.set_from_str(&mut data, "192.168.1.256").is_err());
    assert!(FieldId::StaticPrefix.set_from_str(&mut data, "0").is_err());
    assert!(FieldId::StaticPrefix.set_from_str(&mut data, "31").is_err());
    FieldId::StaticPrefix.set_from_str(&mut data, "24").unwrap();

    // Per-digit editing: digit 5 is the ones digit of the second octet
    // (168 -> 169); the cursor for it sits past one dot.
    FieldId::StaticIp.adjust(&mut data, 5, true);
    assert_eq!(data.static_ip.octets(), [192, 168 + 1, 1, 50]);
    assert_eq!(FieldId::StaticIp.cursor_column(5), 6);
    // An octet cannot be pushed past 255: 192 -> 292 is rejected
    FieldId::StaticIp.adjust(&mut data, 0, true);
    assert_eq!(data.static_ip.octets()[0], 192);
    // ...and wraps 9 -> 0 without carry like every other digit editor
    FieldId::StaticIp.adjust(&mut data, 5, true); // 169 -> 160
    assert_eq!(data.static_ip.octets()[1], 160);

    // The edit merges like any other field
    let mut committed = defaults();
    FieldId::StaticIp.transfer(&data, &mut committed);
    assert_eq!(committed.static_ip, data.static_ip);
    assert_eq!(committed.static_prefix, 8, "only the transferred field moves");
}

#[test]
fn colour_modes_are_both_offered() {
    let mut data = defaults();
    FieldId::ColorMode.adjust(&mut data, 0, true);
    assert_eq!(FieldId::ColorMode.display(&data), "RGBW");
    FieldId::ColorMode.adjust(&mut data, 0, true);
    assert_eq!(FieldId::ColorMode.display(&data), "RGB");
    assert!(FieldId::ColorMode.set_from_str(&mut data, "rgbw").is_ok());
    assert!(FieldId::ColorMode.set_from_str(&mut data, "rgb").is_ok());
}

#[test]
fn bound_universes_follow_the_mode() {
    // 150 RGB LEDs per port (defaults) = 450 B = 1 universe per port
    let mut data = defaults();
    assert!(!FieldId::UniversesBound.editable());
    FieldId::InputMode.set_from_str(&mut data, "artnet").unwrap();
    assert_eq!(data.bound_universes(), 8);
    assert_eq!(FieldId::UniversesBound.display(&data), "8 @ 0:0:0");

    // Mirror: every port shows the same universe(s)
    FieldId::PortMode.set_from_str(&mut data, "mirror").unwrap();
    assert_eq!(data.bound_universes(), 1);
    FieldId::PortMode.set_from_str(&mut data, "individual").unwrap();

    // 600 RGB LEDs = 4 universes per port = 32; clamped by the buffer above the base
    for p in 0..8 {
        FieldId::LedsPerPort(p).set_from_str(&mut data, "600").unwrap();
    }
    assert_eq!(data.bound_universes(), 32);
    FieldId::ArtNetSubNet.set_from_str(&mut data, "3").unwrap(); // base = 48 of 64
    assert_eq!(data.bound_universes(), 16);
    assert_eq!(FieldId::UniversesBound.display(&data), "16 @ 0:3:0");

    // Wired DMX and USB are one universe whatever the LED count
    FieldId::InputMode.set_from_str(&mut data, "dmx").unwrap();
    assert_eq!(data.bound_universes(), 1);
    assert_eq!(FieldId::UniversesBound.display(&data), "1 (wired DMX)");
    FieldId::InputMode.set_from_str(&mut data, "sacn").unwrap();
    FieldId::SacnUniverse.set_from_str(&mut data, "100").unwrap();
    assert_eq!(FieldId::UniversesBound.display(&data), "32 @ 100");
}

#[test]
fn schema_2_fields_are_reachable_and_bounded() {
    let mut data = defaults();
    assert_eq!(FieldId::SacnUniverse.display(&data), "00001");
    assert!(FieldId::SacnUniverse.set_from_str(&mut data, "0").is_err());
    assert!(FieldId::SacnUniverse.set_from_str(&mut data, "64000").is_err());
    FieldId::SacnUniverse.set_from_str(&mut data, "63999").unwrap();
    assert_eq!(data.sacn_universe, 63999);
    // The ten-thousands digit of 63999 rolling 6 -> 7 would exceed the range
    // and must be rejected without overflowing the u16 arithmetic.
    FieldId::SacnUniverse.adjust(&mut data, 0, true);
    assert_eq!(data.sacn_universe, 63999);

    assert!(FieldId::Backlight.set_from_str(&mut data, "0").is_err(), "backlight 0 is a dark menu");
    FieldId::Backlight.set_from_str(&mut data, "255").unwrap();
    assert_eq!(data.backlight, 255);
    assert_eq!(FieldId::Backlight.display(&data), "255");

    // LED counts are capped at what the output can transmit.
    let cap = common::MAX_LEDS_PER_PORT as u16;
    assert!(FieldId::LedsPerPort(0).set_from_str(&mut data, &(cap + 1).to_string()).is_err());
    FieldId::LedsPerPort(0).set_from_str(&mut data, &cap.to_string()).unwrap();
    assert!(FieldId::GroupSize(0).set_from_str(&mut data, &(cap + 1).to_string()).is_err());
}

#[test]
fn transfer_merges_only_the_edited_field() {
    let mut committed = defaults();
    let mut draft = committed;

    // Edit the DMX address on the draft while something else changes the
    // committed group size (e.g. a Load event during the edit)
    FieldId::DmxAddress.set_from_str(&mut draft, "100").unwrap();
    FieldId::GroupSize(1).set_from_str(&mut committed, "50").unwrap();

    FieldId::DmxAddress.transfer(&draft, &mut committed);

    assert_eq!(committed.dmx_address, 100, "edited field must transfer");
    assert_eq!(group_sizes(&committed)[1], 50, "concurrent change must survive");
}

#[test]
fn set_from_str_parses_every_editable_kind() {
    let mut data = defaults();

    FieldId::DmxAddress.set_from_str(&mut data, "512").unwrap();
    assert_eq!(data.dmx_address, 512);

    // Enum values are case-insensitive
    FieldId::InputMode.set_from_str(&mut data, "ArtNet>DMX").unwrap();
    assert_eq!(data.input_mode, InputMode::ArtNetToDmx);
    FieldId::InputMode.set_from_str(&mut data, "usb>dmx").unwrap();
    assert_eq!(data.input_mode, InputMode::UsbToDmx);

    FieldId::InputMode.set_from_str(&mut data, "sacn").unwrap();
    assert_eq!(data.input_mode, InputMode::Sacn);

    FieldId::IpMode.set_from_str(&mut data, "static").unwrap();
    FieldId::PortMode.set_from_str(&mut data, "mirror").unwrap();
    FieldId::ColorMode.set_from_str(&mut data, "rgbw").unwrap();

    FieldId::EthernetEnabled.set_from_str(&mut data, "on").unwrap();
    assert!(data.ethernet_enabled);
    FieldId::EthernetEnabled.set_from_str(&mut data, "0").unwrap();
    assert!(!data.ethernet_enabled);
}

#[test]
fn set_from_str_rejects_invalid_input() {
    let mut data = defaults();

    assert!(FieldId::DmxAddress.set_from_str(&mut data, "0").is_err());
    assert!(FieldId::DmxAddress.set_from_str(&mut data, "513").is_err());
    assert!(FieldId::DmxAddress.set_from_str(&mut data, "abc").is_err());
    assert!(FieldId::InputMode.set_from_str(&mut data, "bogus").is_err());

    // Read-only fields cannot be set from the console
    assert!(FieldId::IpAddr.set_from_str(&mut data, "1.2.3.4").is_err());
    assert!(FieldId::UniverseOffsets.set_from_str(&mut data, "0").is_err());
    assert!(FieldId::UniversesBound.set_from_str(&mut data, "4").is_err());

    // Nothing above may have modified the settings
    assert_eq!(data, defaults());
}


#[test]
fn every_page_fits_the_display() {
    // The menu is paged, not scrolling, so a page that overflows puts fields
    // somewhere the user cannot reach them. 320x172 at 9x15 is 35x11
    // characters, less one row for the page title / status line.
    for page in PAGES {
        assert!(
            page.fields.len() <= common::ui::fields::MAX_FIELDS_PER_PAGE,
            "page {:?} has {} fields, over the {} the display can show",
            page.title,
            page.fields.len(),
            common::ui::fields::MAX_FIELDS_PER_PAGE
        );
    }
}

#[test]
fn every_port_is_reachable_from_the_menu() {
    // Guards the failure this replaced: raising SMARTLED_PORT_COUNT without
    // extending PAGES left the extra ports configurable over the console but
    // invisible - and worse, sharing a label with port 4.
    let all: Vec<FieldId> = all_fields().collect();
    for port in 0..common::SMARTLED_PORT_COUNT {
        assert!(
            all.contains(&FieldId::GroupSize(port)),
            "port {port} has no group-size field"
        );
        assert!(
            all.contains(&FieldId::LedsPerPort(port)),
            "port {port} has no LED-count field"
        );
    }
}

#[test]
fn values_fit_the_usable_width() {
    // Labels and values share one row, and the panel's rounded corners cost a
    // column at each side (`tft_ui::CORNER_INSET`), so a value has
    // `USABLE_COLUMNS - VALUE_COLUMN` to render in. Anything longer runs off
    // the right-hand edge of the glass, where it cannot be read at all.
    let room = USABLE_COLUMNS - common::ui::fields::VALUE_COLUMN as usize;
    let mut data = defaults();
    // Enum fields render a word, not a number, so walk each one through its
    // whole cycle rather than trusting the default variant to be the longest.
    for field in all_fields() {
        for _ in 0..16 {
            field.adjust(&mut data, 0, true);
            let text = field.display(&data);
            assert!(
                text.chars().count() <= room,
                "{:?} renders {:?} ({} chars), over the {} columns available",
                field,
                text,
                text.chars().count(),
                room
            );
        }
        assert!(
            field.display_width() <= room,
            "{:?} declares a {}-column value, over the {} available",
            field,
            field.display_width(),
            room
        );
    }
}

#[test]
fn backlight_never_goes_below_the_readable_floor() {
    // Menu level 1 used to be the PWM duty itself: 0.4 %, which is not visible
    // on the Rev 2 panel. Level 1 now maps to the dimmest duty that is.
    assert_eq!(backlight_duty(1), MIN_BACKLIGHT_DUTY);
    assert_eq!(backlight_duty(0), MIN_BACKLIGHT_DUTY, "0 is not a menu value, but clamp anyway");
    assert_eq!(backlight_duty(255), 255, "the top of the scale is still full brightness");

    let mut last = 0;
    for level in 1..=255u8 {
        let duty = backlight_duty(level);
        assert!(duty >= MIN_BACKLIGHT_DUTY, "level {level} gave duty {duty}");
        assert!(duty >= last, "level {level} is dimmer than {}", level - 1);
        last = duty;
    }
    // Above the floor the rescale is within a count of the identity it
    // replaced, so settings saved by older firmware look unchanged.
    for level in [10u8, 64, 128, 200, 254] {
        let delta = backlight_duty(level).abs_diff(level);
        assert!(delta <= 1, "level {level} moved by {delta}");
    }
}

#[test]
fn labels_fit_before_the_value_column() {
    // Labels and values share one row on the TFT; a label that runs into the
    // value column overwrites the value it is labelling.
    for field in all_fields() {
        assert!(
            field.label().len() < common::ui::fields::VALUE_COLUMN as usize,
            "label {:?} is too long for the display",
            field.label()
        );
    }
}

#[test]
fn per_port_labels_are_distinct() {
    // The old wildcard match arm gave every port past the third the same label.
    let labels: Vec<&str> = (0..common::SMARTLED_PORT_COUNT)
        .map(|p| FieldId::GroupSize(p).label())
        .collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), labels.len(), "duplicate labels: {labels:?}");
}

#[test]
fn led_power_defaults_to_external_and_is_reachable() {
    // `External` is a safety property, not a preference: the same USB-C
    // connector takes a PC, and a host port must never be asked to run strips.
    let data = defaults();
    assert_eq!(data.led_power, LedPower::External);
    assert_eq!(FieldId::LedPower.display(&data), "External");
    assert!(all_fields().any(|f| f == FieldId::LedPower), "LED Power missing from the menu");
    assert!(FieldId::LedPower.editable());
    assert_eq!(FieldId::LedPower.digits(), 0, "edited as a whole, not per digit");
}

#[test]
fn led_power_toggles_parses_and_transfers() {
    let mut data = defaults();

    // Up and Down both toggle a two-state field.
    FieldId::LedPower.adjust(&mut data, 0, true);
    assert_eq!(data.led_power, LedPower::UsbBrick);
    assert_eq!(FieldId::LedPower.display(&data), "USB brick");
    FieldId::LedPower.adjust(&mut data, 0, false);
    assert_eq!(data.led_power, LedPower::External);

    // Console spellings documented in the readme, case-insensitive.
    for text in ["usb", "USB", "brick", "usb_brick"] {
        FieldId::LedPower.set_from_str(&mut data, text).unwrap();
        assert_eq!(data.led_power, LedPower::UsbBrick, "{text}");
    }
    for text in ["external", "Ext", "j26"] {
        FieldId::LedPower.set_from_str(&mut data, text).unwrap();
        assert_eq!(data.led_power, LedPower::External, "{text}");
    }
    assert!(FieldId::LedPower.set_from_str(&mut data, "on").is_err());

    // A committed edit moves only this field.
    let mut draft = defaults();
    draft.led_power = LedPower::UsbBrick;
    let mut committed = defaults();
    FieldId::DmxAddress.set_from_str(&mut committed, "77").unwrap();
    FieldId::LedPower.transfer(&draft, &mut committed);
    assert_eq!(committed.led_power, LedPower::UsbBrick);
    assert_eq!(committed.dmx_address, 77, "unrelated field must survive the merge");
}
