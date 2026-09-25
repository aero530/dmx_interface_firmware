//! Per-port fixture layouts: the entry encoding, the running-sum arithmetic,
//! and how `PortLayout` counts items once a map is in play.
//!
//! The encoding is what the EEPROM blob stores verbatim, so these tests are as
//! much a format spec as a unit test — a change that breaks one breaks stored
//! layouts on every board in the field.

use common::ui::fixtures::{FixtureEntry, FixtureMap, MAX_FIXTURES_PER_PORT};
use common::ui::{
    LedGrouping, SmartLedColorMode, SmartLedDmxGroupSize, SmartLedPortMode, SmartLedSettings,
};
use common::{MAX_LEDS_PER_PORT, SMARTLED_PORT_COUNT};

fn settings(leds: u16, group: u16, grouping: LedGrouping) -> SmartLedSettings {
    SmartLedSettings {
        leds_per_port: [leds; SMARTLED_PORT_COUNT],
        dmx_group_size: SmartLedDmxGroupSize([group; SMARTLED_PORT_COUNT]),
        grouping,
        port_mode: SmartLedPortMode::Individual,
        color_mode: SmartLedColorMode::Rgb,
    }
}

/// Port 0 = the worked example from the design: a 10-LED wash, a 5-LED accent,
/// a 5-LED gap, then a 30-LED bar on a 60-LED strip.
fn worked_example() -> FixtureMap {
    let mut map = FixtureMap::EMPTY;
    map.0[0][0] = FixtureEntry::fixture(10);
    map.0[0][1] = FixtureEntry::fixture(5);
    map.0[0][2] = FixtureEntry::gap(5);
    map.0[0][3] = FixtureEntry::fixture(30);
    map
}

// --------------------------------------------------------------- the entry

#[test]
fn an_entry_carries_a_length_and_a_kind() {
    let f = FixtureEntry::fixture(10);
    assert_eq!(f.leds(), 10);
    assert!(!f.is_gap());
    assert!(!f.is_none());

    let g = FixtureEntry::gap(5);
    assert_eq!(g.leds(), 5);
    assert!(g.is_gap());
    assert!(!g.is_none());
}

#[test]
fn zero_length_is_the_terminator_whatever_the_kind() {
    // No count is stored, so a zero-length entry ends the list. A "gap of
    // nothing" that kept its flag bit would be a non-terminating zero and would
    // silently swallow every entry behind it.
    assert!(FixtureEntry::fixture(0).is_none());
    assert!(FixtureEntry::gap(0).is_none());
    assert_eq!(FixtureEntry::gap(0), FixtureEntry::NONE);
    assert_eq!(FixtureEntry::default(), FixtureEntry::NONE);
}

#[test]
fn the_raw_form_round_trips_and_is_the_stored_format() {
    for len in [1_u16, 2, 255, 256, 600] {
        for entry in [FixtureEntry::fixture(len), FixtureEntry::gap(len)] {
            let back = FixtureEntry::from_raw(entry.raw());
            assert_eq!(back, entry, "raw round-trip failed for {entry:?}");
            assert_eq!(back.leds(), len);
        }
    }
    // A raw word whose length bits are zero is the terminator even if the gap
    // flag is set — that is what a torn or stale blob looks like.
    assert!(FixtureEntry::from_raw(0x8000).is_none());
}

#[test]
fn a_length_holds_any_strip_the_hardware_can_drive() {
    let longest = FixtureEntry::fixture(MAX_LEDS_PER_PORT as u16);
    assert_eq!(longest.leds(), MAX_LEDS_PER_PORT as u16);
    assert!(!longest.is_gap(), "the top of the range must not collide with the flag");
}

// ----------------------------------------------------------------- the map

#[test]
fn entries_stop_at_the_terminator() {
    let map = worked_example();
    assert_eq!(map.entries(0).count(), 4);
    assert!(!map.is_empty(0));
    assert_eq!(map.entries(1).count(), 0, "an unconfigured port has no entries");
    assert!(map.is_empty(1));
}

#[test]
fn starts_are_the_running_sum_and_gaps_advance_it() {
    // This is the whole point of storing runs: the start of every fixture falls
    // out of the lengths before it, so two fixtures cannot overlap.
    let spans: heapless::Vec<(u16, u16), 8> = worked_example().spans(0).collect();
    assert_eq!(spans.as_slice(), &[(0, 10), (10, 5), (20, 30)]);
}

#[test]
fn a_gap_costs_leds_but_no_channels() {
    let map = worked_example();
    assert_eq!(map.fixture_count(0), 3, "three lit runs, the gap is not addressable");
    assert_eq!(map.used(0), 50, "10 + 5 + 5 + 30, the gap included");
}

