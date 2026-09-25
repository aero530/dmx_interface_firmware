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
- [ ] **VSYS monitor / brownout margin** — **measured 2026-09-21 on J26 alone, and the margin is thin.**

  | Load | `V_LED` | VSYS | Margin to the 3.6 V brownout |
  |---|---|---|---|
  | none | 4.97 V | **4.28 V** | 680 mV |
  | 2 strips | 4.60 V | **3.90 V** | **300 mV** |
  | 3 strips | 4.25 V | ~3.55 V (extrapolated) | **at or below** |

  The two clean readings differ by 690 and 700 mV, which is the two series Schottkys (R6-P1 diode,
  then D2) behaving exactly as expected. An earlier set showing VSYS *above* `V_LED` was taken with
  the module USB connected: the diode-OR (ARCHITECTURE §7) fed VSYS from the module’s own internal
  Schottky at ~4.80 V and hid the sag completely. **Always confirm `pwr=` reads `ftdi_vbus=0
  rpi_vbus=0` before trusting a VSYS number.**

  - **This is the risk ARCHITECTURE §7 item 3 names.** The module regulator is a buck: below ~3.5 V
    it passes its input straight through and IOVDD follows, so logic and DMX brown out together at
    VSYS ≈ 3.6 V. Two strips leaves 300 mV. Eight ports at full white is not survivable on this
    supply as it stands.
  - **R6-P1 cost ~350 mV of that margin** and this is where it lands. The fix was right - it closes a
    real hazard - but "VSYS ~4.3 V, far above the module buck’s floor" was judged against an idle
    rail, not against the rail sagging under LED load, which is the case that matters. Recorded so
    the trade is visible rather than implied.
  - **`VSYS_LOW_MV` (4400) now fires at idle.** Do **not** raise or lower it yet: at these voltages
    the board really is operating near its floor, so the warning is telling the truth and silencing
    it would hide the problem. Re-set it only once the rail is fixed, and from a measurement.

  **Fix the rail first, in this order — the first two are free:**

  1. **Trim the J26 PSU up.** 4.97 V no-load is *below* the ≥ 5.1 V the design review §87 asks for.
     Every millivolt here reaches VSYS one-for-one. Trimming to ~5.2 V buys back most of what R6-P1 cost.
  2. **Check the J26 harness.** 720 mV of droop across three strips on the `V_LED` plane is
     resistance, not regulation. Review §62 specifies **4 × 18 AWG minimum** for the V_LED and GND
     groups, and §§57–62 note the harness was never drawn and is unfused. A thin bench pigtail would
     produce exactly this. **Record where each probe sat** — plane (C17 / F9 input) or after a port
     PTC (F10–F17) — because only a plane measurement implicates the harness.
  3. Only if 1 and 2 leave it short: replace the R6-P1 Schottky with an ideal diode (P-FET +
     controller, ~20 mV instead of ~350 mV). The review already specs an LTC4412 + P-FET for the
     other path in §90, so the approach and the parts are known.

  **Then re-measure** at idle, 2, 3 and all 8 ports, and check for watchdog resets at 3 ports — the
  extrapolation puts that case at the brownout threshold, and a reset there would confirm it.

- [ ] TCA9555 (U12) after init: P04 is an **output driven low** — `USB_LED_EN` (U14 pin 3) = 0 V; P05 = 0 and P06 = 0 with nothing on either USB port; `USB_LED_FLT` (U14 pin 4 / R35) = 3.3 V and P07 reads 1.

## Stage 3 — Display and front panel

