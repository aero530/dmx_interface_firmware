# DMX Interface Rev 2 — Architecture

Why this firmware is shaped the way it is. Everything here is a decision with a
reason behind it, written for whoever picks the project up next — including a
later version of the person who built it.

Companion documents: [BRINGUP.md](BRINGUP.md) is the bench procedure,
[ft232rnl-eeprom.md](ft232rnl-eeprom.md) covers per-unit provisioning, and the
board design, its reviews and the as-built diagrams live in the hardware repo at
[../../dmx_interface_dev_board_v2/docs/](../../dmx_interface_dev_board_v2/docs/).
The build-out history is in git; the hardware decision trail — particularly the
USB power path — is in that repo's `REV2_ALTIUM_REVIEW.md`.

---

## 1. What the product is

A lighting-control box. It ingests DMX-512 from four possible sources and drives
eight WS2812 strings:

```
 DMX-512 in/out ──RS-485 (isolated)──┐        Art-Net / sACN controller
                                     │              │ UDP 6454 / 5568
 PC (QLC+ etc.) ──USB-C──► FT232RNL ─┤ UART0        │ Ethernet
 PC (any)       ──USB-C──► module USB (CDC: Enttec widget + console)
                                     ▼              ▼
                        W6300-EVB-Pico2 (RP2350 + W6300)
                                     │ 8× data, level-shifted to 5 V
                                     ▼
                       8 strings, ≤ 600 RGB / 450 RGBW each
```

Settings live on a 172×320 TFT menu, a USB console, and an on-board EEPROM.

The hardware is two boards: a **logic board** carrying the module, the isolated
DMX front end, the FTDI port, the UI and eight PTC-fused aux/data outputs; and an
**LED Power Board** that takes the PSU and fans it out through eight 5 A blade
fuses, so strip current never crosses the logic board. They join at J26.

## 2. Why the W6300-EVB-Pico2

The board wires the W6300's SCLK to GPIO17, which is a CSn mux position —
hardware SPI0 cannot reach it in **any** mode. PIO is therefore mandatory even
for plain single SPI. That costs one state machine and eight GPIOs (GP15–22)
against a W5500 board's zero and six.

**Net cost: two LED strings** (eight instead of ten). It buys the QSPI upgrade
path, and one other thing that turned out to matter more — see §10.

The transport stack uses released crates only:

```
embassy_rp::pio_programs::spi::Spi      (async, DMA-backed, impl SpiBus<u8>, 1 SM)
  └─ embedded_hal_bus::spi::ExclusiveDevice
       └─ embassy_net_wiznet::Device    (W6300, MACRAW mode)
            └─ embassy-net / smoltcp
```

MACRAW puts smoltcp underneath `embassy-net`, so the networking API is identical
to the STM32 generation this replaced — the Art-Net task ported unchanged.

## 3. Crate layout, and the line between them

| Crate | Target | Role |
|---|---|---|
| `common/` | any (`no_std`) | Everything that names no peripheral |
| `pico2/` | RP2350 (`thumbv8m.main-none-eabihf`) | The firmware: peripheral setup, PIO programs, executor wiring |
| `host_tests/` | host | Tests `common` directly — 71 tests |
| `dmx_console/` | host | Desktop settings/monitor app |
| `xtask/` | host | `cargo uf2`: builds pico2 and converts the ELF to an RP2350 UF2 for BOOTSEL flashing |
| `rp2040_dmx/` | RP2040 | Rev 1 bench firmware, kept for the FT232R emulator |
| `nucleo/` | STM32H563ZI | **Frozen.** The previous generation, kept as the record of the extraction |

**The dividing line is a concrete peripheral type or `#[embassy_executor::task]`.**
A signature naming `Spi<'static, Async>` stays in `pico2/`; the logic it drives
and the messages it passes do not. `event_router` is the worked example: the
routing lives in `common`, a ~20-line task wrapper in `pico2/`.

`common/` has **no HAL dependency** — not `embassy-rp`, not `embassy-executor`.
That is what makes `host_tests` possible: the settings model, field metadata,
protocol parsers, addressing rules and the LED current budget are all testable on
a PC without a board.

