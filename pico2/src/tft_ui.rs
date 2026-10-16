//! ST7789 172x320 TFT menu.
//!
//! Ratatui drawn through mousefood's embedded-graphics backend straight onto
//! the panel (mipidsi draws directly; there is no framebuffer to flush). The
//! settings model and the field table are shared with the console in `common`.
//!
//! # Wiring, and why the init sequence is ordered the way it is
//!
//! | Signal | Where |
//! |---|---|
//! | SCK / MOSI | SPI1, GP14 / GP11 |
//! | DC | GP12 |
//! | RES | TCA9555 P10 |
//! | CS | TCA9555 P11 — driven low once, stays low (sole device on SPI1) |
//! | BL | PCA9633 LED0 (PWM), I²C 0x62 |
//!
//! Both RES and CS sit behind the I²C expander, which the button task owns and
//! initialises. `init()` here is blocking SPI: if it ran before the expander
//! had released RES and asserted CS, every command would be ignored on the
//! write-only bus and the panel would stay dark forever. Hence the
//! `display_ready` handshake, then the ST7789's 120 ms post-reset settling
//! time, then init — and only then the backlight, so the user never sees the
//! controller's power-on noise.
//!
//! # Grid
//!
//! `FONT_9X15` on 320x172 (landscape) gives **35 columns x 11 rows** — one row
//! for the page title and status, nine for fields (`MAX_FIELDS_PER_PAGE`), and
//! the bottom row for the button hints. Change the font and that budget changes
//! with it; the test in `host_tests` is what keeps the page table honest.
//!
//! The panel's corners are radiused, so `CORNER_INSET` keeps text one column
//! clear of each side and **33 columns are usable**. Filled bars (the title,
//! the selected row, the footer) run edge to edge underneath the corners: a
//! bar that stops a cell short reads as a highlighted word rather than as
//! chrome, and a clipped corner on a solid colour is invisible.
//!
//! # Three surfaces on one panel
//!
//! The menu is the character grid. Two things are drawn *around* it with
//! embedded-graphics primitives on the same display: the boot splash, before
//! the terminal exists, and the idle screen, which borrows the display back
//! through the backend once the menu has been untouched for `IDLE_SECS`. The
//! menu redraws in full on the way back (`Terminal::clear`), so neither leaves
//! anything behind.
//!
//! Pictures *inside* the grid — the strip maps on the Layout page, the universe
//! budget bar, the activity lamp — are cells of a three-glyph font handed to
//! mousefood as the bold face (see `glyphs.rs`). A cell background fills the
//! whole 9x15 cell, so bars drawn as backgrounds would fuse across rows; a
//! glyph can leave a pixel of its cell alone.
//!
//! # Why blocking SPI
//!
//! mipidsi is blocking. Ratatui only redraws cells that changed, so a keypress
//! costs a few hundred bytes on the bus; the one full redraw is the first
//! frame (~110 KB, ~25 ms at 40 MHz). LED and DMX timing is carried by PIO and
//! DMA in hardware, so a stalled poll on this core shifts scheduling rather
//! than output. Worth revisiting only if jitter shows up on the bench.

use alloc::format;
use alloc::string::String;