- ✓ **Straps first**: JP4 bridged **1-2 (GND)** so U12 answers at 0x20 (A1/A2 are R29/R30 to GND); firmware probes 0x20 only. 2-3 would put it at 0x21 and the panel stays dark.
- ✓ Backlight (U11 LED0 → J4 pin 8) comes on **after** the menu is drawn (no visible power-on noise). J4 pin 8 averages ≈ 3.3 V × brightness/256 on a meter — **≈ 2.6 V at the default 200**. Reading ≈ 0.7 V instead means the PCA9633 is running with INVRT clear (sink polarity: on-time low) and the backlight is at 22 %; found on the first board 2026-09-15 and fixed in `pca9633.rs` (MODE2 = 0x14).
- [ ] **Visual refresh (2026-09-25), to look at on the glass.** Built to RGB565-quantized mockups; the
  panel's own gamma is the one thing the mockups could not model.
  - The selected row is a dark blue bar (`0x18,0x30,0x50`). Check it is visible at backlight 1, the
    dimmest usable level; if it is not, the fix is the constant `SELECTED_BG` in `tft_ui.rs`.
  - Splash: the wordmark centred, the flag's six stripes in order red, orange, yellow, green, blue,
    violet, version bottom-left, `ETH dhcp...` bottom-right turning to the IP if the link comes up
    inside the 1.5 s. A button ends it early. The flag's green and violet are the dimmest colours
    on the panel — confirm they read as green and violet and not as black.
  - Activity lamp: the disc left of the IP goes blue within a second of frames arriving and grey
    within a second of them stopping, in **every** input mode — wired DMX on J5, Art-Net, sACN and
    `USB>DMX` from `tools/enttec.py`. It is set from the router's DMX event, so a mode whose input
    never posts one would show grey with a live fixture.
  - Layout page under `Fixtures`: each live port draws as a strip, fixtures alternating blue and
    green, gaps dark, to scale against the longest port; a 1 px black gap between rows and between
    fixtures. Selecting a strip row highlights only its label. The LED page shows the universe
    budget bar under `Universes`.
  - Footer on every page (`Up Dn move   Sel edit  Esc page`); `Sel open` on a port row under
    `Fixtures`, `Esc back` inside a table, and the edit wording while editing. Nothing on any page
    is hidden behind it (Layout, the fullest, has nine rows; System's power note ends on row 7).
  - Idle screen after 30 s without a button: digits match `DMX Address`, one bar per port to the
    same scale as the Layout strips, the lamp follows the input, the IP and power note match the
    title bar. Any button returns to the menu on the same row, redrawn cleanly (no remnants of the
    digits).
- ✓ **Backlight, menu and UI confirmed working** (2026-09-15, first board). Getting
  there took a day; the fault was mipidsi's `SWRESET`, written up below.
- ✓ **If a board comes up blank with the backlight on** — work down, stop at the
  first failure. Check the write-up below first: on this hardware the answer has
  already been found once, and `tft_ui::NoReset` is the guard against it coming back.
  1. Is the box resetting? Module LED must blink steadily at 1 Hz and the USB CDC ports must not re-enumerate; a ~4 s cycle is a panic → watchdog loop (with the probe, the log shows it). A build from before 2026-09-08 has a 32 KB heap that the display's two cell buffers alone overrun — rebuild.
  2. **No probe needed**: on the USB console, `info` prints `diag=` with every display step reached — `expander_ok tft_res_released tft_init_ok tft_splash_done backlight_ok terminal_ok first_frame_drawn` is a complete pass; a capitalised token (`TFT_INIT_FAIL`, `TERMINAL_FAIL`, `DRAW_ERROR`, `EXPANDER_FAIL`) names the failing step. Note the SPI bus is write-only: mipidsi's `init` **cannot** detect an absent or unresponsive panel, so `tft_init_ok` does not prove the panel heard anything — the splash does.
  2b. **Boot splash** (was a red/green/blue fill until 2026-09-25; now the `DMX INTERFACE` wordmark over a six-stripe flag, 1.5 s): the same three answers come from it — no wordmark, wrong colours in the flag, or a wordmark but no menu. No splash = SPI data is not reaching the panel or the controller is not what the firmware expects (steps 3–6). Splash in the wrong colours (cyan / magenta / yellow) = `ColorInversion` is wrong; swapped red/blue = colour order. Splash correct but no menu = the Ratatui/mousefood draw path (check `first_frame_drawn` vs `DRAW_ERROR` in `diag=`).
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

**Ports start on universe boundaries.** A port's first universe is the base plus the universes every *earlier* port consumes, and each port rounds up on its own (`universe_offset` / `universes_per_port` in `common/src/ui/types.rs`): `ceil(LEDs × bytes_per_LED / 512)`. With every port set to the strip it actually has — 64 RGB LEDs, 192 B, one universe each — the ports are consecutive from the base. Set `LEDs Port 1` to 600 instead and it claims 4 universes, so port 2 moves to base + 4. Worth knowing before deciding a port is dead: check the LED page before the wiring. Art-Net and sACN allocate identically.

**Bench configuration these commands assume** (2026-09-16): **2 universes bound at base `0:0:1`**, i.e. `0:0:1` on port 1 and `0:0:2` on port 2, with **64 RGB LEDs per universe** (192 of 512 channels used). Pass `--leds 64` to `dmxsend.py` for anything with a visible pattern — its default walks all 170 pixels a universe can hold, so on a 64-LED strip the chase would spend most of its time past the end and the output would look dead.

A rig with different strip lengths or a different bound count only changes `--count`, `--leds` and the base address; nothing below depends on the numbers being these.

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
- ✓ **Art-Net data** (confirmed 2026-09-16) — `python tools/dmxsend.py --target <node ip> --universe 0:0:1 --count 2 --leds 64 --pattern chase`. A white pixel walks each strip, starting 16 LEDs apart between the two so the ports are told apart at a glance. Port 1 (J1) follows `0:0:1`, port 2 (J3) follows `0:0:2`.
  - Use `--target` rather than broadcast: that is what a real controller does once it has seen an ArtPollReply, and it is the path that matters.
  - `--pattern solid --color 255,0,0 --leds 64` is the blunt version if a chase is hard to see — it lights exactly 64 pixels, so an LED 65 coming on means the port is configured for more than is connected.
  - **Sub-net boundary**: set the node's Art-Net base to `0:0:15` and its 2 universes then span `0:0:15` and `0:1:0` — the boundary falls between them. Drive it with `--universe 0:0:15 --count 2 --leds 64`. Both must render. This is the case `artpoll.py` already showed the node *advertising* correctly (a bind cut short at universe 15); this checks it also *renders* correctly. Put the base back to `0:0:1` afterwards.
- ✓ **Other-Net traffic** (confirmed 2026-09-16) — with the node on Net 0, run `python tools/dmxsend.py --target <node ip> --universe 1:0:1 --leds 64 --pattern solid`. The output must not change and the log must carry **one** `ArtNet: ignoring traffic on Net 1 (configured 0)` line, not one per packet at 44 Hz. The latch that guarantees that is `warned_other_net` in `artnet.rs`; it re-arms on a mode or address change, so switching pages and back should let it fire once more.
- ✓ **sACN** — set mode `sACN` and `sACN Universe = 1`, then `python tools/dmxsend.py --protocol sacn --universe 1 --count 2 --leds 64 --pattern chase`. Ports 1 and 2 follow universes 1 and 2. Confirmed 2026-09-16 at base 1, and again after rebasing the node to 5 — the base always lands on port 1, so `sACN Universe = N` means "port 1 listens on N". sACN is multicast, so there is no `--target`; the tool sends to 239.255.0.1 and pins the outgoing interface, because Windows otherwise picks by routing table and can send it out a VPN or virtual adapter.
  - Then change the base to 100. The log must show the re-join with **32 of 32 groups joined** — fewer means smoltcp's multicast table is back to its default of 4, i.e. the `iface-max-multicast-group-count-32` feature in `pico2/Cargo.toml` was lost.
  - `--protocol sacn --universe 100 --count 2 --leds 64` then drives the re-based pair. Behind an IGMP-snooping switch this is the test that proves the joins reached the switch rather than only the stack.
  - **Expect a pause of several seconds after a rebase before anything lights** — seen on the bench 2026-09-16 rebasing to 105, and it is the network rather than the node. `rejoin()` leaves 32 groups and joins 32 more with no awaits, so the firmware is done in microseconds and logs `sACN: universes N..M - joined 32 groups` straight away. The switch only starts forwarding the new groups once it has seen the IGMP membership reports and reprogrammed its snooping table, and one waiting on its querier interval can take tens of seconds. A pause that clears on its own is IGMP working as designed; one that never clears is a real failure. The log line tells you which side to look at.
  - To exercise the whole 32-group window rather than just the two bound universes, `--universe 100 --count 32 --pattern ramp`. Only the bound pair renders; the point is that the joins hold and nothing is dropped or logged as an error.
- ✓ **Throughput ceiling** — **closed 2026-09-16 at 30 of 32 back to back (95.6 %, 1346/s), 32 of
  32 paced.** Started at 9 of 32 (439/s); five measured experiments, four kept; 32 back to back
  is out of reach on this chip (both limits sit at 30; it would need ≤ 0.37 ms per frame during
  the 1.5 ms arrival). Everything below is the record of how the number was found and moved.
  (Phase 0 measurement — record the number). Read `eth_hz=` from `info` first: that is the clock the boot probe settled on and the hard ceiling on everything below it. Then:

  `python tools/dmxsend.py --target <node ip> --universe 0:0:1 --count 32 --rate 44 --seconds 60 --pattern ramp`

  **32 universes deliberately, not the 2 that are bound.** The receiver buffers every universe below `DMX_UNIVERSE_COUNT` whether or not a port renders it, so this loads the network path and the W6300 transport at the level the design is budgeted for, on a bench rig that only has two strips. Only ports 1 and 2 will light; that is expected.

  Watch the node's log for `ArtNet socket receive error` and the UI for lost frames, and check the LED output on the two live ports stays smooth. The tool prints the achieved rate and the wire bandwidth at the end.
  - **Result, 2026-09-16 — the node does not meet the design point, and the reason is measured.**
    Full-size frames: **439/s of 1408 offered**. Two CS/SCK logic captures of the W6300 bus during
    the flood (`digital.csv` in Art-Net mode, `dmx_digital.csv` in DMX mode; `python
    tools/busscope.py <file>`) show **exactly 9 of every 32-frame burst survive, in both modes**:
    the driver gives the chip's socket 0 a **4 KB** RX buffer — seven frames — plus ~2 read while
    the burst is still arriving. The other 23 are discarded inside the W6300, and the driver then
    idles 8–12 ms of every 22.7 ms with nothing to read. 9 × 44 = 396/s. That is why halving the
    SPI clock (−13 %), bypassing the render (+12 %) and growing the socket buffer (0 %) all barely
    moved it. The payload DMA itself is perfect: 228.7 µs against 228.8 µs of wire. Per-frame
    time inside a burst is 1.02 ms with the render, 0.66 ms without — a tight distribution; the
    earlier "6.96 ms p90" was inter-burst idle misattributed. Full account and fix order in §11 of
    `docs/ARCHITECTURE.md`.
  - **Fix 1 applied and confirmed 2026-09-16: chip RX buffer 4 → 16 KB**, in
    `vendor/embassy-net-wiznet` (see its `PATCHES.md`). DMX mode: **497 → 1064/s**. Counted in fixed
    22.7 ms windows (`busscope.py digital_03.csv --period-ms 22.7`): survivors **median 28, p90 and
    max 30, min 15**. A burst landing on an empty buffer keeps 30 — 28 held plus 2 read during
    arrival, the 16 KB prediction exactly — and the average is pulled down to 24 by **the sender**,
    not the node: `dmxsend.py` was sleeping in Windows' 15.6 ms steps, so bursts landed in
    pairs ~15 ms apart and the second of each pair found a half-drained buffer. Both tools are fixed
    (1 ms timer + spin wait + a spacing report in `dmxsend.py`; `--period-ms` in `busscope.py`).
    **Always read the sender's spacing block before reading the node's count.**
  - **Re-tested with the fixed sender, 2026-09-16.** DMX mode (`digital_04.csv`): **29–30 per
    22.7 ms window, every window; 1342/s**; per-frame 0.608 ms; regular 5.6 ms idle. Fix 1 confirmed
    to the frame. Art-Net mode (`digital_05.csv`): **866/s, median 19 per window, 1.227 ms/frame,
    zero idle** — and the driver's post-DMA wait is **434 µs against 43 µs in DMX mode**: the
    per-packet LED rebuild costs 0.62 ms/frame. That is fix 2's target.
  - **Fix 2 applied 2026-09-16: render on a 16 ms timer**, `pico2/src/event_router.rs` only (see
    §11 of `docs/ARCHITECTURE.md`). **Test it** — Art-Net mode, then DMX mode once to confirm
    nothing moved there, `stats` either side and a capture each:

    `python tools/dmxsend.py --target <node ip> --universe 0:0:1 --count 32 --rate 44 --seconds 30 --pattern ramp`
    `python tools/busscope.py <capture>.csv --period-ms 22.7`

    **Measured (`digital_06.csv`): Art-Net 866 → 1150/s over 30 s; 29 per window in the capture
    (28–31); per-frame 1.227 → 0.775 ms; tail 434 → 140 µs.** About 60 % of the predicted gain: at
    0.775 ms the drain takes 22.5 of the 22.7 ms window, so it sits at the edge and jitter costs
    whole bursts — the 30 s average (26/burst) comes in under the steady second the capture caught.
    The rest of the gap to DMX mode is the Art-Net task's per-packet store path. Full account in §11.
    Two things only eyes can check, still to do: the chase pattern is smooth (a 62.5 Hz tick renders
    each 44 Hz frame exactly once), and there is now up to 16 ms between the first packet of a frame
    and the LEDs — under one Art-Net frame; Stage 6 will see the same on wired DMX.
  - **Fix 3 applied 2026-09-16: SPI round trips coalesced**, `pico2/src/spi_coalesce.rs` (see §11).
    Every register access is now one blocking burst instead of four DMA round trips; only the payload
    uses DMA. **Test it** — Art-Net mode, `stats` either side, and a capture:

    `python tools/dmxsend.py --target <node ip> --universe 0:0:1 --count 32 --rate 44 --seconds 30 --pattern ramp`
    `python tools/busscope.py <capture>.csv --period-ms 22.7`

    **Measured (`digital_07.csv`): 1150 → 1304/s over 30 s (92.6 %); 30 per window (29–31); the
    30 s figure now matches the capture** — the edge is cleared. Coalescing proven: sub-burst gaps
    4.5 → 0.2 µs, header gaps at offsets 1/3 from 100 % of reads to 2 %, register phase
    0.285 → 0.141 ms with p90 0.496 → 0.194. **Not predicted: the tail rose 140 → 263 µs** and
    per-frame moved only 0.775 → 0.751 — the register phase is now blocking, so the other core-0
    tasks run during the payload DMA instead and are mid-flight when it completes. The tail is now
    35 % of a frame. Full account in §11. Still to do by eye: DMX mode once (~1342) and the LEDs
    chasing.
  - **Fix 4 applied 2026-09-16: `yield_now()` in the Art-Net task right after `recv_from`**
    (`pico2/src/artnet.rs`, one line), so the driver — already woken by the payload DMA — runs
    before the parse-lock-copy path. **Test it** — Art-Net mode, `stats` either side, and a capture:

    `python tools/dmxsend.py --target <node ip> --universe 0:0:1 --count 32 --rate 44 --seconds 30 --pattern ramp`
    `python tools/busscope.py <capture>.csv --period-ms 22.7`

    **Measured (`digital_08.csv`): 1304 → 1346/s over 30 s (95.6 %) — equal to DMX mode; 30 per
    window in every window (min 30, max 31); tail 263 → 216 µs.** So the Art-Net store path was
    ~50 µs of the tail and the remaining ~216 µs is smoltcp in `net_task`. The ceiling is now
    explicit: buffer + arrival = 28 + 2 = 30, drain = 22.7 ÷ 0.755 = 30.1 — both limits at 30. 32
    back-to-back would need ≤ 0.37 ms/frame during the arrival, out of reach on this chip. **30–31
    of 32 is the practical back-to-back ceiling; a paced controller has had 32 of 32 since fix 2.**
    Full account in §11.
  - **Fix 5 applied 2026-09-16: `N_RX` 8 → 2** (`pico2/src/w6300.rs`, one constant), so `net_task`
    handles at most two frames per poll instead of up to eight. **Test it** — Art-Net mode, `stats`
    either side, and a capture:

    `python tools/dmxsend.py --target <node ip> --universe 0:0:1 --count 32 --rate 44 --seconds 30 --pattern ramp`
    `python tools/busscope.py <capture>.csv --period-ms 22.7`

    **Measured (`digital_09.csv`): tail 216 → 211 µs (unchanged); per window 27–33 (was 30–31
    every window); `stats` 1346 → 1289/s. Reverted to 8.** Queue depth was not the lever —
    `net_task` already handled about one frame per poll, so the ~210 µs tail is smoltcp's per-frame
    work — and two slots added episodic starvation. 4 was not tried: with no benefit to trade against,
    it has nothing to buy. **Reflash `pico2.uf2` to return the board to the measured-best build.**
    Full account and the closing table in §11.
  - **Recommendation for this item:** close at *30 of 32 back-to-back (95.6 %), 32 of 32 paced*, with
    32 back-to-back recorded as out of reach on this chip rather than as a target. Started at 9.
  - **Fix 2 next: per-frame under 0.7 ms** so all 32 drain inside 22.7 ms — render on a timer first
    (removes ~0.36 ms/frame and the tail inflation), then coalesce the SPI round trips for margin.
    Each as its own build; re-measure frames per burst with `busscope.py`, not just `stats`.
  - **When re-measuring**, the number to watch is **frames per burst** (section E of `busscope.py`).
    `stats` gives the rate; the burst count says which limit you are hitting — the buffer (flat at
    ~9 regardless of frame speed) or the drain (rises as per-frame time falls).
  - **Progress, Art-Net mode, 32 full universes at 44 Hz back to back:** start 9/window 439/s →
    fix 1 (16 KB chip buffer) 19 / 866 → fix 2 (render on a timer) 29 / 1150 → fix 3 (SPI
    coalescing) 30 / 1304 → fix 4 (yield after receive) **30 every window / 1346/s** → fix 5
    (queue depth 2) 27–33 / 1289, reverted. **Recommended close: 30 of 32 back to back (95.6 %),
    32 of 32 paced; 32 back to back is out of reach on this chip.**
  - **Take the node's own count, not the sender's.** `dmxsend.py` can only report what it transmitted; a link dropping a few percent looks identical from that end. Run `stats` on the USB console **twice** — once before the flood and once after — and the second call prints the rate over the interval:

    ```text
    rx=84512 stored=84480
    ignored=32 malformed=0 errors=0 sacn=0
    since last: 60.2s  1404 rx/s  1404 stored/s
    ```

    32 universes at 44 Hz is **1408 packets/s**. `stored/s` at or near that means every frame landed. `errors` must be 0. `malformed` above 0 means bytes are being corrupted on the way in, which at this point would mean the SPI clock is too fast — see `eth_hz=`/`eth_max=` in `info`.
  - **Read the sender's own numbers too.** If it reports frames sent late, the PC could not keep the rate and the shortfall is yours, not the node's. Baseline measured 2026-09-16 on `orbital-emu`: 32 universes at 44 Hz sustained exactly, 5.97 Mbit/s, zero late frames — confirmed against the node at 192.168.86.36.
  - 5.97 Mbit/s is the real number to compare against §6 of `docs/ARCHITECTURE.md`, which budgets 6.5 Mbps for the 1800 B/port design point.
  - If `eth_hz` came back under 20000k, raising the top of `w6300::PROBE_FREQS_HZ` will not help — the limit is sample timing, and the fix is a PIO SPI that samples mid-bit rather than at the rising edge.

## Stage 6 — Wired DMX

- ✓ Receive: console into **J5** (male XLR, "Input"), DMX mode (2026-09-17). Values arrive — confirmed from the bytes with `tools/enttec.py --monitor`, not just from LEDs — and pulling the cable logs `DMX: no data` within 1 s. Replug → `signal restored` is the same `quiet` flag flipping back (`dmx.rs:119`) and follows from receiving again.
  - **Two windows, and they show different things.** `--monitor` (first CDC port) shows *values*, and only when they change. `DMX: receiving` / `DMX: no data` / `DMX: signal restored` are **log lines** (`dmx.rs`, 1 s silence timeout) and appear on the **log port** — the third CDC, with `pico2-usb.uf2` — or over RTT with a probe. They are not Enttec messages and will never show in the tool.
  - **`--monitor` correctly goes silent on unplug.** The firmware stops writing `DMX_BUFFER` rather than zeroing it, so the last look holds and there is no change to forward. That is deliberate — a dropped cable must not black out a show mid-scene — but it does mean loss of signal is invisible to a host watching the widget, and only the log reports it.
- [ ] Alternate start codes (RDM traffic from the console) do not disturb channel values.
- ✓ **Driver enable (GP10)** (2026-09-17): **high** in `ArtNet>DMX` and `USB>DMX`, low in every other mode — exactly the two modes `InputMode::is_dmx_output()` names. That is the check that matters most on a shared bus: a stuck-high enable makes the board fight a console.
- ✓ **DMX line timing at the source** (2026-09-17): GP9 (U1 pin 12, → R9 → U3) in `ArtNet>DMX` driven by `tools/dmxsend.py` measures **BREAK 177 µs, MAB 16 µs** — exactly what the PIO program computes at 1 µs/cycle (`set x, 21 side 0` = 1, then 22 × 8 = 176; MAB is `nop` 8 + `pull` 8). Deterministic, so any spread in a measurement means something else is on the wire.
  - **Unplug the console from J5 first.** With the board in plain `DMX` mode, GP10 is low and the transceiver is not driving J15 at all, so a scope on the bus reads the *console* — BREAK 111–180 µs and MAB 8–12 µs, varying frame to frame, which is what a real console looks like and nothing like a PIO program. That cost a measurement on 2026-09-17.
  - MAB can only ever be **longer** than 16 µs, never shorter: `pull` stalls with the line high if the DMA has not filled the FIFO.
- ✓ **DMX line timing on J15 itself** (2026-09-17): packets reach J15 through the isolator and transceiver (U3/U4) and a **fixture follows**, in both `ArtNet>DMX` and `USB>DMX`. Measured **43.5–43.8 Hz**.
  - That rate is the expected one, and the small spread has a cause rather than being noise. A frame is `BREAK 177 + MAB 16 + 513 slots × 44 µs` = 22.765 ms → **43.93 Hz** if the transmitter never paused. `PioDmxTx::write` then waits for the state machine to drain by **polling every 100 µs** (`Timer::after_micros(100)`), plus 10 µs for the last stop bits, so each frame lands 10–110 µs late: 43.7 Hz ± 0.2. Measurement and arithmetic agree.
  - A slot is 44 µs because it is 11 bits at 4 µs: `pull side 1 [7]` = 2 stop bits, `set x, 7 side 0 [3]` = the start bit, then 8 × 4 cycles of `bitloop`.
  - Do **not** chase the last 0.2 Hz by shortening the poll interval. DMX has no minimum refresh rate that this is near, and the poll is what guarantees the frame is fully on the wire before the next BREAK.
- ✓ `ArtNet>DMX`: the configured Art-Net universe appears on J15 (2026-09-17) — the same data reaches J15 and the LED strip, which is what `is_dmx_output()` promises: the wired output is a *copy* of what the strips render, not an alternative to it.
- ✓ Undriven state: with the module removed, U4's driver is **disabled** (fail-safe receive: R1 biases the U7 LED on, DE/RE pulled low through the opto).
- ✓ Isolation: `3V3ISO` to `GND` still open with the bus connected (2026-09-17); no ground current through the XLR shield (CHGND → R4 → GND only). This is the check that matters on a rig where the console and the board are on different mains circuits.
- [ ] **Short-frame consoles**: with a console configured for fewer than 512 slots (e.g. 24), press buttons on the TFT while a fixture above the console's slot count is patched. Any flicker there is the known short-frame/redraw interaction (ARCHITECTURE §11), not a wiring fault; a 512-slot console must show none.

## Stage 7 — LED outputs

- ✓ Boot: all eight strings dark (blank frame), no flash of stale colour at power-up. Confirmed 2026-09-17.
- ✓ Scope one data line (J1 pin 1, after R21): 800 kHz bit rate, 1.25 µs/bit, **≥ 150 µs** idle between frames; frame time ≈ 30 µs × LED count (18 ms at 600). Confirmed 2026-09-17.
- ✓ RGB (WS2812/WS2815 strip): DMX channels 1-3 = R,G,B of LED 1 — **colour order correct** (GRB on the wire). Confirmed 2026-09-17.
- [ ] **BLOCKED — no SK6812 strip on the bench (2026-09-17).** **RGBW (SK6812 strip)**: `Color Mode = RGBW`, channels 1-4 → R,G,B,W of LED 1; white channel lights the white die only; a WS2812 strip in RGBW mode shows the expected garbage (proves the mode switch reaches the wire).
- ✓ Mixed length: `LEDs Port 1 = 10` — the strip beyond LED 10 keeps its last latched value only until the next blank; frame rate rises (shorter DMA). Confirmed 2026-09-17 on port 1.
- ✓ Group size 3 on a port: three physical LEDs per DMX pixel. Confirmed 2026-09-17 on port 1.
- [ ] **Fixtures: runs of different sizes on one strip.** `Grouping = Fixtures`, then Layout → select a port → its table. Set `F010`, `F005`, `G005`, `F030` on a 60-LED port: DMX items 1–3 drive LEDs 1–10, 11–15 and 21–50, and the panel shows those spans in blue beside each row. Walk them with `tools/enttec.py --map` to confirm which channels own which pixels.
  - **The gap check, done the way that can fail**: light the whole strip first (`--pattern solid`), *then* apply the layout. LEDs 16–20 and 51–60 must go dark **immediately**. Applying a layout to an already-dark strip proves nothing — WS2812s latch, so the failure mode is a gap that keeps the previous colour.
  - Lower `LEDs Port n` below the fixture total afterwards: the entries must still be editable *down*. If they freeze, the length bound is using "what is left" rather than "what is left plus this entry's own".
  - Repeat with `Mirror Ports = On`: every port shows the same items, each through its own fixture list.
  - Console equivalent: `set fixture_1_1 F010` …, then `get` to read them back. A value that does not survive `get` → `set` means the rendered form has a space in it.
- ✓ **Mirror mode: every port shows the same addressable items** (reworked and confirmed on hardware
  2026-09-17, including the greyed-out menu values and the computed number beside them). Port 1's
  LED count and group size fix how many items there are; every other port spreads that many items
  across its own strip, so `Group Size Port 2..8` is **computed** while Mirror is selected. 24 LEDs
  grouped by 4 is six groups, so a 12-LED port mirrors it grouped by 2 and a 48-LED port grouped
  by 8 — the same six items, each stretched to the strip it is on.
  - The size **scales from port 1** (`leds_n × size_1 / leds_1`), it is not the length divided by
    the item count. The two agree whenever port 1’s group size divides its LED count and differ
    otherwise — see the 2026-09-17 bench finding at the bottom of this item.
  - **On the panel** those rows show two numbers: the stored value dimmed grey, and the computed
    one in blue a column to its right. The stored value stays editable on purpose — it is what
    `Individual` mode goes back to, so hiding it would mean leaving Mirror just to set a port up.
  - **Rounding is down**: 50 LEDs across 6 groups is 8.33, so the port groups by 8, covers 48, and
    the last two LEDs are driven black rather than making one group visibly longer.
  - A port with fewer LEDs than there are groups shows the leading groups, one LED each.
  - Check: P1 24/4 and P2 12, P3 48, P4 50 — all four must show six identical items, P4 with two
    dark LEDs on the end, and the LED page must read `004` grey + `002`/`008`/`008` blue on those
    three rows. Switch to `Individual` and the blue column disappears.
  - Then the original case: P1 10 LEDs grouped by 3 with P2 at 64 — **three** items on both
    (`floor(10/3)`), P2 grouped by 19 and lighting 57 of its 64. Port 1 leaves its 10th LED dark,
    so its mirror leaves the same tenth of its own strip dark; that is the mode working, not a
    rounding bug.
  - **Two identical ports must group identically** — the 2026-09-17 bench report: P1 and P2 both
    64 LEDs, P1 grouped by 3, P2 computed **2**. Two causes, both now fixed and pinned by an
    exhaustive sweep in `host_tests/tests/mirror.rs`:
    1. The item count used `ceil` (`virtual_leds_per_port`), giving 22 items for 64/3. The 22nd
       holds one LED on port 1 but a full group elsewhere, so `64 / 22 = 2` on port 2 — 44 LEDs
       lit beside port 1’s 64. The count now rounds **down**: 21 items.
    2. Dividing the length by the item count is wrong even with the count fixed. 10 LEDs grouped
       by 4 is two items, so a second 10-LED port got `10 / 2 = 5` against port 1’s 4. The size
       now scales from port 1, which returns port 1’s own size for any port of its length by
       construction.
  - Quick check for both: set P1 and P2 to the same LED count and a group size that does **not**
    divide it (64 and 3). The two strips must be indistinguishable, and the panel must show the
    same number in blue on the P2 row as P1 has in white.
  - Found by the case that prompted it: P1 10/3 with P2 64/2 lit LEDs 1–12 on P2 against 1–10 on
    P1, because `ceil(10/3)` paints four groups of three and only P1's length truncated it. The
    arithmetic is pinned by `host_tests/tests/mirror.rs`.

- [ ] 600 LEDs on all 8 ports at 44 Hz for 10 min: no dropped frames, U10 cool, `5V_LVL_SHIFT` (after L3, at C41) = 5.0 V.
- [ ] Per-port protection: short a strip's V+ to GND through a 5 A load → that port's PTC (F10–F17) trips, and on a power-board-fed strip the blade insert (F3/F4/F7/F8/F9/F12/F13/F14) opens, with no effect on other ports; the PTC recovers when cleared.

### LED Power = USB brick (5 V build only)

- ✓ Menu **System → LED Power** offers `External` / `USB brick`; console `set led_power usb` and `get` agree with the display. Confirmed 2026-09-21.
- ✓ With `External`, **U14 is off** (2026-09-18): `V_LED` = **4.66 V** with a 5 V source on J2 and nothing else connected — one B360A drop below VBUS, so the rail is being fed through D4 and *not* through the switch. `USB_LED_EN` (U14 pin 3) = 0 V.
  - **But the strips still light, and the rail is still live.** See "The `External` setting does not isolate the LED rail on a 5 V build" under Known issues. The firmware is behaving correctly here; the isolation the item assumed does not exist on this build variant. The original wording checked only EN, so it would have passed with the rail live — measure `V_LED`, not just EN.
- ✓ With `USB brick` **and no brick attached**: EN stays low (the firmware also requires VBUS present on P05, via R39/R40). Confirmed 2026-09-21.
- ✓ With `USB brick` and a 5 V brick on J2 (2026-09-18): `V_LED` = **5.00 V**, i.e. through U14's FET rather than a diode — the 0.34 V step from `External` is the whole proof that the two paths are different silicon. EN goes high through U14 → F2.
  - [ ] Still to check on this item: the title row shows `PWR usb`.
- ✓ **Budget**: drive all eight ports to full white. The frame is scaled so the draw stays near 1.3 A — measure it in the brick lead; U14 must not latch. The title row must read **`PWR usb NN%`** while it is scaling (the percentage appears only below full output, so `PWR usb` alone means nothing is being held back). That indicator was added 2026-09-18: without it, dim strips look like a fault rather than the budget working, which is the confusion the `External`/`USB brick` brightness difference caused on the bench. Note the budget only covers ~21 full-white RGB pixels unscaled, so heavy content will visibly dim. That is the intended behaviour. Confirmed 2026-09-21.
- ✓ **Fault latch**: short an output briefly (current-limited bench supply). `FAULT` (U14 pin 4) pulls low, firmware logs `USB LED power: FAULT`, EN drops, and it retries ~2 s later with `PWR flt` on the title row meanwhile. After three failures it logs `staying off until LED Power is changed`, and toggling the setting clears it. Confirmed 2026-09-21.
- ✓ Unplug the brick while enabled: EN drops (logged as `brick removed`, not as a fault), no glitch on `V_SYS`, logic keeps running from D4/F1 until the PSU path takes over. Confirmed 2026-09-21.

## Stage 8 — USB

**U2 (FT232RNL) is not fitted** — the part could not be sourced (2026-09-16). Three things
follow, and they are not the same thing:

* **The FTDI *data* path is blocked** and is marked so below: J2 enumeration, the EEPROM
  identity in [ft232rnl-eeprom.md](ft232rnl-eeprom.md), baud hunting on UART0, and verifying
  the JP1/JP2 straps against live traffic. Set the straps now anyway so the board is ready.
* **The J2 *power* path is unaffected.** The USB-C connector, R22/R23 CC resistors, D4, F1,
  U14 and the LED-power switching involve no U2 at all, so every brick-on-J2 test below is
  testable today and worth doing — it is most of Stage 8 by count.
* **The Enttec widget is not blocked, and the module’s own USB is the interface `USB>DMX`
  uses.** The widget is transport-agnostic in the firmware (`enttec_widget.rs`); what changed
  on 2026-09-16 is that the module’s USB now **enumerates differently depending on the stored
  mode**, because a CDC port is invisible to QLC+ and that was confirmed on the bench:

  | Stored mode | The PC sees | Console |
  |---|---|---|
  | `USB>DMX` | an **FT232R**, `0403:6001`, `ENTTEC` / `DMX USB PRO` (`ftdi.rs`) | none |
  | anything else | composite CDC: widget, console (, logger) | yes |

  Changing between those two means re-enumerating, and **the board reboots itself to do it**
  (`usb_identity_watch_task`, 2026-09-17): change the mode, the panel stays up for ~750 ms, the
  board restarts and comes back with the right identity. It only fires when crossing the
  `USB>DMX` boundary, and only once boot has completed and the EEPROM has confirmed the write —
  resetting earlier would count against the Ethernet lockout guard, and a failed write simply
  leaves the board as it was rather than looping.

Two firmware notes while U2 is absent. GP13 (UART0 RX) floats with nothing driving it, so the
baud-hunt loop in `enttec_uart.rs` may log `nothing framed, trying N baud` on line noise —
harmless, and a one-line guard if it gets tiresome. And `FTDI widget: locked at … baud` will
never appear: correct, not a fault.

FTDI emulation (`pico2/src/ftdi.rs`, 2026-09-16) is a vendor-class interface, not a VID/PID
change: `bcdDevice` = `0x0600` so the host knows which chip to drive, two status bytes in front
of every IN packet, and the vendor control requests answered. It is **test scaffolding, not the
product path** — on a board with U2 fitted the identity comes from that chip’s EEPROM and this
module is unused — and it claims FTDI’s vendor ID, which is legitimate for real silicon and a
decision to revisit if a unit ever ships without any.

One upside worth knowing: where Windows has FTDI’s VCP driver installed (usually alongside
D2XX), an FTDI device also appears as a COM port. So in `USB>DMX` mode QLC+ should find it
through D2XX *and* xLights / OLA `usbpro` should find the COM port — strictly better reach than
the CDC port had, at the cost of our own console.


- [ ] **Identity**: `info` on the console shows a MAC of `02:44:4D:xx:xx:xx` when none is programmed, and the USB serial (Device Manager / `lsusb -v`) is 16 hex digits — **different on every unit**. Two units on one PC must enumerate as two COM ports.
- [ ] **FTDI straps** — *set now, verify when U2 arrives.* JP1 and JP2 both bridged **1-2** = straight (U2 TXD ← `FTDI.TX` net → GP13; U2 RXD ← `FTDI.RX` net ← GP28). 2-3 on both swaps TX/RX if a board is wired the other way round — never mix.
- ✓ Module USB-C on a PC: two (three with the `usb` feature) CDC ports enumerate; port order = Enttec widget, console (, logger). Confirmed 2026-09-17 with `pico2-usb.uf2` in `DMX` mode — widget answers `--params` on the first, `info` answers on the second, the log streams on the third. With U2 unfitted this is the board’s only USB data port.
- [ ] **Console fixture patch — the thing that will look like a firmware bug and is not.** The board
  maps **raw DMX channels** to LED components, three per pixel, with no concept of a fixture. A console
  patched with a fixture whose definition is not exactly 3 channels of R, G, B shifts every pixel after
  the first, and the fixture's other channels land on LED components at apparently random places. An
  RGBW definition is the classic: it claims 4 channels per pixel, so its *white* fader drives the next
  pixel's *red*. Seen on the bench 2026-09-17 — QLC+ behaved correctly with no fixtures patched and
  erratically with them. Patch 3 channels per pixel in R, G, B order starting at address 1, and use the
  console's own DMX monitor to confirm fader N moves channel N before suspecting the board.
  `tools/enttec.py --map 12` is the reference: it drives known channels with no console in the loop.
- ✓ **Automatic re-enumeration** (2026-09-17). Change the mode into `USB>DMX` and out again. Each crossing reboots the board after ~750 ms and it comes back as the other device; changes that do not cross the boundary (`DMX` ↔ `ArtNet`, say) must **not** reboot. After two crossings in a row, `info` must still show `boot=Some(Success)` and Ethernet must still come up — that is the lockout guard staying clear.
- ✓ **Widget in a non-`USB>DMX` mode (CDC)** (2026-09-17, `DMX` mode). `tools/enttec.py --port COMn --params` round-trips on the **first** CDC port: `7e0305002c01100228e7` → firmware 1.44, break 16, MAB 2, refresh 40 Hz — the same reply the FT232R identity gives, so the framing is identity-independent. `OUTPUT_DMX` is refused by design (`enttec_widget.rs:68`) so a PC left plugged in cannot overwrite live wired-DMX or Art-Net data; `tools/enttec.py` now warns before those commands instead of appearing dead.
  - Also proves the **other direction**, which no earlier test had: `--monitor` decoded the unprompted label-5 frames the board sends when the universe it receives changes, e.g. `start 0x00 | 0 0 255 ...`. That is `payload[1]` = channel 1, so a lit channel 3 is pixel 1 blue — the same convention the QLC+ session showed, confirmed from the bytes rather than inferred from the LEDs.
  - And it clears the risk the `usb_toggle` wrapper introduced: that wrapper sits under **all three** CDC functions, and the widget and console both work with it in place.
- ✓ **`USB>DMX` mode: the board enumerates as an FT232R** (2026-09-17). Windows binds the FTDI driver and the VCP layer gives it a COM port; `tools/enttec.py --port COMn --params` round-trips `GET_PARAMS` (firmware 1.44, break 16, MAB 2, refresh 40 Hz), which exercises the bulk IN path including the two status bytes, and `--map 12` gives red/green/blue per pixel in the correct order. **The widget, the FTDI transport and the channel mapping are proven** — anything anomalous from a lighting console is that console's patch, not the board. `tools/enttec.py --pattern chase` is the known-good sender to compare against.
- ✓ **`USB>DMX` output on J15** (2026-09-17): a fixture on J15 follows host DMX in this mode, so the FT232R identity, the widget, the router and the wired transmitter work as one path. Recorded with the timing in Stage 6. If Windows ever shows a driver error instead of an FTDI device, FTDI's driver is missing: install it from FTDI rather than from Windows Update's generic list.
  - Faders in QLC+ drive the LEDs, and `USB>DMX` output appears on **J15**.
  - If QLC+ enumerates it but reads fail, the likely culprit is the EEPROM read request (`0x90`), which `ftdi.rs` answers with zeros — see the note there.
  - If Windows shows a driver error rather than an FTDI device, FTDI’s driver is missing: install it from FTDI rather than from Windows Update’s generic list.
  - With the VCP driver present it should also appear as a COM port, so xLights / OLA `usbpro` work here too.
  - **There is no console in this mode.** The TFT still shows everything; `stats`, `info` and `dmx` return when you leave `USB>DMX`. A Python Enttec host is the other route and the only one that exercises every label (`GET_PARAMS`, `SET_PARAMS`, `OUTPUT_DMX`, `RECEIVE_ON_CHANGE`, `GET_SERIAL`) rather than the two a console happens to use.
- [ ] **BLOCKED — U2 not fitted.** **FTDI port (J2)**: after programming U2 per [ft232rnl-eeprom.md](ft232rnl-eeprom.md), Windows shows `ENTTEC` / `DMX USB PRO`; QLC+ lists a DMX USB Pro; log `FTDI widget: locked at 250000 baud`; faders drive the LEDs in `USB>DMX` mode.
- [ ] **BLOCKED — U2 not fitted**, and vacuous without it: the test is of U2’s `RESET#` gate. With the box **unpowered**, plugging J2 into a PC does nothing (U2 `RESET#` follows VBUS through R24/R25 and U2 is self-powered from 3V3: no enumeration, no back-drive). With the box powered, it enumerates.
- [ ] **BLOCKED — U2 not fitted**: baud hunting detects the rate on UART0, and nothing drives UART0. Baud hunting: open the port at 57600 with a Python Enttec script → `locked at 57600 baud`. (Over the module’s CDC port baud is meaningless — there is no UART — so this check has no CDC equivalent.)
- ✓ **Module USB only** (PSU off, nothing on J2) — confirmed 2026-09-21: `V_LED` = **4.49 V** through D3/F18 (in the 4.3–4.6 V band), `rpi_vbus=1` on the new `info` → `pwr=` line, strips lit in both `LED Power` modes and the amber note shown. Everything here is correct behaviour — see the sub-points below for why lit strips are the expected result on this feed.- [ ] 
  - **The strips WILL be lit, in either `LED Power` mode. That is correct** — corrected 2026-09-18, this item previously said "strips stay dark … firmware must not drive them" and would have logged a pass as a failure. `D3 → F18 → V_LED` is unswitched and `LED Power` does not reach it; the R6-P1 Schottky is in the logic branch and deliberately does not touch this path. Firmware cannot reliably detect the case either — with no J26 present-detect it cannot tell "module USB is the only source" from "J26 is up and someone plugged in a debug cable", and dimming the second case would be worse. See the Known-issues entry for the arithmetic.
  - What to check instead: the panel shows the amber note **`Module USB also powers LEDs / This setting cannot stop it / 0.5 A fuse - may flicker`** under `LED Power` while VBUS is present on that port.
  - **F18 is 0.5 A**, so at high brightness expect it to hold briefly then cycle. Strips that **pulse** are F18 recovering, not a firmware fault; strips that **dim smoothly** are the budget, and the budget only applies in `USB brick` mode. Both are expected behaviour on this feed.
- ✓ **Brick on J2 only** (5 V, ≥ 2 A, no PD, 3 A-rated cable), `LED Power` still `External`: `USB_LED_EN` = 0 V (R38), `V_LED` = 0 V (U14 off — a disabled USB power switch passes nothing), `5V_LOGIC` ≈ 4.65 V via F1/D4, P05 = 1 (R39/R40), FAULT high, the unit boots and the display works. Fail → EN high with the setting off = R38 missing / P04 driven high. **`V_LED` at ≈ 4.66 V with EN low is R6-P1** — the `5V_LOGIC → L2 → F9` back-feed, fixed by the series Schottky between F9 and `V_LOGIC_IN`; check that diode is fitted and the right way round before suspecting U14. (The earlier wording here blamed "something with a body diode in U14's place", which is what sent the first investigation down the wrong path.) Verified dark on a reworked board 2026-09-18 — this item is now measuring the numbers behind that. Confirmed 2026-09-21.
- ✓ Set `LED Power = USB brick`: `USB_LED_EN` = 3.3 V (P04 driven high), `V_LED` ramps up (U14 soft start) to ≈ 4.9 V, drop across U14 ≤ 150 mV at 1.3 A full-white on two short strips, FAULT stays high, U14 under 40 °C rise; F2 never trips. Push the load to ≈ 1.6 A: U14 latches off within 10 ms and FAULT goes low (P07 = 0); firmware backs off and re-arms as in stage 7. Then plug the PSU in (trimmed ≥ 5.2 V) while the brick runs: U14 opens once `V_LED` is ≈ 135 mV above the brick (FAULT low, `V_LED` follows the PSU, no `V_SYS` glitch on the scope); unplug the brick: nothing changes. Confirmed 2026-09-21.
- ✓ **PC on J2** with the PSU off (2026-09-21): **the board does not boot at all**, in either `LED Power` mode. Safe, and not what this item predicted — it said "logic runs from D4 (≈ 0.35 A from the port)", which assumed the port would supply that.
  - **Why.** An unconfigured USB device may draw **one unit load (100 mA)** until it enumerates and its configuration is selected. U2 is not fitted, so nothing on J2 ever enumerates and the host never grants more. The logic wants ~0.35–0.7 A, so the port limits or the rail collapses. A brick has no such gate, which is why the identical connector works from one and not the other.
  - **This confirms review §77 on hardware**: the FT232RNL is configured self-powered and declares ≤ 100 mA, so **fitting U2 will not fix it** — the descriptor still will not ask for the current the board draws through D4. Expect the same behaviour on a populated board.
  - **Consequence for the product**: J2 is a *data* port. A unit must be powered from J26 or a brick; a PC alone will not run it. The manual has to say so, and the failure is at least quiet and self-limiting rather than a brownout loop. Worth re-checking for oscillation once U2 is fitted.
  - Note: P05 is 1 for a PC and a brick alike — the board cannot tell them apart, so the `USB brick` setting is the user’s declaration; the manual must say a PC port is not a brick.

### Carried over from the design record

- [ ] **The core-split claim**: saturate core 0 with Art-Net while editing on the TFT — the UI must stay responsive. This is the specific claim the two-core architecture rests on.
- [ ] **8 strings × 600 LEDs at 44 Hz sustained**; scope one WS2812 line and confirm the ~18 ms frame.
- [ ] **PS1 input current**: measure the actual 3V3 draw into PS1 pin 1 (through L1). The 90 mA in the power budget is derived from its output rating and an assumed efficiency, not from a datasheet.
- [ ] **W6300 diagnostics**: version register reads correctly, and the three failure states (chip dead / link down / no DHCP) are distinguishable in the log — the only diagnostic path left for the undiagnosed Rev 1 Ethernet failure.
- [ ] **Buttons**: chord detection (two at once); a quick tap registers at the 20 ms poll; no phantom presses while the EEPROM task is writing on the shared bus.
- [ ] **Power transients**: scope `V_SYS` (J9 is a GND/3V3 probe point; V_SYS at C38/C45) and `3V3` through power-on, power-off and a full-brightness LED step, with and without USB attached. This is the behaviour that may have been killing Nucleos, so it gets measured rather than assumed.

## Known issues found on the bench

### USB CDC dropped the first connection every time (2026-09-17, fixed)

Symptom: with `pico2-usb.uf2`, opening the **logging** COM port dropped
immediately; the second attempt held. The board never reset — which is what
ruled out the watchdog and the identity watcher and pointed at USB itself.

Cause, two parts:

1. **`embassy-rp` 0.10 does not reset the endpoint data toggle on
   `ClearFeature(ENDPOINT_HALT)`**, which USB 2.0 §9.4.5 requires. Its
   `endpoint_set_stalled` (`usb.rs:431`) clears the `STALL` bit and stops;
   `endpoint_set_enabled` twelve lines below does reset the PID, so it is an
   omission on one path. Windows’ `usbser.sys` resets the bulk pipes when an
   application opens a COM port, so after every open the host is on `DATA0` and
   the device is wherever it left off. Mismatched → the host discards the first
   IN packet → `usbser` reports a failure → the port drops. That transfer
   realigns them, hence “second time works”.
2. **`embassy-usb-logger` starts writing at SET_CONFIGURATION, not at DTR.** Its
   `wait_connection()` means *endpoint enabled*. So the boot log went into the
   IN endpoint within milliseconds of enumeration, advancing the device’s toggle
   long before anyone opened the port — and was thrown away unread. Only the
   logging interface did this, which is why the Enttec and console ports never
   showed the fault: they stay silent until spoken to, so their toggles were
   still at the enumeration value and agreed with the host by luck.

Fixes, both in place:

- `pico2/src/usb_toggle.rs` — a `Driver`/`Bus` wrapper that delegates everything
  and adds the missing toggle reset. Covers every endpoint, including
  close-then-reopen, which the logger change alone does not.
- `pico2/src/usb_log.rs` — replaces `embassy-usb-logger`. The sink is installed
  in `main` so the boot log is captured, and the drain waits for **DTR** so
  nothing reaches the endpoint before a host opens the port. As a bonus the boot
  log now survives to be read instead of being spent on a closed port.

### The USB logger carried almost nothing (2026-09-17, fixed)

Found immediately after the above: the logging port opened first time, and
printed nothing. Not a fault in the transport - there was simply nothing to
send. The `usb` feature only swapped the logging macros in **8** of 18 files, so
53 statements reached USB and 82 stayed on `defmt`/RTT, including every one that
fires during boot (`main`, `w6300`, `eeprom`, `tft_ui`, `identity`,
`supervisor`). On an idle board in `DMX` mode, the eight shimmed files have
nothing to say, so the terminal was correctly empty.

Every file now picks its macros through `cfg_if`, so the two builds carry the
same 135 statements to different places. Cost of the conversion was eight real
incompatibilities: five `{=u8}` type hints (defmt-only syntax) and three
`{:02x}` prints of a `[u8; 6]` MAC, which `defmt` renders element by element and
`core::fmt` cannot - those now go through `identity::mac_text`.

**Trade-off worth knowing:** in the `usb` build the RTT log is now empty, because
everything went to USB. Panics still reach RTT through `panic-probe`. Use the
default build when you have a probe.

### The `External` setting did not isolate the LED rail on a 5 V build (2026-09-18, **fixed and verified**)

**Verified on a reworked board, 2026-09-18.** With the Schottky fitted:

| Powered from | `External` | `USB brick` |
|---|---|---|
| **J2 only** | **strips dark** ✓ | strips lit |
| Module USB only | strips lit | strips lit |
| Both | strips lit, brighter | strips lit, dimmer |

**The J2-only row is the fix.** Before the diode, `External` with a brick on J2 lit the
strips off the D4 back-feed; it now leaves them dark while the logic still boots, which
is exactly what the menu promises.

The other two rows are the **D3 → F18 → `V_LED`** module-USB feed, which the diode does
not touch and was never meant to — it sits in the logic branch, not in D3's path. That
feed is unswitched and ignores `LED Power` entirely, which is why the strips light in
both modes, and the brightness difference in the "both" row is the budget applying in one
mode and not the other (see the bullet on that below). Since that port is **exposed on a
finished unit** rather than being bench-only as originally assumed, the panel now says so:
`Module USB also powers LEDs / This setting cannot stop it / 0.5 A fuse - may flicker` appears in amber under
`LED Power` while the module port has VBUS. Added 2026-09-18 — the setting cannot control
that path, so the honest fix is to explain it rather than rename the control.

---

Original finding follows.


**Hardware, not firmware.** With a 5 V source on J2 and nothing else connected, the
board boots *and the strips light* while `LED Power` is set to `External`. Booting is
intended; the strips are not.

**Measured**, which is what makes this certain rather than a theory:

| `LED Power` | `V_LED` | Path |
|---|---|---|
| `External` | **4.66 V** | one B360A drop below VBUS — a diode |
| `USB brick` | **5.00 V** | U14's FET, ~100 mΩ — no drop |

The 0.34 V step is the signature. The switch *is* being controlled correctly; the rail
is simply reachable without it.

**The path.** The menu setting controls U14 only (`IN` = `V_BUS_FTDI`, `OUT` =
`V_USB_LED` → F2 → `V_LED`). The brick also has a second, unswitched route to the same
node:

```
V_BUS_FTDI → F1 (1.1 A) → D4 (B360A, cathode on 5V_LOGIC) → 5V_LOGIC
                                                              ↓  L2      (bidirectional)
                                                          V_LOGIC_IN
                                                              ↓  F9      (bidirectional)
                                                            V_LED
```

F1/D4 exists on purpose — it is what boots the logic so the operator can *reach* the
menu and choose. D4 blocks `V_LED` → brick, not this direction. L2 and F9 are passive
and conduct both ways, so `5V_LOGIC` ties straight back to `V_LED`. **U14 is bypassed.**

**Why 5 V builds only.** `V_LOGIC_IN` → `5V_LOGIC` is **L2 *or* U6, exclusive**. On
12/24 V builds U6 (OKI-78SR) is fitted and is unidirectional, so the strips genuinely
stay dark — which is the case the Rev 2 design review reasoned about when it concluded
"strips dark there, correctly". The 5 V variant, where that element is a plain
inductor, was never re-checked against the same claim.

**What it costs.**

- **No current limiting.** The brightness budget is gated on `usb_power::brick_powering()`,
  which requires `ST_ENABLED` — false here, because EN really is low. So the 1300 mA cap
  never applies on this path. The only protection is F1 and F9, both 1.1 A hold.
- **`External` is therefore the *least* protected mode, and it looks like the best one.**
  Observed 2026-09-18 with the module USB and J2 both connected: the strips are visibly
  **brighter** on `External` than on `USB brick`. That is not a power-path difference — it
  is the budget doing its job in one mode and being absent in the other. At 20 mA per
  channel the cap covers ~21 full-white RGB pixels, so 64 of them scale to 86/256 ≈ **34 %**
  on `USB brick` and run at **100 %** on `External`, off the same USB sources. The mode
  named for the mains supply is the one drawing unlimited current from a laptop.
- **F18 is in the firing line too** when the module USB is also plugged in: that feed is
  `D3 → F18 (0.5 A)`, in parallel with the F1/D4 route, and at full brightness it will
  hold up briefly and then cycle. Watch for the strips pulsing rather than dimming.
- **The property `External` exists for does not hold.** "A PC must never be loaded" is the
  stated reason for the default, and on a 5 V build a PC on J2 feeds the strips through
  this path. The design review separately noted the FT232RNL is declared self-powered at
  ≤ 100 mA.
- The module-USB dev feed (`D3` → F18 0.5 A → `V_LED`) is the same shape, and its
  checklist item likewise expects "strips stay dark" with nothing in firmware to enforce it.

**Fix — a series Schottky in the F9 branch. Adopted, and in the Altium design as of
2026-09-18**; recorded on the hardware side as R6-P1 in
`dmx_interface_dev_board_v2/docs/REV2_ALTIUM_REVIEW.md`. One part, between `F9` and
`V_LOGIC_IN`, cathode toward `V_LOGIC_IN`:

- Passes the normal direction (`V_LED` → logic), so J26 and the `USB brick` path are
  unaffected.
- Blocks `5V_LOGIC` → `V_LED`, so F1/D4 can still boot the logic while the strips stay
  dark until the operator asks for them. That is exactly the behaviour the menu promises.
- **Cost: ~0.35–0.4 V and ~0.25 W** at the ~0.7 A logic load. `5V_LOGIC` becomes ~4.6 V,
  `VSYS` ~4.3 V after D2 — far above the module buck's floor. `5V_LVL_SHIFT` drops with
  it, so the SN74ACT245 swings ~4.6 V into a WS2812 V_IH of 0.7 × V_LED = 3.5 V: still
  comfortable, but note this breaks the "level shifter and strips track the same rail"
  property in ARCHITECTURE §7 item 4, which is worth stating rather than discovering.
- Placing it before the L2/U6 fork means one part covers every build variant; a
  5 V-only variant part would avoid the drop on 12/24 V builds, at the cost of another
  BOM line.

**Part: B340A-13-F** (40 V, 3 A, SMA) — **D2 is already a B340A**, so this adds no new
BOM line and no new footprint to a board that is already managing 5 V / 12 V / 24 V
variants. Against the duty: ~0.5–0.7 A logic load (F9 sizing, review §173) into a 3 A
part, and a worst-case reverse of ~4.65 V (`5V_LOGIC` back-feed with `V_LED` collapsed)
against 40 V. Both comfortable, and the current headroom buys a lower V_f at the
operating point. Confirm stock and price on the live product page rather than from the
review’s 2026-09-03 figures.

Two things about it that are easy to trip over:

- **The drop only costs anything on 5 V builds.** On 12/24 V builds the diode sits in
  front of U6, which *regulates*, so `5V_LOGIC` is untouched and the 0.4 V comes off a
  12 or 24 V input U6 does not care about. That is also the only variant that needs the
  fix, so fitting it everywhere is simply harmless.
- **Schottky leakage means "dark", not "dead".** Reverse leakage at ~5 V is tens to
  hundreds of µA over temperature, flowing the way this part exists to block, and
  `V_LED` carries a 10 k bleed (R36). On a bare board that can hold the rail at a
  meaningful voltage with nothing connected, which reads like the fix has not worked.
  **Test with the strips attached** — their quiescent draw swamps it and nothing lights.
  A silicon diode would leak far less but costs 0.7 V, pushing `VSYS` to ~3.9 V and
  eating most of the margin; the Schottky is the better trade.

**Decision (2026-09-18): the Schottky, and nothing else.**

A J26 present-detect was considered as a complement, so firmware could apply the budget
whenever the strips run off USB however they got there. **It was rejected because it
does not work**, and the reasoning is recorded here so it is not re-proposed:

- **There is no J26 node to sense.** J26 pins 5–8 *are* `V_LED` (review §55).
- **J26 and the brick are ~100 mV apart by design.** Review §87 trims the J26 PSU to
  ≥ 5.1 V so it carries the load when both are attached; the brick through U14 measures
  5.00 V. Through a divider of the existing 10 k/20 k style that is **67 mV** at the pin,
  inside a TCA9555 dead band of roughly **1.32 V** (0.3·V_CC to 0.7·V_CC). A logic input
  cannot resolve it, and is off by a factor of twenty rather than marginally.
- A single divider cannot even do plain presence across builds: ≥ 0.46 ratio is needed to
  reach V_IH at 5 V, ≤ 0.14 to stay under 3.3 V at 24 V.
- The ADC *could* resolve it — 8.2 k/10 k on a 5 V build puts J26 and the brick 68 counts
  apart — but **all four ADC pins are allocated** (GP26/27 I²C, GP28 FTDI_TX, GP29 VSYS).
  The only opening is GP28, unused in practice while U2 is unfitted. That is a pin
  reassignment, not two resistors.

With the diode fitted the real hazard is closed: a PC on J2 can no longer be loaded by
the LED rail. The residual is the D3/F18 module-USB dev feed — bench-only, fused at
0.5 A, and already visible to firmware through P06 if a policy is ever wanted. Policing
that with an ADC channel and a pin reassignment is a large board change for little
return.

### The DTR gate silenced the logger (2026-09-17, reverted)

The first cut of `usb_log.rs` held the drain until the host raised **DTR**, so
that nothing reached the endpoint before the port was opened - a way of
sidestepping the data-toggle bug without touching the driver. It also meant
total silence on any terminal that does not raise DTR, which is what the bench
saw: the port opened first time and printed nothing, button presses included.

The gate is gone. It was a workaround, and `usb_toggle.rs` fixes the toggle at
its source for every endpoint, so nothing depends on it. The drain now writes
whenever the endpoint will take bytes, as `embassy-usb-logger` did - the
backlog still survives, because `write_packet` blocks once the endpoint is
full and the rest waits in the 4 KB pipe.

Re-test: flash `pico2-usb.uf2`, open the **third** CDC port (highest COM number
— Enttec widget, console, log, in interface order). It should connect first
time, print the boot log from `DMX interface starting on RP2350` onwards, and
survive a close and reopen. Check the Enttec widget and console ports still work
too: the wrapper sits under all three.

In `USB>DMX` the module enumerates as an **FT232R with one interface**, so only
one port exists and it is not a CDC one. Windows keeps showing the three CDC
entries from the other identity, greyed out as "not present"; they cannot be
opened, and that is correct rather than a fault. There is no logging interface
in that mode - use the default build and a probe to debug `USB>DMX`.

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
