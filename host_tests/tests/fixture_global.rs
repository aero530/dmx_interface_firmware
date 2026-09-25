//! The figures a person reads must come from the map the board is using.
//!
//! **Its own test binary on purpose.** These set the global `FIXTURE_MAP`, and
//! cargo runs the tests inside one binary on threads that share it — a second
//! test here would see whatever this one left behind. Every other fixture test
//! passes a map explicitly for exactly that reason; this file exists because
//! the bug below lives in the path that does *not* take one.
//!
//! The bug: `SmartLedSettings`' convenience accessors were introduced reading
//! `FixtureMap::EMPTY`, so a refactor could land without touching call sites.
//! Once fixtures existed that became silent wrong arithmetic — every caller on
//! a path that must honour the map got the layout of an empty one. Two ports
//! carrying three fixtures between them reported **6** channels instead of 9,
//! and the BindIndex ArtPollReply advertised the wrong universe count to
//! controllers that auto-patch from it.

use common::buffers::set_fixture_map;
use common::ui::fields::FieldId;
use common::ui::fixtures::{FixtureEntry, FixtureMap};
use common::ui::{
    LedGrouping, MenuData, ModuleSettings, SmartLedColorMode, SmartLedDmxGroupSize,
    SmartLedPortMode, SmartLedSettings,
};
use common::SMARTLED_PORT_COUNT;

#[test]
fn the_reported_figures_come_from_the_live_map() {
    // The bench configuration that found this: two ports of 64 pixels, two
    // fixtures on the first and one on the second.
    let mut leds = [0_u16; SMARTLED_PORT_COUNT];
    leds[0] = 64;
    leds[1] = 64;

    let mut map = FixtureMap::EMPTY;
    map.0[0][0] = FixtureEntry::fixture(32);
    map.0[0][1] = FixtureEntry::fixture(32);
    map.0[1][0] = FixtureEntry::fixture(64);
    set_fixture_map(map);

    let settings = SmartLedSettings {
        leds_per_port: leds,
        color_mode: SmartLedColorMode::Rgb,
        port_mode: SmartLedPortMode::Individual,
        grouping: LedGrouping::Fixtures,
        dmx_group_size: SmartLedDmxGroupSize([1; SMARTLED_PORT_COUNT]),
    };

    // Through the convenience accessor — the one the UI and the Art-Net task
    // actually call.
    assert_eq!(
        settings.total_channels(),
        9,
        "three fixtures at RGB. 6 means the empty map is being consulted"
    );
    assert_eq!(settings.virtual_leds_per_port()[0], 2);
    assert_eq!(settings.virtual_leds_per_port()[1], 1);
    assert_eq!(
        settings.virtual_leds_per_port()[2],
        0,
        "a port with no LEDs costs nothing, default entry or not"
    );

    // And through the field the panel renders, which is where it was noticed.
    let mut data = MenuData::default();
    data.module = ModuleSettings::SmartLed(settings);
    assert_eq!(FieldId::TotalChannels.display(&data), "9 / 128 px");

    // The universe count the ArtPollReply advertises has to agree too.
    assert_eq!(settings.bound_universes(), 2, "one universe per live port");
}