#[test]
fn a_full_list_is_five_entries() {
    let mut map = FixtureMap::EMPTY;
    for slot in 0..MAX_FIXTURES_PER_PORT {
        map.0[0][slot] = FixtureEntry::fixture(10);
    }
    assert_eq!(map.entries(0).count(), MAX_FIXTURES_PER_PORT);
    assert_eq!(map.fixture_count(0), MAX_FIXTURES_PER_PORT as u16);
    assert_eq!(map.used(0), 10 * MAX_FIXTURES_PER_PORT as u16);
}

#[test]
fn an_out_of_range_port_reads_as_the_last_one() {
    // Reached from a corrupt stored index; every other per-port table in the
    // firmware clamps rather than indexing blind. Asserting it lands on the
    // LAST port rather than merely "does not panic" — the weaker version passes
    // whatever the clamp does, including clamping to the wrong end.
    let mut map = worked_example();
    map.0[SMARTLED_PORT_COUNT - 1][0] = FixtureEntry::fixture(7);

    let over = SMARTLED_PORT_COUNT + 5;
    assert_eq!(map.fixture_count(over), map.fixture_count(SMARTLED_PORT_COUNT - 1));
    assert_eq!(map.used(over), 7);
    assert!(!map.is_empty(over));
}

// ------------------------------------------------------------- the layout

#[test]
fn fixtures_make_the_item_count_the_fixture_count() {
    let s = settings(60, 4, LedGrouping::Fixtures);
    let map = worked_example();
    let layout = s.layout(&map);

    assert_eq!(layout.virtual_leds_per_port()[0], 3, "three lit runs");
    assert_eq!(layout.virtual_leds_per_port()[1], 1, "unconfigured: the default, one run");
}

#[test]
fn an_unconfigured_port_uses_the_default_entry_not_its_group_size() {
    // Falling back to `Groups` was the first design and it was rejected: a port
    // silently behaving as though it were in another mode is hidden state, and
    // nothing on the panel distinguishes "not configured yet" from "configured
    // to look like Groups". One visible `F001` is a starting point instead.
    let s = settings(60, 4, LedGrouping::Fixtures);
    let empty = s.layout(&FixtureMap::EMPTY);

    assert_eq!(
        empty.virtual_leds_per_port()[0],
        1,
        "one item, not the fifteen a group size of 4 would give"
    );
    assert_eq!(empty.item_span(0, 0), Some((0, 1)), "one LED, at the start");
    assert_eq!(empty.item_span(0, 1), None, "and nothing after it");

    let groups = settings(60, 4, LedGrouping::Groups);
    assert_ne!(
        empty.virtual_leds_per_port(),
        groups.virtual_leds_per_port(),
        "the two modes must be distinguishable, or the default is not visible"
    );
}

#[test]
fn a_port_with_no_leds_costs_no_channels_even_with_the_default() {
    // Otherwise the default entry would reserve a universe slot for a strip
    // that does not exist — and `bound_universes` is what ArtPollReply
    // advertises, so it would misaddress a rig.
    let mut s = settings(60, 4, LedGrouping::Fixtures);
    s.leds_per_port[3] = 0;
    assert_eq!(s.layout(&FixtureMap::EMPTY).virtual_leds_per_port()[3], 0);
}

#[test]
fn fixtures_cost_the_channels_they_use_and_no_more() {
    // The budget chain hangs off the item count, so a fixture layout must be
    // cheaper than the per-pixel layout it replaces. This is what reaches
    // ArtPollReply, so getting it wrong misaddresses a rig.
    let s = settings(600, 1, LedGrouping::Fixtures);
    let mut map = FixtureMap::EMPTY;
    map.0[0][0] = FixtureEntry::fixture(300);
    map.0[0][1] = FixtureEntry::fixture(300);

    let layout = s.layout(&map);
    assert_eq!(layout.virtual_leds_per_port()[0], 2);
    assert_eq!(layout.bytes_per_port()[0], 6, "two items, RGB");
    assert_eq!(layout.universes_per_port()[0], 1);
}

// ---------------------------------------------------------------- the codec
//
// This is the on-EEPROM format. A change that breaks a test here breaks stored
// layouts on every board in the field, so these read as a spec, not a unit test.

use common::ui::fixtures::{crc16, FIXTURE_HEADER_LEN, FIXTURE_PAYLOAD_LEN};

fn stored(map: &FixtureMap) -> ([u8; FIXTURE_HEADER_LEN], [u8; FIXTURE_PAYLOAD_LEN]) {
    let mut payload = [0_u8; FIXTURE_PAYLOAD_LEN];
    map.encode(&mut payload);
    (FixtureMap::header(&payload), payload)
}

#[test]
fn the_payload_is_eighty_bytes_and_fills_five_pages() {
    // Page-aligned at 0xB0 and ending exactly at the top of the 256-byte part.
    // `write_page` rejects a write crossing a 16-byte boundary, so this is what
    // keeps the chunker a plain `chunks(16)` with no special first case.
    assert_eq!(FIXTURE_PAYLOAD_LEN, 80);
    assert_eq!(FIXTURE_PAYLOAD_LEN % 16, 0);
    assert_eq!(0xB0 + FIXTURE_PAYLOAD_LEN, 256);
}

