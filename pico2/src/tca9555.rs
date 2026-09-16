//! TCA9555 16-bit I²C I/O expander — front-panel buttons and the TFT's static
//! control lines.
//!
//! Four register pairs, so a driver rather than a dependency.
//!
//! | Port | Bits | Function on the Rev 2 carrier |
//! |---|---|---|
//! | 0 | 0..=3 | buttons Down / Up / Select / Esc (inputs, active low, J6) |
//! | 0 | 4 | `USB_LED_EN` — TPS2553-1 enable for the USB-brick LED supply (output) |
//! | 0 | 5 | `VBUS_FTDI_DET` — 5 V present on the FTDI USB-C (input) |
//! | 0 | 6 | `VBUS_RPI_DET` — 5 V present on the module USB (input) |
//! | 0 | 7 | `USB_LED_FLT` — TPS2553-1 FAULT, low = latched off (input) |
//! | 1 | 0 | TFT `RES` (output) |
//! | 1 | 1 | TFT `CS` (output, held low — the panel is alone on SPI1) |
//! | 1 | 2..=7 | spares P12–P17 on J7 (inputs); P17 has no header pin |
//!
//! # `INT`
//!
//! Not wired on the Rev 2 carrier — GP28 went to the FT232RNL UART, so the
//! buttons are polled (`buttons.rs`). The pin goes to test point TP1 only.
//! [`inputs`] still reads **both** ports in one auto-incrementing transfer:
//! it is one transaction either way, and on any board that does wire `INT`
//! that is the read that clears the latch for both ports.
//!
//! # Pull-ups
//!
//! The TCA9555 (unlike the TCA9535) has ~100 kΩ pull-ups on every input, which
//! is why the buttons are wired straight to ground with no external resistors —
//! and also why an input-configured `CS` would float *high*: the display's chip
//! select must be an output, driven low, or the panel ignores every byte.

use embedded_hal_async::i2c::I2c;

/// Address with A0/A1/A2 strapped to ground. The part answers 0x20–0x27, so up
/// to eight can share the bus; the M24C02 at 0x56 and the PCA9633 at 0x62 do not
/// collide.
pub const ADDR: u8 = common::EXPANDER_ADDRESS;

// Register map. Each is a pair: port 0 then port 1.
const REG_INPUT: u8 = 0x00;
const REG_OUTPUT: u8 = 0x02;
const REG_POLARITY: u8 = 0x04;
const REG_CONFIG: u8 = 0x06;

/// Port 0, bits 0..=3 — the front-panel buttons, active low.
pub const BTN_DOWN: u8 = 1 << 0;
pub const BTN_UP: u8 = 1 << 1;
pub const BTN_SELECT: u8 = 1 << 2;
pub const BTN_ESC: u8 = 1 << 3;

/// Port 0, bit 4 — `USB_LED_EN`, the TPS2553-1 enable. Active high with a
/// 100 k pull-down (R38) on the board, so the brick path stays off through
/// reset and only closes when firmware drives this bit.
pub const USB_LED_EN: u8 = 1 << 4;
/// Port 0, bit 5 — VBUS present on the FTDI USB-C connector, through the
/// 10 k/20 k divider (3.33 V at 5 V, against a 2.31 V VIH).
pub const VBUS_FTDI_DET: u8 = 1 << 5;
/// Port 0, bit 6 — VBUS present on the module's own USB port.
pub const VBUS_RPI_DET: u8 = 1 << 6;
/// Port 0, bit 7 — the TPS2553-1 `FAULT` output: open drain, pulled up by R35.
/// **Low means the switch has latched off** (overcurrent or reverse voltage)
/// and it will not conduct again until EN is toggled.
pub const USB_LED_FLT: u8 = 1 << 7;