## 4. Two cores, and the claim the design rests on

| Core 0 | Core 1 |
|---|---|
| W6300 + smoltcp, Art-Net, sACN, USB device, FTDI widget, event router, EEPROM, watchdog | TFT UI, buttons, wired DMX (PIO), LED render (PIO ×8) |

The split exists for one reason: **network load must not make the UI laggy.** A
console flooding 32 universes at 44 Hz is 6.5 Mbps that core 0 has to ingest and
mostly discard; the menu still has to respond to a button press. That claim is
the architecture's load-bearing assumption and is on the bench list.

Consequences that are easy to get wrong later:

- Both shared buffers use `CriticalSectionRawMutex` (not `ThreadModeRawMutex`,
  which is `cortex-m`-only and unsound across two cores) with embassy-rp's
  hardware-spinlock `critical-section` implementation.
- Lock ordering is `LED_COLORS` → `DMX_BUFFER`, and only the router takes both.
  Everywhere else takes one. No inversion path exists.
- `LED_COLORS` is held only for the staged copy, never across the 18 ms render.
- The button poll doubles as core 1's watchdog check-in. If core 1 stops
  scheduling, the 4 s watchdog resets the box.

## 5. The data path

Two shared buffers:

- **`DMX_BUFFER`** — flat 64 × 512 B universe buffer that every input writes into.
- **`LED_COLORS`** — per-port RGBW arrays (8 × 600) the output renders from. In
  RGB mode the white byte is zero and never transmitted.

On every packet the router maps `DMX_BUFFER` → `LED_COLORS` applying start
address, group size, Individual/Mirror and universe offsets, then pings the LED
task.

### The addressing convention

The one genuine subtlety, and the source of a long-lived bug (N18 in the 2026-07
review). The two input families store a universe differently:

| Family | Storage | A 1-based address indexes |
|---|---|---|
| Wired DMX, USB | DMX-style: start code at index 0, channel N at index N | directly |
| Art-Net, sACN | no start code: channel 1 at slot offset 0 | one lower |

`event_router::buffer_index` is the single place this is reconciled, and
`host_tests/tests/addressing.rs` pins it: **the same configured address must
select the same channel whichever input is feeding the box.** Change that
function and the tests will tell you what you broke.

### Rendering

Eight PIO state machines clock all strings out concurrently, so frame time is set
by the longest string, not the total. Each frame is packed off the shared buffer
into a staging copy as 24-bit GRB or 32-bit GRBW per the colour-mode setting —
one driver, no re-init on a mode change — and only the configured LED count is
sent. The packers live in `common::ws2812_pack` so the word-boundary maths is
host-tested.

## 6. Budgets

### Bandwidth — it is a byte budget, not a colour mode

Load is set by *bytes per port per frame*. Colour mode and LED count are two ways
of spending the same budget, which is why RGBW needs no separate plan, just a
shorter maximum string. Universe allocation is per-port, so the count is
`8 × ceil(bytes_per_port / 512)`.

| Bytes/port | RGB LEDs | RGBW LEDs | Universes | Mbps @ 44 Hz | Strip frame | Strip ceiling |
|---|---|---|---|---|---|---|
| 900 | 300 | 225 | 16 | 3.2 | 9.0 ms | 111 Hz |
| 1350 | 450 | 338 | 24 | 4.9 | 13.5 ms | 74 Hz |
| **1800** | **600** | **450** | **32** | **6.5** | **18.0 ms** | **56 Hz** |
| 2400 | 800 | 600 | 40 | 8.1 | 24.0 ms | 42 Hz |

RGBW at 450/string is byte-for-byte identical to RGB at 600/string.

Two ceilings apply, and **the strip one bites first**: WS2812 is 30 µs/LED,
SK6812 RGBW 40 µs/LED, so at 2400 B/port the strip alone needs 24 ms — a 42 Hz
ceiling already below the 44 Hz Art-Net rate. Above roughly 2400 B/port the
WS2812 protocol is the wall, not the link, **which makes QSPI insurance rather
than a dependency**.