use common::activity;
use common::buffers::{fixture_map, set_fixture_map};
use common::channels::{RouterChannelTx, UiChannelRx, CHANNEL_EEPROM};
use common::event_router::RouterEvent;
use common::events::{EepromEvent, NetStatus, RebootRequest, UiEvent};
use common::ui::fields::{FieldId, Page, MAX_FIELDS_PER_PAGE, VALUE_COLUMN};
use common::ui::fixtures::{FixtureMap, StripCell};
use common::ui::{backlight_duty, LedGrouping, MenuData, ModuleSettings, DEFAULT_BACKLIGHT, PAGES};
use common::usb_power;
use common::{DISPLAY_HEIGHT, DISPLAY_OFFSET, DISPLAY_WIDTH, DMX_UNIVERSE_COUNT, SMARTLED_PORT_COUNT};
// Logging goes wherever the build says: `defmt` over RTT by default, the
// `log` crate over USB with `--features usb`. Format strings therefore have
// to parse as both - no `{=type}` hints. See `usb_log.rs`.
use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::{error, info};
    } else {
        use defmt::{error, info};
    }
}
use embassy_futures::select::{select, Either};
use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI1;
use embassy_rp::spi::{Blocking, Spi};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_9X15};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_graphics::primitives::{Circle, Primitive, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use embedded_graphics::Drawable;
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::{Builder, Display};
use mousefood::{EmbeddedBackend, EmbeddedBackendConfig, TerminalAlignment};
use ratatui_core::buffer::Buffer;
use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::terminal::Terminal;
use static_cell::StaticCell;

use crate::diag;
use crate::glyphs;
use crate::pca9633::Pca9633;
use crate::pixfont;

/// Character grid the font and panel produce; the field table is checked
/// against `ROWS - 2` in `host_tests`.
pub const COLUMNS: u16 = 35;
pub const ROWS: u16 = 11;

/// Columns given up at each side to the panel's **rounded corners**.
///
/// The glass corners are radiused, so the first and last character cell of the
/// top and bottom rows is clipped — measured at about one character per corner
/// on the 1.47" module (2026-09-15). Only those two rows are actually affected,
/// but the inset applies to every row: the labels all start in one column, and
/// indenting just the last row would read as a bug rather than as a margin.
const CORNER_INSET: u16 = 1;

/// First column text may use, and one past the last.
const CONTENT_LEFT: u16 = CORNER_INSET;
const CONTENT_RIGHT: u16 = COLUMNS - CORNER_INSET;

/// Value text starts here, allowing for the inset.
const VALUE_X: u16 = CONTENT_LEFT + VALUE_COLUMN;

/// The widest value any field renders must still fit inside the inset area.
/// `IpAddrMenu` is the longest at `255.255.255.255`.
const _: () = core::assert!(VALUE_X + 15 <= CONTENT_RIGHT);

/// A computed value needs a second copy of the widest field that has one, plus
/// the gap between them. `GroupSize` is three digits.
const _: () = core::assert!(VALUE_X + 3 + 1 + 3 <= CONTENT_RIGHT);

/// Cells a value may occupy - and the width of a strip map.
const VALUE_WIDTH: usize = (CONTENT_RIGHT - VALUE_X) as usize;

/// The button-hint row. Title, fields, footer: the field table is bounded so
/// that they never meet.
const FOOTER_ROW: u16 = ROWS - 1;
const _: () = core::assert!((MAX_FIELDS_PER_PAGE as u16) < FOOTER_ROW);

/// Columns text may occupy on a row.
const USABLE_COLUMNS: usize = (CONTENT_RIGHT - CONTENT_LEFT) as usize;

/// Cells in the universe budget bar, and how many universes each stands for.
/// 64 universes over 32 cells is two per cell; a single universe still shows
/// as one cell because the count rounds up.
const BUDGET_CELLS: usize = 32;
const UNIVERSES_PER_CELL: usize = DMX_UNIVERSE_COUNT.div_ceil(BUDGET_CELLS);
const _: () = core::assert!(CONTENT_LEFT as usize + BUDGET_CELLS <= CONTENT_RIGHT as usize);

/// How long the boot splash stays, unless a button ends it sooner. The old
/// colour splash was 600 ms; this is what the wordmark needs to be read.
const SPLASH_MS: u64 = 1500;

/// Seconds without a button before the menu gives way to the idle screen.
const IDLE_SECS: u32 = 30;

// ---------------------------------------------------------------------------
// Colours in the grid. Everything the panel shows is one of these, so the
// vocabulary stays small: white is a value you set, grey a label or something
// that does not apply, blue what the board worked out, amber a note.

/// Labels: a step back from the values, so the eye lands on the numbers.
const LABEL_FG: Color = Color::Rgb(0xA0, 0xA0, 0xA0);

/// A stored setting the current mode computes for you - see
/// `FieldId::derived`. Dimmed rather than hidden, because it still applies in
/// the other mode and is still editable here.
const OVERRIDDEN_FG: Color = Color::Rgb(0x70, 0x70, 0x70);

/// The value actually in force, drawn beside the dimmed one. Blue reads as
/// "the board worked this out", and is far enough from white that the pair
/// cannot be mistaken for one number that happens to be spaced oddly.
const IN_FORCE_FG: Color = Color::Rgb(0x40, 0xC0, 0xFF);

/// A value the board is telling *you*, rather than one you set: universe
/// offsets, the address in force, the totals. The same blue as a computed
/// value, for the same reason - both are the board's arithmetic.
const INFO_FG: Color = IN_FORCE_FG;

/// Advisory text. Amber reads as "worth knowing" without the alarm of red -
/// nothing here is a fault.
const NOTE_FG: Color = Color::Rgb(0xFF, 0xC0, 0x40);

/// Title-bar text. The bar's colour is the page's - see `page_accent`.
const HEADER_FG: Color = Color::Rgb(0xE0, 0xE8, 0xF0);

/// The selected row: one dark blue bar, edge to edge, with the text on it in
/// its usual colours. It used to be reverse video, which was also what the
/// title bar was, and two white bars on one screen left no way to tell which
/// one the buttons act on.
const SELECTED_BG: Color = Color::Rgb(0x18, 0x30, 0x50);

/// The digit under the cursor while editing: the one bright block on screen.
const CURSOR_FG: Color = Color::Black;
const CURSOR_BG: Color = Color::Rgb(0x40, 0xC0, 0xFF);

/// The button hints: present, never louder than the content.
const FOOTER_BG: Color = Color::Rgb(0x20, 0x20, 0x20);
const FOOTER_FG: Color = Color::Rgb(0xA0, 0xA0, 0xA0);

/// The activity lamp in the title bar: blue while DMX is arriving, grey when
/// it is not.
const LAMP_ON: Color = Color::Rgb(0x40, 0xC0, 0xFF);
const LAMP_OFF: Color = Color::Rgb(0x60, 0x60, 0x60);

/// Strip maps: fixtures alternate between these two so neighbours separate,
/// LEDs nothing drives are dark.
const STRIP_FG: [Color; 2] = [Color::Rgb(0x40, 0xC0, 0xFF), Color::Rgb(0x40, 0xE0, 0x80)];
const STRIP_DARK: Color = Color::Rgb(0x38, 0x38, 0x38);

/// The universe budget bar: what is free, and the colour it turns when the
/// configuration wants more universes than there are.
const BUDGET_FREE: Color = Color::Rgb(0x2A, 0x2A, 0x2A);
const BUDGET_OVER: Color = Color::Rgb(0xFF, 0x40, 0x40);

/// The title bar's colour, per page, so the page is known before its name is
/// read. Keyed by the page's first field rather than its title: titles are
/// display text and have been renamed before. The fixture tables take
/// Layout's colour because that is where they belong.
fn page_accent(page: &Page) -> Color {
    match page.fields.first() {
        Some(FieldId::IpMode) => Color::Rgb(0x0F, 0x5C, 0x5A),
        Some(FieldId::PortMode) => Color::Rgb(0x2A, 0x6A, 0x2A),
        Some(FieldId::LedsPerPort(_)) => Color::Rgb(0x7A, 0x52, 0x10),
        Some(FieldId::Grouping | FieldId::GroupSize(_) | FieldId::Fixture(..)) => {
            Color::Rgb(0x4C, 0x2A, 0x80)
        }
        Some(FieldId::EthernetEnabled) => Color::Rgb(0x4A, 0x4A, 0x55),
        _ => Color::Rgb(0x1C, 0x3A, 0x66),
    }
}

/// A cell of the picture font (`glyphs.rs`): bold selects that face.
fn picture(fg: Color, bg: Color) -> Style {
    Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
}

// ---------------------------------------------------------------------------
// Colours outside the grid: the splash and the idle screen draw in Rgb565
// directly. Same values as above where they mean the same thing.

const fn rgb(r: u8, g: u8, b: u8) -> Rgb565 {
    Rgb565::new(r >> 3, g >> 2, b >> 3)
}

const PX_CAPTION: Rgb565 = rgb(0x84, 0x84, 0x84);
const PX_INFO: Rgb565 = rgb(0x40, 0xC0, 0xFF);
const PX_NOTE: Rgb565 = rgb(0xFF, 0xC0, 0x40);
const PX_LAMP_ON: Rgb565 = rgb(0x40, 0xC0, 0xFF);
const PX_LAMP_OFF: Rgb565 = rgb(0x60, 0x60, 0x60);
const PX_STRIP: [Rgb565; 2] = [rgb(0x40, 0xC0, 0xFF), rgb(0x40, 0xE0, 0x80)];
const PX_STRIP_DARK: Rgb565 = rgb(0x38, 0x38, 0x38);
const PX_STUB: Rgb565 = rgb(0x30, 0x30, 0x30);
/// Lit segments in the menu's label grey, not white: the address is the
/// biggest thing on the panel and at full white it shouts from across the
/// room, which is the opposite of what an idle screen is for.
const PX_DIGIT_ON: Rgb565 = rgb(0xC0, 0xC0, 0xC0);
/// Unlit segments, just visible, the way a real seven-segment display ghosts.
const PX_DIGIT_GHOST: Rgb565 = rgb(0x18, 0x18, 0x18);

/// The six-stripe flag under the wordmark: red, orange, yellow, green, blue,
/// violet.
const PRIDE: [Rgb565; 6] = [
    rgb(0xE4, 0x03, 0x03),
    rgb(0xFF, 0x8C, 0x00),
    rgb(0xFF, 0xED, 0x00),
    rgb(0x00, 0x80, 0x26),
    rgb(0x00, 0x4D, 0xFF),
    rgb(0x73, 0x29, 0x82),
];

/// The panel in landscape, in pixels. `DISPLAY_WIDTH`/`HEIGHT` are the
/// controller's native portrait geometry.
const PANEL_W: i32 = DISPLAY_HEIGHT as i32;
const PANEL_H: i32 = DISPLAY_WIDTH as i32;

/// Pixel margin matching the grid's corner inset, and the row the bottom line
/// of text sits on for both the splash and the idle screen.
const PX_MARGIN: i32 = 9;
const PX_BOTTOM_LINE: i32 = PANEL_H - 22;

/// Shown under `LED Power` while the module's own USB port has VBUS.
///
/// That port feeds `V_LED` through D3 and F18 whatever `LED Power` says, so the
/// strips can be lit while the setting reads `External` - and on a finished
/// unit that port is exposed, so it is an operator-facing surprise rather than
/// a bench-only one. The 0.5 A is worth the space because it is the diagnostic:
/// strips that *pulse* rather than dim are F18 cycling, not a firmware fault.
///
/// Only drawn when something is plugged in. Permanent text would be noise on a
/// page whose whole job is three settings, and the point is to explain lit
/// strips at the moment they are lit.
///
/// **Every line has to stand on its own.** The first attempt read "Module USB
/// also powers LEDs / whatever this is set to (0.5A)", where the second line
/// was a dangling clause that meant nothing without the first and left the
/// number with no stated referent. Read as three separate statements - what is
/// happening, what the control can do about it, and what to expect - it needs
/// no reassembly by the reader.
const USB_NOTE: [&str; 3] = [
    "Module USB also powers LEDs",
    "This setting cannot stop it",
    "0.5 A fuse - may flicker",
];
const _: () = core::assert!(USB_NOTE[0].len() <= USABLE_COLUMNS);
const _: () = core::assert!(USB_NOTE[1].len() <= USABLE_COLUMNS);
const _: () = core::assert!(USB_NOTE[2].len() <= USABLE_COLUMNS);

/// The footer while editing; the navigating one is built per row.
const FOOTER_EDIT: &str = "Up Dn change Sel next  Esc cancel";
const _: () = core::assert!(FOOTER_EDIT.len() <= USABLE_COLUMNS);

/// The footer on an action row, and once the action is armed.
const FOOTER_ACTION: &str = "Up Dn move   Sel update  Esc page";
const FOOTER_CONFIRM: &str = "Sel reboot   Esc cancel";
const _: () = core::assert!(FOOTER_ACTION.len() <= USABLE_COLUMNS);
const _: () = core::assert!(FOOTER_CONFIRM.len() <= USABLE_COLUMNS);

/// An armed action that is not confirmed within this many seconds disarms
/// itself - long enough to read the footer, short enough that a row left
/// armed by mistake is not a trap for the next person at the panel.
const CONFIRM_SECS: u32 = 10;

/// What the `Firmware` row shows when it is not armed. The target crate's
/// version, which `common` cannot see, so it is drawn here rather than by
/// `FieldId::display`.
const FW_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Chip select is a static line on the expander, so the SPI bus abstraction
/// gets a pin that does nothing.
///
/// Verified on the bench 2026-09-15: this panel works with CS held statically
/// low, so it does not need per-transaction framing and CS can stay on the
/// I²C expander.
pub struct NoCs;

/// A reset pin that does nothing — the *real* RES is on the expander and the
/// button task has already pulsed it before this task runs.
///
/// This exists to steer mipidsi, not to drive hardware. Its builder sends
/// `SWRESET` **only** when no reset pin is supplied; given one, it toggles the
/// pin instead and issues no software reset. On this ST7789P3 panel the
/// combination Rev 2 used — a hardware RES pulse from the expander *followed*
/// by mipidsi's `SWRESET` — leaves the controller unresponsive: a textbook init
/// and a full frame of correct pixels go out on the wire (confirmed with a
/// logic capture on the panel's own connector) and nothing reaches the glass.
/// Handing over this no-op pin reproduces the Rev 1 path, which works.
pub struct NoReset;

impl embedded_hal::digital::ErrorType for NoReset {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::OutputPin for NoReset {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl embedded_hal::digital::ErrorType for NoCs {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::OutputPin for NoCs {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

type SpiDev = ExclusiveDevice<Spi<'static, SPI1, Blocking>, NoCs, embedded_hal_bus::spi::NoDelay>;
type Di = SpiInterface<'static, SpiDev, Output<'static>>;
type Tft = Display<Di, ST7789, NoReset>;

/// mipidsi's SPI interface batches pixel data through this before each
/// transfer; 512 B keeps a row of text in one transaction.
static DI_BUFFER: StaticCell<[u8; 512]> = StaticCell::new();

/// Whether the menu is navigating or mid-edit.
///
/// Editing is a **copy-on-edit transaction**: adjustments land on a private
/// draft, and only a completed edit is sent to the router, which merges that one
/// field into the live settings. Esc discards the draft, so a half-finished edit
/// cannot leak into stored settings.
enum Mode {
    Navigate,
    /// An action row is armed: the next Select carries it out, anything else
    /// disarms it, and so does `CONFIRM_SECS` of silence. Entered only from
    /// `Navigate` by Select on an action row, and buttons report releases
    /// only, so one held press cannot arm *and* fire.
    Confirm,
    Edit { digit: u8, draft: MenuData },
    /// Editing a fixture row.
    ///
    /// A separate variant with its own draft because the fixture map is not in
    /// `MenuData` — but the transaction is the same one: adjustments land on
    /// this copy, Esc throws it away, and only a completed edit reaches the
    /// store. Sharing `Edit` would have meant either widening its draft for
    /// every field or giving fixtures no way to cancel.
    EditFixture { digit: u8, draft: FixtureMap },
}

/// Menu cursor: which page, and which field within it.
struct Cursor {
    page: usize,
    field: usize,
}

impl Cursor {
    /// Move by one, rolling onto the next or previous **ring** page at the
    /// edges.
    ///
    /// The eight fixture tables are `Detail` pages and are skipped: they are in
    /// `PAGES` so the console can address their rows, but walking into them
    /// from the ring would mean eight tables between `System` and `Main`, and
    /// eight of them even in a mode that has no fixtures. They are reached by
    /// selecting a port on the Layout page instead.
    fn step(&mut self, down: bool) {
        let len = PAGES[self.page].fields.len();
        if down {
            if self.field + 1 < len {
                self.field += 1;
            } else {
                self.page = Self::next_ring(self.page, true);
                self.field = 0;
            }
        } else if self.field > 0 {
            self.field -= 1;
        } else {
            self.page = Self::next_ring(self.page, false);
            self.field = PAGES[self.page].fields.len().saturating_sub(1);
        }
    }

    /// The next page in the ring, in either direction.
    ///
    /// Bounded by `PAGES.len()` rather than looping until it finds one: if every
    /// page were somehow `Detail` this would spin forever, and a menu that hangs
    /// is worse than one that shows the wrong page.
    fn next_ring(from: usize, forward: bool) -> usize {
        let n = PAGES.len();
        for step in 1..=n {
            let candidate = if forward {
                (from + step) % n
            } else {
                (from + n - step) % n
            };
            if PAGES[candidate].in_ring() {
                return candidate;
            }
        }
        from
    }
}

/// Right-hand end of the title row.
fn net_text(status: NetStatus) -> String {
    match status {
        NetStatus::Off => String::from("ETH off"),
        NetStatus::Guard => String::from("ETH guard"),
        NetStatus::NoChip => String::from("ETH no chip"),
        NetStatus::Dhcp => String::from("ETH dhcp..."),
        NetStatus::Up(ip) => format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
    }
}

/// Title-row power note: empty unless the strips are running off the brick.
///
/// When the budget is scaling the frame the percentage goes with it, because
/// that is the state an operator would otherwise misread. `PWR usb` alone says
/// where the power comes from; `PWR usb 34%` says why the strips look dim, and
/// that it is the firmware protecting the switch rather than a fault. At full
/// output the percentage is left off - there is nothing to explain.
fn usb_power_text() -> String {
    let s = usb_power::status();
    if usb_power::brick_selected() && (s & usb_power::ST_FAULT) != 0 {
        String::from("PWR flt")
    } else if usb_power::brick_powering() {
        let pct = usb_power::scale_percent();
        if pct >= 100 {
            String::from("PWR usb")
        } else {
            format!("PWR usb {}%", pct)
        }
    } else {
        String::new()
    }
}

/// The button hints for the bottom row.
///
/// Built per row so `Sel` says what it will actually do here: `edit` a value,
/// `open` a port's fixture table, or nothing on a row that has nothing to do.
/// `Esc` is `page` in the ring and `back` inside a fixture table, because that
/// is what it does there.
fn footer_text(mode: &Mode, page: &Page, field: FieldId, data: &MenuData) -> String {
    match mode {
        Mode::Navigate if field.is_action() => String::from(FOOTER_ACTION),
        Mode::Confirm => String::from(FOOTER_CONFIRM),
        Mode::Navigate => {
            let sel = if field.port_unused(data) {
                "-   "
            } else if matches!(field, FieldId::GroupSize(_)) && grouping(data) == LedGrouping::Fixtures {
                "open"
            } else if field.fixture_slot().is_some() || field.editable_now(data) {
                "edit"
            } else {
                "-   "
            };
            let esc = if page.in_ring() { "page" } else { "back" };
            format!("Up Dn move   Sel {sel}  Esc {esc}")
        }
        _ => String::from(FOOTER_EDIT),
    }
}

/// The port a Layout row draws as a strip map, if it is one: a port row under
/// `Fixtures` with LEDs on it. Everything else stays text.
fn strip_port(field: FieldId, data: &MenuData) -> Option<usize> {
    match field {
        FieldId::GroupSize(port)
            if grouping(data) == LedGrouping::Fixtures && !field.port_unused(data) =>
        {
            Some(port)
        }
        _ => None,
    }
}

/// LEDs on the longest port: the scale every strip map on a page shares.
fn longest_port(data: &MenuData) -> u16 {
    let ModuleSettings::SmartLed(s) = data.module;
    s.leds_per_port.iter().copied().max().unwrap_or(0)
}

/// LEDs configured on `port`.
fn leds_on(data: &MenuData, port: usize) -> u16 {
    let ModuleSettings::SmartLed(s) = data.module;
    s.leds_per_port[port.min(s.leds_per_port.len() - 1)]
}

/// A strip map in the value column: the port's fixtures to scale, one glyph
/// per cell, the last cell of every run drawn a pixel short so runs separate.
fn draw_strip(buf: &mut Buffer, y: u16, map: &FixtureMap, port: usize, leds: u16, longest: u16) {
    let cells = map.strip_cells::<VALUE_WIDTH>(port, leds, longest);
    for (i, cell) in cells.iter().enumerate() {
        let colour = match cell {
            StripCell::Off => continue,
            StripCell::Dark => STRIP_DARK,
            StripCell::Lit(n) => STRIP_FG[usize::from(*n) % STRIP_FG.len()],
        };
        let run_ends = cells.get(i + 1).is_none_or(|next| next != cell);
        let glyph = if run_ends { glyphs::BLOCK_END } else { glyphs::BLOCK };
        buf.set_string(VALUE_X + i as u16, y, glyph, picture(colour, Color::Reset));
    }
}

/// The universe budget as a bar: used cells in blue, free ones dark, all of
/// them red when the configuration asks for more universes than exist. Each
/// cell is drawn apart so they can be counted.
fn draw_budget(buf: &mut Buffer, y: u16, data: &MenuData) {
    let ModuleSettings::SmartLed(s) = data.module;
    let total = s.total_universes() as usize;
    let over = s.over_budget();
    let used = total.div_ceil(UNIVERSES_PER_CELL).min(BUDGET_CELLS);
    for i in 0..BUDGET_CELLS {
        let colour = if i >= used {
            BUDGET_FREE
        } else if over {
            BUDGET_OVER
        } else {
            STRIP_FG[0]
        };
        buf.set_string(CONTENT_LEFT + i as u16, y, glyphs::BLOCK_END, picture(colour, Color::Reset));
    }
}

/// The bottom row: a dark bar, edge to edge, with the hints on it.
fn draw_footer(buf: &mut Buffer, text: &str) {
    let style = Style::new().fg(FOOTER_FG).bg(FOOTER_BG);
    for x in 0..COLUMNS {
        buf.set_string(x, FOOTER_ROW, " ", style);
    }
    buf.set_string(CONTENT_LEFT, FOOTER_ROW, text, style);
}

// ---------------------------------------------------------------------------
// Drawn directly, outside the grid.

/// One line of `FONT_9X15`, top-left at (`x`, `y`).
fn text(display: &mut Tft, s: &str, x: i32, y: i32, colour: Rgb565) {
    let _ = Text::with_baseline(s, Point::new(x, y), MonoTextStyle::new(&FONT_9X15, colour), Baseline::Top)
        .draw(display);
}

/// One line of `FONT_9X15`, ending at the right margin.
fn text_right(display: &mut Tft, s: &str, y: i32, colour: Rgb565) {
    let x = PANEL_W - PX_MARGIN - (s.len() as i32) * 9;
    text(display, s, x, y, colour);
}

fn fill_rect(display: &mut Tft, x: i32, y: i32, w: u32, h: u32, colour: Rgb565) {
    let _ = Rectangle::new(Point::new(x, y), Size::new(w, h))
        .into_styled(PrimitiveStyle::with_fill(colour))
        .draw(display);
}

/// The boot splash: wordmark, the flag, the firmware version, and the network
/// status where the menu will show it.
fn draw_splash(display: &mut Tft, net: NetStatus) {
    let _ = display.clear(Rgb565::BLACK);
    let wordmark = "DMX";
    let w = pixfont::text_width(wordmark.len() as u32, 6) as i32;
    let _ = pixfont::draw_text(display, wordmark, (PANEL_W - w) / 2, 30, 6, Rgb565::WHITE);
    let subtitle = "INTERFACE";
    let w = pixfont::text_width(subtitle.len() as u32, 2) as i32;
    let _ = pixfont::draw_text(display, subtitle, (PANEL_W - w) / 2, 80, 2, PX_INFO);
    let stripe = 42;
    let x0 = (PANEL_W - stripe * PRIDE.len() as i32) / 2;
    for (i, colour) in PRIDE.iter().enumerate() {
        fill_rect(display, x0 + stripe * i as i32, 104, stripe as u32, 8, *colour);
    }
    let version = format!("Rev 2   fw {}", env!("CARGO_PKG_VERSION"));
    text(display, &version, PX_MARGIN, PX_BOTTOM_LINE, PX_CAPTION);
    draw_splash_status(display, net);
}

/// The last thing the panel shows before a software reboot.
///
/// For a firmware update it stays up for as long as the module is a USB drive:
/// the bootrom reboot resets the RP2350, not the carrier's I²C parts that hold
/// the panel's reset and backlight, so what is drawn here is what the operator
/// reads while copying the file.
fn draw_update_screen(display: &mut Tft, req: RebootRequest) {
    let _ = display.clear(Rgb565::BLACK);
    match req {
        RebootRequest::Bootsel { .. } => {
            text(display, "Firmware update", PX_MARGIN, 30, Rgb565::WHITE);
            text(display, "USB drive mode", PX_MARGIN, 60, PX_INFO);
            text(display, "Copy pico2.uf2 to the drive", PX_MARGIN, 90, PX_CAPTION);
            text(display, "Power-cycle to cancel", PX_MARGIN, PX_BOTTOM_LINE, PX_NOTE);
        }
        RebootRequest::Flash => text(display, "Rebooting...", PX_MARGIN, 60, PX_CAPTION),
    }
}

/// The splash's status line, redrawn on its own as the network comes up.
fn draw_splash_status(display: &mut Tft, net: NetStatus) {
    // Clear the right half of the line first: a shorter status must not leave
    // the tail of a longer one behind it.
    fill_rect(display, PANEL_W / 2, PX_BOTTOM_LINE, (PANEL_W / 2) as u32, 15, Rgb565::BLACK);
    let colour = if matches!(net, NetStatus::Up(_)) { PX_INFO } else { PX_NOTE };
    text_right(display, &net_text(net), PX_BOTTOM_LINE, colour);
}

/// Seven-segment digits from rectangles. Segment bits a..g are 0..6; a digit
/// is 36x60 with 6-pixel strokes.
const SEGMENTS: [u8; 10] = [0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F];
const SEGMENT_RECTS: [(i32, i32, u32, u32); 7] = [
    (6, 0, 24, 6),   // a
    (30, 3, 6, 27),  // b
    (30, 30, 6, 27), // c
    (6, 54, 24, 6),  // d
    (0, 30, 6, 27),  // e
    (0, 3, 6, 27),   // f
    (6, 27, 24, 6),  // g
];
const DIGIT_W: i32 = 36;
const DIGIT_GAP: i32 = 10;

fn draw_digit(display: &mut Tft, x: i32, y: i32, digit: u8) {
    let mask = SEGMENTS[usize::from(digit.min(9))];
    for (i, (sx, sy, w, h)) in SEGMENT_RECTS.iter().enumerate() {
        let on = mask & (1 << i) != 0;
        fill_rect(display, x + sx, y + sy, *w, *h, if on { PX_DIGIT_ON } else { PX_DIGIT_GHOST });
    }
}

/// Where the idle screen's port bars sit and how long the longest one is.
const BAR_X: i32 = 200;
const BAR_PX: usize = 106;
const BAR_H: u32 = 9;
const BAR_PITCH: i32 = 12;
const BAR_TOP: i32 = 44;

/// The idle screen: readable from across the room. The DMX address in
/// seven-segment digits, a bar per port drawn to scale with its fixtures, the
/// input and where it is bound, the IP, the power note - and the same activity
/// lamp the title bar has.
///
/// `full` redraws everything from a cleared panel; otherwise only the lamp is
/// touched, which is all that changes from one second to the next.
fn draw_home(display: &mut Tft, data: &MenuData, net: NetStatus, lamp: bool, full: bool) {
    if full {
        let _ = display.clear(Rgb565::BLACK);
        let mode = format!("{}  {}", data.input_mode, FieldId::UniversesBound.display(data));
        text(display, &mode, 21, 4, PX_CAPTION);
        text_right(display, &net_text(net), 4, Rgb565::WHITE);
        text(display, "DMX ADDRESS", 18, 28, PX_CAPTION);

        let address = data.dmx_address;
        let digits = [(address / 100) % 10, (address / 10) % 10, address % 10];
        for (i, d) in digits.iter().enumerate() {
            draw_digit(display, 18 + (DIGIT_W + DIGIT_GAP) * i as i32, 46, *d as u8);
        }

        let ModuleSettings::SmartLed(s) = data.module;
        let longest = longest_port(data);
        let map = fixture_map();
        let small = MonoTextStyle::new(&FONT_6X10, PX_CAPTION);
        for port in 0..SMARTLED_PORT_COUNT {
            let y = BAR_TOP + BAR_PITCH * port as i32;
            let label = format!("{}", port + 1);
            let _ = Text::with_baseline(&label, Point::new(BAR_X - 16, y), small, Baseline::Top).draw(display);
            let leds = s.leds_per_port[port];
            if leds == 0 {
                fill_rect(display, BAR_X, y, 2, BAR_H, PX_STUB);
                continue;
            }
            if s.grouping == LedGrouping::Fixtures {
                // One cell per pixel of bar, then runs of equal cells become
                // rectangles, each a pixel short so neighbours separate.
                let cells = map.strip_cells::<BAR_PX>(port, leds, longest);
                let mut start = 0;
                while start < BAR_PX {
                    let cell = cells[start];
                    let mut end = start + 1;
                    while end < BAR_PX && cells[end] == cell {
                        end += 1;
                    }
                    let colour = match cell {
                        StripCell::Off => None,
                        StripCell::Dark => Some(PX_STRIP_DARK),
                        StripCell::Lit(n) => Some(PX_STRIP[usize::from(n) % PX_STRIP.len()]),
                    };
                    if let Some(colour) = colour {
                        let w = (end - start).max(2) as u32 - 1;
                        fill_rect(display, BAR_X + start as i32, y, w, BAR_H, colour);
                    }
                    start = end;
                }
            } else {
                let w = (u32::from(leds) * BAR_PX as u32 / u32::from(longest.max(1))).max(2);
                fill_rect(display, BAR_X, y, w, BAR_H, PX_STRIP[0]);
            }
        }

        text(display, &FieldId::TotalChannels.display(data), PX_MARGIN, PX_BOTTOM_LINE, PX_CAPTION);
        let power = usb_power_text();
        if !power.is_empty() {
            text_right(display, &power, PX_BOTTOM_LINE, PX_NOTE);
        }
    }
    let _ = Circle::new(Point::new(PX_MARGIN, 8), 8)
        .into_styled(PrimitiveStyle::with_fill(if lamp { PX_LAMP_ON } else { PX_LAMP_OFF }))
        .draw(display);
}

/// Bring the panel up, show the splash, then draw the menu and react to
/// button events.
#[embassy_executor::task]
pub async fn ui_task(
    spi: Spi<'static, SPI1, Blocking>,
    dc: Output<'static>,
    mut backlight: Pca9633<crate::I2cDev>,
    rx: UiChannelRx,
    tx: RouterChannelTx,
    display_ready: &'static Signal<CriticalSectionRawMutex, ()>,
) {
    // RES released and CS asserted by the button task over I²C — see the
    // module docs for why init must wait for it.
    display_ready.wait().await;
    diag::set(diag::TFT_RES_RELEASED);
    // ST7789: 120 ms after reset before the first command is honoured.
    Timer::after_millis(120).await;

    // Bare-metal panel test, before anything else touches the bus. See
    // `panel_test.rs` for why this exists and how to read the result.
    let (spi, dc) = if crate::panel_test::ENABLED {
        let mut spi = spi;
        let mut dc = dc;
        info!("panel test: full ST7789 init, filling frame RAM red then green");
        crate::panel_test::raw_init_and_fill(&mut spi, &mut dc, 0xF800).await; // red
        Timer::after_millis(1500).await;
        crate::panel_test::raw_init_and_fill(&mut spi, &mut dc, 0x07E0).await; // green
        Timer::after_millis(1500).await;
        diag::set(diag::PANEL_TEST_DONE);
        (spi, dc)
    } else {
        (spi, dc)
    };

    let dev = ExclusiveDevice::new_no_delay(spi, NoCs).expect("TFT chip select");
    let di = SpiInterface::new(dev, dc, DI_BUFFER.init([0; 512]));
    let mut display: Tft = match Builder::new(ST7789, di)
        // Suppresses mipidsi's SWRESET — see `NoReset`. The expander has
        // already pulsed the real RES line.
        .reset_pin(NoReset)
        // Native portrait geometry; the controller's 240-wide frame memory is
        // centred on the 172-pixel glass, hence the offset.
        .display_size(DISPLAY_WIDTH, DISPLAY_HEIGHT)
        .display_offset(DISPLAY_OFFSET, 0)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .invert_colors(ColorInversion::Inverted)
        .init(&mut Delay)
    {
        Ok(d) => {
            diag::set(diag::TFT_INIT_OK);
            d
        }
        Err(_) => {
            diag::set(diag::TFT_INIT_FAIL);
            error!("TFT: init failed - no display");
            return;
        }
    };

    let mut data = MenuData::default();
    let mut brightness = DEFAULT_BACKLIGHT;
    let mut net = NetStatus::Off;

    // The splash, drawn before the backlight so the first thing visible is
    // the wordmark and not the panel filling in. It stays `SPLASH_MS` or until
    // a button, and meanwhile keeps up with the network status and the saved
    // settings: a boot-time `Load` carries the backlight level, and dropping
    // it here would leave the menu at the default brightness.
    //
    // This replaced the red/green/blue bring-up splash. That answered three
    // questions from across the bench - does the panel hear SPI, is
    // `ColorInversion` right, is a blank screen the panel or the draw path -
    // and this one answers the same three: no wordmark, wrong colours in the
    // flag, or a wordmark but no menu. `panel_test::ENABLED` still does the
    // bare-metal version if a future panel needs it.
    draw_splash(&mut display, net);
    if backlight.init(backlight_duty(DEFAULT_BACKLIGHT)).await.is_ok() {
        diag::set(diag::BACKLIGHT_OK);
    } else {
        diag::set(diag::BACKLIGHT_FAIL);
        error!("PCA9633: init failed - backlight off");
    }
    let deadline = Instant::now() + Duration::from_millis(SPLASH_MS);
    loop {
        match select(rx.receive(), Timer::at(deadline)).await {
            Either::First(UiEvent::Load(new_data)) => {
                data = new_data;
                if data.backlight != brightness {
                    brightness = data.backlight;
                    if backlight.set_brightness(backlight_duty(brightness)).await.is_err() {
                        error!("PCA9633: brightness write failed");
                    }
                }
            }
            Either::First(UiEvent::Net(status)) => {
                net = status;
                draw_splash_status(&mut display, net);
            }
            // picotool can ask for an update inside the splash; show it rather
            // than letting the arm below swallow it.
            Either::First(UiEvent::Rebooting(req)) => {
                draw_update_screen(&mut display, req);
                core::future::pending::<()>().await;
            }
            // A button, or time: on to the menu.
            Either::First(_) | Either::Second(()) => break,
        }
    }
    let _ = display.clear(Rgb565::BLACK);
    diag::set(diag::TFT_SPLASH_DONE);
    info!("TFT: up, {}x{} grid", COLUMNS, ROWS);

    let config = EmbeddedBackendConfig {
        // Direct-draw display: nothing to flush.
        flush_callback: alloc::boxed::Box::new(|_d: &mut Tft| {}),
        font_regular: FONT_9X15,
        // Not bold text: the bar and lamp glyphs. See `glyphs.rs`.
        font_bold: Some(glyphs::FONT),
        // 35x9 px = 315 of 320 and 11x15 px = 165 of 172, so the grid leaves 5
        // columns and 7 rows of pixels spare. Centring spreads them evenly
        // instead of banking the whole margin at the right and bottom edges,
        // which buys a couple of pixels of clearance from the rounded corners.
        horizontal_alignment: TerminalAlignment::Center,
        vertical_alignment: TerminalAlignment::Center,
        ..Default::default()
    };
    let backend = EmbeddedBackend::<Tft, Rgb565>::new(&mut display, config);
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => {
            diag::set(diag::TERMINAL_OK);
            t
        }
        Err(_) => {
            diag::set(diag::TERMINAL_FAIL);
            error!("TFT: terminal init failed");
            return;
        }
    };

    let mut cursor = Cursor { page: 0, field: 0 };
    let mut mode = Mode::Navigate;
    // The activity lamp, refreshed once a second from the router's mark.
    let mut lamp = false;
    // Seconds since the last button; the idle screen takes over at `IDLE_SECS`.
    let mut idle_secs: u32 = 0;
    // Whether the idle screen is showing, and whether it needs drawing in full
    // (on entry, and whenever the settings or the network change under it).
    let mut home = false;
    let mut home_full = false;
    loop {
        if home {
            draw_home(terminal.backend_mut().display_mut(), &data, net, lamp, home_full);
            home_full = false;
        } else {
            let page = &PAGES[cursor.page];
            let status = net_text(net);
            let power = usb_power_text();
            let footer = footer_text(&mode, page, page.fields[cursor.field], &data);
            // One snapshot for the whole frame, so a row cannot show a value
            // from one map and a strip from another.
            let live_fixtures = fixture_map();
            let longest = longest_port(&data);
            let drawn = terminal.draw(|frame| {
                let buf = frame.buffer_mut();
                // Title row: page name left, activity lamp and network status
                // right, in the page's colour, edge to edge.
                let accent = page_accent(page);
                let header = Style::new().fg(HEADER_FG).bg(accent);
                for x in 0..COLUMNS {
                    buf.set_string(x, 0, " ", header);
                }
                buf.set_string(CONTENT_LEFT, 0, page.title, header);
                let status_x = CONTENT_RIGHT.saturating_sub(status.len() as u16);
                buf.set_string(status_x, 0, &status, header);
                let lamp_x = status_x.saturating_sub(2);
                buf.set_string(lamp_x, 0, glyphs::DISC, picture(if lamp { LAMP_ON } else { LAMP_OFF }, accent));
                // Between the page name and the lamp: only shown when the
                // strips are actually running off the USB brick, so an
                // operator can tell at a glance why the output is dimmed.
                if !power.is_empty() {
                    let px = CONTENT_LEFT + (page.title.len() as u16) + 1;
                    if px + (power.len() as u16) < lamp_x {
                        buf.set_string(px, 0, &power, header);
                    }
                }

                for (row, field) in page.fields.iter().enumerate() {
                    let selected = row == cursor.field;
                    let y = row as u16 + 1;
                    // A port with no LEDs is greyed **including its label**, so
                    // the row reads as one dead thing rather than a live name
                    // with an odd value next to it.
                    let unused = field.port_unused(&data);
                    let strip = strip_port(*field, &data);
                    // The selection bar. A strip map keeps its own black
                    // ground, so on those rows the bar stops at the label: a
                    // bar under the strip would show through its gaps and box
                    // it in.
                    if selected {
                        let end = if strip.is_some() { VALUE_X } else { COLUMNS };
                        for x in 0..end {
                            buf.set_string(x, y, " ", Style::new().bg(SELECTED_BG));
                        }
                    }
                    let base = Style::new().bg(if selected { SELECTED_BG } else { Color::Reset });
                    let label_style = base.fg(if unused { OVERRIDDEN_FG } else { LABEL_FG });
                    // Label left, value from VALUE_COLUMN; labels are kept
                    // shorter than that in the field table.
                    buf.set_string(CONTENT_LEFT, y, field.label(), label_style);

                    if let Some(port) = strip {
                        draw_strip(buf, y, &live_fixtures, port, leds_on(&data, port), longest);
                        continue;
                    }

                    // Fixture rows read a different store, and while one is
                    // being edited the row must show the draft rather than the
                    // committed map - same contract as a numeric edit.
                    let fixture_view = field.fixture_slot().map(|_| match &mode {
                        Mode::EditFixture { draft, .. } if selected => *draft,
                        _ => live_fixtures,
                    });

                    // A row the mode decides outright shows ONE value: the
                    // pinned 1 under `Individual`, or `has 0 LEDs`. The
                    // grey/blue pair is kept for `Mirror`, where both numbers
                    // say something. See `layout_row_value`.
                    let summary = field.layout_row_value(&data, &live_fixtures);

                    // Fields the current mode computes rather than reads.
                    let derived = match fixture_view {
                        // The computed span, in the same blue column the Mirror
                        // work put computed values in.
                        Some(map) => field.fixture_span(&map),
                        None => field.derived(&data),
                    };

                    match &mode {
                        // An action row: the running version, or the armed
                        // prompt in amber once Select has been pressed on it.
                        _ if field.is_action() => {
                            let armed = selected && matches!(mode, Mode::Confirm);
                            if armed {
                                buf.set_string(VALUE_X, y, "Sel again", base.fg(NOTE_FG));
                            } else {
                                buf.set_string(VALUE_X, y, format!("fw {FW_VERSION}"), base.fg(INFO_FG));
                            }
                        }
                        // Show the draft, not the live value, and mark the digit
                        // being changed so it is obvious which one Up/Down moves.
                        Mode::Edit { digit, draft } if selected => {
                            let text = field.display(draft);
                            buf.set_string(VALUE_X, y, &text, base);
                            // Dotted quads have dots between the digit
                            // positions; the field says where its digit is.
                            let col = field.cursor_column(*digit) as usize;
                            if let Some(ch) = text.chars().nth(col) {
                                let mut one = String::new();
                                one.push(ch);
                                buf.set_string(
                                    VALUE_X + col as u16,
                                    y,
                                    &one,
                                    Style::new().fg(CURSOR_FG).bg(CURSOR_BG),
                                );
                            }
                        }
                        Mode::EditFixture { digit, draft } if selected => {
                            let text = field.fixture_display(draft);
                            buf.set_string(VALUE_X, y, &text, base);
                            // Digit 0 is the Fixture/Gap kind, so the cursor
                            // lands on `F`/`G` rather than a numeral - see
                            // `FieldId::digit_is_numeric`.
                            let col = field.cursor_column(*digit) as usize;
                            if let Some(ch) = text.chars().nth(col) {
                                let mut one = String::new();
                                one.push(ch);
                                buf.set_string(
                                    VALUE_X + col as u16,
                                    y,
                                    &one,
                                    Style::new().fg(CURSOR_FG).bg(CURSOR_BG),
                                );
                            }
                        }
                        _ => {
                            let text = match (&summary, fixture_view) {
                                (Some(list), _) => list.clone(),
                                // Reached by opening the table before setting a
                                // LED count, or by clearing one afterwards.
                                (None, Some(_)) if unused => String::from("has 0 LEDs"),
                                (None, Some(map)) => field.fixture_display(&map),
                                (None, None) => field.display(&data),
                            };
                            // A fixture row's value is the thing being set, so
                            // it stays in the normal colour; only a value the
                            // mode overrides is dimmed.
                            let value_style = if unused
                                || (derived.is_some() && fixture_view.is_none() && summary.is_none())
                            {
                                base.fg(OVERRIDDEN_FG)
                            } else if !field.editable() {
                                // Nothing to set here - the board is reporting.
                                base.fg(INFO_FG)
                            } else {
                                base
                            };
                            buf.set_string(VALUE_X, y, text, value_style);
                        }
                    }

                    if let Some(actual) = derived.as_ref().filter(|_| summary.is_none()) {
                        // Placed off `display_width`, not the rendered length,
                        // so the computed column stays straight down the page.
                        buf.set_string(
                            VALUE_X + field.display_width() as u16 + 1,
                            y,
                            actual,
                            base.fg(IN_FORCE_FG),
                        );
                    }
                }

                // The universe budget, on the row under `Universes`.
                if let Some(row) = page.fields.iter().position(|f| matches!(f, FieldId::TotalUniverses)) {
                    let y = row as u16 + 2;
                    if y < FOOTER_ROW {
                        draw_budget(buf, y, &data);
                    }
                }

                // The `LED Power` caveat, on whichever page carries that field.
                let carries_led_power = page.fields.iter().any(|f| matches!(*f, FieldId::LedPower));
                if carries_led_power && usb_power::status() & usb_power::ST_RPI_VBUS != 0 {
                    let note = Style::new().fg(NOTE_FG);
                    // Below the page's last row with one blank row between, so
                    // the amber text reads as a note on the page and not as the
                    // value of whatever row happens to be last.
                    let y = page.fields.len() as u16 + 2;
                    for (i, line) in USB_NOTE.iter().enumerate() {
                        let ny = y + i as u16;
                        if ny < FOOTER_ROW {
                            buf.set_string(CONTENT_LEFT, ny, *line, note);
                        }
                    }
                }

                draw_footer(buf, &footer);
            });
            diag::set(if drawn.is_ok() { diag::FIRST_FRAME } else { diag::DRAW_ERROR });
        }

        // Wake on a button or a settings change, or once a second to refresh
        // the lamp and count towards the idle screen.
        let event = match select(rx.receive(), Timer::after_secs(1)).await {
            Either::First(event) => event,
            Either::Second(()) => {
                lamp = activity::take();
                idle_secs = idle_secs.saturating_add(1);
                // An armed action left alone disarms itself.
                if matches!(mode, Mode::Confirm) && idle_secs >= CONFIRM_SECS {
                    mode = Mode::Navigate;
                }
                if !home && idle_secs >= IDLE_SECS && matches!(mode, Mode::Navigate) {
                    home = true;
                    home_full = true;
                }
                continue;
            }
        };
        let field = PAGES[cursor.page].fields[cursor.field];

        // A button ends the idle screen and is otherwise dropped: it was
        // pressed to get the menu back, not to act on whichever row happened
        // to be selected when the screen went idle.
        if matches!(event, UiEvent::Up | UiEvent::Down | UiEvent::Select | UiEvent::Esc) {
            idle_secs = 0;
            if home {
                home = false;
                // Resets the back buffer too, so the next draw is a full one.
                if terminal.clear().is_err() {
                    error!("TFT: clear failed leaving the idle screen");
                }
                continue;
            }
        }

        match (&mut mode, event) {
            // A settings update from the router always wins; an in-flight edit
            // would be working from a stale base.
            (m, UiEvent::Load(new_data)) => {
                data = new_data;
                // An armed action stays armed: there is no draft to go stale,
                // and a `Load` can arrive on its own (DHCP, a console `set`)
                // under the operator's finger.
                if !matches!(m, Mode::Confirm) {
                    *m = Mode::Navigate;
                }
                home_full = true;
                if data.backlight != brightness {
                    brightness = data.backlight;
                    if backlight.set_brightness(backlight_duty(brightness)).await.is_err() {
                        error!("PCA9633: brightness write failed");
                    }
                }
            }
            (_, UiEvent::Net(status)) => {
                net = status;
                home_full = true;
            }
            // Asked for from elsewhere (picotool): show what is about to
            // happen and stop drawing the menu. `reboot_task` ends this boot.
            (_, UiEvent::Rebooting(req)) => {
                draw_update_screen(terminal.backend_mut().display_mut(), req);
                core::future::pending::<()>().await;
            }

            // Up and Down are **inverted while navigating** and upright while
            // editing. That asymmetry is deliberate, chosen on the panel
            // itself (2026-09-15): moving through a list feels like scrolling
            // the list under a fixed highlight, so Down brings the entries
            // below into reach and the highlight travels up the page — while a
            // number under the cursor has to go up when Up is pressed, because
            // there the button is acting on the value and not on the view.
            // Do not "fix" one of these to match the other.
            (Mode::Navigate, UiEvent::Up) => cursor.step(true),
            (Mode::Navigate, UiEvent::Down) => cursor.step(false),
            // Esc jumps to the next page.
            (Mode::Navigate, UiEvent::Esc) => {
                // From a fixture table, Esc goes back to Layout rather than
                // onward - it is the way out of a detail page, which is what
                // Esc means once you are inside one.
                cursor.page = if PAGES[cursor.page].in_ring() {
                    Cursor::next_ring(cursor.page, true)
                } else {
                    layout_page()
                };
                cursor.field = 0;
            }
            (Mode::Navigate, UiEvent::Select) => {
                if field.port_unused(&data) {
                    // Nothing on a port with no LEDs can take effect, so there
                    // is nothing to open or edit. The way forward is the `LEDs`
                    // page, which stays live for exactly this reason.
                } else if field.fixture_slot().is_some() {
                    mode = Mode::EditFixture { digit: 0, draft: fixture_map() };
                } else if field.is_action() {
                    // Arm it; the next Select carries it out.
                    mode = Mode::Confirm;
                } else if let Some(page) = group_size_port(field)
                    .filter(|_| grouping(&data) == LedGrouping::Fixtures)
                    .and_then(fixture_page)
                {
                    // Under `Fixtures` a Layout row is a doorway, not a value:
                    // the per-port group size it would otherwise edit is not
                    // what that port is using.
                    cursor.page = page;
                    cursor.field = 0;
                } else if field.editable_now(&data) {
                    // `editable_now`, not `editable`: a port row is only worth
                    // editing under `Groups`. Under `Individual` the size is
                    // pinned to 1 whatever the stored value says, so letting
                    // someone dial a number that changes nothing is the
                    // confusion this avoids.
                    mode = Mode::Edit { digit: 0, draft: data };
                }
            }

            // Upright here — see the note on the navigate arms above.
            (Mode::Edit { digit, draft }, UiEvent::Up) => field.adjust(draft, *digit, true),
            (Mode::Edit { digit, draft }, UiEvent::Down) => field.adjust(draft, *digit, false),
            // Esc discards the draft outright.
            (m @ Mode::Edit { .. }, UiEvent::Esc) => *m = Mode::Navigate,

            // The second Select on an armed action. Today the one action is
            // the firmware update: draw the screen the operator will read while
            // the module is a USB drive, then hand over to `reboot_task`.
            (Mode::Confirm, UiEvent::Select) => {
                let req = RebootRequest::Bootsel { disable_mask: 0 };
                draw_update_screen(terminal.backend_mut().display_mut(), req);
                crate::reboot::request(req);
                core::future::pending::<()>().await;
            }
            // Anything else disarms it; the cursor stays where it was.
            (m @ Mode::Confirm, UiEvent::Esc | UiEvent::Up | UiEvent::Down) => *m = Mode::Navigate,

            (Mode::EditFixture { digit, draft }, UiEvent::Up) => {
                field.fixture_adjust(draft, port_leds(&data, field), *digit, true);
            }
            (Mode::EditFixture { digit, draft }, UiEvent::Down) => {
                field.fixture_adjust(draft, port_leds(&data, field), *digit, false);
            }
            (m @ Mode::EditFixture { .. }, UiEvent::Esc) => *m = Mode::Navigate,
            (Mode::EditFixture { digit, draft }, UiEvent::Select) => {
                if *digit + 1 < field.digits() {
                    *digit += 1;
                } else {
                    // Straight to the store: there is no router event for the
                    // map, and nothing else needs to merge it.
                    set_fixture_map(*draft);
                    if CHANNEL_EEPROM.try_send(EepromEvent::WriteFixtures).is_err() {
                        error!("UI: fixture save dropped, EEPROM channel full");
                    }
                    mode = Mode::Navigate;
                }
            }
            // Select walks to the next digit and commits past the last one.
            (Mode::Edit { digit, draft }, UiEvent::Select) => {
                if *digit + 1 < field.digits() {
                    *digit += 1;
                } else {
                    let committed = *draft;
                    if tx
                        .try_send(RouterEvent::WriteFieldToEeprom(field, committed))
                        .is_err()
                    {
                        error!("UI: commit dropped, router channel full");
                    }
                    mode = Mode::Navigate;
                }
            }
        }
    }
}

/// Index of the `Layout` page — where Esc returns to from a fixture table, and
/// where Select opens one from.
///
/// Found by looking for a `GroupSize` row rather than by page title or a
/// hard-coded index: titles are display text and have already been renamed once
/// (`Groups` → `Layout`), and an index would silently point at the wrong page
/// the next time one is inserted.
fn layout_page() -> usize {
    PAGES
        .iter()
        .position(|p| p.fields.iter().any(|f| matches!(f, FieldId::GroupSize(_))))
        .unwrap_or(0)
}

/// The fixture table belonging to `port`, if there is one.
fn fixture_page(port: usize) -> Option<usize> {
    PAGES
        .iter()
        .position(|p| p.fields.first().and_then(|f| f.fixture_slot()) == Some((port, 0)))
}

/// The port a `GroupSize` row belongs to, if that is what it is.
fn group_size_port(field: FieldId) -> Option<usize> {
    match field {
        FieldId::GroupSize(port) => Some(port),
        _ => None,
    }
}

/// The grouping mode in force.
fn grouping(data: &MenuData) -> LedGrouping {
    let ModuleSettings::SmartLed(s) = data.module;
    s.grouping
}

/// LEDs configured on the port a fixture row belongs to — the bound its length
/// is checked against.
fn port_leds(data: &MenuData, field: FieldId) -> u16 {
    let port = field.fixture_slot().map_or(0, |(p, _)| p);
    leds_on(data, port)
}
