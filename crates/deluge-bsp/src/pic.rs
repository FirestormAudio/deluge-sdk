//! Deluge PIC32 co-processor interface.
//!
//! The PIC32 handles the entire UI layer: 144 RGB pad matrix, 36 button
//! matrix, 6 rotary encoders, 36 indicator LEDs, two "gold knob" rings, a
//! 7-segment display, and OLED display handshaking.  It communicates with the
//! RZ/A1L main CPU over a UART link (SCIF1).
//!
//! ## Baud rate
//! The link starts at **31,250 bps** (shared with MIDI for robustness at boot)
//! and is switched to **200,000 bps** after the PIC has been configured.  Both
//! sides must switch in close sequence (PIC first, then host 50 ms later).
//!
//! ## Message framing
//! There is no framing layer — the protocol is a flat byte stream in both
//! directions.  Command bytes are sent from CPU → PIC; events are returned PIC
//! → CPU.
//!
//! ## Event byte ranges (PIC → CPU)
//! | Range   | Meaning                                                  |
//! |---------|----------------------------------------------------------|
//! | 0–143   | Pad event (press if normal, release if preceded by 252)  |
//! | 144–179 | Button event (same logic)                                |
//! | 180–244 | Undefined / ignored                                      |
//! | 245     | FIRMWARE_VERSION_NEXT (followed by version byte)         |
//! | 246     | OLED-related (ignore)                                    |
//! | 247     | ENABLE_OLED echo (ignore)                                |
//! | 248–249 | OLED chip-select handshake (SELECT / DESELECT)           |
//! | 252     | NEXT_PAD_OFF prefix                                      |
//! | 254     | NO_PRESSES_HAPPENING                                     |
//!
//! **Note:** Rotary encoders are wired directly to RZ/A1L GPIO pins and are
//! read by a dedicated encoder task via [`rza1_hal::gpio`] — the PIC does not
//! transmit encoder data over UART.
//!
//! ## Commands (CPU → PIC) — selected subset
//! | Byte | Command                    | Payload                         |
//! |------|----------------------------|---------------------------------|
//! |10+c  | SET_FLASH_COLOUR           | — (colour index c, next flash)  |
//! |  18  | SET_DEBOUNCE_TIME          | time  (value × 4 ms)            |
//! |  19  | SET_REFRESH_TIME           | time  (ms)                      |
//! |  20  | SET_GOLD_KNOB_0_INDICATORS | 4 × brightness bytes            |
//! |  21  | SET_GOLD_KNOB_1_INDICATORS | 4 × brightness bytes            |
//! |  22  | RESEND_BUTTON_STATES       | —                               |
//! |  23  | SET_FLASH_LENGTH           | time  (ms)                      |
//! |24+p  | SET_PAD_FLASHING           | — (pad index p)                 |
//! | 225  | SET_UART_SPEED             | divider (baud = 4 MHz / (d+1))  |
//! | 243  | SET_DIMMER_INTERVAL        | interval                        |
//! | 244  | SET_MIN_INTERRUPT_INTERVAL | time  (ms)                      |
//! | 245  | REQUEST_FIRMWARE_VERSION   | —                               |
//! | 247  | ENABLE_OLED                | —                               |

#[cfg(target_os = "none")]

#[cfg(target_os = "none")]
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
#[cfg(target_os = "none")]
use embassy_sync::mutex::Mutex;

// ── Channel / baud constants ──────────────────────────────────────────────────

/// SCIF channel wired to the PIC32 (matches `deluge_bsp::uart::PIC_CH`).
pub const UART_CH: usize = 1;

/// Initial baud rate — PIC uses 31 250 at power-on.
#[allow(dead_code)]
const BAUD_INIT: u32 = 31_250;

/// High-speed baud rate after the handshake.
const BAUD_FAST: u32 = 200_000;

/// PIC's internal oscillator used for the UART speed divider formula:
/// `baud = PIC_CLK / (divider + 1)`.
const PIC_CLK_HZ: u32 = 4_000_000;

// ── Command byte constants ────────────────────────────────────────────────────

const CMD_SET_DEBOUNCE_TIME: u8 = 18;
const CMD_SET_REFRESH_TIME: u8 = 19;
pub const CMD_SET_GOLD_KNOB_0_INDICATORS: u8 = 20;
pub const CMD_SET_GOLD_KNOB_1_INDICATORS: u8 = 21;
const CMD_RESEND_BUTTON_STATES: u8 = 22;
const CMD_SET_FLASH_LENGTH: u8 = 23;
const CMD_SET_FLASH_COLOUR_BASE: u8 = 10;
const CMD_SET_PAD_FLASHING_BASE: u8 = 24;
const CMD_SET_DIMMER_INTERVAL: u8 = 243;
const CMD_SET_UART_SPEED: u8 = 225;
const CMD_SET_MIN_INTERRUPT_INTERVAL: u8 = 244;
const CMD_REQUEST_FIRMWARE_VERSION: u8 = 245;
const CMD_ENABLE_OLED: u8 = 247;
const CMD_SELECT_OLED: u8 = 248;
const CMD_DESELECT_OLED: u8 = 249;
const CMD_SET_DC_LOW: u8 = 250;
const CMD_SET_DC_HIGH: u8 = 251;