**Measured, 2026-09-15:** the boot probe settled the transport at **12 MHz**,
not the 20 MHz this table was drawn against — 20 and 16 MHz both failed the
version read (see §11). Single-SPI MACRAW at 12 MHz realistically delivers
6–9 Mbps against the 6.5 Mbps the 1800 B/port row needs, so **the design point
still fits but the margin is thin**, and the row below it does not. Confirm with
the Stage 7 flood before treating 600 LEDs/port as a supported configuration.
This is the number QSPI buys back, and the first real argument for doing it.

The ingest risk is *broadcast* Art-Net, not your own universes. Art-Net 4 pushes
controllers to unicast, driven by ArtPollReply — which is why that packet being
correct matters operationally and not just for spec compliance.

### Memory (RP2350 has 520 KB)

| Item | Size |
|---|---|
| `DMX_BUFFER`, 64 universes | 32 KB |
| `LED_COLORS` 8 × 600 × 4 | 19.2 KB |
| WS2812 PIO/DMA buffers | 19.2 KB |
| Staging copy | 14.4 KB |
| Heap: two Ratatui 35×11 cell buffers (30.8 KB, measured at 40 B/cell) + menu strings | 64 KB |
| smoltcp + MACRAW RX | ~48 KB |

Comfortably inside 520 KB. flip-link guards core 0's stack; core 1's 16 KB
static stack has no guard, hence the margin.

### PIO — 9 state machines across 3 blocks

| Block | Contents | Strings |
|---|---|---|
| PIO0 | SPI transport (1 SM, ~12 instr) + WS2812 | 3 |
| PIO1 | WS2812 | 4 |
| PIO2 | DMX in + DMX out (2 SM) + WS2812 (23/32 instr) | 1 (+1 SM spare) |

### Pins — 18 usable, none spare

| Function | Pins |
|---|---|
| WS2812 ×8 | GP0–GP7 |
| DMX RX / TX / DE | GP8 / GP9 / GP10 |
| TFT SPI1 — MOSI / DC / SCK | GP11 / GP12 / GP14 |
| FT232RNL UART0 — TX / RX | GP28 / GP13 |
| I²C1 — SDA / SCL | GP26 / GP27 |
| W6300 (on-module, never connect) | GP15–GP22 |

GP23/24/25/29 are absent by design — the module keeps them internal (buck mode
pin, VBUS sense, user LED, VSYS/3 ADC).

Two pins deserve their history. **GP28** was the TCA9555 `INT` line; polling the
buttons instead freed the only uncommitted UART0 TX, and **GP13** — the former
spare — is a UART0 RX. Together they are what makes the FTDI port possible at
all. The display's `CS` and `RES` live on the expander, so neither costs a pin.

**Do not move DMX `DE` or display `DC` onto the expander.** `DE` switches
per-frame against PIO output timing and `DC` toggles per-transaction at SPI
speed; an I²C round trip is orders of magnitude too slow for either.

## 7. Power

Three sources can feed `V_LED` on the logic board, and any one of them boots the
logic:

| Source | Path | Strips |
|---|---|---|
| **J26 from the LED Power Board** | the PSU, through blade fuses on the other board | full power |
| **5 V brick on the FTDI USB-C** | U14 TPS2553-1 switch → F2 2 A PTC | ≤ 1.45 A, firmware-budgeted (5 V build only) |
| **Module USB** | D3 → F18 0.5 A PTC | no — bench feed only |

Logic branch: `V_LED` → F9 → L2 (5 V build) or U6 OKI-78SR (12/24 V) →
`5V_LOGIC` → D2 → `VSYS` → the module's SGM62112 buck and U9 TLV75733 for the
carrier 3V3.

Four things here are deliberate and worth not undoing:

1. **Feed VSYS, never back-drive 3V3.** Rev 1 tied carrier 3V3 to the Nucleo's
   3V3 pin through an unannotated 0 Ω link — two regulators fighting on one node
   with no ORing. That is the leading suspect for the dead Nucleos (§10). The
   module's internal Schottky and D2 form a diode-OR instead; `3V3(OUT)` carries
   no carrier load at all.