#[test]
fn a_map_round_trips_through_the_stored_form() {
    let map = worked_example();
    let (header, payload) = stored(&map);
    assert_eq!(FixtureMap::decode(&header, &payload), Some(map));
}

#[test]
fn a_blank_region_decodes_as_empty_not_as_garbage() {
    // An erased EEPROM reads 0xFF everywhere. Every bit pattern is a legal
    // layout, so without the magic and CRC this would decode as eight ports of
    // 1023-LED fixtures and paint nonsense.
    let blank_header = [0xFF_u8; FIXTURE_HEADER_LEN];
    let blank_payload = [0xFF_u8; FIXTURE_PAYLOAD_LEN];
    assert_eq!(FixtureMap::decode(&blank_header, &blank_payload), None);
}

#[test]
fn a_corrupt_payload_is_rejected_by_the_crc() {
    let (header, mut payload) = stored(&worked_example());
    payload[7] ^= 0x01; // one bit, anywhere
    assert_eq!(
        FixtureMap::decode(&header, &payload),
        None,
        "a single flipped bit must not survive"
    );
}

#[test]
fn a_torn_write_reads_back_as_no_map() {
    // The reason the header is written LAST and the magic invalidated FIRST.
    // Power lost mid-write leaves a complete payload with no valid header; that
    // has to be "no map" and not half a layout, because half a layout is
    // indistinguishable from a deliberate one.
    let (_, payload) = stored(&worked_example());
    let invalidated = [0xFF_u8; FIXTURE_HEADER_LEN];
    assert_eq!(FixtureMap::decode(&invalidated, &payload), None);

    // And the other order: a header that survived from an older, different map
    // must not validate the new payload.
    let (stale_header, _) = stored(&FixtureMap::EMPTY);
    assert_eq!(FixtureMap::decode(&stale_header, &payload), None);
}

#[test]
fn a_wrong_format_version_is_rejected() {
    let (mut header, payload) = stored(&worked_example());
    header[2] = header[2].wrapping_add(1);
    assert_eq!(FixtureMap::decode(&header, &payload), None);
}

#[test]
fn a_short_read_is_rejected_rather_than_panicking() {
    let (header, payload) = stored(&worked_example());
    assert_eq!(FixtureMap::decode(&header[..2], &payload), None);
    assert_eq!(FixtureMap::decode(&header, &payload[..10]), None);
}

#[test]
fn the_crc_is_ccitt_false() {
    // Pinned against the standard check value so the polynomial and seed cannot
    // drift: CRC-16/CCITT-FALSE of "123456789" is 0x29B1.
    assert_eq!(crc16(b"123456789"), 0x29B1);
}

// ------------------------------------------------------------- the blanking
//
// WS2812s latch: a pixel the render does not write keeps its last colour. Every
// test here starts from a DIRTY buffer, because rendering into an already-black
// one passes whether or not the blanking happens at all.

use common::event_router::paint_port;
use smart_leds::{White, RGBW};

const LIT: RGBW<u8> = RGBW { r: 9, g: 9, b: 9, a: White(9) };
const OFF: RGBW<u8> = RGBW { r: 0, g: 0, b: 0, a: White(0) };

fn dirty(n: usize) -> [RGBW<u8>; 64] {
    let mut buf = [LIT; 64];
    buf[n..].fill(LIT);
    buf
}

/// Paint through the layout exactly as the router does.
fn render(map: &FixtureMap, s: &SmartLedSettings, port: usize, out: &mut [RGBW<u8>]) {
    let layout = s.layout(map);
    let items = layout.virtual_leds_per_port()[port] as usize;
    paint_port(
        out,
        s.leds_per_port[port] as usize,
        items,
        |item| layout.item_span(port, item),
        |_| LIT,
    );
}

#[test]
fn a_declared_gap_goes_dark() {
    // The 5-LED gap at 16..=20 of the worked example must be black, not the
    // colour whatever was there before left behind.
    let mut buf = dirty(60);
    let s = settings(60, 1, LedGrouping::Fixtures);
    render(&worked_example(), &s, 0, &mut buf);

    for led in 0..15 {
        assert_eq!(buf[led], LIT, "LED {led} is inside a fixture");
    }
    for led in 15..20 {
        assert_eq!(buf[led], OFF, "LED {led} is the gap and must be dark");
    }
    for led in 20..50 {
        assert_eq!(buf[led], LIT, "LED {led} is inside the last fixture");
    }
}

#[test]
fn the_tail_past_the_last_fixture_goes_dark() {
    // The worked example covers 50 of 60. The last ten have never been written
    // this frame and would otherwise hold the previous layout's colour.
    let mut buf = dirty(60);
    let s = settings(60, 1, LedGrouping::Fixtures);
    render(&worked_example(), &s, 0, &mut buf);

    for led in 50..60 {
        assert_eq!(buf[led], OFF, "LED {led} is past the last fixture");
    }
}