/// Port 1, bit 0 — TFT reset. Asserted only at boot, which is why it can live
/// behind an I²C round trip at all.
pub const DISPLAY_RES: u8 = 1 << 0;
/// Port 1, bit 1 — TFT chip select. Static: the display is the only device on
/// SPI1, so it is asserted once at init and stays asserted.
///
/// **It must be HIGH while the panel is held in reset.** The ST7789's serial
/// interface initialises its bit counter when CS is high, so the controller
/// has to see a clean high-to-low chip-select edge *after* RES is released or
/// it never synchronises to the byte stream. Rev 2's first build drove RES and
/// CS low in the same write, so the panel left reset with CS already low and
/// ignored a perfectly formed init sequence — the Rev 1 board hid this because
/// `ExclusiveDevice` held CS high except during transfers.
pub const DISPLAY_CS: u8 = 1 << 1;

/// Direction masks: a 1 bit is an input, a 0 bit is an output.
///
/// On port 0 only `USB_LED_EN` is an output — the buttons and the three USB
/// power status lines are inputs. On port 1 only the two display lines are
/// outputs; the remaining six stay inputs so a floating spare on J7 cannot
/// fight anything.
const CONFIG_PORT0: u8 = !USB_LED_EN;
const CONFIG_PORT1: u8 = !(DISPLAY_RES | DISPLAY_CS);

pub struct Tca9555<I2C> {
    i2c: I2C,
    addr: u8,
    /// Shadow of the output register — the part has no readback of what was
    /// written, only of the pin state.
    out: [u8; 2],
}

impl<I2C: I2c> Tca9555<I2C> {
    pub fn new(i2c: I2C, addr: u8) -> Self {
        Self {
            i2c,
            addr,
            // Port 0: USB_LED_EN low — never power strips from a USB port
            // before the firmware has decided to. Port 1: RES low (display held
            // in reset until `release_display_reset`) and **CS high**, so the
            // panel is deselected while it is being reset; `select_display`
            // asserts CS afterwards. Everything else high.
            out: [!USB_LED_EN, !DISPLAY_RES],
        }
    }

    /// Set directions and drive the initial output state.
    ///
    /// Outputs are written *before* the direction register so no pin is ever
    /// briefly driven to the wrong level as it turns around.
    pub async fn init(&mut self) -> Result<(), I2C::Error> {
        self.i2c
            .write(self.addr, &[REG_POLARITY, 0x00, 0x00])
            .await?;
        self.i2c
            .write(self.addr, &[REG_OUTPUT, self.out[0], self.out[1]])
            .await?;
        self.i2c
            .write(self.addr, &[REG_CONFIG, CONFIG_PORT0, CONFIG_PORT1])
            .await
    }

    /// Read both input ports in one transfer.
    pub async fn inputs(&mut self) -> Result<[u8; 2], I2C::Error> {
        let mut buf = [0u8; 2];
        self.i2c
            .write_read(self.addr, &[REG_INPUT], &mut buf)
            .await?;
        Ok(buf)
    }

    /// Drive one output bit on the given port.
    pub async fn set(&mut self, port: usize, mask: u8, high: bool) -> Result<(), I2C::Error> {
        if high {
            self.out[port] |= mask;
        } else {
            self.out[port] &= !mask;
        }
        self.i2c
            .write(self.addr, &[REG_OUTPUT, self.out[0], self.out[1]])
            .await
    }

    /// Take the TFT out of reset. Held low from `init` so the controller sees a
    /// clean edge once its supply has settled. CS is still high at this point.
    pub async fn release_display_reset(&mut self) -> Result<(), I2C::Error> {
        self.set(1, DISPLAY_RES, true).await
    }

    /// Assert the TFT's chip select, after [`release_display_reset`].
    ///
    /// The falling edge here is what synchronises the ST7789's serial
    /// interface; see [`DISPLAY_CS`]. Separate from the reset release so the
    /// two land in different I²C transactions and the edge is unambiguous.
    pub async fn select_display(&mut self) -> Result<(), I2C::Error> {
        self.set(1, DISPLAY_CS, false).await
    }
}
