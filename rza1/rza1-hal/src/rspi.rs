//! Renesas Serial Peripheral Interface (RSPI) driver for the RZ/A1L.
//!
//! Implements a blocking / interrupt-driven SPI master following the
//! `R_RSPI_Create` + `R_RSPI_Start` + `R_RSPI_SendBasic32/8` sequences from the
//! C firmware.  The channel runs 32-bit frames for the MAX5136 CV DAC and is
//! switched to 8-bit frames for the OLED that shares it.
//!
//! Build one with [`Rspi::new`] from its peripheral singleton; it is ready in 32-bit frames.
//!
//! ## Channel map
//! | Channel | Base address  | Use on Deluge         |
//! |---------|---------------|-----------------------|
//! | RSPI0   | `0xE800_C800` | CV DAC + OLED display |
//! | RSPI1   | `0xE800_D000` | (unused)              |
//! | RSPI2   | `0xE800_D800` | (unused)              |
//!
//! ## Clock gating
//! RSPI0–2 (the RZ/A1L has three channels) are clocked by `stb::init()` via STBCR10.
//!
//! ## Register offsets inside `struct st_rspi`
//! | Register | Offset | Width | Description                     |
//! |----------|--------|-------|---------------------------------|
//! | SPCR     | 0x00   | u8    | Control (SPE, master, int ena)  |
//! | SSLP     | 0x01   | u8    | SSL polarity                    |
//! | SPPCR    | 0x02   | u8    | Pin control                     |
//! | SPSR     | 0x03   | u8    | Status (SPRF, TEND, SPTEF, …)   |
//! | SPDR     | 0x04   | u32   | Data register (32-bit access)   |
//! | SPSCR    | 0x08   | u8    | Sequence control                |
//! | SPSSR    | 0x09   | u8    | Sequence status                 |
//! | SPBR     | 0x0A   | u8    | Baud rate divider               |
//! | SPDCR    | 0x0B   | u8    | Data control (word size)        |
//! | SPCKD    | 0x0C   | u8    | Clock delay                     |
//! | SSLND    | 0x0D   | u8    | SSL negation delay              |
//! | SPND     | 0x0E   | u8    | Next-access delay               |
//! | SPCMD0   | 0x10   | u16   | Command register 0              |
//! | SPBFCR   | 0x20   | u8    | Buffer control                  |
//! | SPBFDR   | 0x22   | u16   | Buffer data count               |

// ── Peripheral base addresses ─────────────────────────────────────────────────

// ── Register offsets ──────────────────────────────────────────────────────────

// ── SPCR bits ─────────────────────────────────────────────────────────────────
/// SPE — SPI function enable
const SPCR_SPE: u8 = 1 << 6;
/// MSTR — master mode select
const SPCR_MSTR: u8 = 1 << 3;
/// SPTIE — SPI transmit interrupt enable (used for DMA trigger; set here as C does)
const SPCR_SPTIE: u8 = 1 << 5;
// ── SPSR bits (status) ────────────────────────────────────────────────────────
/// SPTEF — transmit buffer empty (TX has room for 4 bytes)
const SPSR_SPTEF: u8 = 1 << 5;
/// TEND  — transmit end (all bits shifted out)
const SPSR_TEND: u8 = 1 << 6;

// ── SPDCR values ──────────────────────────────────────────────────────────────
const SPDCR_8BIT: u8 = 0x20;
const SPDCR_32BIT: u8 = 0x60;

// ── SPBFCR values ─────────────────────────────────────────────────────────────
/// 8-bit OLED mode: TX trigger = 4 bytes free, RX trigger = 1 byte
const SPBFCR_8BIT: u8 = 0b0110_0000;
/// Initial value: TX trigger = 4 bytes free, RX trigger = 2 bytes available
const SPBFCR_INIT_32BIT: u8 = 0b0010_0010;
/// Reset RX buffer (bit 6 = RXRST)
const SPBFCR_RX_RESET: u8 = 1 << 6;

// ── SPCMD0 values ─────────────────────────────────────────────────────────────
// TRM §16 SPCMD bit fields: bit[1] = CPOL (0 = RSPCK idle low), bit[0] = CPHA
// (0 = sample on odd/leading edge).  Reset value = H'070D (CPOL=0, CPHA=1).
//
// The Deluge peripherals (MAX5136 CV DAC, SSD1309 OLED) both specify SPI
// Mode 0 (CPOL=0, CPHA=0).  The C firmware's SPCMD0 = 0x0302/0x0702 selects
// CPOL=1 (Mode 2), which both parts tolerate in practice; this driver uses the
// datasheet polarity.

