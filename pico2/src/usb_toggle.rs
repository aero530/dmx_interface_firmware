//! A USB driver wrapper that resets the endpoint data toggle on
//! `ClearFeature(ENDPOINT_HALT)`, which `embassy-rp` does not.
//!
//! # The bug this exists for
//!
//! USB 2.0 §9.4.5 is explicit: clearing `ENDPOINT_HALT` **must** reset the
//! endpoint's data toggle to `DATA0`. Both ends then agree on the PID of the
//! next packet.
//!
//! `embassy-rp` 0.10's [`Bus::endpoint_set_stalled`] clears the hardware's
//! `STALL` bit and wakes the endpoint's waker, and stops there - the buffer
//! control register's `PID` field is left alone. (`endpoint_set_enabled`, a
//! dozen lines further down the same file, does reset it, so this is an
//! omission on one path rather than a missing capability.)
//!
//! # How it shows up
//!
//! Windows' `usbser.sys` resets the bulk pipes when an application opens a COM
//! port, which is a `ClearFeature(ENDPOINT_HALT)` on the wire. The host's
//! toggle goes to `DATA0`; the device's stays where it was. If they disagree,
//! the host discards the first IN packet after the open, `usbser` reports a
//! failure, and the application sees the port drop. The discarded transfer
//! realigns the two, so the *second* attempt succeeds - which is exactly how it
//! presented on the bench, 2026-09-17: the logging CDC dropped the first
//! connection every time and held the second.
//!
//! Only the logging port showed it, because only the logging port has data
//! queued before anyone opens it. The Enttec and console interfaces stay silent
//! until spoken to, so their toggles are still at the enumeration value when
//! the host resets its own, and they agree by luck. [`crate::usb_log`] now
//! holds the log back until DTR for the same reason - but that only stops the
//! *first* open from tripping it. A close and reopen leaves the device's toggle
//! mid-stream with the host back at `DATA0`, and nothing but this wrapper
//! covers that.
//!
//! # What it costs
//!
//! One extra register write per `ClearFeature`, on a path that runs when a host
//! opens a port. Everything else delegates.

use embassy_rp::pac;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver as RpDriver, Instance};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::{
    Bus, Driver, EndpointAddress, EndpointAllocError, EndpointType, Event, Unsupported,
};

/// What the rest of the firmware passes around in place of `embassy_rp`'s.
pub type UsbDriver = ToggleResetDriver<'static, USB>;

/// A CDC-ACM interface on that driver. Generic over the builder's lifetime,
/// not `'static`: the descriptor buffers `Builder::new` borrows are locals.
pub type UsbClass<'d> = CdcAcmClass<'d, ToggleResetDriver<'d, USB>>;

/// Wraps [`embassy_rp::usb::Driver`] so the bus it starts is a
/// [`ToggleResetBus`]. Allocation and the control pipe are untouched.
pub struct ToggleResetDriver<'d, T: Instance>(RpDriver<'d, T>);

impl<'d, T: Instance> ToggleResetDriver<'d, T> {
    pub fn new(inner: RpDriver<'d, T>) -> Self {
        Self(inner)
    }
}

impl<'d, T: Instance> Driver<'d> for ToggleResetDriver<'d, T> {
    type EndpointOut = <RpDriver<'d, T> as Driver<'d>>::EndpointOut;
    type EndpointIn = <RpDriver<'d, T> as Driver<'d>>::EndpointIn;
    type ControlPipe = <RpDriver<'d, T> as Driver<'d>>::ControlPipe;
    type Bus = ToggleResetBus<<RpDriver<'d, T> as Driver<'d>>::Bus>;

    fn alloc_endpoint_out(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointOut, EndpointAllocError> {
        self.0
            .alloc_endpoint_out(ep_type, ep_addr, max_packet_size, interval_ms)
    }

    fn alloc_endpoint_in(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointIn, EndpointAllocError> {
        self.0
            .alloc_endpoint_in(ep_type, ep_addr, max_packet_size, interval_ms)
    }

    fn start(self, control_max_packet_size: u16) -> (Self::Bus, Self::ControlPipe) {
        let (bus, control) = self.0.start(control_max_packet_size);
        (ToggleResetBus(bus), control)
    }
}

/// Delegates every [`Bus`] method, and adds the toggle reset that USB 2.0
/// §9.4.5 requires when a halt is cleared.
pub struct ToggleResetBus<B: Bus>(B);

impl<B: Bus> Bus for ToggleResetBus<B> {
    async fn enable(&mut self) {
        self.0.enable().await
    }

    async fn disable(&mut self) {
        self.0.disable().await
    }

    async fn poll(&mut self) -> Event {
        self.0.poll().await
    }

    fn endpoint_set_enabled(&mut self, ep_addr: EndpointAddress, enabled: bool) {
        // Already resets the toggle upstream; nothing to add.
        self.0.endpoint_set_enabled(ep_addr, enabled);
    }

    fn endpoint_is_stalled(&mut self, ep_addr: EndpointAddress) -> bool {
        self.0.endpoint_is_stalled(ep_addr)
    }

    fn force_reset(&mut self) -> Result<(), Unsupported> {
        self.0.force_reset()
    }

    async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
        self.0.remote_wakeup().await
    }

    fn endpoint_set_stalled(&mut self, ep_addr: EndpointAddress, stalled: bool) {
        self.0.endpoint_set_stalled(ep_addr, stalled);
        if !stalled {
            reset_data_toggle(ep_addr);
        }
    }
}

/// Put one endpoint's buffer-control `PID` back to where enumeration leaves it.
///
/// The values mirror `embassy_rp::usb`'s own `endpoint_set_enabled`, and they
/// are not the same for the two directions because the driver treats them
/// differently: the IN path computes the PID of each packet as `!pid`, so the
/// stored bit is **inverted** and `true` is what makes the next packet `DATA0`.
/// The OUT path arms the buffer with the PID it expects to receive, so `false`
/// *is* `DATA0`.
///
/// This is a read-modify-write of the `PID` bit alone: `AVAILABLE` and `LENGTH`
/// are preserved, so a packet already armed is sent with the corrected toggle
/// rather than being dropped, and no buffer is handed to the hardware here.
/// That matters because the RP2350 wants a settling delay between arming a
/// buffer and marking it available, and this write never arms one.
///
/// Endpoint 0 is skipped: control transfers set their own PIDs per stage, and
/// a halt on EP0 is cleared by the next SETUP regardless.
fn reset_data_toggle(ep_addr: EndpointAddress) {
    let n = ep_addr.index();
    if n == 0 {
        return;
    }
    if ep_addr.is_in() {
        pac::USB_DPRAM
            .ep_in_buffer_control(n)
            .modify(|w| w.set_pid(0, true));
    } else {
        pac::USB_DPRAM
            .ep_out_buffer_control(n)
            .modify(|w| w.set_pid(0, false));
    }
}