2. **The carrier 3V3 LDO is enable-gated by the module's 3V3_OUT** (JP3 → R26).
   Ungated, the LDO wins the race at every power-up and back-injects ~15 mA into
   unpowered, non-failsafe RP2350 pads through the I²C pull-ups and opto anodes.
3. **The logic branch is star-fed off the LED rail.** Eight strings is a ~288 A
   theoretical node; the rail sags hard. The RP2350 is not immune — the module
   regulator is a *buck*, so below ~3.5 V VSYS it passes the input straight
   through and IOVDD follows it down. Logic and DMX brown out together at
   VSYS ≈ 3.6 V.
4. **`5V_LVL_SHIFT` taps the logic 5 V, not VSYS**, so the SN74ACT245's output
   swing references the same rail the WS2812 VIH threshold does and the two track
   together — including on 12/24 V builds, where the strips run at rail voltage
   but the data must stay 5 V.

### The USB-brick path

`LedPower` (System page, or `set led_power external|usb`) is `External` by
default, and that default is a safety property: the same connector takes a PC,
and a host port must never be asked to run LEDs.

The switch closes only when the brick is **both** selected and actually present
(VBUS on TCA9555 P05). `FAULT` on P07 is the TPS2553-1's latch indicator — the
`-1` part stays off after an overcurrent or reverse-voltage event until EN is
toggled — so the firmware drops EN, waits ~2 s, retries, and after three
attempts stays off until the setting changes. Hammering EN into a real short is
how you destroy the switch.

`common::usb_power` holds a per-frame current budget of 1.3 A, under the
switch's 1.34 A *minimum* limit. **That only covers about 21 full-white RGB
pixels**, so brick mode visibly dims heavy content — intended, and the reason
the title row shows `PWR usb` (or `PWR flt`) when it is active.

## 8. Settings, persistence and boot

Settings are bincode-encoded `MenuData` in an M24C02 at 0x56:

| Address | Contents |
|---|---|
| `0x01` | liveness |
| `0x02–0x07` | MAC address |
| `0x10` | boot-success flag |
| `0x11` | **settings schema version** |
| `0x20–0x9F` | settings, 128 B |
| `0xA0` | consecutive-incomplete-boot counter |

`MenuData` is bincode with no self-describing header, so a field change would
otherwise mis-decode an old EEPROM into plausible-looking garbage. The schema
byte turns that into a detected mismatch and a clean fall back to defaults, and
is written *last* so a torn write fails safe. Schema history: 2 added
`sacn_universe` and `backlight`; 3 the static-IP fields; **4** `led_power`, and
removed the dead PWM module variants.

`host_tests` asserts the worst-case encoding leaves real margin in the 128 B
slot, so the next field added fails there first rather than silently on hardware.

### Unit identity

Two values must differ between otherwise identical units, and both come from the
RP2350's factory **OTP chip ID** (`pico2/src/identity.rs`), so a board straight
off the line is already distinguishable with no provisioning step: the **USB
serial-number string** (the 64-bit ID as hex — a host that sees two devices with
one serial treats them as one, and Windows reuses the COM-port binding), and the
**MAC fallback** used until the EEPROM has one programmed (`02:44:4D` + the low
24 bits of the ID, locally-administered). The programmed MAC at 0x02–0x07 still
wins when present.

### Boot ladder

Read boot flag, module type and settings → count consecutive incomplete boots →
write flag = `Failed` → bring Ethernet up (DHCP, or the Network page's static
address) unless disabled or **two boots in a row were incomplete** → spawn the
network tasks → write flag = `Success`.

Two failure-mode decisions in there: the lockout guard needs *two* consecutive
bad boots, not one, because a single interrupted boot is normal during
development; and if the EEPROM will not confirm the success write, **the local
result is authoritative** — a dead settings chip must not black out the lights.

The SO8N footprint also takes an M24256 (32 KB) if presets or a fault log ever
appear. That costs no PCB change but two firmware ones: 16-bit internal
addressing (the M24256 sends two address bytes) and `PAGE_SIZE` 16 → 64.

## 9. The firmware ↔ board contract

Every line below was verified against the Altium netlist. If any of it changes on
a future board, these are the places the firmware has to follow.

