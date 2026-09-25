//! Mirror mode divides every port into the same number of addressable items.
//!
//! The worked examples are the ones the behaviour was specified from
//! (2026-09-17): port 1 with 24 LEDs grouped by 4 is six groups, so a 12-LED
//! port mirrors it grouped by 2 and a 48-LED port grouped by 8.
//!
//! The invariant underneath them, and the one the bench found broken the same
//! day, is that **two ports of the same length must group the same way**.
//! `two_identical_ports_get_identical_group_sizes` checks it exhaustively
//! rather than at the one configuration that was reported.

use common::ui::fields::{FieldId, VALUE_COLUMN};
use common::ui::{MenuData, ModuleSettings, SmartLedPortMode};

/// Columns a value has on the panel: the 35-column grid less a rounded corner
/// at each side. Kept in step with `tft_ui::CORNER_INSET` by hand, as in
/// `tests/fields.rs`.
const USABLE_COLUMNS: usize = 35 - 2;

fn menu(
    leds: [u16; common::SMARTLED_PORT_COUNT],
    groups: [u16; common::SMARTLED_PORT_COUNT],
    mode: SmartLedPortMode,
) -> MenuData {
    let mut data = MenuData::default();
    let mut s = settings(leds, groups);
    s.port_mode = mode;
    data.module = ModuleSettings::SmartLed(s);
    data
}

fn settings(leds: [u16; common::SMARTLED_PORT_COUNT], groups: [u16; common::SMARTLED_PORT_COUNT])
    -> common::ui::SmartLedSettings
{
    let ModuleSettings::SmartLed(mut s) = MenuData::default().module;
    s.leds_per_port = leds;
    s.dmx_group_size.0 = groups;
    s
}

/// Fill the unused ports with port 1's values so a test only says what it means.
fn spread(first: u16, rest: u16) -> [u16; common::SMARTLED_PORT_COUNT] {
    let mut out = [rest; common::SMARTLED_PORT_COUNT];
    out[0] = first;
    out
}

#[test]
fn port_one_sets_the_group_count_and_keeps_its_own_size() {
    let s = settings(spread(24, 24), spread(4, 4));
    assert_eq!(s.mirror_group_count(), 6, "24 LEDs grouped by 4 is six groups");
    assert_eq!(s.mirror_group_size()[0], 4, "port 1 keeps the size it was configured with");
}

#[test]
fn shorter_and_longer_ports_divide_into_the_same_six_groups() {
    let mut leds = spread(24, 24);
    leds[1] = 12; // half the length -> half the group
    leds[2] = 48; // twice the length -> twice the group
    let s = settings(leds, spread(4, 4));
    let sizes = s.mirror_group_size();
    assert_eq!(sizes[1], 2, "12 LEDs across 6 groups");
    assert_eq!(sizes[2], 8, "48 LEDs across 6 groups");
    // Every port shows the same number of addressable items.
    for port in 0..3 {
        let covered = sizes[port] as usize * 6;
        assert!(covered <= s.leds_per_port[port] as usize,
                "port {port} covers {covered} of {} LEDs - must never overrun",
                s.leds_per_port[port]);
    }
}

#[test]
fn a_length_that_does_not_divide_rounds_down_and_leaves_the_tail_dark() {
    let mut leds = spread(24, 24);
    leds[3] = 50; // 50 / 6 = 8.33
    let s = settings(leds, spread(4, 4));
    assert_eq!(s.mirror_group_size()[3], 8, "rounds down rather than stretching a group");
    assert_eq!(8 * 6, 48, "so 48 of the 50 are covered and the last two stay dark");
}

#[test]
fn a_port_shorter_than_the_group_count_still_groups_by_one() {
    let mut leds = spread(24, 24);
    leds[4] = 3; // fewer LEDs than there are groups
    let s = settings(leds, spread(4, 4));
    assert_eq!(s.mirror_group_size()[4], 1,
               "never zero: the port shows the leading groups, one LED each");
}