/// 8-bit Mode 0 (CPOL=0, CPHA=0), SSL0.
/// SPB[3:0] (bits 11:8) = 0b0111 → 8 bits per frame.
/// BRDV[1:0] (bits 3:2) = 0b00  → base bit rate / 1.
const SPCMD0_8BIT: u16 = 0b0000_0111_0000_0000;
/// 32-bit Mode 0 (CPOL=0, CPHA=0), SSL0.
/// SPB[3:0] (bits 11:8) = 0b0011 → 32 bits per frame.
/// BRDV[1:0] (bits 3:2) = 0b00  → base bit rate / 1.
const SPCMD0_32BIT: u16 = 0b0000_0011_0000_0000;

/// SPBR for `bit_rate` at `p1_hz`: `ceil(P1 / (2 × bit_rate)) - 1`, so the actual rate
/// `P1 / (2 × (SPBR + 1))` never exceeds the request.
fn spbr_for(p1_hz: u32, bit_rate: u32) -> u8 {
    p1_hz.div_ceil(bit_rate * 2).saturating_sub(1) as u8
}

// ── Register access helpers ───────────────────────────────────────────────────

/// RSPI channel `ch`'s registers (channels 0–2 on the RZ/A1L, 0–4 on the RZ/A1H).
fn rspi(ch: u8) -> crate::pac::rspi::Rspi {
    #[cfg(feature = "rza1l")]
    let channels = [crate::pac::RSPI0, crate::pac::RSPI1, crate::pac::RSPI2];
    #[cfg(feature = "rza1h")]
    let channels = [
        crate::pac::RSPI0,
        crate::pac::RSPI1,
        crate::pac::RSPI2,
        crate::pac::RSPI3,
        crate::pac::RSPI4,
    ];
    channels[ch as usize]
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise RSPI channel `ch` as an SPI master.
///
/// - `bit_rate`: desired SCK frequency in Hz (e.g. `10_000_000` for 10 MHz).
///   The actual rate is `⌊P1 / (2 × (SPBR + 1))⌋`.
/// - Clock phase and polarity are both 0 (SPI mode 0).
/// - Frame size starts at 32 bits for the CV DAC; switch with
///   [`configure_8bit`] / [`configure_32bit`].
///
/// Mirrors `R_RSPI_Create()` + `R_RSPI_Start()` from the C firmware (apart
/// from the SPI mode; see `SPCMD0_*`).
///
/// # Panics
/// If [`crate::clocks::init`] has not been called.
///
/// # Safety
/// Writes to memory-mapped RSPI registers; must be called once before any
/// SPI transfers on channel `ch`.
unsafe fn init(ch: u8, bit_rate: u32) {
    // SPPCR = 0 (no loopback, MOSI idle = low)
    rspi(ch).sppcr().write_value(0);
    let _ = rspi(ch).sppcr().read();

    // SPBR: baud rate divisor. SPBR = ceil(P1/(bitRate×2)) - 1
    let spbr = spbr_for(crate::clocks::get().p1_hz(), bit_rate);
    rspi(ch).spbr().write_value(spbr);
    let _ = rspi(ch).spbr().read();

    // SPDCR: 32-bit longword data
    rspi(ch).spdcr().write_value(SPDCR_32BIT);
    let _ = rspi(ch).spdcr().read();

    // SPSCR, SPCKD, SSLND, SPND, SSLP, SPSSR: all zero (defaults)
    rspi(ch).spscr().write_value(0);
    let _ = rspi(ch).spscr().read();
    rspi(ch).spckd().write_value(0);
    let _ = rspi(ch).spckd().read();
    rspi(ch).sslnd().write_value(0);
    let _ = rspi(ch).sslnd().read();
    rspi(ch).spnd().write_value(0);
    let _ = rspi(ch).spnd().read();
    rspi(ch).sslp().write_value(0);
    let _ = rspi(ch).sslp().read();
    rspi(ch).spssr().write_value(0);
    let _ = rspi(ch).spssr().read();

    // SPBFCR: TX trigger=4 bytes free, RX trigger=2 bytes
    rspi(ch).spbfcr().write_value(SPBFCR_INIT_32BIT);
    let _ = rspi(ch).spbfcr().read();

    // SPCMD0: 32-bit frame, mode 0
    rspi(ch).spcmd0().write_value(SPCMD0_32BIT);
    let _ = rspi(ch).spcmd0().read();

    // SPCR: master mode, TX interrupt enable (for DMA trigger compatibility)
    let v = rspi(ch).spcr().read();
    rspi(ch).spcr().write_value(v | SPCR_MSTR | SPCR_SPTIE);
    let _ = rspi(ch).spcr().read();

    // ---- R_RSPI_Start: clear errors, set SPE --------------------------------
    let _ = rspi(ch).spsr().read(); // clear error sources
    rspi(ch).spsr().write_value(0x00);

    let v = rspi(ch).spcr().read();
    rspi(ch).spcr().write_value(v | SPCR_SPE);
    let _ = rspi(ch).spcr().read();
}

/// Blocking 32-bit SPI send without interrupt — wait for TEND.
///
/// Intended for the init sequence (MAX5136 linearity words) where no ISR
/// infrastructure is needed yet.
///
/// # Safety
/// Channel must have been initialised with [`init`].
unsafe fn send32_blocking(ch: u8, data: u32) {
    log::debug!(
        "rspi: ch{} blocking send {:#010x}, SPSR={:#04x}",
        ch,
        data,
        rspi(ch).spsr().read()
    );
    // Wait for TX buffer space (bounded to guard against ungated clock).
    // ~1 M polls ≈ a few ms at 400 MHz; SPTEF should assert in microseconds.
    let mut ready = false;
    for _ in 0..1_000_000u32 {
        if rspi(ch).spsr().read() & SPSR_SPTEF != 0 {
            ready = true;
            break;
        }
    }
    if !ready {
        log::warn!(
            "rspi: ch{} send32_blocking: SPTEF timeout (STBCR10 cleared?)",
            ch
        );
    }
    log::debug!("rspi: ch{} SPTEF ok", ch);

    // Reset RX buffer (prevents overflow)
    let v = rspi(ch).spbfcr().read();
    rspi(ch).spbfcr().write_value(v | SPBFCR_RX_RESET);

    // Write data
    rspi(ch).spdr().write_value(data);
    log::debug!("rspi: ch{} SPDR written, waiting TEND", ch);

    // Wait for all bits to be shifted out (bounded; TEND should follow SPTEF
    // by at most frame_bits / SCK_freq ≈ 32 / 10 MHz = 3.2 µs).
    let mut done = false;
    for _ in 0..1_000_000u32 {
        if rspi(ch).spsr().read() & SPSR_TEND != 0 {
            done = true;
            break;
        }
    }
    if !done {
        log::warn!("rspi: ch{} send32_blocking: TEND timeout", ch);
    }
    log::debug!("rspi: ch{} TEND ok", ch);
}

/// Switch RSPI channel `ch` to 8-bit frame mode for OLED (SSD1309) transfers.
///
/// Sets SPDCR=0x20 (8-bit), SPCMD0=0x0700 (SPB=8, Mode 0), SPBFCR=0x60.
/// Must be called before [`send8`].
///
/// After OLED rendering is done, call [`configure_32bit`] to restore the CV
/// DAC word size.
///
/// # Safety
/// Writes RSPI registers.  No concurrent SPI transfers must be in progress.
unsafe fn configure_8bit(ch: u8) {
    rspi(ch).spdcr().write_value(SPDCR_8BIT);
    rspi(ch).spcmd0().write_value(SPCMD0_8BIT);
    rspi(ch).spbfcr().write_value(SPBFCR_8BIT);
}

/// Restore RSPI channel `ch` to 32-bit frame mode for CV DAC transfers.
///
/// # Safety
/// Writes RSPI registers.  No concurrent SPI transfers must be in progress.
unsafe fn configure_32bit(ch: u8) {
    rspi(ch).spdcr().write_value(SPDCR_32BIT);
    rspi(ch).spcmd0().write_value(SPCMD0_32BIT);
    rspi(ch).spbfcr().write_value(SPBFCR_INIT_32BIT);
}

/// Send a single byte via 8-bit SPI.
///
/// Mirrors `R_RSPI_SendBasic8()`:
/// - Polls SPTEF until the TX buffer has 4 bytes of free space.
/// - Resets the RX buffer so unread received bytes cannot overflow it.
/// - Writes the byte to SPDR (SPDR.BYTE.LL = lowest byte = base + 0x04 on LE ARM).
///
/// Does **not** poll TEND.  After the last byte of a sequence, call [`wait_end`]
/// to ensure the shift register has finished.
///
/// # Safety
/// Channel must have been initialised with [`init`] and configured for 8-bit
/// mode via [`configure_8bit`].
#[inline]
unsafe fn send8(ch: u8, data: u8) {
    // Wait until TX buffer has 4 free bytes (SPTEF=1).
    while rspi(ch).spsr().read() & SPSR_SPTEF == 0 {}
    // Reset RX buffer to prevent overflow accumulation.
    let v = rspi(ch).spbfcr().read();
    rspi(ch).spbfcr().write_value(v | SPBFCR_RX_RESET);
    // Write byte to SPDR.BYTE.LL (little-endian: lowest byte of SPDR = base+0x04).
    rspi(ch).spdr_byte().write_value(data);
}

/// Poll until the shift register is idle (TEND=1).
///
/// Call after the last [`send8`] in a sequence to ensure data has been fully
/// clocked out before de-asserting chip-select.
///
/// # Safety
/// Reads SPSR register.
#[inline]
unsafe fn wait_end(ch: u8) {
    while rspi(ch).spsr().read() & SPSR_TEND == 0 {}
}

// ── Owned driver ──────────────────────────────────────────────────────────────

use core::marker::PhantomData;

use crate::Peri;

mod sealed {
    pub trait Sealed {}
}

/// An RSPI channel singleton (`peripherals::RSPI0`–`RSPI2` on the RZ/A1L, `RSPI0`–`RSPI4` on the
/// RZ/A1H).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number: 0–2 on the RZ/A1L, 0–4 on the RZ/A1H.
    const INDEX: u8;
}