#[test]
fn shrinking_an_entry_darkens_what_it_gave_up() {
    let s = settings(60, 1, LedGrouping::Fixtures);
    let mut big = FixtureMap::EMPTY;
    big.0[0][0] = FixtureEntry::fixture(40);

    let mut buf = dirty(60);
    render(&big, &s, 0, &mut buf);
    assert_eq!(buf[39], LIT, "the wide layout reaches LED 40");

    let mut small = FixtureMap::EMPTY;
    small.0[0][0] = FixtureEntry::fixture(10);
    render(&small, &s, 0, &mut buf);

    assert_eq!(buf[9], LIT, "the narrow layout still lights its own ten");
    for led in 10..40 {
        assert_eq!(buf[led], OFF, "LED {led} was given up and must go dark");
    }
}

#[test]
fn dropping_an_entry_darkens_its_run() {
    let s = settings(60, 1, LedGrouping::Fixtures);
    let mut two = FixtureMap::EMPTY;
    two.0[0][0] = FixtureEntry::fixture(10);
    two.0[0][1] = FixtureEntry::fixture(10);

    let mut buf = dirty(60);
    render(&two, &s, 0, &mut buf);
    assert_eq!(buf[15], LIT, "the second fixture is lit");

    // Setting a length to zero is how an entry is removed - it terminates the
    // list, so there is no separate delete.
    let mut one = two;
    one.0[0][1] = FixtureEntry::NONE;
    render(&one, &s, 0, &mut buf);
    for led in 10..20 {
        assert_eq!(buf[led], OFF, "LED {led} belonged to the dropped entry");
    }
}

#[test]
fn switching_from_a_longer_groups_layout_darkens_the_difference() {
    // The case a change-triggered blank would be most likely to miss: the mode
    // changes, not the fixture list.
    let mut buf = dirty(60);
    let groups = settings(60, 1, LedGrouping::Groups);
    render(&FixtureMap::EMPTY, &groups, 0, &mut buf);
    assert_eq!(buf[59], LIT, "Groups with size 1 lights the whole strip");

    let mut short = FixtureMap::EMPTY;
    short.0[0][0] = FixtureEntry::fixture(5);
    let fixtures = settings(60, 1, LedGrouping::Fixtures);
    render(&short, &fixtures, 0, &mut buf);

    for led in 5..60 {
        assert_eq!(buf[led], OFF, "LED {led} is not covered by the new layout");
    }
}

#[test]
fn pixels_past_the_configured_count_are_left_alone() {
    // Nothing beyond `leds_per_port` is clocked out, so blanking it would be
    // work for pixels no one can see - and would hide a count that is wrong.
    let mut buf = dirty(64);
    let s = settings(20, 1, LedGrouping::Fixtures);
    let mut map = FixtureMap::EMPTY;
    map.0[0][0] = FixtureEntry::fixture(5);
    render(&map, &s, 0, &mut buf);

    assert_eq!(buf[19], OFF, "inside the configured strip, so blanked");
    assert_eq!(buf[20], LIT, "past it, so untouched");
}

// ------------------------------------------------------------- the editing
//
// Fixture rows are edited through the map, not through `MenuData`, so the
// generic guards in tests/fields.rs skip them. These are the replacement.

use common::ui::fields::FIXTURE_VALUE_WIDTH;
use common::ui::FieldId;

const P0: FieldId = FieldId::Fixture(0, 0);

#[test]
fn an_entry_renders_as_one_token_four_characters_wide() {
    // One token because `set` keeps only the second whitespace-delimited word;
    // fixed width so the column does not jump as entries come and go.
    let mut map = FixtureMap::EMPTY;
    assert_eq!(
        P0.fixture_display(&map),
        "F001",
        "an unconfigured port shows its default, not a blank"
    );
    // `----` is a slot past the end of a list that does exist.
    map.0[0][0] = FixtureEntry::fixture(10);
    assert_eq!(FieldId::Fixture(0, 1).fixture_display(&map), "----");
    assert_eq!(P0.fixture_display(&map), "F010");

    map.0[0][0] = FixtureEntry::gap(5);
    assert_eq!(P0.fixture_display(&map), "G005");

    map.0[0][0] = FixtureEntry::fixture(600);
    assert_eq!(P0.fixture_display(&map), "F600");

    for entry in [FixtureEntry::NONE, FixtureEntry::fixture(1), FixtureEntry::gap(600)] {
        map.0[0][0] = entry;
        assert_eq!(
            P0.fixture_display(&map).len(),
            FIXTURE_VALUE_WIDTH,
            "every state must be the same width"
        );
        assert!(
            !P0.fixture_display(&map).contains(char::is_whitespace),
            "a space would be truncated by the console's `set`"
        );
    }
}