#[test]
fn two_identical_ports_get_identical_group_sizes() {
    // The bench report of 2026-09-17: P1 and P2 both 64 LEDs, P1 grouped by 3,
    // and P2 computed 2. The group count was `ceil(64 / 3) = 22`, whose last
    // group holds a single LED on port 1 but would be a full group of
    // `64 / 22 = 2` on port 2 - so the same strip, configured the same way,
    // rendered differently. Rounding the count down fixes it by construction.
    let s = settings(spread(64, 64), spread(3, 9));
    assert_eq!(s.mirror_group_count(), 21, "floor(64/3), not ceil");
    let sizes = s.mirror_group_size();
    assert_eq!(sizes[1], 3, "an identical strip must group identically");

    // The narrower case the exhaustive sweep below turned up while fixing the
    // reported one: 10 grouped by 4 is two items, so dividing the length by the
    // item count gives a second 10-LED port `10 / 2 = 5`. Scaling from port 1
    // gives 4, which is the only answer a mirror can have.
    let s = settings(spread(10, 10), spread(4, 9));
    assert_eq!(s.mirror_group_count(), 2);
    assert_eq!(s.mirror_group_size()[1], 4, "not 10 / 2");

    // Stronger than the one case: any port the same length as port 1 must come
    // out the same, whatever the grouping.
    for leds in 1..=600u16 {
        for group in 1..=32u16 {
            let s = settings(spread(leds, leds), spread(group, 9));
            let sizes = s.mirror_group_size();
            assert_eq!(
                sizes[1], sizes[0],
                "{leds} LEDs grouped by {group}: port 2 got {} against port 1's {}",
                sizes[1], sizes[0]
            );
        }
    }
}

#[test]
fn every_port_lights_the_same_fraction_of_its_strip() {
    // What "the same addressable items" means physically. Port 1 here wastes
    // its 10th LED (10 grouped by 3 is three items covering 9), and a mirror of
    // it wastes the same proportion - it does not quietly fill the gap with a
    // longer group, which would make the strips look different.
    let mut leds = spread(10, 10);
    leds[1] = 64;
    leds[2] = 50;
    leds[3] = 12;
    let s = settings(leds, spread(3, 2));
    let groups = s.mirror_group_count() as usize;
    assert_eq!(groups, 3, "floor(10/3) - the 10th LED is the rounding tail");

    let sizes = s.mirror_group_size();
    let first_len = s.leds_per_port[0] as usize;
    let first_covered = sizes[0] as usize * groups;
    assert_eq!(first_covered, 9, "port 1 lights 9 of its 10");

    for (port, size) in sizes.iter().enumerate() {
        let len = s.leds_per_port[port] as usize;
        let covered = *size as usize * groups;
        assert!(covered <= len, "port {port} covers {covered} of {len} - overruns");
        // Cross-multiplied rather than divided, to stay in integers: this port
        // lights no larger a fraction than port 1 does...
        assert!(
            covered * first_len <= first_covered * len,
            "port {port} lights {covered}/{len}, more than port 1's {first_covered}/{first_len}"
        );
        // ...and no smaller a fraction, bar the last item it cannot fit.
        assert!(
            (covered + groups) * first_len > first_covered * len,
            "port {port} lights {covered}/{len}, over an item short of port 1's \
             {first_covered}/{first_len}"
        );
    }
}

#[test]
fn mirror_reports_the_computed_group_size_as_derived() {
    // The UI asks each field whether the mode is computing it; that is what
    // tells it to dim the stored value and show this one beside it.
    let mut leds = spread(24, 24);
    leds[1] = 12;
    leds[2] = 48;
    let data = menu(leds, spread(4, 9), SmartLedPortMode::Mirror);

    assert_eq!(
        FieldId::GroupSize(0).derived(&data),
        None,
        "port 1 is the one being mirrored, so its own size is not computed"
    );
    assert_eq!(FieldId::GroupSize(1).derived(&data).as_deref(), Some("002"));
    assert_eq!(FieldId::GroupSize(2).derived(&data).as_deref(), Some("008"));
    // Zero-padded exactly like `display`, so the two columns line up.
    assert_eq!(FieldId::GroupSize(1).display(&data), "009", "the stored value is untouched");
}

#[test]
fn individual_mode_computes_nothing() {
    let data = menu(spread(24, 12), spread(4, 9), SmartLedPortMode::Individual);
    for port in 0..common::SMARTLED_PORT_COUNT {
        assert_eq!(
            FieldId::GroupSize(port).derived(&data),
            None,
            "port {} uses its stored group size in Individual mode",
            port + 1
        );
    }
}

#[test]
fn a_derived_value_fits_beside_the_stored_one() {
    // The panel draws the stored value at `VALUE_COLUMN` and the computed one a
    // column past the end of it, so the row has to hold both plus the gap.
    let room = USABLE_COLUMNS - VALUE_COLUMN as usize;
    let width = FieldId::GroupSize(1).display_width();
    assert!(
        width * 2 + 1 <= room,
        "a {width}-column value drawn twice needs {} of the {room} columns available",
        width * 2 + 1
    );
}
