//! Per-port fixture layouts: a strip carved into runs of different sizes.
//!
//! [`LedGrouping::Groups`] gives a strip one uniform run length. This is the
//! layout for a strip carrying fixtures that differ - a 10-LED wash, a 5-LED
//! accent, a 30-LED bar - each answering to its own DMX channels, with dark
//! LEDs allowed between them.
//!
//! # Runs, not absolute starts
//!
//! An entry stores a **length** and whether the run is lit or a deliberate gap.
//! A fixture's start is the running sum of everything before it, computed on
//! demand and never stored. Two consequences:
//!
//! * **Overlaps are unrepresentable.** "Do not let a fixture run past the start
//!   of the next one" stops being a rule to enforce and becomes a property of
//!   the format. The only check left is that a port's total fits its strip.
//! * **Half the bytes.** One `u16` per entry rather than two, which is what
//!   lets the whole map fit the EEPROM the board already has.
//!
//! [`LedGrouping::Groups`]: crate::ui::LedGrouping::Groups

use crate::{MAX_LEDS_PER_PORT, SMARTLED_PORT_COUNT};

use defmt::Format;

/// Entries a port may hold.
///
/// Five, because the stored blob is 80 bytes and 8 ports x 5 entries x 2 bytes
/// fills it exactly. **A gap costs an entry**, so five *fixtures* only if there
/// are none: three fixtures separated by two gaps is the full five.
pub const MAX_FIXTURES_PER_PORT: usize = 5;

/// Gap flag, in the bit no length can reach.
const GAP_BIT: u16 = 1 << 15;
/// Length occupies the low ten bits: 0..=1023.
const LEN_MASK: u16 = 0x03FF;

/// A length has to hold any strip the hardware will clock out.
const _: () = core::assert!(MAX_LEDS_PER_PORT <= LEN_MASK as usize);

/// One run on a strip: a length, and whether it is lit or a deliberate gap.
///
/// Stored exactly as it is written to the EEPROM - bit 15 the gap flag, bits
/// 0..=9 the length - so there is no conversion layer between the model and the
/// blob to disagree with itself. **A length of zero terminates the port's
/// list**, which is why no count is stored.
#[derive(Clone, Copy, PartialEq, Eq, Format, Debug, Default)]
pub struct FixtureEntry(u16);

impl FixtureEntry {
    /// The terminator: no run, and the end of the list.
    pub const NONE: Self = Self(0);

    /// A lit run of `len` LEDs. Zero gives [`Self::NONE`].
    pub fn fixture(len: u16) -> Self {
        Self(len.min(LEN_MASK))
    }

    /// A dark run of `len` LEDs. Zero gives [`Self::NONE`] - a gap of nothing
    /// is not a gap, and storing one would truncate the list behind it.
    pub fn gap(len: u16) -> Self {
        match len.min(LEN_MASK) {
            0 => Self::NONE,
            n => Self(n | GAP_BIT),
        }
    }

    /// LEDs this run occupies, lit or not.
    pub fn leds(&self) -> u16 {
        self.0 & LEN_MASK
    }

    /// True when the run is dark by request rather than lit.
    pub fn is_gap(&self) -> bool {
        self.0 & GAP_BIT != 0
    }

    /// True for the terminator.
    pub fn is_none(&self) -> bool {
        self.leds() == 0
    }

    /// The stored form, for the EEPROM codec.
    pub fn raw(&self) -> u16 {
        self.0
    }

    /// Rebuild from the stored form. A raw value whose length is zero is the
    /// terminator whatever its flag bit says.
    pub fn from_raw(raw: u16) -> Self {
        if raw & LEN_MASK == 0 {
            Self::NONE
        } else {
            Self(raw & (LEN_MASK | GAP_BIT))
        }
    }
}

/// Every port's fixture list.
///
/// 80 bytes, and `Copy` so the render can take a snapshot rather than hold a
/// lock across a frame. Deliberately **not** part of `MenuData`: that struct is
/// copied by value through the router channels and the TFT edit draft on every
/// button press, and it has 18 bytes of EEPROM headroom against this map's 80.
#[derive(Clone, Copy, PartialEq, Format, Debug)]
pub struct FixtureMap(pub [[FixtureEntry; MAX_FIXTURES_PER_PORT]; SMARTLED_PORT_COUNT]);

impl Default for FixtureMap {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl FixtureMap {
    /// What a port with nothing stored uses: **one fixture of one LED**.
    ///
    /// Not a fallback to the port's group size. That was the first design and
    /// it was rejected on sight: a port silently behaving as though it were in
    /// a different mode is hidden state, and the operator has no way to tell
    /// "I have not configured this yet" from "this is configured to look like
    /// Groups". A visible `F001` is a starting point you can see and edit.
    pub const DEFAULT_ENTRY: FixtureEntry = FixtureEntry(1);