/// Base command byte for indicator LED off: `CMD_LED_OFF_BASE + id` (id 0–35).
pub const CMD_LED_OFF_BASE: u8 = 152;
/// Base command byte for indicator LED on: `CMD_LED_ON_BASE + id` (id 0–35).
pub const CMD_LED_ON_BASE: u8 = 188;
/// Base command byte for setting two-column RGB data: `CMD_SET_COLOUR_FOR_COLS_BASE + pair`
/// where pair is 0–8.
pub const CMD_SET_COLOUR_FOR_COLS_BASE: u8 = 1;
/// Sent after all column-pair data to trigger the PIC display refresh.
pub const CMD_DONE_SENDING_ROWS: u8 = 240;

/// Smooth-scroll animation opcodes. The PIC keeps its own off-screen framebuffer and smears it to
/// animate a scroll, so the app sends one colour per row per tick instead of resending the whole
/// grid with [`set_column_pair_rgb`].
///
/// `SET_SCROLL_ROW` is a base: the row index is ADDED to it (228..=235 for the 8 rows).
pub const CMD_SET_SCROLL_ROW_BASE: u8 = 228;
/// Horizontal-scroll setup is also a base with the app's flag bits ADDED: bit 0 = scrolling
/// right/positive, bit 1 = the scrolled area includes the sidebar (18 columns rather than 16).
///
/// Encoded arithmetically, as the C++ BSP does (`PIC::setupHorizontalScroll`), rather than via
/// named per-combination constants: the C++ header's names for 238/239
/// (`SET_SCROLL_RIGHT_FULL`/`SET_SCROLL_LEFT_FULL`) are transposed with respect to the flag
/// encoding, and the PIC's interpretation of `base + flags` is authoritative.
pub const CMD_SET_SCROLL_HORIZONTAL_BASE: u8 = 236;
pub const CMD_SET_SCROLL_UP: u8 = 241;
pub const CMD_SET_SCROLL_DOWN: u8 = 242;

// ── Response byte sentinels ───────────────────────────────────────────────────

const RESP_NEXT_PAD_OFF: u8 = 252;
const RESP_NO_PRESSES: u8 = 254;
const RESP_FIRMWARE_VERSION: u8 = 245;

// ── Public types ──────────────────────────────────────────────────────────────

/// An event decoded from the PIC byte stream.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A pad was pressed.  `id` is 0–143.
    PadPress { id: u8 },
    /// A pad was released.  `id` is 0–143.
    PadRelease { id: u8 },
    /// A button was pressed.  `id` is 0–35 (raw PIC value − 144).
    ButtonPress { id: u8 },
    /// A button was released.  `id` is 0–35.
    ButtonRelease { id: u8 },
    /// PIC firmware version byte (one-off, received after [`request_firmware_version`]).
    FirmwareVersion(u8),
    /// OLED chip-select asserted (PIC is ready for SPI data).
    OledSelected,
    /// OLED chip-select de-asserted.
    OledDeselected,
    /// PIC reports no pads are currently active.
    NoPresses,
}

/// Pad ID → (column, row) coordinate.
///
/// The PIC encodes pads in a packed 9-column × 16-per-column layout.
/// This returns `(x, y)` where `x` ∈ 0..17 and `y` ∈ 0..7.
#[inline]
pub fn pad_coords(id: u8) -> (u8, u8) {
    let y_raw = id / 9;
    let mut x = (id - y_raw * 9) * 2;
    let mut y = y_raw;
    if y >= 8 {
        y -= 8;
        x += 1;
    }
    (x, y)
}

/// Grid coordinate → pad ID: the inverse of [`pad_coords`]. `x` ∈ 0..18, `y` ∈ 0..8.
#[inline]
pub fn pad_id(x: u8, y: u8) -> u8 {
    let y_raw = if x % 2 == 0 { y } else { y + 8 };
    y_raw * 9 + x / 2
}

/// Stateful parser for the PIC → CPU byte stream.
///
/// Feed bytes from the UART one at a time via [`Parser::push`]; it returns
/// `Some(Event)` when a complete event has been decoded and `None` when more
/// bytes are needed.
///
/// Maintains two bits of state:
/// - Whether the next pad/button byte is a release (preceded by 0xFC).
/// - Whether we are waiting for the second byte of a firmware-version sequence.
pub struct Parser {
    next_is_off: bool,
    firmware_version_next: bool,
}

