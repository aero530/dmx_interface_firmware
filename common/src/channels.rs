//! Communication channels between tasks.
//!
//! All inter-task messaging goes through the statics defined here. Most are
//! bounded `Channel`s (point-to-point queues); broadcast-style state uses
//! `Watch` (every receiver sees the latest value). The intended senders and
//! receivers are noted on each static.
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::watch::{Receiver as WatchReceiver, Sender as WatchSender, Watch};

use crate::event_router::{DmxEvent, DmxFeedbackEvent, GlobalData, MainEvent, RouterEvent};
use crate::events::{EepromEvent, SmartLedEvent, UiEvent};

pub type RouterChannel = Channel<CriticalSectionRawMutex, RouterEvent, 10>;
pub type RouterChannelRx = Receiver<'static, CriticalSectionRawMutex, RouterEvent, 10>;
pub type RouterChannelTx = Sender<'static, CriticalSectionRawMutex, RouterEvent, 10>;
/// Everything -> event router: the main event bus (buttons, settings
/// writes, EEPROM results, console commands).
pub static CHANNEL: RouterChannel = Channel::new();

pub type DmxChannel = Channel<CriticalSectionRawMutex, DmxEvent, 1>;
pub type DmxChannelRx = Receiver<'static, CriticalSectionRawMutex, DmxEvent, 1>;
pub type DmxChannelTx = Sender<'static, CriticalSectionRawMutex, DmxEvent, 1>;
/// Input tasks (dmx / artnet / sacn_rx / usb_device / enttec_uart) -> router:
/// "new frame is in `DMX_BUFFER`". Depth 1 on purpose: frames are latest-wins.
pub static CHANNEL_DMX: DmxChannel = Channel::new();

// 5 receivers: dmx_task, artnet_task, sacn_task, usb_device_task (CDC widget),
// enttec_uart_task (FT232RNL widget)
pub type DmxFeedbackChannel = Watch<CriticalSectionRawMutex, DmxFeedbackEvent, 5>;
pub type DmxFeedbackChannelRx = WatchReceiver<'static, CriticalSectionRawMutex, DmxFeedbackEvent, 5>;
pub type DmxFeedbackChannelTx = WatchSender<'static, CriticalSectionRawMutex, DmxFeedbackEvent, 5>;
/// Router -> input tasks: broadcast of the current operating mode and
/// Art-Net universe whenever settings change.
pub static CHANNEL_DMX_FEEDBACK: DmxFeedbackChannel = Watch::new();


pub type SmartLedChannel = Channel<CriticalSectionRawMutex, SmartLedEvent, 1>;
pub type SmartLedChannelRx = Receiver<'static, CriticalSectionRawMutex, SmartLedEvent, 1>;
pub type SmartLedChannelTx = Sender<'static, CriticalSectionRawMutex, SmartLedEvent, 1>;
/// Router -> smart_led_task: "`LED_COLORS` changed, push it to the strips".
pub static CHANNEL_SMART_LED: SmartLedChannel = Channel::new();

pub type UiChannel = Channel<CriticalSectionRawMutex, UiEvent, 4>;
pub type UiChannelRx = Receiver<'static, CriticalSectionRawMutex, UiEvent, 4>;
pub type UiChannelTx = Sender<'static, CriticalSectionRawMutex, UiEvent, 4>;
/// Router -> UI task: button presses translated to Up/Down/Select/Esc,
/// plus `Load` events carrying refreshed settings. Depth 4 so a `Load`
/// arriving while the UI task is mid-draw (e.g. a DHCP address update)
/// is not silently dropped.
pub static CHANNEL_UI: UiChannel = Channel::new();

// Three receivers: the USB console, the USB identity watch, and one spare. A
// `Watch` hands out a fixed number and `receiver()` returns `None` past it, so
// this is the ceiling on how many tasks can observe global state.
pub type GlobalDataChannel = Watch<CriticalSectionRawMutex, GlobalData, 3>;
pub type GlobalDataChannelRx = WatchReceiver<'static, CriticalSectionRawMutex, GlobalData, 3>;
/// Router -> observers: broadcast of the router's `GlobalData` (settings,
/// module type, MAC, boot status) after every store. Consumed by the USB
/// console task.
pub static CHANNEL_LOG: GlobalDataChannel = Watch::new();

pub type EepromChannel = Channel<CriticalSectionRawMutex, EepromEvent, 1>;
pub type EepromChannelRx = Receiver<'static, CriticalSectionRawMutex, EepromEvent, 1>;
pub type EepromChannelTx = Sender<'static, CriticalSectionRawMutex, EepromEvent, 1>;
/// Router / main -> EEPROM task: read and write requests. Results come back
/// to the router as `RouterEvent::Store*` messages.
pub static CHANNEL_EEPROM: EepromChannel = Channel::new();

pub type MainChannel = Channel<CriticalSectionRawMutex, MainEvent, 2>;
pub type MainChannelTx = Sender<'static, CriticalSectionRawMutex, MainEvent, 2>;
/// Router -> main: replies to the `Get*` requests issued during the boot
/// sequence.
pub static CHANNEL_MAIN: MainChannel = Channel::new();