    /// No port configured.
    pub const EMPTY: Self =
        Self([[FixtureEntry::NONE; MAX_FIXTURES_PER_PORT]; SMARTLED_PORT_COUNT]);

    fn port(&self, port: usize) -> &[FixtureEntry; MAX_FIXTURES_PER_PORT] {
        &self.0[port.min(SMARTLED_PORT_COUNT - 1)]
    }

    /// The port's entries in order, stopping at the terminator.
    pub fn entries(&self, port: usize) -> impl Iterator<Item = FixtureEntry> + '_ {
        self.port(port)
            .iter()
            .copied()
            .take_while(|e| !e.is_none())
    }

    /// True when the port has nothing **stored**.
    ///
    /// Such a port still renders - as [`Self::DEFAULT_ENTRY`] - so this is the
    /// question "has anyone configured this?", not "does it do anything?".
    pub fn is_empty(&self, port: usize) -> bool {
        self.port(port)[0].is_none()
    }

    /// The port's entries **as they are used**: what is stored, or the default
    /// when nothing is.
    ///
    /// Every render and every count goes through here, so what the panel shows
    /// and what the strip does cannot disagree.
    pub fn effective_entries(&self, port: usize) -> impl Iterator<Item = FixtureEntry> + '_ {
        let defaulted = self.is_empty(port);
        core::iter::once(Self::DEFAULT_ENTRY)
            .take(usize::from(defaulted))
            .chain(self.entries(port))
    }

    /// The entry a slot shows, defaulting like [`Self::effective_entries`].
    pub fn effective_entry(&self, port: usize, slot: usize) -> FixtureEntry {
        self.effective_entries(port).nth(slot).unwrap_or(FixtureEntry::NONE)
    }

    /// `(start, len)` of each **lit** run, zero-based. Gaps advance the cursor
    /// and yield nothing, which is what makes them dark rather than absent.
    pub fn spans(&self, port: usize) -> impl Iterator<Item = (u16, u16)> + '_ {
        let mut at = 0_u16;
        self.effective_entries(port).filter_map(move |e| {
            let start = at;
            at = at.saturating_add(e.leds());
            (!e.is_gap()).then_some((start, e.leds()))
        })
    }

    /// Addressable items the port has: lit runs only, since a gap consumes no
    /// DMX channels.
    pub fn fixture_count(&self, port: usize) -> u16 {
        self.effective_entries(port).filter(|e| !e.is_gap()).count() as u16
    }

    /// LEDs the **stored** entries occupy, gaps included - the figure that must
    /// fit `leds_per_port`.
    ///
    /// Deliberately not defaulted: this bounds what an edit may set, and
    /// counting a default the operator has not made would take a LED off the
    /// budget of the first entry they create.
    pub fn used(&self, port: usize) -> u16 {
        self.entries(port)
            .fold(0_u16, |acc, e| acc.saturating_add(e.leds()))
    }

    /// The port as a picture: `W` cells across, scaled so that `longest`
    /// LEDs fill the whole width.
    ///
    /// Every port on a page shares one scale (the longest port's), so a 32-LED
    /// strip draws half as long as a 64-LED one and the picture compares
    /// lengths as well as layouts. Cells past the strip's own length are
    /// [`StripCell::Off`]; LEDs no fixture covers are [`StripCell::Dark`],
    /// which is what the render does to them.
    ///
    /// A lit fixture always gets at least one cell, even a one-LED fixture on
    /// a 600-LED port, because a fixture that is configured but invisible
    /// would read as missing. The cell is taken from whatever follows, so the
    /// total never exceeds the strip.
    pub fn strip_cells<const W: usize>(&self, port: usize, leds: u16, longest: u16) -> [StripCell; W] {
        let mut out = [StripCell::Off; W];
        if leds == 0 || W == 0 {
            return out;
        }
        let longest = u32::from(longest.max(leds));
        let scale = |led: u32| ((led * W as u32) / longest) as usize;
        // The strip itself: as many cells as its length earns, at least one.
        let strip_end = scale(u32::from(leds)).clamp(1, W);
        out[..strip_end].fill(StripCell::Dark);
        let mut at = 0_u32;
        let mut next = 0_usize;
        let mut fixture = 0_u8;
        for e in self.effective_entries(port) {
            if at >= u32::from(leds) || next >= strip_end {
                break;
            }
            let len = u32::from(e.leds());
            let start = scale(at).max(next);
            let mut end = scale(at + len).min(strip_end);
            if !e.is_gap() {
                if end <= start {
                    end = (start + 1).min(strip_end);
                }
                if start < end {
                    out[start..end].fill(StripCell::Lit(fixture));
                }
                fixture = fixture.wrapping_add(1);
            }
            next = end.max(next);
            at += len;
        }
        out
    }
}