impl Parser {
    pub const fn new() -> Self {
        Self {
            next_is_off: false,
            firmware_version_next: false,
        }
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    /// Push one byte received from the PIC.  Returns a decoded event, if any.
    pub fn push(&mut self, byte: u8) -> Option<Event> {
        // ---- Second byte of a firmware-version sequence -------------------
        if self.firmware_version_next {
            self.firmware_version_next = false;
            return Some(Event::FirmwareVersion(byte));
        }

        // ---- Dispatch on first byte ----------------------------------------
        match byte {
            0..=143 => {
                // Pad event
                let off = self.next_is_off;
                self.next_is_off = false;
                if off {
                    Some(Event::PadRelease { id: byte })
                } else {
                    Some(Event::PadPress { id: byte })
                }
            }
            144..=179 => {
                // Button event
                let off = self.next_is_off;
                self.next_is_off = false;
                let id = byte - 144;
                if off {
                    Some(Event::ButtonRelease { id })
                } else {
                    Some(Event::ButtonPress { id })
                }
            }
            180..=244 => {
                // Undefined bytes in the PIC protocol — silently ignored.
                // (Rotary encoders are wired to RZ/A1L GPIO, not the PIC.)
                None
            }
            RESP_FIRMWARE_VERSION => {
                // Firmware version prefix (byte 245) — next byte is the actual version value
                self.firmware_version_next = true;
                None
            }
            248 => Some(Event::OledSelected),
            249 => Some(Event::OledDeselected),
            RESP_NEXT_PAD_OFF => {
                self.next_is_off = true;
                None
            }
            RESP_NO_PRESSES => {
                self.next_is_off = false;
                Some(Event::NoPresses)
            }
            _ => None, // Ignore other bytes (246, 250, 251, 253, 255, etc.)
        }
    }
}

// ── PIC initialisation sequence ───────────────────────────────────────────────

/// Run the full PIC32 initialisation sequence asynchronously.
///
/// 1. Sends configuration commands at 31 250 bps (OLED enable, debounce,
///    refresh rate, interrupt interval, flash length, UART speed command).
/// 2. Waits 50 ms for the PIC to switch to 200 000 bps.
/// 3. Switches the host SCIF1 to 200 000 bps.
/// 4. Requests firmware version and button states; waits another 50 ms.
///
/// **Must be called after** `deluge_bsp::uart::init_pic` has set up
/// SCIF1 and the Embassy executor is running (uses `Timer`).
#[cfg(target_os = "none")]
pub async fn init() {
    use embassy_time::Timer;
    log::debug!("pic: init at 31250 bps, will switch to {} bps", BAUD_FAST);

    // ---- Configure PIC while still at 31 250 bps --------------------------
    // Enable OLED
    tx(&[CMD_ENABLE_OLED]).await;
    // Debounce: 5  → 5 × 4 ms = 20 ms
    tx(&[CMD_SET_DEBOUNCE_TIME, 5]).await;
    // Refresh time: 23 ms
    tx(&[CMD_SET_REFRESH_TIME, 23]).await;
    // Min interrupt interval: 8 ms
    tx(&[CMD_SET_MIN_INTERRUPT_INTERVAL, 8]).await;
    // Flash length: 6 ms
    tx(&[CMD_SET_FLASH_LENGTH, 6]).await;
    // Tell PIC to switch to 200 000 bps: divider = PIC_CLK / BAUD_FAST − 1
    let speed_divider = (PIC_CLK_HZ / BAUD_FAST).saturating_sub(1) as u8;
    tx(&[CMD_SET_UART_SPEED, speed_divider]).await;

    // ---- Give PIC 50 ms to switch baud rate --------------------------------
    Timer::after_millis(50).await;

    // ---- Switch host SCIF1 to 200 000 bps ----------------------------------
    // The last write has completed, so the FIFO has drained. SCIF1 was brought up by
    // `deluge_bsp::uart::init_pic`; this updates SCBRR.
    crate::uart::set_pic_baud(BAUD_FAST);

    // ---- Request firmware version and initial button state -----------------
    tx(&[CMD_REQUEST_FIRMWARE_VERSION]).await;
    tx(&[CMD_RESEND_BUTTON_STATES]).await;

    // Give PIC time to respond
    Timer::after_millis(50).await;
    log::debug!("pic: ready at {} bps", BAUD_FAST);

    // Signal that init is complete — other tasks waiting on wait_ready() unblock.
    ready_signal::signal();
}

/// Host/QEMU stand-in for [`init`]: no PIC32/UART hardware to bring up, so
/// this just signals ready — mirroring the device's final step, "signal that
/// init is complete" — and returns. Resolves without ever suspending.
#[cfg(not(target_os = "none"))]
pub async fn init() {
    log::debug!("pic(host): init (no-op)");
    ready_signal::signal();
}

// ── Transport serialization ───────────────────────────────────────────────────
//
// The PIC link is a flat byte stream with no framing (see the module docs), and
// several tasks send to it concurrently: OLED chip-select handshakes, pad-LED
// updates, gold-knob indicators, refresh-rate changes. If two senders' bytes
// interleave on SCIF1 the PIC mis-parses both messages. `tx` serializes whole
// commands behind a mutex so each command's bytes are contiguous on the wire.
//
// This is *per-command* exclusion, which is sufficient: RSPI0 (OLED pixel data)
// is a physically separate bus, and no PIC command other than SELECT/DESELECT
// (248/249) touches the OLED chip-select — so pad-LED traffic arriving between
// an OLED select and deselect cannot corrupt a frame. See the Advanced
// developer guide (`docs/advanced-guide.md`, §7 — *Dropping down to the BSP &
// HAL*).

/// Serializes the PIC UART so concurrent senders cannot interleave their bytes.
/// Held only for the duration of a single command.
#[cfg(target_os = "none")]
static PIC_TX: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

/// Send one complete PIC command, atomically with respect to other senders.
///
/// All outbound helpers below funnel through here; do not call
/// `crate::uart::write_pic` directly.
#[cfg(target_os = "none")]
async fn tx(bytes: &[u8]) {
    let _guard = PIC_TX.lock().await;
    crate::uart::write_pic(bytes).await;
}

/// Host/QEMU stand-in for [`tx`]: no PIC UART off-target. With a host panel installed
/// ([`crate::host_panel`]), the command is applied to it as the PIC would apply it; otherwise it
/// is logged and dropped. Every outbound helper funnels through here, one whole command per call.
/// Resolves without ever suspending.
#[cfg(not(target_os = "none"))]
async fn tx(bytes: &[u8]) {
    tx_via(crate::host_panel::installed(), bytes).await
}

#[cfg(not(target_os = "none"))]
async fn tx_via(panel: Option<&dyn crate::host_panel::HostPanel>, bytes: &[u8]) {
    if let Some(panel) = panel {
        panel.pic_command(bytes);
    }
    log::debug!("pic(host): tx {:?}", bytes);
}

/// Await one decoded byte from the PIC co-processor's SCIF1 UART (DMA RX ring).
/// This is the RX transport counterpart to the serialized TX path: the control pump loops on it,
/// feeding [`Parser`]. Device only — pops the DMA ring via `crate::uart::read_pic_byte`.
#[cfg(target_os = "none")]
pub async fn read_byte() -> u8 {
    crate::uart::read_pic_byte().await
}

/// Host: no PIC co-processor. With a host panel installed ([`crate::host_panel`]), its input
/// arrives as the bytes the PIC would send. Otherwise no bytes ever arrive and this parks forever —
/// the pump task stays alive but quiescent. That is a clean idle, not a hang: nothing downstream is
/// waiting on it to make progress.
#[cfg(not(target_os = "none"))]
pub async fn read_byte() -> u8 {
    read_byte_via(crate::host_panel::installed()).await
}

/// How often the host [`read_byte`] looks for panel input when it has none.
#[cfg(all(not(target_os = "none"), any(test, feature = "host-panel")))]
const HOST_INPUT_POLL_MS: u64 = 1;

#[cfg(not(target_os = "none"))]
async fn read_byte_via(panel: Option<&dyn crate::host_panel::HostPanel>) -> u8 {
    let Some(panel) = panel else {
        return core::future::pending().await;
    };
    #[cfg(any(test, feature = "host-panel"))]
    loop {
        if let Some(byte) = panel.next_pic_byte() {
            return byte;
        }
        embassy_time::Timer::after_millis(HOST_INPUT_POLL_MS).await;
    }
    // Without `host-panel` nothing can call `host_panel::install`, so no panel exists.
    #[cfg(not(any(test, feature = "host-panel")))]
    {
        let _ = panel;
        unreachable!("a host panel needs deluge-bsp's `host-panel` feature")
    }
}

// ── Outbound helpers ──────────────────────────────────────────────────────────

/// Set indicator LED `id` (0–35) on.
#[inline]
pub async fn led_on(id: u8) {
    tx(&[CMD_LED_ON_BASE + id]).await;
}

/// Set indicator LED `id` (0–35) off.
#[inline]
pub async fn led_off(id: u8) {
    tx(&[CMD_LED_OFF_BASE + id]).await;
}

/// Set RGB colours for one column-pair of the main pad grid.
///
/// `pair` is 0–8:
/// - 0–7 → main pad column pairs 0–15 (pair n = physical columns 2n, 2n+1)
/// - 8   → sidebar columns 16–17
///
/// `colours[0..8]`  = rows 0–7 of the **left** column (col 2n).
/// `colours[8..16]` = rows 0–7 of the **right** column (col 2n+1).
///
/// Each entry is `[r, g, b]` with 0–255 per channel.
///
/// After sending all 9 pairs, call [`done_sending_rows`] to trigger the
/// PIC's display refresh.
pub async fn set_column_pair_rgb(pair: u8, colours: &[[u8; 3]; 16]) {
    // 1 command byte + 16 × 3 colour bytes = 49 bytes per pair
    let mut buf = [0u8; 49];
    buf[0] = CMD_SET_COLOUR_FOR_COLS_BASE + pair;
    for (i, [r, g, b]) in colours.iter().enumerate() {
        buf[1 + i * 3] = *r;
        buf[2 + i * 3] = *g;
        buf[3 + i * 3] = *b;
    }
    tx(&buf).await;
}

/// Begin a horizontal scroll animation.
///
/// `flags` is the app's encoding: bit 0 set = scrolling right (positive direction), bit 1 set = the
/// scrolled area spans all 18 columns (main grid plus sidebar) rather than the 16 main columns. The
/// opcode is sent as `base + flags`, matching the PIC's own decoding.
#[inline]
pub async fn setup_horizontal_scroll(flags: u8) {
    // Mask to the two meaningful bits so a stray high bit cannot walk into DONE_SENDING_ROWS (240).
    tx(&[CMD_SET_SCROLL_HORIZONTAL_BASE + (flags & 0b11)]).await;
}

/// Supply the incoming colour for one row of an in-progress horizontal scroll.
///
/// `row` is 0–7. Called once per row per animation tick, after
/// [`setup_horizontal_scroll`]; finish the tick with [`done_sending_rows`].
#[inline]
pub async fn send_scroll_row(row: u8, colour: [u8; 3]) {
    if row >= 8 {
        return; // out of range would collide with the horizontal-scroll opcodes at 236
    }
    tx(&[
        CMD_SET_SCROLL_ROW_BASE + row,
        colour[0],
        colour[1],
        colour[2],
    ])
    .await;
}

/// Perform a vertical scroll, supplying the whole incoming row in one command.
///
/// `up` picks the direction. `colours` is the incoming row, one entry per column — the Deluge sends
/// all 18 (16 main + 2 sidebar). Unlike the horizontal case this is a single command, so there is no
/// per-row follow-up.
pub async fn vertical_scroll(up: bool, colours: &[[u8; 3]]) {
    // 1 command byte + 18 x 3 colour bytes.
    let mut buf = [0u8; 1 + 18 * 3];
    buf[0] = if up {
        CMD_SET_SCROLL_UP
    } else {
        CMD_SET_SCROLL_DOWN
    };
    let n = colours.len().min(18);
    for (i, [r, g, b]) in colours.iter().take(n).enumerate() {
        buf[1 + i * 3] = *r;
        buf[2 + i * 3] = *g;
        buf[3 + i * 3] = *b;
    }
    tx(&buf[..1 + n * 3]).await;
}

/// Signal the PIC that all column-pair colour data has been sent for this
/// frame, triggering a display refresh.  Call after the last
/// [`set_column_pair_rgb`] for a complete grid update.
#[inline]
pub async fn done_sending_rows() {
    tx(&[CMD_DONE_SENDING_ROWS]).await;
}

/// SET_PAD_FLASHING for pad `idx` (`y + x * 8` over the 18 × 8 grid): the PIC lights it for the
/// flash length in the current flash colour, then restores it.
const fn pad_flash_command(idx: u8) -> [u8; 1] {
    [CMD_SET_PAD_FLASHING_BASE + idx]
}

/// SET_FLASH_COLOUR for the PIC's colour index `colour` (1–3), used by the next pad flash.
const fn flash_colour_command(colour: u8) -> [u8; 1] {
    [CMD_SET_FLASH_COLOUR_BASE + colour]
}

/// SET_DIMMER_INTERVAL with `interval`, the PIC's pad-dimming interval.
const fn dimmer_interval_command(interval: u8) -> [u8; 2] {
    [CMD_SET_DIMMER_INTERVAL, interval]
}

/// Flash pad `idx` (`y + x * 8`) in the current flash colour.
#[inline]
pub async fn flash_pad(idx: u8) {
    tx(&pad_flash_command(idx)).await;
}

/// Flash pad `idx` (`y + x * 8`) in the PIC's colour index `colour` (1–3).
#[inline]
pub async fn flash_pad_colour(idx: u8, colour: u8) {
    tx(&flash_colour_command(colour)).await;
    flash_pad(idx).await;
}

/// Set the PIC's pad-dimming interval (SET_DIMMER_INTERVAL, command 243).
#[inline]
pub async fn set_dimmer_interval(interval: u8) {
    tx(&dimmer_interval_command(interval)).await;
}

/// Set the PIC display refresh interval (SET_REFRESH_TIME, command 19).
///
/// `interval` is in milliseconds.  Lower values mean faster refresh and
/// brighter LEDs; higher values dim them.  The Deluge uses the range 0–25,
/// where `interval = 25 - brightness_level`.
#[inline]
pub async fn set_refresh_time(interval: u8) {
    tx(&[CMD_SET_REFRESH_TIME, interval]).await;
}

/// Request the PIC to re-send all button/pad pressed states.
#[inline]
pub async fn resend_button_states() {
    tx(&[CMD_RESEND_BUTTON_STATES]).await;
}

/// Request the PIC firmware version (arrives asynchronously as
/// [`Event::FirmwareVersion`]).
#[inline]
pub async fn request_firmware_version() {
    tx(&[CMD_REQUEST_FIRMWARE_VERSION]).await;
}

/// Set both gold-knob LED rings.
/// `knob`: 0 or 1.  `brightnesses`: four brightness values (0–255).
#[inline]
pub async fn set_gold_knob_indicators(knob: u8, brightnesses: [u8; 4]) {
    let cmd = if knob == 0 {
        CMD_SET_GOLD_KNOB_0_INDICATORS
    } else {
        CMD_SET_GOLD_KNOB_1_INDICATORS
    };
    tx(&[
        cmd,
        brightnesses[0],
        brightnesses[1],
        brightnesses[2],
        brightnesses[3],
    ])
    .await;
}

// ── OLED SPI handshake helpers ────────────────────────────────────────────────

/// Enable OLED power via the PIC (send ENABLE_OLED = 247).
///
/// Must be called once during initialisation, before the first [`oled_select()`].
#[inline]
pub async fn oled_enable() {
    tx(&[CMD_ENABLE_OLED]).await;
}

/// Assert OLED chip-select via the PIC.
#[inline]
pub async fn oled_select() {
    tx(&[CMD_SELECT_OLED]).await;
}

/// De-assert OLED chip-select via the PIC.
#[inline]
pub async fn oled_deselect() {
    tx(&[CMD_DESELECT_OLED]).await;
}

/// Pull OLED Data/!Command line low (command mode).
#[inline]
pub async fn oled_dc_low() {
    tx(&[CMD_SET_DC_LOW]).await;
}

/// Pull OLED Data/!Command line high (data mode).
#[inline]
pub async fn oled_dc_high() {
    tx(&[CMD_SET_DC_HIGH]).await;
}

// ── Blocking OLED handshake (panic path) ───────────────────────────────────────
//
// Polling, interrupt-free variants of the OLED handshake commands for use from a
// panic handler, where the executor and IRQs are dead so `tx` (async, mutex,
// TXI/DMA) cannot run. Best-effort: each write is bounded so it cannot hang.

/// Push `bytes` out the PIC UART by polling the TX FIFO. Bounded so a wedged
/// peripheral can't hang the panic path; drops the remainder if it stalls.
#[cfg(target_os = "none")]
fn send_blocking(bytes: &[u8]) {
    let mut sent = 0;
    let mut idle = 0u32;
    while sent < bytes.len() {
        let n = crate::uart::try_write_pic(&bytes[sent..]);
        if n == 0 {
            idle += 1;
            if idle > 1_000_000 {
                break;
            }
        } else {
            sent += n;
            idle = 0;
        }
    }
}

/// Assert OLED chip-select (blocking). See [`oled_select`].
#[cfg(target_os = "none")]
pub(crate) fn oled_select_blocking() {
    send_blocking(&[CMD_SELECT_OLED]);
}

/// De-assert OLED chip-select (blocking). See [`oled_deselect`].
#[cfg(target_os = "none")]
pub(crate) fn oled_deselect_blocking() {
    send_blocking(&[CMD_DESELECT_OLED]);
}

/// Command mode (blocking). See [`oled_dc_low`].
#[cfg(target_os = "none")]
pub(crate) fn oled_dc_low_blocking() {
    send_blocking(&[CMD_SET_DC_LOW]);
}

/// Data mode (blocking). See [`oled_dc_high`].
#[cfg(target_os = "none")]
pub(crate) fn oled_dc_high_blocking() {
    send_blocking(&[CMD_SET_DC_HIGH]);
}

// ── PIC-ready signal ─────────────────────────────────────────────────────────
//
// Set by `pic::init()` once the full initialisation sequence has completed
// (baud switched, firmware version and button-state resend requested, 50 ms
// settling time elapsed).  Any task that depends on the PIC being fully
// configured (notably `oled_task`) should `pic::wait_ready().await` before
// issuing its own PIC commands.

#[cfg(target_os = "none")]
mod ready_signal {
    use core::sync::atomic::{AtomicBool, Ordering};

