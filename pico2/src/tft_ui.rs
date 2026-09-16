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
//! for the page title and network status, ten for fields, which is exactly
//! `MAX_FIELDS_PER_PAGE`. Change the font and that budget changes with it; the
//! test in `host_tests` is what keeps the page table honest about it.
//!
//! The panel's corners are radiused, so `CORNER_INSET` keeps content one column
//! clear of each side and **33 columns are usable**. Rows are unaffected: the
//! bottom two are spare at the page sizes in the field table today.
//!
//! # Why blocking SPI
//!
//! mipidsi is blocking. Ratatui only redraws cells that changed, so a keypress
//! costs a few hundred bytes on the bus; the one full redraw is the first
//! frame (~110 KB, ~25 ms at 40 MHz). LED and DMX timing is carried by PIO and
//! DMA in hardware, so a stalled poll on this core shifts scheduling rather
//! than output. Worth revisiting only if jitter shows up on the bench.

use alloc::string::String;

use common::channels::{RouterChannelTx, UiChannelRx};
use common::event_router::RouterEvent;
use common::events::{NetStatus, UiEvent};
use common::ui::fields::VALUE_COLUMN;
use common::ui::{backlight_duty, MenuData, DEFAULT_BACKLIGHT, PAGES};
use common::usb_power;
use common::{DISPLAY_HEIGHT, DISPLAY_OFFSET, DISPLAY_WIDTH};
use defmt::*;
use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI1;
use embassy_rp::spi::{Blocking, Spi};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Delay, Timer};
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::mono_font::ascii::FONT_9X15;
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::{Builder, Display};
use mousefood::{EmbeddedBackend, EmbeddedBackendConfig, TerminalAlignment};
use ratatui_core::style::{Modifier, Style};
use ratatui_core::terminal::Terminal;
use static_cell::StaticCell;

use crate::diag;
use crate::pca9633::Pca9633;

/// Character grid the font and panel produce; the field table is checked
/// against `ROWS - 1` in `host_tests`.
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

/// First column content may use, and one past the last.
const CONTENT_LEFT: u16 = CORNER_INSET;
const CONTENT_RIGHT: u16 = COLUMNS - CORNER_INSET;

/// Value text starts here, allowing for the inset.
const VALUE_X: u16 = CONTENT_LEFT + VALUE_COLUMN;

/// The widest value any field renders must still fit inside the inset area.
/// `IpAddrMenu` is the longest at `255.255.255.255`.
const _: () = core::assert!(VALUE_X + 15 <= CONTENT_RIGHT);

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
    Edit { digit: u8, draft: MenuData },
}

/// Menu cursor: which page, and which field within it.
struct Cursor {
    page: usize,
    field: usize,
}

impl Cursor {
    /// Move by one, rolling onto the next or previous page at the edges.
    fn step(&mut self, down: bool) {
        let len = PAGES[self.page].fields.len();
        if down {
            if self.field + 1 < len {
                self.field += 1;
            } else {
                self.page = (self.page + 1) % PAGES.len();
                self.field = 0;
            }
        } else if self.field > 0 {
            self.field -= 1;
        } else {
            self.page = (self.page + PAGES.len() - 1) % PAGES.len();
            self.field = PAGES[self.page].fields.len().saturating_sub(1);
        }
    }
}

/// Right-hand end of the title row.
fn net_text(status: NetStatus) -> String {
    use alloc::format;
    match status {
        NetStatus::Off => String::from("ETH off"),
        NetStatus::Guard => String::from("ETH guard"),
        NetStatus::NoChip => String::from("ETH no chip"),
        NetStatus::Dhcp => String::from("ETH dhcp..."),
        NetStatus::Up(ip) => format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
    }
}

/// Title-row flag for the USB-brick supply. Empty unless it is carrying the
/// strips, so the row stays uncluttered in the normal case.
fn usb_power_text() -> String {
    let s = usb_power::status();
    if usb_power::brick_selected() && (s & usb_power::ST_FAULT) != 0 {
        String::from("PWR flt")
    } else if usb_power::brick_powering() {
        String::from("PWR usb")
    } else {
        String::new()
    }
}