macro_rules! instances {
    ($($rspi:ident = $n:literal),* $(,)?) => {$(
        impl sealed::Sealed for crate::peripherals::$rspi {}
        impl Instance for crate::peripherals::$rspi {
            const INDEX: u8 = $n;
        }
    )*};
}

instances!(RSPI0 = 0, RSPI1 = 1, RSPI2 = 2);
#[cfg(feature = "rza1h")]
instances!(RSPI3 = 3, RSPI4 = 4);

/// An RSPI channel as an SPI master, SPI mode 0, in 32-bit or 8-bit frames.
pub struct Rspi<'d> {
    ch: u8,
    _rspi: PhantomData<&'d mut ()>,
}

impl<'d> Rspi<'d> {
    /// Bring RSPI `T` up as an SPI master at no more than `bit_rate` Hz, in 32-bit frames.
    /// Mirrors `R_RSPI_Create` + `R_RSPI_Start` from the C firmware (apart from the SPI mode; see
    /// `SPCMD0_*`).
    ///
    /// # Panics
    /// If [`crate::clocks::init`] has not been called.
    pub fn new<T: Instance>(_rspi: Peri<'d, T>, bit_rate: u32) -> Self {
        // SAFETY: RSPI T is owned, so nothing else drives it.
        unsafe { init(T::INDEX, bit_rate) };
        Self { ch: T::INDEX, _rspi: PhantomData }
    }

