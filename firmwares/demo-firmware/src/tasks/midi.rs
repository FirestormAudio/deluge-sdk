//! USB MIDI 1.0 ↔ MIDI DIN (SCIF0) bridge tasks.
//!
//! Two independent embassy tasks handle each direction:
//! - [`midi_usb_rx_task`]: receives 4-byte USB MIDI packets from the host
//!   and writes decoded raw MIDI bytes to SCIF0 (MIDI DIN TX).
//! - [`midi_din_tx_task`]: reads raw bytes from SCIF0 (MIDI DIN RX) and
//!   encodes them as 4-byte USB MIDI packets sent to the host.
//!
//! ## USB MIDI packet format (USB MIDI 1.0, §4)
//! ```text
//! byte 0: [cable_number(7:4)] [code_index_number(3:0)]
//! byte 1: MIDI byte 0 (status or first data)
//! byte 2: MIDI byte 1
//! byte 3: MIDI byte 2
//! ```

use embassy_usb::class::midi::{Receiver, Sender};
use log::debug;

use deluge_bsp::midi_stream;
use deluge_bsp::uart as bsp_uart;
use rza1l_hal::usb::Rusb1Driver;

// ── USB → DIN direction ───────────────────────────────────────────────────────

/// USB MIDI OUT → MIDI DIN TX task.
///
/// Reads 4-byte USB MIDI packets, decodes the CIN to determine payload byte
/// count, and writes the raw MIDI bytes to SCIF0 TX.
#[embassy_executor::task]
pub(crate) async fn midi_usb_rx_task(mut receiver: Receiver<'static, Rusb1Driver>) {
    // Must be >= the bulk OUT max packet size (512 at high speed).  One USB
    // packet can carry many 4-byte MIDI events, so iterate over all of them.
    let mut pkt = [0u8; 512];
    loop {
        receiver.wait_connection().await;
        debug!("MIDI USB RX: connected");
        // Loop until the endpoint is disabled (read_packet returns Err).
        while let Ok(n) = receiver.read_packet(&mut pkt).await {
            for ev in pkt[..n].chunks_exact(4) {
                let count = midi_stream::payload_len(ev[0]);
                if count > 0 {
                    bsp_uart::write_midi(&ev[1..1 + count]).await;
                }
            }
        }
        debug!("MIDI USB RX: disconnected");
    }
}

// ── DIN → USB direction ───────────────────────────────────────────────────────

/// MIDI DIN RX → USB MIDI IN task.
///
/// Reads raw bytes from SCIF0 DMA RX, runs the MIDI 1.0 parser (running
/// status, SysEx, realtime), and writes complete 4-byte USB MIDI packets to
/// the USB IN endpoint.
#[embassy_executor::task]
pub(crate) async fn midi_din_tx_task(mut sender: Sender<'static, Rusb1Driver>) {
    let mut parser = midi_stream::DinParser::default();
    loop {
        sender.wait_connection().await;
        debug!("MIDI DIN TX: connected");
        loop {
            let byte = bsp_uart::read_midi_byte().await;
            if let Some(pkt) = parser.push(byte)
                && sender.write_packet(&pkt).await.is_err()
            {
                break; // endpoint disabled
            }
        }
        debug!("MIDI DIN TX: disconnected");
    }
}
