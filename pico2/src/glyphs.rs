//! Three picture glyphs for the menu, supplied to mousefood as its **bold**
//! face.
//!
//! The menu is a character grid: a cell is a glyph in a foreground colour on a
//! background colour, and a background fills the whole 9x15 cell. That is fine
//! for text and useless for a bar chart, where two bars in adjacent rows would
//! fuse into one block. These glyphs are the bars: a block that stops one pixel
//! short of the cell's top and bottom, a second one that also stops a pixel
//! short of the right edge (the end of a run), and a disc for the activity
//! lamp. Drawn as `Modifier::BOLD` text in the bar's colour they inset
//! themselves, and the row background shows through as the gap.
//!
//! Nothing else on the panel is bold, so the modifier is free to mean "picture
//! font". The image is 27 pixels wide - three cells - and each row is padded to
//! four bytes, which is how `ImageRaw` wants a 1-bit image.

use embedded_graphics::geometry::Size;
use embedded_graphics::image::ImageRaw;
use embedded_graphics::mono_font::mapping::StrGlyphMapping;
use embedded_graphics::mono_font::{DecorationDimensions, MonoFont};

/// A full-width bar cell: 9 wide, rows 1..=13 of 15.
pub const BLOCK: &str = "#";
/// A bar cell with a one-pixel gap on its right: the last cell of a run.
pub const BLOCK_END: &str = "=";
/// A 7-pixel disc, centred: the activity lamp.
pub const DISC: &str = "o";

/// Glyph order in the image, one cell each, left to right.
const MAPPING: StrGlyphMapping<'static> = StrGlyphMapping::new("#=o", 0);

#[rustfmt::skip]
const DATA: [u8; 60] = [
    0x00, 0x00, 0x00, 0x00, // ...........................
    0xFF, 0xFF, 0x80, 0x00, // #################..........
    0xFF, 0xFF, 0x80, 0x00, // #################..........
    0xFF, 0xFF, 0x80, 0x00, // #################..........
    0xFF, 0xFF, 0x8F, 0x80, // #################...#####..
    0xFF, 0xFF, 0x9F, 0xC0, // #################..#######.
    0xFF, 0xFF, 0x9F, 0xC0, // #################..#######.
    0xFF, 0xFF, 0x9F, 0xC0, // #################..#######.
    0xFF, 0xFF, 0x9F, 0xC0, // #################..#######.
    0xFF, 0xFF, 0x9F, 0xC0, // #################..#######.
    0xFF, 0xFF, 0x8F, 0x80, // #################...#####..
    0xFF, 0xFF, 0x80, 0x00, // #################..........
    0xFF, 0xFF, 0x80, 0x00, // #################..........
    0xFF, 0xFF, 0x80, 0x00, // #################..........
    0x00, 0x00, 0x00, 0x00, // ...........................
];

/// The picture font. Same cell as `FONT_9X15`, so the grid does not change.
pub const FONT: MonoFont<'static> = MonoFont {
    image: ImageRaw::new(&DATA, 27),
    glyph_mapping: &MAPPING,
    character_size: Size::new(9, 15),
    character_spacing: 0,
    baseline: 11,
    underline: DecorationDimensions::new(13, 1),
    strikethrough: DecorationDimensions::new(7, 1),
};