    /// RSPI `T` without its singleton, for code that can't reach the owner: a panic handler
    /// drawing to a display. Touches no register.
    ///
    /// # Safety
    /// [`Rspi::new`] has run for `T`, and its owner doesn't run while the stolen handle is in use
    /// (a panic handler with interrupts masked).
    pub unsafe fn steal<T: Instance>() -> Rspi<'static> {
        Rspi { ch: T::INDEX, _rspi: PhantomData }
    }

    /// Switch to 8-bit frames: SPDCR, SPCMD0, and SPBFCR (which also resets the receive FIFO).
    pub fn configure_8bit(&mut self) {
        // SAFETY: owned channel.
        unsafe { configure_8bit(self.ch) }
    }

    /// Switch to 32-bit frames.
    pub fn configure_32bit(&mut self) {
        // SAFETY: owned channel.
        unsafe { configure_32bit(self.ch) }
    }

    /// Queue one byte in 8-bit mode. Doesn't wait for it to shift out: call
    /// [`wait_end`](Self::wait_end) after the last.
    pub fn send8(&mut self, byte: u8) {
        // SAFETY: owned channel.
        unsafe { send8(self.ch, byte) }
    }

    /// Send one word in 32-bit mode and wait until it has shifted out.
    pub fn send32_blocking(&mut self, word: u32) {
        // SAFETY: owned channel.
        unsafe { send32_blocking(self.ch, word) }
    }

    /// Wait until the shift register is idle (TEND).
    pub fn wait_end(&mut self) {
        // SAFETY: owned channel.
        unsafe { wait_end(self.ch) }
    }
}