| Function | Firmware | Board |
|---|---|---|
| WS2812 ports 1–8 | `pins::WS2812 = [0..7]` | U1 1,2,4,5,6,7,9,10 → U10 A8..A1 → B8..B1 → J1,J3,J13,J16,J20,J21,J24,J25 (straight) |
| DMX RX / TX / DE | GP8 / GP9 (12 mA) / GP10 | pins 11 / 12 / 14 → U8 out, R9→U3, R12→Q1 gate |
| TFT MOSI / DC / SCK | GP11 / GP12 / GP14 | pins 15 / 16 / 19 → J4 4 / 6 / 3 |
| TFT RES / CS / BL | expander P10 / P11 (output, held low) / PCA9633 LED0 | P10 → J4.5, P11 → J4.7, U11 → J4.8 |
| FTDI UART | TX GP28 → RXD, RX GP13 ← TXD, 8N2, baud hunt | pins 34 / 17; **JP1 and JP2 both bridged 1-2** = straight |
| I²C1 | GP26 / GP27, 400 kHz | pins 31 / 32, R27/R28 2k2 pull-ups |
| TCA9555 | 0x20; P00–P03 buttons active-low; P04 USB_LED_EN out; P05/P06 VBUS sense; P07 FAULT in; INT → TP1 only | A0 = **JP4 bridged 1-2** (GND), A1/A2 = R29/R30 to GND; J6 buttons to GND |
| M24C02 | 0x56 | E1/E2 = R33/R34 to 3V3, E0 = **JP5 bridged 1-2** (GND); WC → R32 → GND (writes enabled) |
| PCA9633 | 0x62, MODE2 OUTDRV=1, LED0 = PWM0 | fixed address |
| W6300 | CS 16, SCLK 17, MOSI 18, MISO 19, INT 15, RST 22 | on-module |
| Heartbeat | GP25 | module user LED |

Conventions that are not obvious from the netlist:

- **mipidsi gets a no-op reset pin, deliberately.** Its builder sends `SWRESET`
  only when no reset pin is supplied. The real RES is on the I²C expander and so
  cannot be an `OutputPin`, but passing `NoResetPin` made mipidsi issue a
  `SWRESET` *after* the expander's hardware pulse — a combination this ST7789P3
  does not recover from, and which cost a day of bring-up to find because the SPI
  bus is write-only and the panel simply stays dark. `tft_ui::NoReset` steers
  mipidsi down the hardware-reset branch while the expander does the real work.
  Do not "simplify" it back to `NoResetPin`.
- **CS is statically low and that is fine.** Rev 1 framed every transfer with CS
  via `ExclusiveDevice`; this panel does not require it, verified on the bench.
  That is what lets CS live on the expander at all.
- **DMX direction is fail-safe receive.** `DE` low = receive. GP10 drives Q1's
  gate, not the opto LED, so *any* undriven state — boot, reset, unflashed,
  crashed — lands in receive and cannot jam the bus. GP10 high = transmit.
- **GP9 idle-high** = opto LED off = RS-485 DI idle-mark, which is consistent
  with the TLP2368's inverted output.
- Buttons rely on the **TCA9555's internal pull-ups**; the board has none.
- The FTDI port is **self-powered**, so `RESET#` follows VBUS through R24/R25.
  No host, no enumeration, no back-drive into a sleeping PC.

## 10. The Ethernet failure — undiagnosed, permanently

The reason this generation exists. Multiple NUCLEO-H563ZI boards lost Ethernet
permanently — unrecoverable by power cycle, MCU otherwise fine. Repeated
failures across boards points at a systematic cause. **The dead boards have been
disposed of, so the mechanism will never be established.** Two candidates were
never separated:

| Candidate | Rev 2 status |
|---|---|
| Carrier-side 3V3 rail contention (two regulators, no ORing) | **Removed by construction** — the VSYS scheme deletes the contention outright |
| Cable-borne surge / ESD / ground-potential difference through the RJ45 | **Not addressed** — the W6300 has an integrated PHY behind an unprotected magjack, the same exposure |