    static READY_FLAG: AtomicBool = AtomicBool::new(false);

    /// Called once by `pic::init()` when the sequence is complete.
    pub fn signal() {
        READY_FLAG.store(true, Ordering::Release);
    }

    /// Poll until `signal()` has been called.  Returns immediately if it
    /// was already called (safe to call multiple times, from any number of tasks).
    pub async fn wait() {
        while !READY_FLAG.load(Ordering::Acquire) {
            embassy_time::Timer::after_millis(1).await;
        }
    }
}

/// Host/QEMU stand-in for [`ready_signal`]: `embassy-time` isn't linked into
/// non-test host builds (it's a dev-dependency there), so this polls via a
/// waker (like [`crate::oled`]'s redraw signal) instead of a timer.
/// [`init`]'s host stand-in calls [`signal`] synchronously before returning,
/// so [`wait`] resolves immediately once `init` has run.
///
/// Uses [`MultiWakerRegistration`] rather than the single-slot `AtomicWaker`
/// `crate::oled`'s redraw signal uses: `wait_ready()` is awaited
/// *concurrently* by several tasks (e.g. the pad and OLED render tasks), and an
/// `AtomicWaker` holds only one registration, so a second waiter's
/// `register()` would clobber the first's and leave that task parked forever.
#[cfg(not(target_os = "none"))]
mod ready_signal {
    use core::cell::RefCell;
    use core::future::poll_fn;
    use core::sync::atomic::{AtomicBool, Ordering};
    use core::task::Poll;
    use embassy_sync::blocking_mutex::Mutex;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::waitqueue::MultiWakerRegistration;