// ── Address tests ─────────────────────────────────────────────────────────────
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn spbr_matches_the_old_hardcoded_p1() {
        let p1 = crate::clocks::TEST_CLOCKS.p1_hz();
        // ceil(66_128_125 / 20_000_000) - 1 = 3 → 8.27 MHz actual.
        assert_eq!(spbr_for(p1, 10_000_000), 3);
        // The 4 MHz OLED rate: ceil(8.27) - 1 = 8 → 3.67 MHz actual.
        assert_eq!(spbr_for(p1, 4_000_000), 8);
    }

    #[test]
    fn rspi_channels_know_their_index() {
        assert_eq!(<crate::peripherals::RSPI0 as Instance>::INDEX, 0);
        assert_eq!(<crate::peripherals::RSPI2 as Instance>::INDEX, 2);
    }

    /// `Rspi::new` programs the master-mode SPI config through the seam: the key registers end
    /// up with the expected values for a 10 MHz setup.
    #[test]
    fn new_programs_master_mode_config() {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        use crate::mmio;
        mmio::test::reset();
        let _rspi = Rspi::new(unsafe { crate::peripherals::RSPI0::steal() }, 10_000_000);
        let b = 0xE800_C800usize; // RSPI0

        assert_eq!(mmio::test::peek8(b + 0x0A), 3, "baud divider for ~10 MHz");
        assert_eq!(
            mmio::test::peek8(b + 0x0B),
            SPDCR_32BIT,
            "32-bit longword frames"
        );
        assert_eq!(mmio::test::peek8(b + 0x20), SPBFCR_INIT_32BIT);
        assert_eq!(
            mmio::test::peek16(b + 0x10),
            SPCMD0_32BIT,
            "32-bit mode-0 cmd"
        );
        let spcr = mmio::test::peek8(b + 0x00);
        assert_eq!(spcr & SPCR_MSTR, SPCR_MSTR, "master mode");
        assert_eq!(spcr & SPCR_SPTIE, SPCR_SPTIE, "TX interrupt enabled");
        assert_eq!(spcr & SPCR_SPE, SPCR_SPE, "SPI enabled (R_RSPI_Start)");
    }

    #[test]
    fn a_stolen_rspi_touches_no_register() {
        crate::mmio::test::reset();
        let _rspi = unsafe { Rspi::steal::<crate::peripherals::RSPI0>() };
        assert!(crate::mmio::test::log().is_empty());
    }

    #[test]
    fn switching_to_8_bit_frames_rewrites_length_command_and_buffer_control() {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        let mut rspi = Rspi::new(unsafe { crate::peripherals::RSPI1::steal() }, 4_000_000);
        crate::mmio::test::reset();
        rspi.configure_8bit();
        let b = 0xE800_D000usize; // RSPI1
        assert_eq!(
            crate::mmio::test::writes(),
            [
                (b + 0x0B, SPDCR_8BIT as u32),
                (b + 0x10, SPCMD0_8BIT as u32),
                (b + 0x20, SPBFCR_8BIT as u32),
            ]
        );
    }

    #[test]
    fn the_pac_names_the_same_rspi_registers() {
        for ch in 0..3u8 {
            let r = rspi(ch);
            let base = 0xE800_C800 + ch as usize * 0x800;
            let got = [
                (r.spcr().addr(), r.spcr().width()),
                (r.sslp().addr(), r.sslp().width()),
                (r.sppcr().addr(), r.sppcr().width()),
                (r.spsr().addr(), r.spsr().width()),
                (r.spdr().addr(), r.spdr().width()),
                (r.spdr_byte().addr(), r.spdr_byte().width()),
                (r.spscr().addr(), r.spscr().width()),
                (r.spssr().addr(), r.spssr().width()),
                (r.spbr().addr(), r.spbr().width()),
                (r.spdcr().addr(), r.spdcr().width()),
                (r.spckd().addr(), r.spckd().width()),
                (r.sslnd().addr(), r.sslnd().width()),
                (r.spnd().addr(), r.spnd().width()),
                (r.spcmd0().addr(), r.spcmd0().width()),
                (r.spbfcr().addr(), r.spbfcr().width()),
            ];
            assert_eq!(
                got,
                [
                    (base, 1),
                    (base + 0x01, 1),
                    (base + 0x02, 1),
                    (base + 0x03, 1),
                    (base + 0x04, 4),
                    (base + 0x04, 1),
                    (base + 0x08, 1),
                    (base + 0x09, 1),
                    (base + 0x0A, 1),
                    (base + 0x0B, 1),
                    (base + 0x0C, 1),
                    (base + 0x0D, 1),
                    (base + 0x0E, 1),
                    (base + 0x10, 2),
                    (base + 0x20, 1),
                ],
                "RSPI{ch}"
            );
        }
    }
}
