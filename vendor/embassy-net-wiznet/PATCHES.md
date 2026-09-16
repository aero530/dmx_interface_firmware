# Vendored `embassy-net-wiznet` 0.3.0 — local patches

**Origin:** crates.io `embassy-net-wiznet` 0.3.0, from
https://github.com/embassy-rs/embassy (`embassy-net-wiznet/`).
**Licence:** MIT OR Apache-2.0, as declared in `Cargo.toml`; the licence texts
are at the root of the embassy repository.
**Why vendored:** the W6300 socket-0 RX buffer size is a hard-coded trait
constant with no configuration path, and upstream `main` (checked 2026-09-16)
still has it. The workspace root `Cargo.toml` points `embassy-net-wiznet` here
through `[patch.crates-io]`.

## Patch 1 — give socket 0 the whole 16 KB of RX/TX memory (2026-09-16)

**Problem, measured on the Rev 2 board.** In MACRAW mode the W6300 is used
through socket 0 only, and the driver sized its RX buffer at **4 KB**
(`Chip::BUF_SIZE = 0x1000`) — about seven full-size Art-Net frames. A
controller emits all its universes back to back, so a 32-universe burst lands
in ~1.5 ms; nine frames survived every burst and the other twenty-three were
discarded inside the chip. Ingest capped at ~440 packets/s against a 1408/s
design point, and nothing above the chip (SPI clock, socket buffer, render)
moved it. Full account: `docs/ARCHITECTURE.md` §11.

**Change.**

* `src/chip/w6300.rs`: `BUF_SIZE 0x1000 → 0x4000`. The driver writes
  `BUF_SIZE / 1024` to `Sn_RX_BSR`/`Sn_TX_BSR`, so socket 0 gets 16 KB — the
  maximum a socket may have (`Sn_RX_BSR` accepts 0/1/2/4/8/16; WIZnet
  ioLibrary `w6300.h`).
* `src/chip/w6300.rs`: `RegisterBlock` gains `Socket1..Socket7` at
  `0x01 + 4N` (WIZnet `WIZCHIP_SREG_BLOCK(N) = 1 + 4N`), and the chip
  implements `release_unused_socket_buffers`, writing 0 to `Sn_TX_BSR`
  (`0x0200`) and `Sn_RX_BSR` (`0x0220`) of sockets 1–7. **This is required,
  not optional:** the chip's 16 KB is shared across all eight sockets, the sum
  may not exceed it, and the 2 KB power-on defaults already reach it.
  Behaviour when over-committed is undefined.
* `src/chip/mod.rs`: `SealedChip` gains
  `async fn release_unused_socket_buffers<SPI>(spi) -> Result<(), SPI::Error>`
  with a no-op default, so W5500/W5100S/W6100 are untouched.
* `src/device.rs`: `WiznetDevice::new` calls it immediately **before**
  writing socket 0's buffer sizes, so the shared memory is never
  over-committed even transiently. (First cut called it after; reordered the
  same day as hygiene - the measurement that prompted the check turned out to
  be sender jitter, not a clamp, see `docs/ARCHITECTURE.md` section 11.)

**Verify:** flash, flood with `tools/dmxsend.py --count 32 --rate 44`, capture
CS/SCK, run `tools/busscope.py` — section E's *frames surviving per burst*
should rise from 9 to the high twenties and then track per-frame speed.

## Housekeeping

`src/lib.rs` carries a crate-level `#![allow]` for four upstream style lints
(`needless_borrow`, `identity_op`, `new_without_default`) that only surface
because the crate is now a workspace member and gets linted with the firmware.
No code changed for them.

## Not changed (yet)

* `SOCKET_MODE_VALUE = 0b0000_0111` leaves MAC filtering **off** because
  upstream found DHCP failed with it on. With the filter off, every frame on
  the wire crosses the SPI and occupies the buffer before smoltcp discards
  it. Worth re-testing with the filter on once Patch 1 is measured — the MAC
  filter normally still passes broadcast, so the DHCP failure may have another
  cause. Kept separate so each change is measured alone.

## Upstreaming

Patch 1 is generic enough to propose upstream as-is (the hook has a no-op
default). Diff against the 0.3.0 tag; this directory is that tag plus the
edits listed here.
