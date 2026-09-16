//! Configuration storage — M24C02 on I²C1.
//!
//! In Rev 1 this sat on the *output module* and doubled as module-type
//! identification. With one board there are no modules, so it moves onto the
//! carrier and is purely a settings store.
//!
//! # Memory map
//!
//! | Address | Contents |
//! |---|---|
//! | `0x01` | module type — now an EEPROM liveness marker (see below) |
//! | `0x02..=0x07` | MAC address |
//! | `0x10` | boot-success flag |
//! | `0x11` | **schema version** |
//! | `0x20..=0x9F` | settings, 128 B, bincode-encoded `MenuData` |
//! | `0xA0` | consecutive incomplete-boot counter (the lockout guard) |
//! | `0xA1..=0xFF` | free |
//!
//! # The boot gate, simplified
//!
//! In Rev 1 the router blocked output until it had read a module type, because
//! that identified which output module was fitted. With one board there is only
//! one valid value, so the read now means simply **"the EEPROM is present and
//! readable"** — which is exactly what the boot gate should be checking. The
//! event stays, its meaning narrows, and no router change is needed.
//!
//! # Schema version
//!
//! `MenuData` is bincode-encoded with no self-describing header, so a field
//! change would silently mis-decode an old EEPROM into plausible-looking
//! garbage. The version byte turns that into a detected mismatch and a clean
//! fall back to defaults. It lives at `0x11`, not `0x01`: the module-type byte
//! is still doing the liveness job above, and overloading one byte with two
//! meanings is how this sort of thing goes wrong later.

use common::channels::{EepromChannelRx, RouterChannelTx};
use common::event_router::RouterEvent;
use common::events::EepromEvent;
use common::ui::{BootStatus, MenuData, ModuleType};

// # No priming writes
//
// Rev 1 preceded every EEPROM write with a throw-away byte write to the same
// address — four of them, each commented "page write only works if you write a
// byte to the device first ... the bus isn't properly cleared on other read /
// write attempts". They were removed on 2026-09-16 after confirming saves
// persist across power cycles without them.
//
// They were treating a symptom that this build cannot produce. `write_page`
// puts the memory address and the data into **one** `i2c.write()`, and the
// shared bus is an `I2cDevice` over a `Mutex`, which holds the lock for the
// whole call — so the expander poll running every 20 ms on core 1 cannot
// interleave between the address byte and the data, which is what "the address
// not being transmitted" describes. Both `write_byte_wait` and
// `write_page_wait` then ACK-poll until the write cycle completes, so back-to-
// back writes are spaced by the part's own timing rather than by the accidental
// ~5 ms that a priming write used to contribute.
//
// If a write ever fails again, look at those three properties before re-adding
// a priming write: it never fixed anything, it only made the gap wider.
use defmt::*;
use embassy_time::{with_timeout, Duration};
use embedded_hal_async::i2c::I2c;

use crate::m24x02::{M24x02, PAGE_SIZE};

/// I²C address. E1/E2 strapped high, E0 low — as on the Rev 1 module, so
/// `common::EEPROM_ADDRESS` does not move.
pub const ADDR: u8 = common::EEPROM_ADDRESS;

/// Module type — retained as the EEPROM liveness marker the boot gate reads.
pub const MODULE_TYPE_ADDR: u8 = 0x01;
pub const MAC_ADDR_LOC: u8 = 0x02;
pub const SCHEMA_VERSION_ADDR: u8 = 0x11;
pub const BOOT_FLAG_ADDR: u8 = 0x10;
pub const SETTINGS_ADDR: u8 = 0x20;
/// Consecutive incomplete boots, maintained by `boot_task`; reset to 0 by a
/// completed boot. A blank byte (0xFF) reads as zero.
pub const BOOT_FAIL_COUNT_ADDR: u8 = 0xA0;

