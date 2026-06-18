//! Write-protect probe: read the SD card's WP signal two independent ways and
//! show both on the OLED, so the correct source + polarity can be determined
//! empirically with a known-locked and a known-unlocked card.
//!
//! The two readings:
//!
//! 1. **SDHI INFO7** — `SD_INFO1` bit 7, the SD host controller's own view of the
//!    dedicated `SD_WP1` alternate-function pin (P7_1, mux 3).  Per the RZ/A1
//!    manual INFO7 = 1 ⇒ the `SD_WP` pin is at level 0 (low), which the current
//!    driver maps to "write protected".
//! 2. **Raw P7_1 level** — the live pin voltage via the Port Pin Read register.
//!    `enable_input_buffer(7, 1)` lets PPR reflect the pin even while it stays
//!    muxed to `SD_WP1`, so this does not disturb the SDHI controller.
//!
//! Procedure: boot with an *unlocked* card, note the readings, then a *locked*
//! card.  Whichever reading flips between the two — and in which direction — is
//! the signal and polarity to trust.

use embassy_time::Timer;
use log::info;

use deluge_bsp::oled::{self, text};
use deluge_bsp::{pic, sd};

/// The SD card is on SDHI port 1 (its WP/CD pins are P7_*).
const SD_PORT: u8 = 1;

/// First on-screen pixel row (the top rows sit off the visible panel area).
const TOP: usize = 10;

#[embassy_executor::task]
pub(crate) async fn probe_task() {
    // oled::init() drives the panel over RSPI0 and waits on the PIC chip-select
    // echo, so the PIC handshake must finish first.
    pic::wait_ready().await;
    oled::init().await;

    // Bring the SDHI controller + pin mux up once (P7_1 → SD_WP1, P7_0 → SD_CD1,
    // etc.).  The controller and pin mux are configured even if no card is
    // present and init ultimately fails, which is all the probe needs.
    info!("WP-PROBE: bringing up SDHI");
    let _ = sd::init().await;

    // Let PPR reflect the live P7_1 level without taking the pin off SD_WP1.
    unsafe { rza1l_hal::gpio::enable_input_buffer(7, 1) };

    let mut fb = oled::FrameBuffer::new();
    loop {
        let card = sd::is_inserted();
        let ready = sd::is_ready();
        // SDHI's view: INFO7 != 0 (the driver's current "write protected" test).
        let info7 = unsafe { rza1l_hal::sdhi::card_write_protected(SD_PORT) };
        // Raw live pin level on P7_1.
        let pin_high = unsafe { rza1l_hal::gpio::read_pin(7, 1) };

        info!(
            "WP-PROBE: card={} ready={} INFO7={} P7_1={}",
            card as u8,
            ready as u8,
            info7 as u8,
            if pin_high { "HIGH" } else { "LOW" },
        );

        fb.fill(0x00);
        text::draw_str(&mut fb, 0, TOP, b"WP PROBE  P7_1");
        text::draw_str(
            &mut fb,
            0,
            TOP + 12,
            if card {
                b"CARD: INSERTED" as &[u8]
            } else {
                b"CARD: ABSENT" as &[u8]
            },
        );
        text::draw_str(
            &mut fb,
            0,
            TOP + 24,
            if info7 {
                b"SDHI WP: ON  (I7=1)" as &[u8]
            } else {
                b"SDHI WP: OFF (I7=0)" as &[u8]
            },
        );
        text::draw_str(
            &mut fb,
            0,
            TOP + 36,
            if pin_high {
                b"PIN P7_1: HIGH" as &[u8]
            } else {
                b"PIN P7_1: LOW" as &[u8]
            },
        );
        oled::send_frame(&fb).await;
        Timer::after_millis(200).await;
    }
}
