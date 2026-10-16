//! Messages passed between tasks.
//!
//! These are the vocabulary of the event router: every task either produces or
//! consumes them. They live here rather than beside the tasks that send them
//! because the router and the channels are target-agnostic, while the tasks
//! themselves are not — a task signature names a concrete `Spi`, `I2c` or
//! `Input`, but the message it sends does not.

use defmt::Format;

use crate::ui::{BootStatus, MenuData, ModuleType, SmartLedColorMode};
use crate::SMARTLED_PORT_COUNT;

/// States to define button actions
#[derive(Copy, Clone, Format, PartialEq, Debug)]
pub enum ButtonEvent {
    /// Null event
    None,
    /// Button was pressed
    Pressed,
    /// Button was released
    Released,
    /// Button is held
    Held,
}

#[derive(Copy, Clone, Format, Debug, PartialEq)]
pub enum KeyPadEvent {
    None,
    Pressed,
    Released,
    Held,
}

// Format
// 1 2 3 A
// 4 5 6 B
// 7 8 9 C
// * 0 # D
#[derive(Copy, Clone, Format, Debug)]
pub enum KeyPadButton {
    A,
    B,
    C,
    D,
    N0,
    N1,
    N2,
    N3,
    N4,
    N5,
    N6,
    N7,
    N8,
    N9,
    Star,
    Pound,
    None,
}

impl KeyPadButton {
    /// Map a scan position to the button at that location. Positions 0..=11 are
    /// the 4x4 matrix in reading order; 12..=15 are the single-row buttons.
    pub fn at(location: usize) -> Self {
        match location {
            0 => Self::N1,
            1 => Self::N2,
            2 => Self::N3,
            3 => Self::A,
            4 => Self::N4,
            5 => Self::N5,
            6 => Self::N6,
            7 => Self::B,
            8 => Self::N7,
            9 => Self::N8,
            10 => Self::N9,
            11 => Self::C,
            12 => Self::Star,  // single row button 0
            13 => Self::N0,    // single row button 1
            14 => Self::Pound, // single row button 2
            15 => Self::D,     // single row button 3
            _ => Self::None,
        }
    }
}

#[allow(unused)]
pub enum SmartLedEvent {
    /// Push `LED_COLORS` to the strips: the configured LED count per port (only
    /// that many pixels are encoded and transmitted) and the colour mode, which
    /// selects 24-bit GRB or 32-bit GRBW on the wire.
    UpdateLEDs {
        counts: [u16; SMARTLED_PORT_COUNT],
        color_mode: SmartLedColorMode,
    },
}

impl Format for SmartLedEvent {
    fn format(&self, f: defmt::Formatter) {
        match self {
            SmartLedEvent::UpdateLEDs { counts, color_mode } => {
                defmt::write!(f, "Update LEDs: {:?} {:?}", counts, color_mode)
            }
        }
    }
}

#[allow(unused)]
#[derive(Clone, Copy, Format, Debug)]
pub enum EepromEvent {
    /// Read module type from EEPROM
    ReadModuleType,
    /// Write module type to EEPROM
    WriteModuleType(ModuleType),
    /// Read settings from EEPROM
    ReadSettings,
    /// Write settings to EEPROM
    WriteSettings(MenuData),
    /// Read MAC address from EEPROM
    ReadMacAddress,
    /// Write MAC address to EEPROM
    WriteMacAddress([u8; 6]),
    /// Read boot_successful flag from EEPROM
    ReadBootStatus,
    /// Write boot_successful flag to EEPROM
    WriteBootStatus(BootStatus),
    /// Read the fixture map from EEPROM into `buffers::FIXTURE_MAP`.
    ReadFixtures,
    /// Persist whatever `buffers::FIXTURE_MAP` currently holds.
    ///
    /// **Carries no payload on purpose.** `CHANNEL_EEPROM` is depth 1 and sent
    /// with `try_send`, so two edits in quick succession would drop one of
    /// them. Because the task reads the static rather than a copy taken at send
    /// time, a dropped duplicate is harmless - whichever write does run
    /// persists the latest state.
    WriteFixtures,
}

/// Where the network stands, for the status line on the display and the
/// console `info` command. Set by the boot sequence and the Art-Net task.
#[derive(Copy, Clone, PartialEq, Eq, Format, Debug, Default)]
pub enum NetStatus {
    /// Ethernet disabled in settings.
    #[default]
    Off,
    /// Skipped this boot by the lockout guard (two consecutive incomplete boots).
    Guard,
    /// The W6300 did not answer, or reported the wrong chip version.
    NoChip,
    /// Chip up, waiting for DHCP.
    Dhcp,
    /// Configured; the address is what the node advertises.
    Up([u8; 4]),
}

/// How the board has been asked to restart itself. See `pico2/src/reboot.rs`.
#[derive(Copy, Clone, PartialEq, Eq, Format, Debug)]
pub enum RebootRequest {
    /// Into the RP2350 bootrom's BOOTSEL mode, where the module is a USB drive
    /// that accepts a UF2. `disable_mask` is bits 0-6 of the picotool reset
    /// request's wValue (1 = no mass storage, 2 = no PICOBOOT); 0 from the
    /// panel, which wants both.
    Bootsel { disable_mask: u8 },
    /// A plain restart into the application.
    Flash,
}

/// Action to be processed by the UI (often user interaction)
#[derive(Format)]
pub enum UiEvent {
    Up,
    Down,
    Select,
    Esc,
    Load(MenuData),
    /// Network status changed; shown on the display's title row.
    Net(NetStatus),
    /// The board is about to restart; the panel shows what is happening and
    /// stops drawing the menu.
    Rebooting(RebootRequest),
}
