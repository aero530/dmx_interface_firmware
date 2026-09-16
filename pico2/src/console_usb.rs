//! USB console: a small line protocol for host tooling (see `dmx_console/`).
//!
//! Runs on a dedicated CDC-ACM interface of the composite USB device. Every
//! command is one line, every response ends with `ok` or `err <reason>`:
//!
//! ```text
//! get                  -> key=value per settings field, then ok
//! set <key> <value>    -> apply one setting (persisted to EEPROM), ok/err
//! dmx <start> <count>  -> "dmx <ch> v v v ..." lines (16 per line) from the active
//!                         input's universe, in that mode's addressing, then ok
//! info                 -> mode/module/ip/mac/net/boot/diag summary, then ok
//! mac [xx:xx:xx:xx:xx:xx|clear] -> show, program, or clear the MAC. Any
//!     unicast address is accepted; `clear` reverts to the built-in one
//!     derived from the chip ID. Both take effect at the next boot.
//! provision            -> write the module-type and schema bytes (fresh EEPROM)
//! help                 -> command list
//! ```
//!
//! Settings keys and value parsing come from the same [`FieldId`] metadata
//! table that drives the on-device TFT menu, so the console automatically
//! stays in sync with the UI.
use cfg_if::cfg_if;
cfg_if! {
    if #[cfg(feature = "usb")] {
        use log::info;
    } else {
        use defmt::info;
    }
}

use alloc::format;

use embassy_rp::peripherals;
use embassy_rp::usb::Driver;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::EndpointError;

use crate::channels::{GlobalDataChannelRx, RouterChannelTx, CHANNEL_EEPROM};
use crate::event_router::{buffer_index, GlobalData, RouterEvent};
use crate::ui::{all_fields, ModuleType};
use crate::{DMX_BUFFER, DMX_UNIVERSE_SIZE};
use common::events::EepromEvent;

/// Parse `aa:bb:cc:dd:ee:ff` (or `-` separated) into a unicast MAC.
///
/// Rejects the values `eeprom::read_mac` would reject at boot — all-zero,
/// all-ones, multicast bit set — so a bad address fails here, on the bench,
/// rather than as a silent fallback in the field.
fn parse_mac(text: &str) -> Result<[u8; 6], &'static str> {
    let mut mac = [0u8; 6];
    let mut n = 0;
    for part in text.split([':', '-']) {
        if n == 6 {
            return Err("expected 6 octets");
        }
        mac[n] = u8::from_str_radix(part, 16).map_err(|_| "octets must be two hex digits")?;
        n += 1;
    }
    if n != 6 {
        return Err("expected 6 octets");
    }
    if mac == [0; 6] || mac == [0xFF; 6] {
        return Err("not a usable address");
    }
    if mac[0] & 0x01 != 0 {
        return Err("multicast bit set - not a valid source address");
    }
    Ok(mac)
}

type UsbClass<'d> = CdcAcmClass<'d, Driver<'d, peripherals::USB>>;

/// Serve the console protocol forever (reconnecting across USB sessions).
pub async fn run(class: &mut UsbClass<'_>, router_tx: RouterChannelTx, mut global_rx: GlobalDataChannelRx) -> ! {
    let mut line = [0_u8; 96];
    let mut packet = [0_u8; 64];

    loop {
        class.wait_connection().await;
        info!("Console: USB host connected");
        let mut len = 0_usize;

        'connected: loop {
            match class.read_packet(&mut packet).await {
                Err(_) => break 'connected,
                Ok(n) => {
                    for &byte in &packet[..n] {
                        if byte == b'\n' || byte == b'\r' {
                            if len > 0 {
                                let done = {
                                    let text = core::str::from_utf8(&line[..len]).unwrap_or("");
                                    handle_line(class, text, &router_tx, &mut global_rx).await
                                };
                                len = 0;
                                if done.is_err() {
                                    break 'connected;
                                }
                            }
                        } else if len < line.len() {
                            line[len] = byte;
                            len += 1;
                        } else {
                            // Overlong line: discard it entirely
                            len = 0;
                        }
                    }
                }
            }
        }
        info!("Console: USB host disconnected");
    }
}

/// Send a string, chunked into full-speed USB packets.
async fn respond(class: &mut UsbClass<'_>, text: &str) -> Result<(), EndpointError> {
    for chunk in text.as_bytes().chunks(64) {
        class.write_packet(chunk).await?;
    }
    if text.len().is_multiple_of(64) {
        class.write_packet(&[]).await?; // flush a packet-aligned transfer
    }
    Ok(())
}

fn current(global_rx: &mut GlobalDataChannelRx) -> GlobalData {
    global_rx.try_get().unwrap_or_default()
}

fn mac_text(mac: Option<[u8; 6]>) -> alloc::string::String {
    match mac {
        Some(m) => format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5]),
        None => alloc::string::String::from("unprogrammed"),
    }
}

