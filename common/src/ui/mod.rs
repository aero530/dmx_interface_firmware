//! Settings model and menu field metadata.
//!
//! Only the target-agnostic half of the UI lives here. The rendering task, the
//! display driver and the button wiring stay in the target crate — this is the
//! data those things operate on, which is why the TFT menu, the USB console
//! protocol and the desktop `dmx_console` app can all be driven from one
//! description and cannot drift apart.

pub mod types;
pub use types::*;

pub mod fields;
pub use fields::{all_fields, FieldId, Page, PAGES};

pub mod fixtures;
pub use fixtures::{FixtureEntry, FixtureMap, MAX_FIXTURES_PER_PORT};
