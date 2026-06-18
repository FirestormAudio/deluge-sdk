use log::{debug, info};

use crate::events::{EVENT_CHANNEL, HardwareEvent};
use deluge_bsp::pic;

/// PIC32 event dispatcher — forwards hardware input to the CDC host.
///
/// The host owns all illumination (pad LEDs, button LEDs, knob indicators,
/// OLED), so this task only forwards decoded input events over
/// [`EVENT_CHANNEL`] and relays the OLED chip-select handshake; it never drives
/// the panel itself.
///
/// | Event            | Action                                       |
/// |------------------|----------------------------------------------|
/// | `PadPress`       | Forward `PadPressed` to the host             |
/// | `PadRelease`     | Forward `PadReleased` to the host            |
/// | `ButtonPress`    | Forward `ButtonPressed` to the host          |
/// | `ButtonRelease`  | Forward `ButtonReleased` to the host         |
/// | `OledSelected`   | Forward to [`pic::notify_oled_selected()`]   |
/// | `OledDeselected` | Forward to [`pic::notify_oled_deselected()`] |
#[embassy_executor::task]
pub(crate) async fn pic_task() {
    info!("PIC: init (31 250 → 200 000 baud)");
    pic::init().await;
    info!("PIC: ready");

    let mut parser = pic::Parser::new();
    debug!("pic_task: entering main loop");

    loop {
        let byte = rza1l_hal::uart::read_byte(pic::UART_CH).await;
        let Some(event) = parser.push(byte) else {
            continue;
        };

        match event {
            // ---- Pads ------------------------------------------------------
            pic::Event::PadPress { id } => {
                let (col, row) = pic::pad_coords(id);
                let _ = EVENT_CHANNEL.try_send(HardwareEvent::PadPressed { col, row });
            }
            pic::Event::PadRelease { id } => {
                let (col, row) = pic::pad_coords(id);
                let _ = EVENT_CHANNEL.try_send(HardwareEvent::PadReleased { col, row });
            }

            // ---- Buttons ---------------------------------------------------
            pic::Event::ButtonPress { id } => {
                info!("button {} press", id);
                let _ = EVENT_CHANNEL.try_send(HardwareEvent::ButtonPressed { id });
            }
            pic::Event::ButtonRelease { id } => {
                let _ = EVENT_CHANNEL.try_send(HardwareEvent::ButtonReleased { id });
            }

            // ---- OLED CS handshake — must be forwarded --------------------
            pic::Event::OledSelected => {
                pic::notify_oled_selected();
            }
            pic::Event::OledDeselected => {
                pic::notify_oled_deselected();
            }

            // ---- Misc ------------------------------------------------------
            pic::Event::FirmwareVersion(v) => info!("PIC fw v{}", v),
            pic::Event::NoPresses => {}

            // Event is #[non_exhaustive]; future variants are silently ignored.
            _ => {}
        }
    }
}