/// Bytes reserved for the settings blob — defined in `common` so the host
/// tests check the worst-case encoding against this exact value.
///
/// Measured worst-case `MenuData` is 40 B at 4 ports and 64 B at 8 — the latter
/// landing exactly on the old 64-byte slot, which is worse than overflowing
/// because a `<=` check passes right up until it does not. 128 restores real
/// margin and still ends at 0x9F. Only the pages the payload covers are
/// written, so the larger reservation costs nothing at runtime.
pub use common::SETTINGS_SIZE;

/// Bump when the encoded shape of `MenuData` changes. A stored value that does
/// not match means the settings blob is from an older layout and must not be
/// decoded.
///
/// * 1 — original Rev 2 layout
/// * 2 — `sacn_universe` and `backlight` appended; Ethernet defaults on
/// * 3 — `static_ip`, `static_prefix`, `static_gateway` appended
/// * 4 — `led_power` appended (USB-brick LED supply); the dead PWM module
///   variants removed from `ModuleType` / `ModuleSettings`
pub const SCHEMA_VERSION: u8 = 4;

/// Read the MAC address, rejecting anything that is not a usable unicast
/// address.
///
/// A blank EEPROM reads as all-`0xFF`, and a half-programmed one can read as
/// all-zero; neither is a valid source address. The multicast bit (bit 0 of the
/// first octet) being set would also be invalid for a source MAC, and is the
/// classic symptom of reading the wrong offset.
pub async fn read_mac<I2C: I2c>(dev: &mut M24x02<I2C>) -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    if dev.read_data(MAC_ADDR_LOC, &mut mac).await.is_err() {
        warn!("EEPROM: MAC read failed");
        return None;
    }

    if mac == [0xFF; 6] {
        info!("EEPROM: MAC unprogrammed");
        return None;
    }
    if mac == [0x00; 6] {
        warn!("EEPROM: MAC reads all zero");
        return None;
    }
    if mac[0] & 0x01 != 0 {
        warn!("EEPROM: MAC {:02x} has the multicast bit set", mac);
        return None;
    }

    info!("EEPROM: MAC {:02x}", mac);
    Some(mac)
}

/// Read the schema version byte, so a settings blob from an older layout can be
/// rejected rather than mis-decoded.
pub async fn read_schema_version<I2C: I2c>(dev: &mut M24x02<I2C>) -> Option<u8> {
    match dev.read_byte(SCHEMA_VERSION_ADDR).await {
        Ok(v) => Some(v),
        Err(_) => {
            warn!("EEPROM: schema version read failed");
            None
        }
    }
}

pub struct Eeprom<I2C: I2c> {
    dev: M24x02<I2C>,
    rx: EepromChannelRx,
    tx: RouterChannelTx,
}

impl<I2C: I2c> Eeprom<I2C> {
    pub fn new(i2c: I2C, address: u8, rx: EepromChannelRx, tx: RouterChannelTx) -> Self {
        info!("Define eeprom dev");
        let dev = M24x02::new(i2c, address);
        Self { dev, rx, tx }
    }

    pub async fn show(&mut self) -> Result<(), ()> {
        if let Ok(new_message) = with_timeout(Duration::from_millis(200), self.rx.receive()).await {
            self.process_event(new_message).await
        } else {
            Ok(())
        }
    }

    pub async fn refresh(&mut self) -> Result<(), ()> {
        // self.dev.write_page_wait(0x), data).await
        self.dev.refresh_bus().await.map_err(|_e| ())
    }

