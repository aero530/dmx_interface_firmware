//! Target-agnostic core of the DMX interface firmware.
//!
//! Everything here compiles for any target — and for the host, which is how
//! `host_tests` exercises it. Nothing in this crate may depend on a HAL
//! (`embassy-stm32`, `embassy-rp`, …); code that names a concrete peripheral
//! type belongs in the target crate that owns the peripheral.
//!
//! The split exists because the firmware is moving from an STM32H563 Nucleo to
//! an RP2350: the modules below are the part that does not have to move.
//!
//! Contents:
//! * [`constants`] — buffer sizes, port counts, I²C addresses (re-exported at
//!   the crate root, so `common::SMARTLED_PORT_COUNT` works)
//! * [`buffers`] — the shared `DMX_BUFFER` and `LED_COLORS`
//! * [`channels`], [`events`] — the inter-task channels and their messages
//! * [`event_router`] — the routing hub and the DMX → LED mapping
//! * [`ui`] — the settings model (`MenuData` and friends) and the field
//!   metadata table that drives the menu, the console protocol, and the host app
//! * [`artnet`] — the Art-Net packet parser and ArtPollReply serialiser
//! * [`sacn`] — the E1.31 data-packet parser
//! * [`enttec_protocol`] — Enttec DMX USB Pro message framing
//! * [`usb_power`] — the USB-brick LED supply state and current budget
//! * [`ws2812_pack`] — pixel packing for the PIO output driver

#![no_std]

// `ui::fields` formats strings for the menu; the target crates provide the
// allocator (embedded-alloc), the host gets it from std.
extern crate alloc;

pub mod constants;
pub use constants::*;

pub mod activity;
pub mod artnet;
pub mod enttec_protocol;
pub mod buffers;
pub use buffers::{DMX_BUFFER, LED_COLORS};

pub mod channels;
pub mod event_router;
pub mod events;
pub mod sacn;
pub mod ui;
pub mod usb_power;
pub mod ws2812_pack;