#[test]
fn an_entry_round_trips_through_the_console_form() {
    let mut map = worked_example();
    for slot in 0..4 {
        let field = FieldId::Fixture(0, slot);
        let rendered = field.fixture_display(&map);
        // Seeded so slot 0 is occupied: on an untouched map the default would
        // answer for it and the round trip would prove nothing.
        let mut dst = worked_example();
        field
            .fixture_set_from_str(&mut dst, 600, &rendered)
            .expect("what `get` prints must be accepted by `set`");
        assert_eq!(field.fixture_display(&dst), rendered);
    }
    // And the spellings a person would actually type.
    let mut m = FixtureMap::EMPTY;
    assert!(P0.fixture_set_from_str(&mut m, 600, "12").is_ok(), "a bare number is a fixture");
    assert_eq!(P0.fixture_display(&m), "F012");
    assert!(P0.fixture_set_from_str(&mut m, 600, "g7").is_ok(), "case-insensitive");
    assert_eq!(P0.fixture_display(&m), "G007");
    assert!(P0.fixture_set_from_str(&mut m, 600, "----").is_ok());
    assert!(m.is_empty(0));
}

#[test]
fn the_span_column_shows_where_a_fixture_lands_and_nothing_for_a_gap() {
    let map = worked_example();
    assert_eq!(FieldId::Fixture(0, 0).fixture_span(&map).as_deref(), Some("001-010"));
    assert_eq!(FieldId::Fixture(0, 1).fixture_span(&map).as_deref(), Some("011-015"));
    assert_eq!(
        FieldId::Fixture(0, 2).fixture_span(&map),
        None,
        "a gap has no channels, so showing a range would suggest it does"
    );
    assert_eq!(FieldId::Fixture(0, 3).fixture_span(&map).as_deref(), Some("021-050"));
    assert_eq!(FieldId::Fixture(0, 4).fixture_span(&map), None, "empty slot");
}

#[test]
fn a_length_that_would_overrun_the_strip_is_rejected() {
    let mut map = FixtureMap::EMPTY;
    map.0[0][0] = FixtureEntry::fixture(30);
    map.0[0][1] = FixtureEntry::fixture(20);

    let second = FieldId::Fixture(0, 1);
    assert_eq!(second.fixture_max(&map, 60), 30, "60 total, 30 already spent elsewhere");
    assert!(second.fixture_set_from_str(&mut map, 60, "F031").is_err());
    assert!(second.fixture_set_from_str(&mut map, 60, "F030").is_ok());
}

#[test]
fn an_entry_can_always_be_lowered_even_when_the_strip_shrank() {
    // The trap `fixture_max` exists for. Bound the entry by what is LEFT and an
    // entry already over the limit can never be reduced: raising is rejected,
    // and the check cannot tell which way the edit went. Lowering `LEDs Port n`
    // below the total would then freeze every entry on that port for good.
    let mut map = FixtureMap::EMPTY;
    map.0[0][0] = FixtureEntry::fixture(50);

    // The operator drops the strip to 20 LEDs; the entry is now over budget.
    assert_eq!(P0.fixture_max(&map, 20), 20, "its own length still counts as available");
    assert!(
        P0.fixture_set_from_str(&mut map, 20, "F010").is_ok(),
        "it must still be possible to fix the layout"
    );
    assert_eq!(P0.fixture_display(&map), "F010");
}

#[test]
fn digit_zero_toggles_the_kind_and_the_rest_edit_the_length() {
    let mut map = FixtureMap::EMPTY;
    map.0[0][0] = FixtureEntry::fixture(10);

    P0.fixture_adjust(&mut map, 600, 0, true);
    assert_eq!(P0.fixture_display(&map), "G010", "kind toggled, length kept");
    P0.fixture_adjust(&mut map, 600, 0, false);
    assert_eq!(P0.fixture_display(&map), "F010", "and back - both directions toggle");

    // Digits 1..=3 are hundreds, tens, units; each wraps without carrying.
    P0.fixture_adjust(&mut map, 600, 3, true);
    assert_eq!(P0.fixture_display(&map), "F011");
    P0.fixture_adjust(&mut map, 600, 2, true);
    assert_eq!(P0.fixture_display(&map), "F021");
    P0.fixture_adjust(&mut map, 600, 1, true);
    assert_eq!(P0.fixture_display(&map), "F121");
}

#[test]
fn toggling_an_empty_slot_creates_an_entry() {
    // The row has no other control, so this is how an entry is added - and
    // setting a length to zero is how it goes away again. No [+]/[-] and no
    // separate count field to drift out of step with the stored form.
    let mut map = FixtureMap::EMPTY;
    assert!(map.is_empty(0));

    P0.fixture_adjust(&mut map, 600, 0, true);
    assert_eq!(
        P0.fixture_display(&map),
        "F001",
        "a fixture, because that is what someone opening an empty table is making"
    );
    assert!(!map.is_empty(0));

    P0.fixture_set_from_str(&mut map, 600, "----").unwrap();
    assert!(map.is_empty(0), "and back to nothing");
}

