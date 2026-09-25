//! A 5x7 pixel alphabet, drawn at any scale, for the boot wordmark.
//!
//! embedded-graphics' largest built-in face is 10x20, which on a 320-pixel
//! panel is a caption, not a title. Scaling a bitmap font by drawing each set
//! pixel as a square keeps the hard pixel edges - the look the splash wants -
//! and costs seven bytes a letter. Only the letters the wordmark uses are
//! here; anything else draws as a blank advance rather than panicking.

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::primitives::{Primitive, PrimitiveStyle, Rectangle};
use embedded_graphics::Drawable;

/// Glyph width in font pixels and the advance (one pixel of gap). Glyphs are
/// seven rows tall.
pub const GLYPH_W: u32 = 5;
pub const ADVANCE: u32 = GLYPH_W + 1;

/// Rows top to bottom, bit 4 is the left column.
#[rustfmt::skip]
fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c {
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'I' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        _ => return None,
    })
}

/// Width in panel pixels of `text` at `scale`, without the trailing gap.
pub const fn text_width(len: u32, scale: u32) -> u32 {
    if len == 0 {
        0
    } else {
        (len * ADVANCE - 1) * scale
    }
}

/// Draw `text` with its top-left corner at (`x`, `y`), each font pixel a
/// `scale`-by-`scale` square.
pub fn draw_text<D: DrawTarget<Color = Rgb565>>(
    target: &mut D,
    text: &str,
    x: i32,
    y: i32,
    scale: u32,
    color: Rgb565,
) -> Result<(), D::Error> {
    let style = PrimitiveStyle::with_fill(color);
    let mut cx = x;
    for c in text.chars() {
        if let Some(rows) = glyph(c) {
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..GLYPH_W {
                    if bits & (0b10000 >> col) != 0 {
                        Rectangle::new(
                            Point::new(cx + (col * scale) as i32, y + (row as u32 * scale) as i32),
                            Size::new(scale, scale),
                        )
                        .into_styled(style)
                        .draw(target)?;
                    }
                }
            }
        }
        cx += (ADVANCE * scale) as i32;
    }
    Ok(())
}