Mitigations, since diagnosis is off the table: the module is **socketed**, so a
failure is a 30-second swap rather than a scrapped carrier; an inline Ethernet
surge protector is worth it in any venue where the switch and the DC supply are
on different circuits, which in stage lighting is most of them.

The one real advantage of the W6300 over a PHY: **a dead Ethernet chip is
detectable in firmware.** It is a hardwired TCP/IP controller behind SPI, so the
driver reads its version register and can distinguish *chip dead* (SPI reads
garbage) from *link down* (chip responds, no link) from *no DHCP* (link up, no
address). Those three point at completely different causes and previously all
looked identical.

**Rev 2 is itself the experiment.** If it runs clean, the contention was the
cause. If Ethernet dies again it is cable-borne, and the answer is protection at
the RJ45 — which means a carrier-mounted magjack with TVS on the MDI pairs, i.e.
a bare-W6300 design with differential-pair layout. Rightly deferred until there
is evidence it is needed.

If it does happen again, **record the circumstances**: what was plugged in and in
what order, whether Ethernet was hot-plugged, whether the switch and supply
shared a circuit, cable length and route, and whether anything else on the run
failed at the same time. That context is now the only diagnostic input left.

## 11. Known limits and future work

Behavioural limits worth knowing before they are mistaken for faults:

- **Wired DMX from a console that sends fewer than 512 slots can glitch during a
  TFT redraw.** The PIO receiver ends a short packet by noticing the DMA has
  stalled for 4 ms — a check the DMX task makes cooperatively on core 1. mipidsi
  is blocking, so a full redraw (~25 ms for the first frame, a few ms per
  keypress after) can hold that check off long enough for the next packet's
  BREAK and data to land in the same buffer, and channels above the console's
  slot count carry garbage for one frame. Consoles sending full 512-slot frames
  are unaffected (the DMA completes exactly and the PIO stalls on a full FIFO
  until the next read). The fix is an async display path; not worth it unless
  the bench shows it.
- **The W6300 SPI clock is measured at boot, and then deliberately derated.**
  The PIO SPI latches MISO at the rising edge of SCK; the W6300 presents each
  bit on the preceding falling edge. A read is correct only while the SCK half
  period exceeds the chip's output delay plus whatever the sampling path adds.
  When it is not, **it fails silently** — every byte arrives shifted one bit
  right, because each sample catches the previous bit. The first board read its
  version register as `0x08` (exactly `0x11 >> 1`) and looked like a dead chip.

  Two things came out of chasing that, 2026-09-15:

  1. **The input synchroniser was costing most of the clock.** Every PIO input
     passes through two flops, so the value latched is the pin as it was ~2
     `clk_sys` cycles (13.3 ns) earlier — pure lag, dragging the effective
     sample from mid-bit back to roughly a quarter of the way in. `main.rs`
     bypasses it for GP19 only. Safe here because MISO is not asynchronous: the
     W6300 clocks it out from the SCK this state machine generates. Ceiling went
     **15 → 24 MHz**.
  2. **The chip's output delay is 19–20 ns**, and that is now the limit. Two
     independent measurements agree: 15 MHz pass / 16 fail through the
     synchroniser implies 17.9–20.0 ns; 24 pass / 26 fail bypassed implies
     19.2–20.8 ns.

  `w6300::PROBE_FREQS_HZ` walks 32 → 2 MHz at boot, accepting a rate only after
  16 consecutive clean reads, and reports the ceiling as `eth_max=`. The link
  then runs at `OPERATING_MAX_HZ` (20 MHz), reported as `eth_hz=`. **The two are
  kept apart on purpose:** running at the 24 MHz ceiling would leave under 2 ns
  of margin against an output delay that moves with temperature, supply and
  part-to-part spread, and the failure corrupts *register* reads — which carry
  the driver's RX pointers and frame sizes, so it can wedge the link rather than
  merely drop a frame. 20 MHz leaves ~5 ns, a quarter of a bit, and costs
  nothing §6 can spend.

  Watch `eth_max=` over a board's life: a falling ceiling is the transport
  degrading, and nothing else reports it.