    static READY_FLAG: AtomicBool = AtomicBool::new(false);
    /// Up to 4 concurrent waiters. If exceeded, `register()` wakes everything
    /// registered so far rather than losing a registration (see its doc
    /// comment).
    static WAKERS: Mutex<CriticalSectionRawMutex, RefCell<MultiWakerRegistration<4>>> =
        Mutex::new(RefCell::new(MultiWakerRegistration::new()));

    /// Called once by the host `init()` stand-in.
    pub fn signal() {
        READY_FLAG.store(true, Ordering::Release);
        WAKERS.lock(|w| w.borrow_mut().wake());
    }

    /// Suspend until `signal()` has been called. Returns immediately if it
    /// already has (safe to call multiple times, from any number of
    /// concurrently-awaiting tasks — unlike [`crate::oled`]'s one-shot redraw
    /// flag, this is never consumed). Registers before checking the flag so a
    /// `signal()` landing between the check and registration can't be missed.
    pub async fn wait() {
        poll_fn(|cx| {
            WAKERS.lock(|w| w.borrow_mut().register(cx.waker()));
            if READY_FLAG.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

/// Suspend until [`init()`] has completed on this PIC channel.
///
/// Call this at the start of any task that issues PIC UART commands, to
/// ensure the baud-rate handshake and initial configuration have already
/// finished.  Returns immediately if `init()` has already run.
#[inline]
pub async fn wait_ready() {
    ready_signal::wait().await;
}

// ── OLED chip-select handshake signals ───────────────────────────────────────
//
// The PIC echoes SELECT_OLED (248) and DESELECT_OLED (249) back when it has
// asserted/de-asserted the OLED chip-select line.  The OLED driver must wait
// for these echoes before starting / finishing a DMA transfer.
//
// The `pic_task` in firmware calls notify_oled_selected() / notify_oled_deselected()
// when it decodes the corresponding PIC events.

#[cfg(target_os = "none")]
mod oled_signal {
    use core::future::poll_fn;
    use core::sync::atomic::{AtomicBool, Ordering};
    use core::task::Poll;
    use embassy_sync::waitqueue::AtomicWaker;

    static SELECTED_FLAG: AtomicBool = AtomicBool::new(false);
    static SELECTED_WAKER: AtomicWaker = AtomicWaker::new();
    static DESELECTED_FLAG: AtomicBool = AtomicBool::new(false);
    static DESELECTED_WAKER: AtomicWaker = AtomicWaker::new();

    /// Called by `pic_task` when it receives an OledSelected event (byte 248).
    pub fn notify_selected() {
        SELECTED_FLAG.store(true, Ordering::Release);
        SELECTED_WAKER.wake();
    }

    /// Called by `pic_task` when it receives an OledDeselected event (byte 249).
    pub fn notify_deselected() {
        DESELECTED_FLAG.store(true, Ordering::Release);
        DESELECTED_WAKER.wake();
    }

    /// Forgets an echo already received, so a wait only returns for one that arrives after it.
    pub fn clear_selected() {
        SELECTED_FLAG.store(false, Ordering::Release);
    }

    /// Forgets an echo already received, so a wait only returns for one that arrives after it.
    pub fn clear_deselected() {
        DESELECTED_FLAG.store(false, Ordering::Release);
    }

    /// Suspend until the PIC confirms OLED CS is asserted (echo 248).
    pub async fn wait_selected() {
        poll_fn(|cx| {
            SELECTED_WAKER.register(cx.waker());
            if SELECTED_FLAG.swap(false, Ordering::AcqRel) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }

    /// Suspend until the PIC confirms OLED CS is de-asserted (echo 249).
    pub async fn wait_deselected() {
        poll_fn(|cx| {
            DESELECTED_WAKER.register(cx.waker());
            if DESELECTED_FLAG.swap(false, Ordering::AcqRel) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

/// Called by `pic_task` when [`Event::OledSelected`] is decoded.
#[cfg(target_os = "none")]
#[inline]
pub fn notify_oled_selected() {
    oled_signal::notify_selected();
}

/// Called by `pic_task` when [`Event::OledDeselected`] is decoded.
#[cfg(target_os = "none")]
#[inline]
pub fn notify_oled_deselected() {
    oled_signal::notify_deselected();
}

/// Host: no OLED chip-select signalling path (the display render task drives the
/// oled sim directly), so these are no-ops.
#[cfg(not(target_os = "none"))]
pub fn notify_oled_selected() {}
#[cfg(not(target_os = "none"))]
pub fn notify_oled_deselected() {}

/// Suspend until the PIC confirms OLED CS is asserted.
///
/// Send [`oled_select()`] before calling this.
#[cfg(target_os = "none")]
#[inline]
pub async fn wait_oled_selected() {
    oled_signal::wait_selected().await;
}

/// Suspend until the PIC confirms OLED CS is de-asserted.
///
/// Send [`oled_deselect()`] before calling this.
#[cfg(target_os = "none")]
#[inline]
pub async fn wait_oled_deselected() {
    oled_signal::wait_deselected().await;
}

/// How long to wait for the PIC to echo an OLED chip-select command before sending it again.
#[cfg(target_os = "none")]
const OLED_ECHO_TIMEOUT_MS: u64 = 50;

/// Asserts OLED chip-select via the PIC and suspends until the PIC echoes it (byte 248).
///
/// The command is sent again every [`OLED_ECHO_TIMEOUT_MS`] without an echo: a lost byte would
/// otherwise leave the caller, and the RSPI0 bus it holds, waiting for good. An echo received
/// before the command was sent is forgotten, so a late echo from an earlier attempt cannot end a
/// later wait early.
#[cfg(target_os = "none")]
pub async fn oled_select_and_wait() {
    loop {
        oled_signal::clear_selected();
        oled_select().await;
        let echoed = embassy_time::with_timeout(
            embassy_time::Duration::from_millis(OLED_ECHO_TIMEOUT_MS),
            oled_signal::wait_selected(),
        )
        .await;
        if echoed.is_ok() {
            return;
        }
        log::warn!("pic: no OLED select echo in {OLED_ECHO_TIMEOUT_MS} ms, sending it again");
    }
}

/// De-asserts OLED chip-select via the PIC and suspends until the PIC echoes it (byte 249).
///
/// Resends as [`oled_select_and_wait`] does.
#[cfg(target_os = "none")]
pub async fn oled_deselect_and_wait() {
    loop {
        oled_signal::clear_deselected();
        oled_deselect().await;
        let echoed = embassy_time::with_timeout(
            embassy_time::Duration::from_millis(OLED_ECHO_TIMEOUT_MS),
            oled_signal::wait_deselected(),
        )
        .await;
        if echoed.is_ok() {
            return;
        }
        log::warn!("pic: no OLED deselect echo in {OLED_ECHO_TIMEOUT_MS} ms, sending it again");
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    // ---- pad_coords ---------------------------------------------------------

    #[test]
    fn pad_coords_first() {
        // id=0: y_raw=0, x=0, y=0 → (0, 0)
        assert_eq!(pad_coords(0), (0, 0));
    }

    #[test]
    fn pad_coords_second_column_first_row() {
        // id=1: y_raw=0, x=2, y=0 → (2, 0)
        assert_eq!(pad_coords(1), (2, 0));
    }

    #[test]
    fn pad_coords_ninth_column() {
        // id=8: y_raw=0, x=16, y=0 → (16, 0)
        assert_eq!(pad_coords(8), (16, 0));
    }

    #[test]
    fn pad_coords_second_half_adds_one_to_x() {
        // id=72: y_raw=8, x=0+1=1, y=8-8=0 → (1, 0)
        assert_eq!(pad_coords(72), (1, 0));
    }

    // ---- Parser -------------------------------------------------------------

    #[test]
    fn parser_pad_press() {
        let mut p = Parser::new();
        assert_eq!(p.push(5), Some(Event::PadPress { id: 5 }));
    }

    #[test]
    fn parser_pad_release() {
        let mut p = Parser::new();
        assert_eq!(p.push(RESP_NEXT_PAD_OFF), None);
        assert_eq!(p.push(5), Some(Event::PadRelease { id: 5 }));
    }

    #[test]
    fn parser_button_press() {
        let mut p = Parser::new();
        // Button ID 0 = raw byte 144
        assert_eq!(p.push(144), Some(Event::ButtonPress { id: 0 }));
    }

    #[test]
    fn parser_button_release_after_off_prefix() {
        let mut p = Parser::new();
        assert_eq!(p.push(RESP_NEXT_PAD_OFF), None);
        assert_eq!(p.push(150), Some(Event::ButtonRelease { id: 6 }));
    }

    #[test]
    fn parser_undefined_bytes_ignored() {
        // Bytes 180–244 are undefined in the PIC protocol and must be dropped
        // without consuming the following byte.
        let mut p = Parser::new();
        assert_eq!(p.push(180), None);
        // The byte after should still parse normally (not consumed as a delta).
        assert_eq!(p.push(5), Some(Event::PadPress { id: 5 }));
    }

    #[test]
    fn parser_no_presses() {
        let mut p = Parser::new();
        assert_eq!(p.push(RESP_NO_PRESSES), Some(Event::NoPresses));
    }

    #[test]
    fn parser_oled_select() {
        let mut p = Parser::new();
        assert_eq!(p.push(248), Some(Event::OledSelected));
        assert_eq!(p.push(249), Some(Event::OledDeselected));
    }

    #[test]
    fn parser_firmware_version() {
        let mut p = Parser::new();
        assert_eq!(p.push(245), None);
        assert_eq!(p.push(42), Some(Event::FirmwareVersion(42)));
    }

    #[test]
    fn parser_next_pad_off_clears_after_event() {
        let mut p = Parser::new();
        // OFF prefix affects only the NEXT 0–179 byte
        p.push(RESP_NEXT_PAD_OFF);
        p.push(10); // release
        // No prefix now — next should be a press
        assert_eq!(p.push(10), Some(Event::PadPress { id: 10 }));
    }

    #[test]
    fn uart_speed_divider_for_200k() {
        // 4_000_000 / 200_000 - 1 = 19
        let d = (PIC_CLK_HZ / BAUD_FAST).saturating_sub(1) as u8;
        assert_eq!(d, 19);
    }

    // ---- Host stand-ins -------------------------------------------------------

    /// `init()` and a couple of outbound commands (which all funnel through the
    /// host `tx` log-and-drop stand-in) must not panic — there is no PIC UART
    /// off-target to crash against.
    #[test]
    fn init_and_outbound_commands_do_not_panic_on_host() {
        embassy_futures::block_on(init());
        embassy_futures::block_on(led_on(0));
        embassy_futures::block_on(led_off(0));
        embassy_futures::block_on(oled_select());
        embassy_futures::block_on(oled_deselect());
        embassy_futures::block_on(set_refresh_time(23));
    }

    /// `wait_ready()` resolves once host `init()` has signalled — it must not
    /// hang (no `embassy-time` polling loop is reachable off-target).
    #[test]
    fn wait_ready_resolves_after_host_init() {
        embassy_futures::block_on(init());
        embassy_futures::block_on(wait_ready());
    }

    #[test]
    fn host_notify_oled_are_noops() {
        // Must not panic / touch hardware.
        notify_oled_selected();
        notify_oled_deselected();
    }

    #[test]
    fn host_read_byte_parks_and_does_not_resolve() {
        // read_byte() must return a never-ready future on host (a clean park), not
        // resolve to a bogus byte. Poll it once: it must be Pending.
        use core::future::Future;
        use core::pin::pin;
        use core::task::{Context, Poll};
        // A no-op waker is enough: we only assert the first poll is Pending.
        let waker = core::task::Waker::noop();
        let mut cx = Context::from_waker(&waker);
        let fut = pin!(read_byte());
        assert!(matches!(fut.poll(&mut cx), Poll::Pending));
    }

    // ---- Pad flash and dimmer commands ------------------------------------

    #[test]
    fn a_pad_flash_is_one_byte_from_24() {
        assert_eq!(pad_flash_command(0), [24]);
        assert_eq!(pad_flash_command(143), [167]);
    }

    #[test]
    fn a_flash_colour_is_one_byte_from_10() {
        assert_eq!(flash_colour_command(1), [11]);
        assert_eq!(flash_colour_command(3), [13]);
    }

    #[test]
    fn the_dimmer_interval_is_243_then_the_interval() {
        assert_eq!(dimmer_interval_command(7), [243, 7]);
    }

    #[test]
    fn tx_without_a_panel_is_a_no_op() {
        embassy_futures::block_on(tx_via(None, &[1, 2, 3]));
    }

    #[test]
    fn tx_forwards_whole_commands_to_the_panel() {
        let panel = crate::host_panel::tests::leak_panel();
        embassy_futures::block_on(tx_via(Some(panel), &[1, 2, 3]));
        assert_eq!(*panel.commands.lock().unwrap(), [vec![1u8, 2, 3]]);
    }

    #[test]
    fn read_byte_returns_the_panels_input() {
        let panel = crate::host_panel::tests::leak_panel();
        panel.input.lock().unwrap().extend([0x90, 7]);
        assert_eq!(embassy_futures::block_on(read_byte_via(Some(panel))), 0x90);
        assert_eq!(embassy_futures::block_on(read_byte_via(Some(panel))), 7);
    }
}
