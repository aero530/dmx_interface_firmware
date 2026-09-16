# DMX Interface Rev 2 — Board Bring-Up Checklist

Everything that has to be **verified on the bench** before a Rev 2 unit is trusted.
Only checks — the reasoning behind each lives in [REV2_ALTIUM_REVIEW.md](../../dmx_interface_dev_board_v2/docs/REV2_ALTIUM_REVIEW.md)
(board), [ARCHITECTURE.md](ARCHITECTURE.md) (firmware) and
[rev2-power-bringup.svg](../../dmx_interface_dev_board_v2/docs/rev2-power-bringup.svg) (sequencing). Work top
to bottom: each stage assumes the one before passed.

Reference designators are those of the **as-built Altium design**
(`dmx_interface_dev_board_v2/DMX Board/Project Outputs for DMX Interface DB v2/`,
netlist `DMX Interface DB v2.NET`, and `LED Power Board/Project Outputs/`).

Log everything with `DEFMT_LOG=debug cargo run --release` (probe-rs on the module's
SWD pads) until stage 6; the USB console (`dmx_console`) is enough afterwards.
Without a probe, flash with `cargo uf2` from the repo root and copy the resulting
`pico2.uf2` onto the module's BOOTSEL drive (hold BOOTSEL while plugging in its
USB-C) — you lose the defmt log but every stage from 3 on can be run from the
display and the USB console.

Legend: **Do** what to apply · **Expect** what a good board shows · **Fail →** what it means.

## Designator key

Logic board (`DMX Interface DB v2`):

| Function | Designators |
|---|---|
| Module, FTDI, USB | U1 W6300-EVB-Pico2 · U2 FT232RNL · U5 USBLC6 · J2 USB-C (FTDI) · JP1/JP2 TX/RX straps · R22/R23 CC · R24/R25 RESET# gate |
| Isolated DMX | U4 THVD1400 · U3 TX opto · U7 EN opto · U8 RX opto (TLP2368) · Q1 direction FET · R1 EN bias · R12 gate · R2 hold-down · R9/R13 LED drive · R7/R8 pull-ups · R10/R11 bias · PS1 1S7BE · J5 XLR in · J15 XLR thru · R3/R4/C1/PTH3 chassis bond |
| Power tree | J26 V_LED in · D1 TVS · F9 PTC → L2 (5 V) / U6 OKI (12/24 V) → C17 → D2 → V_SYS · U9 LDO (3V3) · JP3/R26/R31 LDO enable gate · L3 → 5V_LVL_SHIFT · R36 bleed |
| USB power | U14 TPS2553-1 · F2 2 A PTC · R37 ILIM · R38 EN pull-down · R35 FAULT pull-up · F1/D4 brick → 5V_LOGIC · D3/F18 module-USB dev feed · R39/R40, R41/R42 VBUS dividers |
| Outputs | U10 SN74ACT245 · R5 DIR strap · R21/R20/R16/R19/R15/R18/R6/R17 series (ports 1–8) · F10–F17 3 A PTC · C11/C18/C3/C24/C26/C28/C7/C6 470 µF · J1/J3/J13/J16/J20/J21/J24/J25 (ports 1–8) |
| UI | U12 TCA9555 (JP4 A0, R29/R30 A1/A2, TP1 INT) · U13 M24C02 (JP5 E0, R33/R34, R32 WC) · U11 PCA9633 · R27/R28 I²C pull-ups · J4 TFT · J6 buttons · J7 spares · J8 PWM · J9 3V3/GND · J14 RUN |

LED Power Board:

| Function | Designators |
|---|---|
| Input | J3 (V_LED, both positions) · J6 (GND, both positions) · D1 5.0SMDJ5.0A TVS · R1 10 k bleed |
| Outputs 1–8 | holders F1/F2/F5/F6/F10/F11/F15/F16 (Keystone 3544-2) with 5 A inserts F3/F4/F7/F8/F9/F12/F13/F14 · caps C1–C8 · headers P1–P4 (two outputs each) · plugs J1/J2/J4/J5/J7/J8/J9/J10 |

---

## Stage 0 — Before first power (both boards, no module fitted)

- ✓ **Visual**: every IC leaded and oriented; no bridges on the SSOP-28 (U2), TSSOP-24 (U12), TSSOP-20 (U10), TSSOP-8 (U11), SOIC-8 (U4, U13), SO-6 (U3, U7, U8), SOT-23 (U9, U14). Electrolytic polarity on the eight 470 µF output caps (C11, C18, C3, C24, C26, C28, C7, C6 — stripe = negative, away from the `+` silk) and on C17 (SP-Cap: polarity per the `+` silk).
- ✓ **Build variant stuffing** matches the intended rail voltage (one voltage per board):
  - 5 V: L2 fitted, U6 **not**; D1 = SMDJ5.0A; F10–F17 = 0ZCF0300BF2C; F2 fitted; C2 is a **50 V** part.
  - 12 V: U6 fitted, L2 **not**; D1 = SMDJ12A; F2 **not fitted** (U14 is a ±7 V part); C2 50 V.
  - 24 V: U6 fitted, L2 not; D1 = SMDJ26A; F10–F17 = the 24 V-variant PTC (≥ 30 V rating confirmed on the datasheet); F2 not fitted; C2 50 V.