async fn handle_line(
    class: &mut UsbClass<'_>,
    line: &str,
    router_tx: &RouterChannelTx,
    global_rx: &mut GlobalDataChannelRx,
) -> Result<(), EndpointError> {
    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or("");

    match command {
        "get" => {
            let data = current(global_rx);
            for field in all_fields() {
                respond(class, &format!("{}={}\n", field.key(), field.display(&data.menu_settings))).await?;
            }
            respond(class, "ok\n").await?;
        }

        "set" => {
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            let result: Result<(), &str> = if key.is_empty() || value.is_empty() {
                Err("usage: set <key> <value>")
            } else {
                match all_fields().find(|f| f.key() == key) {
                    None => Err("unknown key"),
                    Some(field) => {
                        // Validate/parse onto a scratch copy, then have the
                        // router merge only this field into its authoritative
                        // settings (same semantics as a TFT commit), so a
                        // console `set` can't clobber a concurrent edit.
                        let mut settings = current(global_rx).menu_settings;
                        field.set_from_str(&mut settings, value).map(|()| {
                            let _ = router_tx.try_send(RouterEvent::WriteFieldToEeprom(field, settings));
                        })
                    }
                }
            };
            match result {
                Ok(()) => respond(class, "ok\n").await?,
                Err(e) => respond(class, &format!("err {}\n", e)).await?,
            }
        }

        "dmx" => {
            // Channels of the *active* universe, 1-based, whichever input is
            // feeding it. The two input families store a universe differently
            // (see `event_router::buffer_index`): wired DMX / USB keep the
            // start code at index 0 so channel N is at index N; Art-Net and
            // sACN store channel 1 at slot offset 0, and Art-Net's universe
            // sits at the configured base. Reading DMX-style regardless of
            // mode showed network data one channel late — the last place the
            // 2026-07 review's N18 was still visible.
            let start: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            let count: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(32);
            let start = start.clamp(1, DMX_UNIVERSE_SIZE);
            let count = count.min(DMX_UNIVERSE_SIZE + 1 - start);

            let settings = current(global_rx).menu_settings;
            let mode = settings.input_mode;
            let universe_base = if mode.is_artnet() {
                settings.artnet_address.buffer_base() * DMX_UNIVERSE_SIZE
            } else {
                0
            };
            let first = universe_base + buffer_index(start as u16, mode.is_network());

            let mut values = [0_u8; DMX_UNIVERSE_SIZE];
            {
                let buffer = DMX_BUFFER.lock().await;
                let end = (first + count).min(buffer.len());
                let n = end.saturating_sub(first);
                values[..n].copy_from_slice(&buffer[first..end]);
            }

            for (i, chunk) in values[..count].chunks(16).enumerate() {
                let mut out = format!("dmx {}", start + i * 16);
                for v in chunk {
                    out.push_str(&format!(" {}", v));
                }
                out.push('\n');
                respond(class, &out).await?;
            }
            respond(class, "ok\n").await?;
        }

        "info" => {
            let data = current(global_rx);
            let ip = data.menu_settings.ip_addr.octets();
            respond(class, &format!("mode={}\n", data.menu_settings.input_mode)).await?;
            respond(class, &format!("module={:?}\n", data.module_type)).await?;
            respond(class, &format!("ip={}.{}.{}.{}\n", ip[0], ip[1], ip[2], ip[3])).await?;
            respond(class, &format!("mac={}\n", mac_text(data.mac))).await?;
            respond(class, &format!("net={:?}\n", data.net_status)).await?;
            respond(class, &format!("boot={:?}\n", data.boot_status)).await?;
            // Display / expander bring-up steps reached so far (see diag.rs).
            respond(class, &format!("diag={}\n", crate::diag::text())).await?;
            respond(class, "ok\n").await?;
        }

        "mac" => match parts.next() {
            None => {
                respond(class, &format!("mac={}\n", mac_text(current(global_rx).mac))).await?;
                respond(class, "ok\n").await?;
            }
            // `mac clear` puts the unit back on its built-in address.
            //
            // Without this there is no way back. `eeprom::read_mac` falls back
            // to `identity::mac_fallback()` when the stored bytes are all 0xFF,
            // all zero, or multicast — and `parse_mac` rejects all three as
            // addresses, exactly as it should. So every value that would
            // restore the fallback is a value the console refuses to set, and a
            // unit that has been provisioned once could only be reverted by
            // rewriting the EEPROM off-board.
            Some(word) if word.eq_ignore_ascii_case("clear") || word.eq_ignore_ascii_case("default") => {
                CHANNEL_EEPROM.send(EepromEvent::WriteMacAddress([0xFF; 6])).await;
                respond(class, "ok (cleared; the built-in address applies at next boot)\n").await?;
            }
            Some(text) => match parse_mac(text) {
                Ok(mac) => {
                    // Straight to the EEPROM task: the MAC is provisioning
                    // data, not a menu setting, and it is read once at boot.
                    CHANNEL_EEPROM.send(EepromEvent::WriteMacAddress(mac)).await;
                    respond(class, "ok (applies at next boot)\n").await?;
                }
                Err(e) => respond(class, &format!("err {}\n", e)).await?,
            },
        },

        "provision" => {
            // Fresh-EEPROM bring-up: the liveness byte the boot gate reads, and
            // the settings blob (which stamps the schema version). The MAC is
            // separate on purpose — it is per unit.
            CHANNEL_EEPROM.send(EepromEvent::WriteModuleType(ModuleType::SmartLed)).await;
            CHANNEL_EEPROM
                .send(EepromEvent::WriteSettings(current(global_rx).menu_settings))
                .await;
            respond(class, "ok (module type + settings written; set the mac separately)\n").await?;
        }

        "help" => {
            respond(class, "get | set <key> <value> | dmx <start> <count> | info | mac [xx:xx:xx:xx:xx:xx|clear] | provision\n").await?;
            respond(class, "ok\n").await?;
        }

        "" => {}

        _ => respond(class, "err unknown command (try help)\n").await?,
    }

    Ok(())
}