#[test]
fn an_adjustment_that_would_overrun_is_refused_rather_than_clamped() {
    // Same contract as `range()` everywhere else: an out-of-range result is
    // discarded, so the displayed value never silently becomes something the
    // operator did not dial in.
    let mut map = FixtureMap::EMPTY;
    map.0[0][0] = FixtureEntry::fixture(95);

    P0.fixture_adjust(&mut map, 100, 1, true); // 95 -> 195, over
    assert_eq!(P0.fixture_display(&map), "F095", "rejected, not clamped to 100");

    P0.fixture_adjust(&mut map, 100, 3, true); // 95 -> 96, fine
    assert_eq!(P0.fixture_display(&map), "F096");
}

// ------------------------------------------------- the Layout page summary

#[test]
fn a_port_row_lists_its_lit_runs_and_omits_the_gaps() {
    // The worked example is F010, F005, G005, F030 — the gap is meaningful in
    // the port's own table, where the running total gives it a position, but
    // here it would sit between the things that answer to DMX channels and say
    // nothing.
    let row = FieldId::GroupSize(0);
    assert_eq!(row.fixture_summary(&worked_example()).as_deref(), Some("F010 F005 F030"));
}

#[test]
fn a_port_with_no_list_summarises_as_its_default() {
    // What the row shows and what the strip does have to agree, and the strip
    // lights one LED.
    assert_eq!(FieldId::GroupSize(1).fixture_summary(&worked_example()).as_deref(), Some("F001"));
    assert_eq!(FieldId::GroupSize(0).fixture_summary(&FixtureMap::EMPTY).as_deref(), Some("F001"));
}

#[test]
fn a_long_list_is_counted_rather_than_truncated() {
    // Five `Fxxx ` groups would be 24 characters against a 17-column value.
    // Cutting mid-token would read as a shorter fixture, which is worse than
    // not showing it at all.
    let mut map = FixtureMap::EMPTY;
    for slot in 0..5 {
        map.0[0][slot] = FixtureEntry::fixture(8);
    }
    let text = FieldId::GroupSize(0).fixture_summary(&map).expect("five runs");
    assert_eq!(text, "F008 F008 F008 +2");

    // The column is `35 - 2 corner insets - 16` = 17.
    for count in 1..=5 {
        let mut m = FixtureMap::EMPTY;
        for slot in 0..count {
            m.0[0][slot] = FixtureEntry::fixture(600);
        }
        let t = FieldId::GroupSize(0).fixture_summary(&m).expect("a run");
        assert!(t.len() <= 17, "{count} runs render {t:?}, {} of 17 columns", t.len());
    }
}

#[test]
fn only_groups_mode_lets_the_panel_edit_a_port_row() {
    // Under `Individual` the size is pinned to 1 whatever is stored, and under
    // `Fixtures` the row is a doorway into the port's table. Dialling a number
    // that changes nothing is the confusion this removes.
    let row = FieldId::GroupSize(0);
    for (mode, editable) in [
        (LedGrouping::Groups, true),
        (LedGrouping::Individual, false),
        (LedGrouping::Fixtures, false),
    ] {
        let mut data = common::ui::MenuData::default();
        data.module = common::ui::ModuleSettings::SmartLed(settings(60, 4, mode));
        assert_eq!(row.editable_now(&data), editable, "{mode} should be editable={editable}");
    }

    // The console still sets it in every mode: it is the value `Groups` goes
    // back to, and refusing it would mean leaving the mode to set it up.
    assert!(row.editable(), "the static property is unchanged");
}

// ------------------------------------------- one value, or a meaningful pair

fn menu(grouping: LedGrouping, mirror: bool, leds: [u16; SMARTLED_PORT_COUNT]) -> common::ui::MenuData {
    let mut data = common::ui::MenuData::default();
    let mut s = settings(0, 4, grouping);
    s.leds_per_port = leds;
    s.port_mode = if mirror { SmartLedPortMode::Mirror } else { SmartLedPortMode::Individual };
    data.module = common::ui::ModuleSettings::SmartLed(s);
    data
}

#[test]
fn individual_shows_the_one_that_matters_and_not_a_pair() {
    // `004 001` puts a number on screen that means nothing and cannot be
    // edited. Only the 1 is real.
    let data = menu(LedGrouping::Individual, false, [150; SMARTLED_PORT_COUNT]);
    let row = FieldId::GroupSize(0);
    assert_eq!(row.layout_row_value(&data, &FixtureMap::EMPTY).as_deref(), Some("001"));
}

#[test]
fn mirror_keeps_the_pair_because_both_numbers_say_something() {
    // The stored size is what the port would use alone; the computed one is
    // what mirroring makes it use. Neither implies the other.
    let mut leds = [150_u16; SMARTLED_PORT_COUNT];
    leds[1] = 300;
    let data = menu(LedGrouping::Groups, true, leds);

    let row = FieldId::GroupSize(1);
    assert_eq!(
        row.layout_row_value(&data, &FixtureMap::EMPTY),
        None,
        "Mirror must fall through to the grey/blue pair"
    );
    assert_eq!(row.derived(&data).as_deref(), Some("008"), "300 LEDs scaled from 150/4");
}