/// One cell of a [`FixtureMap::strip_cells`] picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StripCell {
    /// Past the end of this strip.
    Off,
    /// A LED on the strip that no fixture drives - a gap, or the tail.
    Dark,
    /// Part of the n-th lit fixture (zero-based), so neighbours can alternate.
    Lit(u8),
}

// ---------------------------------------------------------------------------
// The stored form.
//
// A second EEPROM blob, not part of the bincode settings: that slot has 18
// bytes of headroom against this map's 80, and `MenuData` is `Copy` and moves
// by value through the router channels on every button press.
//
// Hand-rolled rather than bincode, because bincode varints anything >= 251 to
// three bytes — and a gap-flagged entry is always >= 32768, which would turn
// this 80-byte payload into 152.
// ---------------------------------------------------------------------------

/// Payload bytes: 8 ports x 5 entries x 2 B. Five 16-byte EEPROM pages exactly.
pub const FIXTURE_PAYLOAD_LEN: usize = SMARTLED_PORT_COUNT * MAX_FIXTURES_PER_PORT * 2;

/// Header bytes: magic, format version, CRC-16.
pub const FIXTURE_HEADER_LEN: usize = 5;

/// Marks the region as ours. A blank EEPROM reads `0xFF`, and a stale byte from
/// an earlier layout reads as something else; neither matches.
const FIXTURE_MAGIC: [u8; 2] = [0x46, 0x58]; // "FX"

/// Bump if the payload layout changes. Independent of the settings schema —
/// the two blobs version separately because they are written separately.
pub const FIXTURE_FORMAT_VERSION: u8 = 1;

const _: () = core::assert!(FIXTURE_PAYLOAD_LEN == 80);

/// CRC-16/CCITT-FALSE over the payload.
///
/// The settings blob gets away with no checksum because bincode fails to decode
/// a torn write. A packed blob has no such self-check: every bit pattern is a
/// legal layout, so a half-written one would render as a plausible rig.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in data {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

impl FixtureMap {
    /// Serialise into the payload region, little-endian per entry.
    pub fn encode(&self, out: &mut [u8; FIXTURE_PAYLOAD_LEN]) {
        for (port, entries) in self.0.iter().enumerate() {
            for (slot, entry) in entries.iter().enumerate() {
                let at = (port * MAX_FIXTURES_PER_PORT + slot) * 2;
                out[at..at + 2].copy_from_slice(&entry.raw().to_le_bytes());
            }
        }
    }

    /// The header for a given payload, written **last** so a torn write reads
    /// back as "no valid map" rather than as half a layout.
    pub fn header(payload: &[u8; FIXTURE_PAYLOAD_LEN]) -> [u8; FIXTURE_HEADER_LEN] {
        let crc = crc16(payload).to_le_bytes();
        [FIXTURE_MAGIC[0], FIXTURE_MAGIC[1], FIXTURE_FORMAT_VERSION, crc[0], crc[1]]
    }

    /// Rebuild from what was read back, or `None` if it is not a map we wrote.
    ///
    /// `None` is not a failure to report loudly: [`Self::EMPTY`] is a safe
    /// result, and every port then falls back to its `Groups` layout.
    pub fn decode(header: &[u8], payload: &[u8]) -> Option<Self> {
        if header.len() < FIXTURE_HEADER_LEN || payload.len() < FIXTURE_PAYLOAD_LEN {
            return None;
        }
        if header[0..2] != FIXTURE_MAGIC || header[2] != FIXTURE_FORMAT_VERSION {
            return None;
        }
        let stored = u16::from_le_bytes([header[3], header[4]]);
        if stored != crc16(&payload[..FIXTURE_PAYLOAD_LEN]) {
            return None;
        }

        let mut map = Self::EMPTY;
        for (port, entries) in map.0.iter_mut().enumerate() {
            for (slot, entry) in entries.iter_mut().enumerate() {
                let at = (port * MAX_FIXTURES_PER_PORT + slot) * 2;
                *entry = FixtureEntry::from_raw(u16::from_le_bytes([payload[at], payload[at + 1]]));
            }
        }
        Some(map)
    }
}