- **The TFT SPI clock is held at 5 MHz by the board, not by the panel.**
  `DISPLAY.SCK` is measurably capacitive — analog scope shows a sine rather than
  a square above ~10 MHz, a 58 % swing at a 52 ns half-period, implying an RC
  time constant near 20 ns on that net. The RP2350 pad defaults to 4 mA drive.
  Rev 1 ran this same panel model at 100 MHz requested (~50 MHz actual), so the
  limit is the net, not the controller. **The fix, if it is ever worth making:
  raise the SCK and MOSI pad drive to 12 mA in `main.rs` and walk `TFT_SPI_HZ`
  back up while watching the waveform.** Nothing needs it today — the only cost
  of 5 MHz is the first full-screen redraw (~110 KB, a few hundred bytes per
  keypress after that), and display timing is not in any critical path. Noted
  because it is a real board characteristic that will look like a new fault to
  whoever meets it next; it is not, and it was **not** the blank-panel bug.
- **The panel's corners are radiused, so the grid is inset one column each
  side** (`tft_ui::CORNER_INSET`): 33 of 35 columns are usable. Measured at
  about one character clipped per corner on the top and bottom rows. The inset
  applies to every row so the label column stays straight.
- **sACN priority and sequence are not honoured.** Two sources on one universe
  interleave last-wins rather than the higher priority winning; out-of-order
  packets are rendered as they arrive. Fine for a single console.
- **ICMP echo reply is a feature flag, not a given.** smoltcp 0.13 moved the
  automatic reply behind `auto-icmp-echo-reply`, which is in its default set —
  and embassy-net depends on smoltcp with `default-features = false`, so nothing
  enables it implicitly. Missed until 2026-09-16, when the node was found
  holding a DHCP lease and passing Art-Net while ignoring every ping. It is
  enabled in `pico2/Cargo.toml`. Worth knowing because the symptom looks like a
  network fault rather than a build-configuration one: ARP works, DHCP works,
  Art-Net works, and only the one thing an installer reaches for first is dead.
- **Exactly one task may `wait_*` on the network stack.** `embassy-net` keeps a
  single-slot `WakerRegistration` for stack-state changes, so a second waiter on
  `wait_config_up`/`wait_link_up` silently displaces the first and the displaced
  task is never woken again — no error, no log, it just stops. The Art-Net task
  is that waiter. Anything else that needs link or address state **polls**
  (`is_link_up`, `config_v4`), which is what `w6300::net_watch_task` does. This
  bit once, 2026-09-15: a diagnostic task added to report link state took the
  slot and its flag never appeared. Had the spawn order been reversed it would
  have hung Art-Net instead, which is the same bug with much worse symptoms.
- **Network status has one writer.** `net_watch_task` derives the reported
  address from the stack every 250 ms rather than latching whatever the boot
  sequence or the Art-Net task saw first. That removes an ordering race over who
  reported last — the symptom was `info` showing a valid `ip=` alongside
  `net=Dhcp` — and it means a DHCP renewal onto a different address, or a lease
  lost with the cable, now updates the title row and `info` instead of showing
  the boot-time value until the next reboot.
- **ArtPoll's target Port-Address range is parsed but not honoured** — the node
  replies to every poll. Controllers tolerate the extra replies.

Open work:

- **Phase 0 throughput ceiling has not been measured.** The 1800 B/port budget is
  reasoned, not measured; [BRINGUP.md](BRINGUP.md) stage 5 establishes the real
  number and the `SPI_FREQ_HZ` that supports it.
- **QSPI transport** waits on embassy PR #5809. It swaps in behind the same
  `embassy-net-wiznet` device, so nothing above the transport changes. Per §6 it
  is headroom, not a dependency.
- **USB PID** for the CDC device still needs allocating — free options are
  [pid.codes](https://pid.codes) or a sub-PID under Raspberry Pi's VID `0x2E8A`.
- **EEPROM priming writes** (`eeprom.rs`) are a Nucleo-era shared-bus workaround;
  bench-confirm page writes without them and delete.
- Panel confirmations (pin order, controller, VCC, backlight drive) are bench
  items — the firmware half is done.