#[test]
fn mirror_beats_individual_when_both_apply() {
    // The ordering bug this guards: `Individual` was tested first and reported
    // 1 for every port, hiding the proportional scaling that is genuinely in
    // force. A 300-LED port mirroring a 150-LED one groups by 2 even here.
    let mut leds = [150_u16; SMARTLED_PORT_COUNT];
    leds[1] = 300;
    let data = menu(LedGrouping::Individual, true, leds);

    let row = FieldId::GroupSize(1);
    assert_eq!(
        row.layout_row_value(&data, &FixtureMap::EMPTY),
        None,
        "under Mirror the row is not decided outright by the grouping"
    );
    assert_eq!(
        row.derived(&data).as_deref(),
        Some("002"),
        "300 LEDs mirroring 150 at size 1 groups by 2, not 1"
    );
}

#[test]
fn groups_shows_the_stored_value_alone() {
    let data = menu(LedGrouping::Groups, false, [150; SMARTLED_PORT_COUNT]);
    let row = FieldId::GroupSize(0);
    assert_eq!(row.layout_row_value(&data, &FixtureMap::EMPTY), None);
    assert_eq!(row.derived(&data), None, "nothing overrides it, so no second column");
}

// -------------------------------------------------------- ports with no LEDs

#[test]
fn a_port_with_no_leds_says_so_instead_of_showing_a_number() {
    let mut leds = [150_u16; SMARTLED_PORT_COUNT];
    leds[2] = 0;
    for mode in [LedGrouping::Individual, LedGrouping::Groups, LedGrouping::Fixtures] {
        let data = menu(mode, false, leds);
        assert_eq!(
            FieldId::GroupSize(2).layout_row_value(&data, &FixtureMap::EMPTY).as_deref(),
            Some("has 0 LEDs"),
            "{mode}: a dead port must not show configuration that cannot take effect"
        );
        let live = FieldId::GroupSize(0).layout_row_value(&data, &FixtureMap::EMPTY);
        assert_ne!(live.as_deref(), Some("has 0 LEDs"), "{mode}: a live port is unaffected");
        match mode {
            LedGrouping::Individual => assert_eq!(live.as_deref(), Some("001")),
            LedGrouping::Groups => assert_eq!(live, None, "the stored size stands alone"),
            LedGrouping::Fixtures => assert_eq!(live.as_deref(), Some("F001"), "its default"),
        }
    }
}

#[test]
fn a_dead_port_cannot_be_edited_but_its_led_count_still_can() {
    // Greying the row you would use to give the port some LEDs would be a trap
    // with no way out.
    let mut leds = [150_u16; SMARTLED_PORT_COUNT];
    leds[2] = 0;
    let data = menu(LedGrouping::Groups, false, leds);

    assert!(!FieldId::GroupSize(2).editable_now(&data));
    assert!(!FieldId::Fixture(2, 0).editable_now(&data));
    assert!(
        FieldId::LedsPerPort(2).editable_now(&data),
        "the way back must stay live"
    );
    assert!(FieldId::GroupSize(0).editable_now(&data), "live ports unaffected");
}

#[test]
fn the_note_fits_the_value_column() {
    let mut leds = [150_u16; SMARTLED_PORT_COUNT];
    leds[0] = 0;
    let data = menu(LedGrouping::Groups, false, leds);
    let text = FieldId::GroupSize(0).layout_row_value(&data, &FixtureMap::EMPTY).unwrap();
    assert!(text.len() <= 17, "{text:?} is {} of the 17 columns", text.len());
}

#[test]
fn creating_an_entry_gives_a_fixture_not_a_gap() {
    // An empty slot is not a gap, so "toggle" produced `G001` and the first
    // thing you could create was the one kind you did not want.
    let mut map = FixtureMap::EMPTY;
    P0.fixture_adjust(&mut map, 600, 0, true);
    assert_eq!(P0.fixture_display(&map), "F001");

    // Both directions create the same thing — there is nothing to toggle yet.
    let mut other = FixtureMap::EMPTY;
    P0.fixture_adjust(&mut other, 600, 0, false);
    assert_eq!(P0.fixture_display(&other), "F001");

    // And once it exists, the toggle works as a toggle.
    P0.fixture_adjust(&mut map, 600, 0, true);
    assert_eq!(P0.fixture_display(&map), "G001");
}

#[test]
fn a_port_with_no_room_cannot_have_an_entry_created() {
    let mut map = FixtureMap::EMPTY;
    P0.fixture_adjust(&mut map, 0, 0, true);
    assert!(map.is_empty(0), "nothing fits on a strip of zero");
}

// ------------------------------------------------------- the channel figure