    async fn process_event(&mut self, event: EepromEvent) -> Result<(), ()> {
        match event {
            EepromEvent::ReadModuleType => {
                let data = self.dev.read_byte(MODULE_TYPE_ADDR).await;

                match data {
                    Ok(data) => {
                        let decoded: ModuleType = bincode::decode_from_slice(&[data], bincode::config::standard()).unwrap_or_default().0;
                        let _ = self.tx.try_send(RouterEvent::StoreModuleType(Some(decoded)));
                    }
                    Err(e) => {
                        let _ = self.tx.try_send(RouterEvent::StoreModuleType(None));
                        error!("Eeprom - error {:?}", e);
                        return Err(());
                    }
                }
            }
            EepromEvent::WriteModuleType(module) => {
                let mut slice = [0u8; 1];

                let length = bincode::encode_into_slice(module, &mut slice, bincode::config::standard()).unwrap_or_else(|e| {
                    match e {
                        bincode::error::EncodeError::UnexpectedEnd => error!("Error encoding module type - UnexpectedEnd"),
                        _ => error!("Error encoding module type"),
                    }
                    0
                });

                if length > 0 {
                    match self.dev.write_byte_wait(MODULE_TYPE_ADDR, slice[0]).await {
                        Ok(_) => {
                            let _ = self.tx.try_send(RouterEvent::StoreModuleType(Some(module)));
                            info!("Wrote module type as {:?}", module);
                        }
                        Err(e) => {
                            error!("Eeprom - error {:?}", e);
                            return Err(());
                        }
                    }
                }
            }
            EepromEvent::ReadMacAddress => {
                let mut buf = [0u8; 6];

                match self.dev.read_data(MAC_ADDR_LOC, &mut buf).await {
                    // .await {
                    Ok(_) => {
                        info!("EEPROM Read mac {:?}", buf);
                        let _ = self.tx.try_send(RouterEvent::StoreMacAddress(Some(buf)));
                    }
                    Err(e) => {
                        error!("Eeprom - error {:?}", e);
                        let _ = self.tx.try_send(RouterEvent::StoreMacAddress(None));
                        return Err(());
                    }
                }
            }
            EepromEvent::WriteMacAddress(mac) => {
                info!("EEPROM Store mac {:?}", mac);

                match self.dev.write_page_wait(MAC_ADDR_LOC, &mac).await {
                    Ok(_) => {
                        let _ = self.tx.try_send(RouterEvent::StoreMacAddress(Some(mac)));
                        info!("EEPROM MAC stored to {:?} -- {:?}", MAC_ADDR_LOC, mac);
                    }
                    Err(e) => {
                        error!("MAC store {:?} error {:?} -- {:?}", MAC_ADDR_LOC, e, mac);
                        return Err(());
                    }
                }
            }
            EepromEvent::ReadSettings => {
                // Gate the decode on the schema byte: `MenuData` is bincode
                // with no self-describing header, so a blob from an older
                // layout would silently mis-decode into plausible garbage.
                // Blank (0xFF) or mismatched schema falls back to defaults —
                // Some(default) rather than None, so the router still loads
                // the UI and broadcasts the operating mode.
                match read_schema_version(&mut self.dev).await {
                    Some(v) if v == SCHEMA_VERSION => {}
                    Some(v) => {
                        if v == 0xFF {
                            info!("EEPROM: blank, settings default");
                        } else {
                            warn!("EEPROM: schema {=u8} != {=u8}, settings default", v, SCHEMA_VERSION);
                        }
                        let _ = self.tx.try_send(RouterEvent::StoreSettings(Some(MenuData::default())));
                        return Ok(());
                    }
                    None => {
                        let _ = self.tx.try_send(RouterEvent::StoreSettings(None));
                        return Err(());
                    }
                }

                let mut buf = [0u8; SETTINGS_SIZE];

                match self.dev.read_data(SETTINGS_ADDR, &mut buf).await {
                    // .await {
                    Ok(_) => {
                        // The schema byte matched, so a decode failure here is
                        // a torn or corrupt blob, not an old layout. Falling
                        // back to defaults is still the right recovery — but
                        // say so, because it means a save did not complete.
                        let decoded: MenuData = match bincode::decode_from_slice(&buf, bincode::config::standard()) {
                            Ok((d, _)) => d,
                            Err(_) => {
                                warn!("EEPROM: settings blob failed to decode under schema {=u8} - corrupt, using defaults", SCHEMA_VERSION);
                                MenuData::default()
                            }
                        };
                        info!("Read eeprom");
                        let _ = self.tx.try_send(RouterEvent::StoreSettings(Some(decoded)));
                    }
                    Err(e) => {
                        error!("Eeprom - error {:?}", e);
                        let _ = self.tx.try_send(RouterEvent::StoreSettings(None));
                        return Err(());
                    }
                }
            }
            EepromEvent::WriteSettings(menu_settings) => {
                let mut slice = [0u8; SETTINGS_SIZE];

                let length = bincode::encode_into_slice(menu_settings, &mut slice, bincode::config::standard()).unwrap_or_else(|e| {
                    match e {
                        bincode::error::EncodeError::UnexpectedEnd => error!("Error encoding menu settings - UnexpectedEnd"),
                        _ => error!("Error encoding menu settings"),
                    }
                    0
                });

                if length > 0 {
                    info!("Store settings {:?}. Data is {:?} bytes long", menu_settings, length);

                    // Only the pages the payload actually covers. SETTINGS_SIZE is
                    // reserved space; writing all of it would spend an extra ~5 ms
                    // EEPROM write cycle per empty page and wear them for nothing.
                    // Trailing bytes from an earlier, longer save are harmless:
                    // bincode decodes the struct prefix and stops.
                    let chunks = slice[..length].chunks(PAGE_SIZE as usize).enumerate();

                    for (num, chunk) in chunks {
                        let mem_addr = SETTINGS_ADDR + (num as u8) * PAGE_SIZE;
                        // self.dev.write_page_wait(mem_addr, chunk).await;
                        match self.dev.write_page_wait(mem_addr, chunk).await {
                            Ok(_) => info!("Data chunk {:?}/{:?} stored -- {:?}", num, mem_addr, chunk),
                            Err(e) => {
                                error!("Data chunk {:?}/{:?} error {:?} -- {:?}", num, mem_addr, e, chunk);
                                return Err(());
                            }
                        }
                    }
                    // Stamp the schema version LAST, after the blob is fully
                    // written, so a write interrupted by power loss reads back
                    // as "no valid settings" rather than as a torn blob.
                    if let Err(e) = self.dev.write_byte_wait(SCHEMA_VERSION_ADDR, SCHEMA_VERSION).await {
                        error!("Schema version write error {:?}", e);
                        return Err(());
                    }
                    let _ = self.tx.try_send(RouterEvent::StoreSettings(Some(menu_settings)));
                }
            }
            EepromEvent::ReadBootStatus => {
                let data = self.dev.read_byte(BOOT_FLAG_ADDR).await;

                match data {
                    Ok(data) => {
                        let decoded: BootStatus = bincode::decode_from_slice(&[data], bincode::config::standard()).unwrap_or_default().0;
                        let _ = self.tx.try_send(RouterEvent::StoreBootStatus(Some(decoded)));
                    }
                    Err(e) => {
                        let _ = self.tx.try_send(RouterEvent::StoreBootStatus(None));
                        error!("Eeprom - error {:?}", e);
                        return Err(());
                    }
                }
            }
            EepromEvent::WriteBootStatus(status) => {
                let mut slice = [0u8; 1];

                let length = bincode::encode_into_slice(status, &mut slice, bincode::config::standard()).unwrap_or_else(|e| {
                    match e {
                        bincode::error::EncodeError::UnexpectedEnd => error!("Error encoding boot status - UnexpectedEnd"),
                        _ => error!("Error encoding boot status"),
                    }
                    0
                });

                if length > 0 {
                    match self.dev.write_byte_wait(BOOT_FLAG_ADDR, slice[0]).await {
                        Ok(_) => {
                            let _ = self.tx.try_send(RouterEvent::StoreBootStatus(Some(status)));
                            info!("Wrote boot successful flag as {:?}", status);
                        }
                        Err(e) => {
                            error!("Eeprom - error {:?}", e);
                            return Err(());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Task to manage the I2C EEPROM device
#[embassy_executor::task]
pub async fn eeprom_task(i2c: crate::I2cDev, address: u8, rx: EepromChannelRx, tx: RouterChannelTx) {
    let mut eeprom = Eeprom::new(i2c, address, rx, tx);
    loop {
        match eeprom.show().await {
            Ok(_) => {}
            Err(_) => {
                error!("EEPROM Error trying to restart");
                let _ = eeprom.refresh().await;
            }
        }
    }
}