- ✓ **USB power path**: U14 TPS2553-1 fitted; R37 = 18 k on ILIM (≈ 1.45 A); R35 100 k FAULT pull-up to 3V3; R38 100 k EN pull-down; D4 anode on `V_BUS_FTDI` through F1 (**never** on U14's output node `V_USB_LED`); present-detect dividers R39/R40 and R41/R42 = 10 k top / 20 k bottom.
- ✓ **Ohmmeter, unpowered**: `V_LED`–GND > 1 kΩ (D1 not shorted, no bridge across the plane; R36 reads 10 k); `5V_LOGIC`–GND and `3V3`–GND not shorted; `CHGND`–GND ≈ 0 Ω (R4 fitted); `3V3ISO`–GND **open** (isolation barrier intact); each XLR shell (J5, J15)–GND ≈ 0 Ω through PTH3/R4.
- ✓ **Power board**: `V_LED`–GND at J3/J6 reads only D1's leakage and R1 (10 k) with no fuse inserts fitted; each 3544-2 holder's fused pins (1/2) isolated from the V_LED plane with the insert out; polarity silk at J3/J6 unambiguous.
- ✓ **Protection present on the power board**: D1 (5.0SMDJ5.0A) fitted across J3/J6. There is **no main fuse on the board** — the 40–50 A MIDI/ANL fuse must be in the PSU harness, and the inter-board feed to main-board J26 must be fused in the harness too. Confirm both before power.

## Stage 1 — Power, no module (main board)

Do: main supply on at the build voltage, nothing else connected, **no module in the socket**.

- ✓ `V_LED` = supply voltage; `V_LOGIC_IN` ≈ `V_LED` (through F9); `5V_LOGIC` = 5.0 V ± 0.25 (5 V build: through L2; 12/24 V: U6 output) at C17.
- ✓ `V_SYS` ≈ 5V_LOGIC − 0.3…0.45 V (D2 B340A drop). 4.59V measured
- ✓ **3V3 = 0 V.** U9 is enable-gated by the module's 3V3_OUT through JP3/R26; with no module it must stay off. Fail → JP3 bridged 2-3 (bench override) or R26/R31 wrong.
- ✓ Idle current < 5 mA on the logic branch.
- ✓ `USB_LED_EN` (U14 pin 3 / R38) = 0 V; nothing on `V_USB_LED`.
- ✓ Reverse-polarity sanity (**power board only, current-limited supply ≤ 1 A**): reversed input forward-biases D1 and the supply limits, no cap venting. Skip on the main board (J26 is keyed).

## Stage 2 — Module fitted

Do: W6300-EVB-Pico2 in the socket (U1), supply on. Nothing on the strips or USB yet.

- ✓ Rise order on a scope: `3V3_OUT` (module, JP3 pin 1) up before carrier `3V3` (U9 output); carrier 3V3 ≥ 0.2 ms behind. Fail → sequencing gate not working; check JP3 bridged 1-2.
- ✓ `3V3` = 3.30 ± 0.05 V; `3V3ISO` = 2.97–3.63 V (PS1 output).
- [ ] Supply current 250–450 mA at 5 V with Ethernet linked (module + TFT backlight).
- ✓ Heartbeat: module user LED blinks once per second.
- [ ] Log shows `DMX interface starting on RP2350`, no `previous reset: WATCHDOG TIMEOUT`, `TCA9555` and `PCA9633` init without error, `TFT: up, 35x11 grid`.
- [ ] **D2 temperature** after 10 min: warm, not hot (< 40 °C rise) — it carries the full logic + TFT current. (D1 is the TVS and should be cold.)
- [ ] **VSYS monitor** log line ≈ 4.55–4.7 V (5 V build) and no low-VSYS warning.
- [ ] TCA9555 (U12) after init: P04 is an **output driven low** — `USB_LED_EN` (U14 pin 3) = 0 V; P05 = 0 and P06 = 0 with nothing on either USB port; `USB_LED_FLT` (U14 pin 4 / R35) = 3.3 V and P07 reads 1.

## Stage 3 — Display and front panel

- ✓ **Straps first**: JP4 bridged **1-2 (GND)** so U12 answers at 0x20 (A1/A2 are R29/R30 to GND); firmware probes 0x20 only. 2-3 would put it at 0x21 and the panel stays dark.
- ✓ Backlight (U11 LED0 → J4 pin 8) comes on **after** the menu is drawn (no visible power-on noise). J4 pin 8 averages ≈ 3.3 V × brightness/256 on a meter — **≈ 2.6 V at the default 200**. Reading ≈ 0.7 V instead means the PCA9633 is running with INVRT clear (sink polarity: on-time low) and the backlight is at 22 %; found on the first board 2026-09-15 and fixed in `pca9633.rs` (MODE2 = 0x14).
- ✓ **Backlight, menu and UI confirmed working** (2026-09-15, first board). Getting
  there took a day; the fault was mipidsi's `SWRESET`, written up below.
- ✓ **If a board comes up blank with the backlight on** — work down, stop at the
  first failure. Check the write-up below first: on this hardware the answer has
  already been found once, and `tft_ui::NoReset` is the guard against it coming back.
  1. Is the box resetting? Module LED must blink steadily at 1 Hz and the USB CDC ports must not re-enumerate; a ~4 s cycle is a panic → watchdog loop (with the probe, the log shows it). A build from before 2026-09-08 has a 32 KB heap that the display's two cell buffers alone overrun — rebuild.
  2. **No probe needed**: on the USB console, `info` prints `diag=` with every display step reached — `expander_ok tft_res_released tft_init_ok tft_splash_done backlight_ok terminal_ok first_frame_drawn` is a complete pass; a capitalised token (`TFT_INIT_FAIL`, `TERMINAL_FAIL`, `DRAW_ERROR`, `EXPANDER_FAIL`) names the failing step. Note the SPI bus is write-only: mipidsi's `init` **cannot** detect an absent or unresponsive panel, so `tft_init_ok` does not prove the panel heard anything — the splash does.
  2b. **Colour splash**: at boot the firmware fills the panel red, green, blue, then black (150 ms each) before the menu. No splash = SPI data is not reaching the panel or the controller is not what the firmware expects (steps 3–6). Splash in the wrong colours (cyan / magenta / yellow) = `ColorInversion` is wrong; swapped red/blue = colour order. Splash correct but no menu = the Ratatui/mousefood draw path (check `first_frame_drawn` vs `DRAW_ERROR` in `diag=`).
  3. J4 pin 7 (`DISPLAY.CS`, U12 P11) = **0 V** after boot and J4 pin 5 (`DISPLAY.RES`, P10) = 3.3 V. CS at 3.3 V means the expander is not driving it (init failed, or the TCA9555's internal pull-up is all that is there) and the panel ignores every byte.
  4. J4 pin 2 = 3.3 V at the panel; pin 1 GND. Header order at J4 is GND · 3V3 · SCK · MOSI · RES · DC · CS · BL — match it to the module's own silk (`GND VCC SCL SDA RES DC CS BLK` on the common 1.47" ST7789 boards); the order was provisional at design time.
  5. Scope J4 pins 3/4/6 during the first second: an SPI burst at `TFT_SPI_HZ` (5 MHz) with DC toggling. Clock rate has never been the fault here — do not spend time sweeping it. `DISPLAY.SCK` is a capacitive net and rounds off above ~10 MHz; that is why the constant sits at 5 MHz, and it is a known limit rather than a fault (ARCHITECTURE §11 has the pad-drive fix if it ever matters).
  6. Controller: the firmware talks ST7789 with a 172×320 window at offset 34. A different controller (ILI9341, ST7735, GC9A01) or glass size gives a blank or off-glass image with the backlight lit; read the module's silk or listing.

**Logic-analyser reference (2026-09-15, first board).** A capture on the panel's own
connector during boot, decoded, is the fastest way to exonerate the firmware. A good
board looks exactly like this:

| Observation | Good value |
|---|---|
| RES | one low pulse ≥ 10 µs, then ≥ 120 ms before any command |
| Init commands (DC low) | `01` SWRESET · `11` SLPOUT · `36` MADCTL=`60` · `21` INVON · `3A` COLMOD=`55` · `13` NORON · `29` DISPON |
| SCK | ≤ 15 MHz (ST7789 spec: 66 ns min write cycle) |
| Address window | `2A` CASET = 0..319, `2B` RASET = 34..205 — **correct**: MADCTL `60` sets MV=1, so the controller exchanges row/column addressing and the 34-pixel centring offset lands on RASET |
| Splash payload | `2C` RAMWR then `F8 00` ×55 040 = exactly 320×172 px; then `07 E0`, `00 1F`, `00 00` |
| BL duty | ~78 % high at the default brightness 200 (PCA9633 INVRT set) |
| CS | held low throughout — by design, the panel is the only SPI1 device |

If the capture matches that at the **panel's** connector and the panel has 3V3 and GND,
the firmware and the board are exonerated and the fault is the module: a dead unit, or a
pin order that differs from J4. Note that narrow (<50 ns) MOSI pulses in a capture are
probe artefacts, not data corruption — check the *byte histogram* instead: each splash
colour must appear exactly 55 040 times.

### Blank-panel investigation, 2026-09-15 — RESOLVED

**Root cause: mipidsi was sending `SWRESET` on top of the expander's hardware
reset, and this ST7789P3 does not recover from the combination.**

mipidsi's builder sends a software reset **only when no reset pin is supplied**;
given one, it toggles the pin instead. Rev 2 used `NoResetPin` — because the real
RES line is on the I²C expander and cannot be an `OutputPin` — so the panel got a
hardware pulse from the expander *and* a `SWRESET` from mipidsi. It then accepted
the entire init, took a full frame of correct pixels, and left the glass dark.
Nothing upstream can detect this: the SPI bus is write-only.

**Fix** (`tft_ui.rs`): hand mipidsi a no-op `NoReset` pin. It takes the
hardware-reset branch, issues no `SWRESET`, and the expander keeps doing the real
pulse. Firmware only — no board change.

Two things this is *not*, both tested and eliminated:

* **Not CS framing.** Rev 1 wrapped every transfer in a CS cycle via
  `ExclusiveDevice`; Rev 2 holds CS statically low on the expander. Probe variant
  B proved the panel is happy with static-low, so CS can stay where it is.
* **Not clock rate.** Swept 40 → 1 MHz with no change. Rev 1 ran this panel model
  at ~50 MHz actual. (Rev 2's SCK net *is* measurably capacitive — a sine, not a
  square, above ~10 MHz — which is why `TFT_SPI_HZ` sits at 5 MHz. Worth fixing
  properly with stronger pad drive, but it was never this bug.)

The bench probes built for this are kept: `cargo uf2 --bin panel_probe` is a
verbatim copy of the Rev 1 display path, and `--bin panel_probe_staticcs` is the
same with CS static, both for a module on a breadboard with the carrier removed.
`panel_test.rs` (disabled) puts a full-frame fill up with no driver stack at all.

### What was eliminated getting there

A full day was spent here. Everything below was **verified, not assumed**, and none
of it was the fault — it is recorded so the next person can skip it, not repeat it.
It is also a useful catalogue of *how* to check each one on this board.

| Eliminated | How |
|---|---|
| Reset loop / heap exhaustion | steady 1 Hz heartbeat, USB never re-enumerates; heap raised to 64 KB |
| Firmware not reaching the draw | console `info` → `diag=` shows every step incl. `first_frame_drawn` |
| I²C / expander / backlight | expander, EEPROM and PCA9633 all init clean; backlight PWM measured at 78 % |
| Backlight polarity | was inverted (PCA9633 sinks); fixed with MODE2 INVRT, confirmed on the scope |
| J4 pin order | module silk `GND VDD SCL SDA RES DC CS BL` maps 1:1 onto J4 |
| Panel power | 3V3 and GND present **at the module**; backlight draws real current through VDD |
| Signal presence | logic capture on the connector: SCK, MOSI, DC, RES, CS all live |
| Signal levels | SCK/MOSI scoped analog — full 0–3V3 swing at 5 MHz; RES, CS, DC likewise clean |
| Clock rate | swept 40 → 10 → 5 → 1 MHz, no change at any of them. (Rev 1 ran this panel model at ~50 MHz actual, so the panel is not the limit — but Rev 2's SCK net is measurably capacitive and rounds badly above ~10 MHz, which is worth fixing on its own.) |
| Init sequence | decoded off the wire: SWRESET, SLPOUT, MADCTL `60`, INVON, COLMOD `55`, NORON, DISPON — textbook |
| Address window | CASET 0..319 / RASET 34..205 — correct, because MADCTL `60` sets MV=1 |
| Pixel data | each splash colour exactly 55 040 px = 320×172, RAMWR byte count exact |
| CS timing | RES and CS were driven low together, so the panel left reset with CS already asserted and never saw a select edge. Fixed: CS now held high through reset (`tca9555::DISPLAY_CS`) |
| Init *content* | `panel_test.rs` sends the full register set mipidsi omits — porch, gate, **VCOM**, power, both gamma tables — with datasheet delays, then fills all 240×320 of frame RAM with no rotation or offset. Still blank |
| A dead unit | two panels from the same batch behave identically; a third from that batch worked on Rev 1 |
| The carrier board | both bench probes ran a module on a breadboard with the carrier removed — same blank panel, so nothing between the module and the glass was ever involved |

**The lesson, for the next one of these:** every item above was measured, and
none of them was the fault. What actually found it was reproducing the *known-good*
configuration verbatim — Rev 1's `nucleo/src/ui/mod.rs` — on bare hardware, then
reverting one difference at a time (that is what the two `panel_probe*` binaries
are). Do that first. Reasoning forward from the datasheet cost most of a day here,
and every measurement it suggested came back clean.

**J4 pin order is CONFIRMED CORRECT** (2026-09-15). Rev 1 drove the panel from flying
leads matched to the module silk, so Rev 2's fixed header was the first time the order
mattered and it had never been checked. It is right: the module (AliExpress 1.47"
172×320, ST7789P3) is silk-screened `GND VDD SCL SDA RES DC CS BL`, which maps
one-to-one onto J4 `1 GND · 2 3V3 · 3 SCK · 4 MOSI · 5 RES · 6 DC · 7 CS · 8 BL`
(SCL = clock → SCK, SDA = data → MOSI). **No board change needed; do not re-open this.**

Worth knowing for the *next* panel, though: a working backlight only proves pins 1, 2
and 8, and a swapped SCK/MOSI would give a perfectly decodable capture on the board's
net names while the panel receives nothing it can parse. So if a future module is
blank, compare its silk to the list above before suspecting anything else — a
different vendor's order (Waveshare-style `VCC GND DIN CLK CS DC RST BL`) will not work
in this header.
Remaining Stage 3 checks, now that the panel is lit:

- ✓ Orientation: landscape, "Main" top-left, `ETH …` status top-right — if mirrored or offset, adjust `Rotation`/`display_offset`/`ColorInversion` in `tft_ui.rs`.
- ✓ **Rounded corners**: the glass clips about one character at each corner of the top and bottom rows (found 2026-09-15). `tft_ui::CORNER_INSET` holds content one column clear of each side — **33 of 35 columns usable** — and mousefood centres the grid in the leftover pixels. A page whose value text would run off the right edge is caught by `values_fit_the_usable_width` in `host_tests` before it reaches a board.
- ✓ Colours: white text on black; REVERSED highlight readable. Inverted colours → toggle `ColorInversion`.
- ✓ Buttons (J6), confirmed 2026-09-15. Wiring is as drawn: J6 pin order 1 Down, 2 Up, 3 Select, 4 Esc, 5 GND → U12 P00–P03, and no board change was needed. Esc changes page; Select enters edit and then walks the digits, committing past the last one.
- ✓ **Up/Down are inverted while navigating and upright while editing** — deliberate, chosen on the panel itself. Moving through a list scrolls the list under a fixed highlight, so Down reaches the entries below and the highlight travels up the page; a digit under the cursor still goes up on Up, because there the button acts on the value rather than on the view. Both arms of the `match` in `tft_ui.rs` carry a note saying so — the asymmetry looks like a bug to anyone reading it cold, and “fixing” one to match the other would undo a bring-up decision.
- The logical order lives in the `BUTTONS` table in `pico2/src/buttons.rs`, so a future panel that reads wrong is a one-line change there rather than a board respin.
- **No `INT` check needed**: the expander is polled every 20 ms, not interrupt-driven — see the module comment in `buttons.rs`. That costs ~1 % of the I²C bus and removes the classic expander failure, an interrupt latch that never re-arms. Chords fall out of it too: each poll reads all four bits independently, so simultaneous presses cannot mask each other. Nothing in the UI acts on a chord today; the input layer simply does not prevent one.
- ✓ Digit edit on `Static IP`: the cursor sits on digits, skips the dots.
- ✓ Backlight setting (System page) changes brightness live. Level 1 is the dimmest *usable* duty, not the dimmest possible: the menu range is rescaled through `common::ui::backlight_duty`, because raw duty 1 (0.4 %) is not readable on this panel and duty 2 is. Above the floor the rescale is within one count of the old straight-through mapping, so saved settings look unchanged.
- ✓ Full-screen redraw (page change) does not disturb a running LED pattern or DMX reception (core-1 jitter check).

## Stage 4 — EEPROM and provisioning

- ✓ **Strap**: JP5 bridged **1-2 (GND)** so E0 = 0 → U13 M24C02 at 0x56 (E1/E2 are R33/R34 to 3V3; WC is R32 to GND, so writes are enabled). 2-3 gives 0x57 and every read reports `no chip`.
- ✓ Fresh EEPROM boot log: `EEPROM: blank, settings default`, `previous boot Failed`, `1 consecutive incomplete boot(s)` — and **Ethernet still comes up** (guard needs two).
- ✓ Second boot: `previous boot Success`, counter cleared, no guard message.
- ✓ Change a setting on the panel, power-cycle: it persists. Change one over the console (`set dmx_address 7`): persists, and the panel updates without a reboot.
- ✓ Console `mac 02:44:4d:58:xx:xx` → `ok`; reboot → `info` shows it and the log says `EEPROM: MAC …`, not `using fallback MAC`. **`02:44:4d:58` is a convention, not a constraint** — all six octets are settable. `parse_mac` enforces only: six hex octets, not all-zero or all-FF, and the low bit of the first octet clear (a set low bit is a multicast address, invalid as a source). Keep bit 1 of the first octet set — the `02` — so the address is *locally administered*: this project owns no OUI, and that bit is what says so.
- ✓ `mac clear` → reverts to the built-in address derived from the RP2350 chip ID; reboot → the log says `EEPROM: MAC unprogrammed` and `using fallback MAC`, and `info` shows an address ending in the chip ID. Added 2026-09-16, because until then there was no way back: `read_mac` falls back only on all-FF, all-zero or multicast bytes, and those are exactly the three values `parse_mac` refuses to write — so a unit provisioned once could only be reverted by rewriting the EEPROM off-board.
- ✓ `provision` → `ok`; reboot log shows module type read and **schema 4**. Until this is run, `info` reports `module=Some(Unknown)` and a fallback MAC — both expected on a fresh board. `Some(Unknown)` means the EEPROM answered but the byte is blank; `None` would mean the chip did not answer at all.
- ✓ **Priming writes removed** (2026-09-16). There were **four**, not two — `MODULE_TYPE_ADDR`, `MAC_ADDR_LOC`, `SETTINGS_ADDR` and `BOOT_FLAG_ADDR` — each a throw-away byte write carried over from Rev 1 to work around "page write only works if you write a byte to the device first". Saves persist across power cycles without them, and the mechanism they guarded against cannot occur in this build: `write_page` sends address and data in one `i2c.write()`, and the shared bus holds its mutex for the whole call, so the 20 ms expander poll on core 1 cannot interleave between them. See the note at the top of `eeprom.rs` before re-adding one.
- ✓ Pull U13's SDA (or lift U13) and boot: output **still renders** after the `flag could not be stored` error (local-authority fallback), display shows the menu with defaults.

## Stage 5 — Ethernet

- ✓ DHCP (confirmed 2026-09-15): `ETH dhcp…` then the address on the title row; `info` → `net=Up(…)`. Link LED on the module RJ45. **Stuck on `ETH dhcp…` with `ip=0.0.0.0`** is two different faults — check `diag=` for `eth_link_up` before suspecting DHCP, because without it the PHY never negotiated and nothing was ever asked for.
- ✓ Static: Network page → `IP Mode = Static`, set `Static IP`/`Prefix`; reboot; the title row shows that address, a PC on the same subnet pings it.
- ✓ **Ping**, static and DHCP. Needs the `auto-icmp-echo-reply` feature on `embassy-net`: smoltcp 0.13 moved the automatic echo reply behind it, and it sits in smoltcp's *default* set which embassy-net disables, so it has to be asked for. Added 2026-09-16 — before that the node answered ARP, held a lease and passed Art-Net while ignoring every ping. If ping fails again, check `arp -a` on the PC first: an entry with the node's MAC means ARP works and the fault is ICMP (this feature); an incomplete entry means the frames are not arriving at all.
- ✓ Boot guard: pull power twice during the first second after power-on; third boot shows `ETH guard` and the log's guard message; a clean boot afterwards restores Ethernet.
- ✓ **W6300 diagnostic** (exercised for real, 2026-09-15 — it caught a shifted version byte, a missing cable and a stolen waker): with the RJ45 unplugged nothing changes; with the chip genuinely dead the title row shows `ETH no chip` — not a hang.

  **`ETH no chip` — read `info` before touching anything.** The title row collapses
  several different faults; `diag=` on the USB console separates them, with no probe:

  | `diag=` | Means |
  |---|---|
  | `eth_ok eth_ver=0x11 eth_hz=20000k eth_max=24000k` | Healthy. `eth_hz` is the operating rate, `eth_max` the measured ceiling; they differ by design |
  | `eth_ok … eth_hz=` **below 20000k** | The ceiling came in under the operating cap, so the link is slower than §6's budget assumes. Not a fault, but check `eth_max=` |
  | `eth_link_up` but no `eth_configured` after ~10 s | The PHY negotiated and the lease did not arrive. DHCP server, VLAN or firewall — not the board |
  | `eth_max=` falling over a board's life | The transport is degrading. Nothing else reports this |
  | `eth_ok …` but no `eth_link_up` | Chip fine, PHY never negotiated. Cable, switch or magjack — DHCP was never attempted |
  | `eth_link_up` without `eth_configured` | PHY fine, the lease failed. DHCP server, VLAN or a firewall — not the board |
  | `ETH_LINK_DROPPED` | The link came up and went away again. Intermittent cable or a switch renegotiating |
  | `ETH_NO_REPLY` | The SPI transaction itself errored. Chip dead, unpowered, or the PIO SPI is misconfigured |
  | `ETH_BAD_VERSION eth_ver=0x00` | Every rate read all zeros — MISO (GP19) is never sampled |
  | `ETH_BAD_VERSION eth_ver=0xff` | MISO floating or stuck high — nothing driving it |
  | `ETH_BAD_VERSION` with any other value | A wrong byte even at 2 MHz, where there is no timing margin left to blame. The module, the wiring, or the read frame — not the clock |

  **The clock is probed at boot, not fixed.** `w6300::PROBE_FREQS_HZ` walks 20 →
  2 MHz on the first power-up of the link — stepped at 1 MHz between 16 and 12,
  where the boundary sits — and keeps the fastest rate that reads `CIDR2` as
  `0x11` **16 times running**. The repeat matters: this is a setup-time
  violation, and right at the boundary it stops being deterministic, so a
  marginal rate can pass a single read and then corrupt traffic under load.
  The whole sweep costs a few hundred microseconds, once.

  **A rate that passes the probe is not yet a rate you can trust.** The probe
  reads one register in a quiet system; Stage 7's flood is what validates it
  with the LED render, DMX and the UI all competing. If `eth_hz=` lands at the
  very top of the passing range, treat the flood result as the deciding
  evidence, not the probe.

  It exists because of what the first board did (2026-09-15): `ETH no chip` with
  `eth_ver=0x08`, which is exactly `0x11 >> 1`. A one-bit right shift means every
  sample caught the *previous* bit — the PIO SPI latches MISO at the rising edge
  through the RP2350's two-flop input synchroniser (~13 ns), the W6300 presents
  each bit on the falling edge, and at 20 MHz the half period is only 25 ns. The
  chip was never dead; the bus was a few nanoseconds too fast. **A shifted version
  byte is the signature to recognise** — `0x08`, `0x22`, `0x88` are all `0x11`
  displaced by a bit — and it is now self-correcting rather than something to
  diagnose.

  `NoChip` is only ever reached by actually running the bring-up, so it never
  means "skipped": a disabled setting shows `ETH off` and the lockout guard
  shows `ETH guard`. The carrier does not touch GP15–22 (U1 pins 20–29 are
  single-node nets in the Altium netlist, confirmed 2026-09-15), so the W6300
  bus is entirely on the module — a fault here is the module or the firmware,
  never the carrier.
- ✓ **Art-Net discovery** — `python tools/artpoll.py` from the repo root does exactly this check and needs nothing installed. It broadcasts an ArtPoll, decodes every ArtPollReply, and asserts the properties that matter: BindIndex contiguous from 1, no reply claiming more than 4 ports, one `BindIp` across the set, no universe bound twice. `--target <ip>` unicasts the poll if broadcast is awkward — useful, but check the node's current address first: a DHCP lease can move it, and polling the old one just times out. Expect the node once per 4 bound universes (BindIndex 1…N), and `Univ Bound` on the Main page equal to the total. Change `LEDs Port 1` from 150 to 600 → bound count and reply count rise together.

  **✓ Discovery confirmed 2026-09-16.** 5 replies, BindIndex 1–5, 17 universes from base `0:0:1`, one `BindIp` throughout, no universe bound twice:

  | Bind | Ports | Universes |
  |---|---|---|
  | 1 | 4 | 0:0:1 … 0:0:4 |
  | 2 | 4 | 0:0:5 … 0:0:8 |
  | 3 | 4 | 0:0:9 … 0:0:12 |
  | 4 | **3** | 0:0:13 … 0:0:15 |
  | 5 | 2 | 0:1:0, 0:1:1 |

  **The 3-port bind is correct, not a short count.** An ArtPollReply covers ports within *one* sub-net, and sub-net 0 ends at universe 15 — so the span is cut there and resumes at `0:1:0` in the next reply. That is `ports = remaining.min(16 - first).min(4)` in `artnet.rs` doing its job, and it is the boundary case the "span crossing a sub-net boundary" check below exercises. A controller that patched 0:0:13–0:0:16 into one bind would be the bug.

  **`Univ Bound` on the Main page reads 17 to match, and tracks the LED-port settings as they change** (confirmed 2026-09-16). Discovery is done.

  **Close other Art-Net software first.** ArtNetominator, QLC+ and the rest hold UDP 6454 as well. The tool sets `SO_REUSEADDR` so it still binds, but Windows delivers each *unicast* datagram to only one listener — and the node's reply is unicast, back to whoever polled — so another app can silently swallow it and the tool reports nothing found. For a GUI view, **ArtNetominator** (free, Windows) does discovery and can send test DMX; it is the lighter option if QLC+ is more than the job needs.
- [ ] Art-Net data: controller drives universe `net:sub:uni` → port 1 (J1) follows; universe +1 → port 2 (J3) (Individual mode, ≤ 170 RGB LEDs/port). A span crossing a sub-net boundary (base 0:0:14, 4-universe port) renders correctly.
- [ ] Other-Net traffic (controller on Net 1, node on Net 0): ignored, one `ArtNet: ignoring traffic on Net` line, no storm.
- [ ] **sACN**: mode `sACN`, `sACN Universe = 1`, controller on universe 1 → port 1; change the base to 100 → log shows the re-join with **32 of 32 groups joined** (fewer means smoltcp's multicast table is too small), universe 100 → port 1.
- [ ] **Throughput ceiling** (Phase 0 measurement — record the number): first read `eth_hz=` from `info`, which is the clock the boot probe settled on and the hard ceiling on everything below it. Then flood 32 universes at 44 Hz and watch for dropped frames / `ArtNet socket receive error`. If `eth_hz` came back under 20000k, raising the top of `w6300::PROBE_FREQS_HZ` will not help — the limit is sample timing, and the fix is a PIO SPI that samples mid-bit rather than at the rising edge.

## Stage 6 — Wired DMX

- [ ] Receive: console into **J5** (male XLR, "Input"), DMX mode → `dmx 1 16` on the console tracks the faders; log `DMX: receiving`. Unplug → `DMX: no data` within 1 s; replug → `signal restored`.
- [ ] Alternate start codes (RDM traffic from the console) do not disturb channel values.
- [ ] Scope GP9 (U1 pin 12, → R9 → U3) and GP10 (U1 pin 14, → R12 → Q1) in `USB>DMX` with QLC+ on the FTDI port: GP10 **high** while transmitting (Q1 on, U7 LED off), low in every other mode; DMX out on **J15** (female XLR, "Output") drives a fixture; BREAK 176 µs, MAB 16 µs, ~43 packets/s.
- [ ] `ArtNet>DMX`: the configured Art-Net universe appears on J15.
- [ ] Undriven state: with the module removed, U4's driver is **disabled** (fail-safe receive: R1 biases the U7 LED on, DE/RE pulled low through the opto).
- [ ] Isolation: `3V3ISO` to `GND` still open with the bus connected; no ground current through the XLR shield (CHGND → R4 → GND only).
- [ ] **Short-frame consoles**: with a console configured for fewer than 512 slots (e.g. 24), press buttons on the TFT while a fixture above the console's slot count is patched. Any flicker there is the known short-frame/redraw interaction (ARCHITECTURE §11), not a wiring fault; a 512-slot console must show none.

## Stage 7 — LED outputs

- [ ] Boot: all eight strings dark (blank frame), no flash of stale colour at power-up.
- [ ] Scope one data line (J1 pin 1, after R21): 800 kHz bit rate, 1.25 µs/bit, **≥ 150 µs** idle between frames; frame time ≈ 30 µs × LED count (18 ms at 600).
- [ ] RGB (WS2812/WS2815 strip): DMX channels 1-3 = R,G,B of LED 1 — **colour order correct** (GRB on the wire).
- [ ] **RGBW (SK6812 strip)**: `Color Mode = RGBW`, channels 1-4 → R,G,B,W of LED 1; white channel lights the white die only; a WS2812 strip in RGBW mode shows the expected garbage (proves the mode switch reaches the wire).
- [ ] Mixed length: `LEDs Port 1 = 10` — the strip beyond LED 10 keeps its last latched value only until the next blank; frame rate rises (shorter DMA).
- [ ] Group size 3 on a port: three physical LEDs per DMX pixel. Mirror mode: all ports identical.
- [ ] 600 LEDs on all 8 ports at 44 Hz for 10 min: no dropped frames, U10 cool, `5V_LVL_SHIFT` (after L3, at C41) = 5.0 V.
- [ ] Per-port protection: short a strip's V+ to GND through a 5 A load → that port's PTC (F10–F17) trips, and on a power-board-fed strip the blade insert (F3/F4/F7/F8/F9/F12/F13/F14) opens, with no effect on other ports; the PTC recovers when cleared.

### LED Power = USB brick (5 V build only)

- [ ] Menu **System → LED Power** offers `External` / `USB brick`; console `set led_power usb` and `get` agree with the display.
- [ ] With `External`: `USB_LED_EN` (U14 pin 3) = 0 V whatever is plugged into J2. This is the safe default and a PC must never be loaded.
- [ ] With `USB brick` **and no brick attached**: EN stays low (the firmware also requires VBUS present on P05, via R39/R40).
- [ ] With `USB brick` and a 5 V brick on J2: EN goes high, `V_LED` ≈ 4.9 V through U14 → F2, title row shows `PWR usb`.
- [ ] **Budget**: drive all eight ports to full white. The frame is scaled so the draw stays near 1.3 A — measure it in the brick lead; U14 must not latch. Note the budget only covers ~21 full-white RGB pixels unscaled, so heavy content will visibly dim. That is the intended behaviour.
- [ ] **Fault latch**: short an output briefly (current-limited bench supply). `FAULT` (U14 pin 4) pulls low, firmware logs `USB LED power: FAULT`, EN drops, and it retries ~2 s later with `PWR flt` on the title row meanwhile. After three failures it logs `staying off until LED Power is changed`, and toggling the setting clears it.
- [ ] Unplug the brick while enabled: EN drops (logged as `brick removed`, not as a fault), no glitch on `V_SYS`, logic keeps running from D4/F1 until the PSU path takes over.

## Stage 8 — USB

- [ ] **Identity**: `info` on the console shows a MAC of `02:44:4D:xx:xx:xx` when none is programmed, and the USB serial (Device Manager / `lsusb -v`) is 16 hex digits — **different on every unit**. Two units on one PC must enumerate as two COM ports.
- [ ] **FTDI straps**: JP1 and JP2 both bridged **1-2** = straight (U2 TXD ← `FTDI.TX` net → GP13; U2 RXD ← `FTDI.RX` net ← GP28). 2-3 on both swaps TX/RX if a board is wired the other way round — never mix.
- [ ] Module USB-C on a PC: two (three with the `usb` feature) CDC ports enumerate; port order = Enttec widget, console (, logger). `dmx_console` connects on the second.
- [ ] Enttec widget over CDC: QLC+ **cannot** see it (expected — FTDI-only stack); xLights/OLA `usbpro` on the serial port can.
- [ ] **FTDI port (J2)**: after programming U2 per [ft232rnl-eeprom.md](ft232rnl-eeprom.md), Windows shows `ENTTEC` / `DMX USB PRO`; QLC+ lists a DMX USB Pro; log `FTDI widget: locked at 250000 baud`; faders drive the LEDs in `USB>DMX` mode.
- [ ] With the box **unpowered**, plugging J2 into a PC does nothing (U2 `RESET#` follows VBUS through R24/R25 and U2 is self-powered from 3V3: no enumeration, no back-drive). With the box powered, it enumerates.
- [ ] Baud hunting: open the port at 57600 with a Python Enttec script → `locked at 57600 baud`.
- [ ] **Module USB only** (PSU off, nothing on J2): `V_SYS` ≈ 4.4–4.7 V, `V_LED` ≈ 4.3–4.6 V through D3/F18, P06 = 1 (R41/R42), P05 = 0, strips stay dark (F18 is 0.5 A — firmware must not drive them). Fail → strips lit or F18 cycling = the brightness budget / power-mode logic is not honouring the source.
- [ ] **Brick on J2 only** (5 V, ≥ 2 A, no PD, 3 A-rated cable), `LED Power` still `External`: `USB_LED_EN` = 0 V (R38), `V_LED` = 0 V (U14 off — a disabled USB power switch passes nothing), `5V_LOGIC` ≈ 4.65 V via F1/D4, P05 = 1 (R39/R40), FAULT high, the unit boots and the display works. Fail → EN high with the setting off = R38 missing / P04 driven high; `V_LED` at ≈ 4.3 V with EN low = something with a body diode was fitted in U14's place.
- [ ] Set `LED Power = USB brick`: `USB_LED_EN` = 3.3 V (P04 driven high), `V_LED` ramps up (U14 soft start) to ≈ 4.9 V, drop across U14 ≤ 150 mV at 1.3 A full-white on two short strips, FAULT stays high, U14 under 40 °C rise; F2 never trips. Push the load to ≈ 1.6 A: U14 latches off within 10 ms and FAULT goes low (P07 = 0); firmware backs off and re-arms as in stage 7. Then plug the PSU in (trimmed ≥ 5.2 V) while the brick runs: U14 opens once `V_LED` is ≈ 135 mV above the brick (FAULT low, `V_LED` follows the PSU, no `V_SYS` glitch on the scope); unplug the brick: nothing changes.
- [ ] **PC on J2** with the PSU off: enumerates as DMX USB Pro, logic runs from D4 (≈ 0.35 A from the port), `V_LED` = 0 V, strips dark. Note: P05 is 1 for a PC and a brick alike — the board cannot tell them apart, so the `USB brick` setting is the user's declaration; the manual must say a PC port is not a brick.

### Carried over from the design record

- [ ] **The core-split claim**: saturate core 0 with Art-Net while editing on the TFT — the UI must stay responsive. This is the specific claim the two-core architecture rests on.
- [ ] **8 strings × 600 LEDs at 44 Hz sustained**; scope one WS2812 line and confirm the ~18 ms frame.
- [ ] **PS1 input current**: measure the actual 3V3 draw into PS1 pin 1 (through L1). The 90 mA in the power budget is derived from its output rating and an assumed efficiency, not from a datasheet.
- [ ] **W6300 diagnostics**: version register reads correctly, and the three failure states (chip dead / link down / no DHCP) are distinguishable in the log — the only diagnostic path left for the undiagnosed Rev 1 Ethernet failure.
- [ ] **Buttons**: chord detection (two at once); a quick tap registers at the 20 ms poll; no phantom presses while the EEPROM task is writing on the shared bus.
- [ ] **Power transients**: scope `V_SYS` (J9 is a GND/3V3 probe point; V_SYS at C38/C45) and `3V3` through power-on, power-off and a full-brightness LED step, with and without USB attached. This is the behaviour that may have been killing Nucleos, so it gets measured rather than assumed.

## Stage 9 — Robustness

- [ ] Watchdog: no `WATCHDOG TIMEOUT` reset across 1 h of normal operation (Art-Net + TFT edits + EEPROM saves).
- [ ] Forced fault: hold core 1 (breakpoint in `buttons::run` with `pause_on_debug` temporarily false) → reset within 4 s and `previous reset: WATCHDOG TIMEOUT` in the next boot log.
- [ ] Brown-out: dip the supply to 3.5 V for 100 ms → clean reboot, settings intact, no corrupted EEPROM page.
- [ ] Hot-plug a strip while running: no reset, other ports unaffected.
- [ ] 8 h soak at full load: no reset, no drift in `V_SYS`; D2, U9 and U10 temperatures stable (D1 stays cold).

## Stage 10 — Power board at load

- [ ] 40 A through J3/J6 (both screws per pole wired): input band and GND return < 20 °C rise after 30 min (2 oz copper check).
- [ ] Each output at 5 A: holder (F1/F2/F5/F6/F10/F11/F15/F16), cap (C1–C8) and header (P1–P4) cool; the 5 A insert (F3/F4/F7/F8/F9/F12/F13/F14) opens at a 10 A overload.
- [ ] Plug orientation: each 2-position plug (J1/J2/J4/J5/J7/J8/J9/J10) seats in its own half of the 4-position header — a plug shifted one position onto pins 2–3 sees **GND, +** (reversed). Mark the headers or fit 2-position headers before shipping.
- [ ] Hold-up: after power-off the caps bleed down through R1 (10 k) — no spark on re-plugging a pigtail.
- [ ] Inter-board feed to J26: 4 × 18 AWG, fused in the harness, keyed plug seated; main-board `V_LED` within 0.15 V of the power board's.

---

**Record when done:** measured supply currents (stages 2, 7), the Art-Net throughput ceiling and `SPI_FREQ_HZ` chosen (stage 5), TFT orientation/inversion settings that proved right (stage 3), whether the EEPROM priming writes were removed (stage 4).