#[test]
fn the_channel_count_is_what_is_used_not_what_is_spanned() {
    // Ports start on universe boundaries, so eight ports of 90 channels reach
    // into eight universes while using 720. The Main page shows the smaller
    // number — what a console operator patches — and `Universes` shows the
    // larger one. Conflating them is the mistake this pins.
    let s = settings(120, 4, LedGrouping::Groups);
    let layout = s.layout(&FixtureMap::EMPTY);

    assert_eq!(layout.bytes_per_port()[0], 90, "30 items, RGB");
    assert_eq!(layout.total_channels(), 720, "eight ports of 90 actually used");
    assert_eq!(layout.total_universes(), 8, "but eight universes spanned");
}

#[test]
fn mirror_counts_its_shared_block_once() {
    let mut s = settings(120, 4, LedGrouping::Groups);
    s.port_mode = SmartLedPortMode::Mirror;
    assert_eq!(
        s.layout(&FixtureMap::EMPTY).total_channels(),
        90,
        "every port reads the same block, so it is one block of channels"
    );
}

#[test]
fn fixtures_cost_only_the_channels_their_runs_need() {
    // The saving that makes the mode worth having: three fixtures on a 600-LED
    // strip is nine channels, not eighteen hundred.
    let mut s = settings(600, 1, LedGrouping::Fixtures);
    s.leds_per_port = [600, 0, 0, 0, 0, 0, 0, 0];
    let map = worked_example();
    let layout = s.layout(&map);
    assert_eq!(layout.total_channels(), 9, "three lit runs, RGB");
}

// ---------------------------------------------------------------------------
// The strip picture the Layout page and the idle screen draw.

mod strip {
    use common::ui::fixtures::{FixtureEntry, FixtureMap, StripCell};

    #[test]
    fn a_layout_scales_to_the_width_and_keeps_its_proportions() {
        let mut map = FixtureMap::EMPTY;
        map.0[0][0] = FixtureEntry::fixture(24);
        map.0[0][1] = FixtureEntry::gap(8);
        map.0[0][2] = FixtureEntry::fixture(32);
        let cells = map.strip_cells::<16>(0, 64, 64);
        // 24 / 64 of 16 cells is 6, the gap is 2, the second fixture 8.
        assert_eq!(&cells[..6], &[StripCell::Lit(0); 6]);
        assert_eq!(&cells[6..8], &[StripCell::Dark; 2]);
        assert_eq!(&cells[8..], &[StripCell::Lit(1); 8]);
    }

    #[test]
    fn a_shorter_port_draws_shorter_and_the_default_entry_is_one_led() {
        // Nothing stored, so the port renders F001: one lit LED, then dark
        // to its own length, then nothing.
        let map = FixtureMap::EMPTY;
        let cells = map.strip_cells::<16>(0, 32, 64);
        assert_eq!(cells[0], StripCell::Lit(0));
        assert_eq!(&cells[1..8], &[StripCell::Dark; 7]);
        assert_eq!(&cells[8..], &[StripCell::Off; 8]);
    }

    #[test]
    fn a_one_led_fixture_still_shows() {
        let mut map = FixtureMap::EMPTY;
        map.0[0][0] = FixtureEntry::fixture(1);
        map.0[0][1] = FixtureEntry::fixture(599);
        let cells = map.strip_cells::<17>(0, 600, 600);
        assert_eq!(cells[0], StripCell::Lit(0), "a configured fixture cannot be invisible");
        assert_eq!(&cells[1..], &[StripCell::Lit(1); 16]);
    }

    #[test]
    fn a_port_with_no_leds_draws_nothing() {
        let map = FixtureMap::EMPTY;
        assert_eq!(map.strip_cells::<17>(0, 0, 64), [StripCell::Off; 17]);
    }

    #[test]
    fn entries_past_the_strip_are_clipped_not_wrapped() {
        // Over budget: 40 + 40 on a 64-LED port. The second fixture is cut at
        // the strip's end rather than spilling into cells the port does not
        // have.
        let mut map = FixtureMap::EMPTY;
        map.0[0][0] = FixtureEntry::fixture(40);
        map.0[0][1] = FixtureEntry::fixture(40);
        let cells = map.strip_cells::<16>(0, 64, 64);
        assert_eq!(&cells[..10], &[StripCell::Lit(0); 10]);
        assert_eq!(&cells[10..], &[StripCell::Lit(1); 6]);
        assert!(!cells.contains(&StripCell::Off));
    }

    #[test]
    fn a_full_pixel_scale_matches_the_led_count() {
        // The idle screen draws one cell per LED-pixel of bar; at 106 wide a
        // 64-LED port fills 106 and a 32-LED one 53.
        let map = FixtureMap::EMPTY;
        let long = map.strip_cells::<106>(0, 64, 64);
        let short = map.strip_cells::<106>(1, 32, 64);
        assert_eq!(long.iter().filter(|c| **c != StripCell::Off).count(), 106);
        assert_eq!(short.iter().filter(|c| **c != StripCell::Off).count(), 53);
    }
}