/// Bring the panel up, then draw the menu and react to button events.
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
    // Colour splash: red, green, blue, then black. A bring-up aid that costs
    // half a second at boot and answers three questions from across the bench
    // with no probe attached: does the panel hear SPI at all (no splash = no),
    // are the colours right (cyan/magenta/yellow = `ColorInversion` is wrong),
    // and if the splash shows but the menu does not, the fault is in the
    // Ratatui/mousefood draw path, not the panel.
    for colour in [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE, Rgb565::BLACK] {
        let _ = display.clear(colour);
        Timer::after_millis(150).await;
    }
    diag::set(diag::TFT_SPLASH_DONE);

    // Backlight last, so the first thing visible is the menu.
    if backlight.init(backlight_duty(DEFAULT_BACKLIGHT)).await.is_ok() {
        diag::set(diag::BACKLIGHT_OK);
    } else {
        diag::set(diag::BACKLIGHT_FAIL);
        error!("PCA9633: init failed - backlight off");
    }
    info!("TFT: up, {}x{} grid", COLUMNS, ROWS);

    let config = EmbeddedBackendConfig {
        // Direct-draw display: nothing to flush.
        flush_callback: alloc::boxed::Box::new(|_d: &mut Tft| {}),
        font_regular: FONT_9X15,
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

    let mut data = MenuData::default();
    let mut brightness = DEFAULT_BACKLIGHT;
    let mut net = NetStatus::Off;
    let mut cursor = Cursor { page: 0, field: 0 };
    let mut mode = Mode::Navigate;
    loop {
        {
            let page = &PAGES[cursor.page];
            let status = net_text(net);
            let power = usb_power_text();
            let drawn = terminal.draw(|frame| {
                let buf = frame.buffer_mut();
                // Title row: page name left, network status right.
                buf.set_string(
                    CONTENT_LEFT,
                    0,
                    page.title,
                    Style::new().add_modifier(Modifier::REVERSED),
                );
                let x = CONTENT_RIGHT.saturating_sub(status.len() as u16);
                buf.set_string(x, 0, &status, Style::new());
                // Between the page name and the network status: only shown
                // when the strips are actually running off the USB brick, so
                // an operator can tell at a glance why the output is dimmed.
                if !power.is_empty() {
                    let px = CONTENT_LEFT + (page.title.len() as u16) + 1;
                    if px + (power.len() as u16) < x {
                        buf.set_string(px, 0, &power, Style::new());
                    }
                }

                for (row, field) in page.fields.iter().enumerate() {
                    let selected = row == cursor.field;
                    let style = if selected {
                        Style::new().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::new()
                    };
                    let y = row as u16 + 1;
                    // Label left, value from VALUE_COLUMN; labels are kept
                    // shorter than that in the field table.
                    buf.set_string(CONTENT_LEFT, y, field.label(), style);

                    match &mode {
                        // Show the draft, not the live value, and mark the digit
                        // being changed so it is obvious which one Up/Down moves.
                        Mode::Edit { digit, draft } if selected => {
                            let text = field.display(draft);
                            buf.set_string(VALUE_X, y, &text, Style::new());
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
                                    Style::new().add_modifier(Modifier::REVERSED),
                                );
                            }
                        }
                        _ => buf.set_string(VALUE_X, y, field.display(&data), style),
                    }
                }
            });
            diag::set(if drawn.is_ok() { diag::FIRST_FRAME } else { diag::DRAW_ERROR });
        }

        let event = rx.receive().await;
        let field = PAGES[cursor.page].fields[cursor.field];

        match (&mut mode, event) {
            // A settings update from the router always wins; an in-flight edit
            // would be working from a stale base.
            (m, UiEvent::Load(new_data)) => {
                data = new_data;
                *m = Mode::Navigate;
                if data.backlight != brightness {
                    brightness = data.backlight;
                    if backlight.set_brightness(backlight_duty(brightness)).await.is_err() {
                        error!("PCA9633: brightness write failed");
                    }
                }
            }
            (_, UiEvent::Net(status)) => net = status,

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
                cursor.page = (cursor.page + 1) % PAGES.len();
                cursor.field = 0;
            }
            (Mode::Navigate, UiEvent::Select) => {
                if field.editable() {
                    mode = Mode::Edit { digit: 0, draft: data };
                }
            }

            // Upright here — see the note on the navigate arms above.
            (Mode::Edit { digit, draft }, UiEvent::Up) => field.adjust(draft, *digit, true),
            (Mode::Edit { digit, draft }, UiEvent::Down) => field.adjust(draft, *digit, false),
            // Esc discards the draft outright.
            (m @ Mode::Edit { .. }, UiEvent::Esc) => *m = Mode::Navigate,
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
