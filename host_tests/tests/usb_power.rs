//! Brightness budget for the USB-brick LED supply (board U14 TPS2553-1).
//!
//! Only the pure maths is exercised here. The state check that sits on top of
//! it (`budget_scale` → `brick_powering`) reads process-wide atomics, so it is
//! deliberately left out: the test harness runs in parallel threads and shared
//! mutable state would make the results depend on scheduling.

use common::usb_power::{estimate_ma, scale_for_sum, BRICK_BUDGET_MA, MA_PER_CHANNEL_FULL};

// The budget-vs-switch-limit relationship is a compile-time assertion in
// `common::usb_power`, so it needs no runtime test here.

/// Colour bytes for `leds` pixels driven to full white in RGB.
fn full_white_rgb(leds: u32) -> u32 {
    leds * 3 * 255
}

#[test]
fn an_idle_frame_is_not_scaled() {
    assert_eq!(scale_for_sum(0), 256);
}

#[test]
fn a_frame_inside_the_budget_passes_through_at_unity() {
    // A full-white RGB pixel draws 60 mA, so the 1.3 A budget only covers
    // about 21 of them — brick mode is genuinely tight, which is the whole
    // reason the scaling exists.
    let sum = full_white_rgb(20);
    assert_eq!(estimate_ma(sum), 20 * 3 * MA_PER_CHANNEL_FULL);
    assert!(estimate_ma(sum) < BRICK_BUDGET_MA);
    assert_eq!(scale_for_sum(sum), 256);
}

#[test]
fn the_budget_covers_about_twenty_one_full_white_pixels() {
    // Documents the practical ceiling: a fully lit strip of any length gets
    // scaled, but a typical low-brightness scene runs unclipped.
    let per_pixel_ma = 3 * MA_PER_CHANNEL_FULL;
    let pixels_at_full = BRICK_BUDGET_MA / per_pixel_ma;
    assert_eq!(pixels_at_full, 21);
    assert_eq!(scale_for_sum(full_white_rgb(pixels_at_full)), 256);
    assert!(scale_for_sum(full_white_rgb(pixels_at_full + 5)) < 256);
}

#[test]
fn the_budget_boundary_itself_is_not_scaled() {
    // Largest channel sum whose estimate still lands on the budget.
    let sum = BRICK_BUDGET_MA * 255 / MA_PER_CHANNEL_FULL;
    assert_eq!(estimate_ma(sum), BRICK_BUDGET_MA);
    assert_eq!(scale_for_sum(sum), 256);
}

#[test]
fn a_frame_over_budget_is_scaled_back_under_it() {
    // The worst case the hardware allows: 8 × 600 LEDs at full white.
    let sum = full_white_rgb(8 * 600);
    let scale = scale_for_sum(sum);
    assert!(scale < 256, "expected scaling, got {scale}");

    let scaled = (u64::from(sum) * u64::from(scale) / 256) as u32;
    assert!(
        estimate_ma(scaled) <= BRICK_BUDGET_MA,
        "scaled draw {} mA exceeds the {} mA budget",
        estimate_ma(scaled),
        BRICK_BUDGET_MA
    );
}

#[test]
fn scaling_never_reaches_zero() {
    // Even an absurd demand leaves the strings faintly lit rather than dark,
    // so an operator can see the box is running.
    assert!(scale_for_sum(u32::MAX) >= 1);
    assert!(scale_for_sum(full_white_rgb(8 * 600)) >= 1);
}

#[test]
fn scaling_is_monotonic_in_demand() {
    let mut last = 257u16;
    for leds in [600u32, 1200, 2400, 4800] {
        let scale = scale_for_sum(full_white_rgb(leds));
        assert!(
            scale <= last,
            "more LEDs must not scale up: {leds} LEDs gave {scale}, previous {last}"
        );
        last = scale;
    }
}

#[test]
fn the_displayed_percentage_never_rounds_up_to_full() {
    use common::usb_power::{scale, scale_percent, set_scale};

    // The panel shows this beside `PWR usb`, and hides it at 100. Anything
    // being held back must therefore never read as 100, or dim strips would
    // once again have no explanation on screen.
    set_scale(256);
    assert_eq!(scale(), 256);
    assert_eq!(scale_percent(), 100, "unity is the one case that may say 100");

    set_scale(255);
    assert_eq!(scale_percent(), 99, "just under unity must not round up");

    set_scale(86); // 64 full-white RGB pixels against the 1300 mA budget
    assert_eq!(scale_percent(), 33);

    set_scale(1); // the floor `scale_for_sum` clamps to
    assert_eq!(scale_percent(), 0, "still visible as 'something is very wrong'");

    // Out of range cannot overflow the u8 or claim more than full output.
    set_scale(u16::MAX);
    assert_eq!(scale_percent(), 100);
}
