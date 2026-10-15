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
//! | RSPI3   | `0xE800_E000` | (unused)              |
//! | RSPI4   | `0xE800_E800` | (unused)              |
//!
//! ## Clock gating
//! RSPI0–4 clocks are enabled in `stb::init()` via STBCR10.
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
const RSPI0_BASE: usize = 0xE800_C800;
const RSPI_STRIDE: usize = 0x800;

// ── Register offsets ──────────────────────────────────────────────────────────
const OFF_SPCR: usize = 0x00;
const OFF_SSLP: usize = 0x01;
const OFF_SPPCR: usize = 0x02;
const OFF_SPSR: usize = 0x03;
const OFF_SPDR: usize = 0x04;
const OFF_SPSCR: usize = 0x08;
const OFF_SPSSR: usize = 0x09;
const OFF_SPBR: usize = 0x0A;
const OFF_SPDCR: usize = 0x0B;
const OFF_SPCKD: usize = 0x0C;
const OFF_SSLND: usize = 0x0D;
const OFF_SPND: usize = 0x0E;
const OFF_SPCMD0: usize = 0x10;
const OFF_SPBFCR: usize = 0x20;

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

#[inline]
fn base(ch: u8) -> usize {
    RSPI0_BASE + RSPI_STRIDE * (ch as usize)
}

// Register handles route through the MMIO seam (real volatile on firmware,
// shadow + log under test), so the init sequence is unit-testable.
#[inline]
fn reg8(ch: u8, off: usize) -> crate::mmio::Reg8 {
    crate::mmio::Reg8(base(ch) + off)
}

#[inline]
fn reg16(ch: u8, off: usize) -> crate::mmio::Reg16 {
    crate::mmio::Reg16(base(ch) + off)
}

#[inline]
fn reg32(ch: u8, off: usize) -> crate::mmio::Reg32 {
    crate::mmio::Reg32(base(ch) + off)
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
    unsafe {
        // SPPCR = 0 (no loopback, MOSI idle = low)
        reg8(ch, OFF_SPPCR).write(0);
        let _ = reg8(ch, OFF_SPPCR).read();

        // SPBR: baud rate divisor. SPBR = ceil(P1/(bitRate×2)) - 1
        let spbr = spbr_for(crate::clocks::get().p1_hz(), bit_rate);
        reg8(ch, OFF_SPBR).write(spbr);
        let _ = reg8(ch, OFF_SPBR).read();

        // SPDCR: 32-bit longword data
        reg8(ch, OFF_SPDCR).write(SPDCR_32BIT);
        let _ = reg8(ch, OFF_SPDCR).read();

        // SPSCR, SPCKD, SSLND, SPND, SSLP, SPSSR: all zero (defaults)
        reg8(ch, OFF_SPSCR).write(0);
        let _ = reg8(ch, OFF_SPSCR).read();
        reg8(ch, OFF_SPCKD).write(0);
        let _ = reg8(ch, OFF_SPCKD).read();
        reg8(ch, OFF_SSLND).write(0);
        let _ = reg8(ch, OFF_SSLND).read();
        reg8(ch, OFF_SPND).write(0);
        let _ = reg8(ch, OFF_SPND).read();
        reg8(ch, OFF_SSLP).write(0);
        let _ = reg8(ch, OFF_SSLP).read();
        reg8(ch, OFF_SPSSR).write(0);
        let _ = reg8(ch, OFF_SPSSR).read();

        // SPBFCR: TX trigger=4 bytes free, RX trigger=2 bytes
        reg8(ch, OFF_SPBFCR).write(SPBFCR_INIT_32BIT);
        let _ = reg8(ch, OFF_SPBFCR).read();

        // SPCMD0: 32-bit frame, mode 0
        reg16(ch, OFF_SPCMD0).write(SPCMD0_32BIT);
        let _ = reg16(ch, OFF_SPCMD0).read();

        // SPCR: master mode, TX interrupt enable (for DMA trigger compatibility)
        let v = reg8(ch, OFF_SPCR).read();
        reg8(ch, OFF_SPCR).write(v | SPCR_MSTR | SPCR_SPTIE);
        let _ = reg8(ch, OFF_SPCR).read();

        // ---- R_RSPI_Start: clear errors, set SPE --------------------------------
        let _ = reg8(ch, OFF_SPSR).read(); // clear error sources
        reg8(ch, OFF_SPSR).write(0x00);

        let v = reg8(ch, OFF_SPCR).read();
        reg8(ch, OFF_SPCR).write(v | SPCR_SPE);
        let _ = reg8(ch, OFF_SPCR).read();
    }
}

/// Blocking 32-bit SPI send without interrupt — wait for TEND.
///
/// Intended for the init sequence (MAX5136 linearity words) where no ISR
/// infrastructure is needed yet.
///
/// # Safety
/// Channel must have been initialised with [`init`].
unsafe fn send32_blocking(ch: u8, data: u32) {
    unsafe {
        log::debug!(
            "rspi: ch{} blocking send {:#010x}, SPSR={:#04x}",
            ch,
            data,
            reg8(ch, OFF_SPSR).read()
        );
        // Wait for TX buffer space (bounded to guard against ungated clock).
        // ~1 M polls ≈ a few ms at 400 MHz; SPTEF should assert in microseconds.
        let mut ready = false;
        for _ in 0..1_000_000u32 {
            if reg8(ch, OFF_SPSR).read() & SPSR_SPTEF != 0 {
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
        let v = reg8(ch, OFF_SPBFCR).read();
        reg8(ch, OFF_SPBFCR).write(v | SPBFCR_RX_RESET);

        // Write data
        reg32(ch, OFF_SPDR).write(data);
        log::debug!("rspi: ch{} SPDR written, waiting TEND", ch);

        // Wait for all bits to be shifted out (bounded; TEND should follow SPTEF
        // by at most frame_bits / SCK_freq ≈ 32 / 10 MHz = 3.2 µs).
        let mut done = false;
        for _ in 0..1_000_000u32 {
            if reg8(ch, OFF_SPSR).read() & SPSR_TEND != 0 {
                done = true;
                break;
            }
        }
        if !done {
            log::warn!("rspi: ch{} send32_blocking: TEND timeout", ch);
        }
        log::debug!("rspi: ch{} TEND ok", ch);
    }
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
    unsafe {
        reg8(ch, OFF_SPDCR).write(SPDCR_8BIT);
        reg16(ch, OFF_SPCMD0).write(SPCMD0_8BIT);
        reg8(ch, OFF_SPBFCR).write(SPBFCR_8BIT);
    }
}

/// Restore RSPI channel `ch` to 32-bit frame mode for CV DAC transfers.
///
/// # Safety
/// Writes RSPI registers.  No concurrent SPI transfers must be in progress.
unsafe fn configure_32bit(ch: u8) {
    unsafe {
        reg8(ch, OFF_SPDCR).write(SPDCR_32BIT);
        reg16(ch, OFF_SPCMD0).write(SPCMD0_32BIT);
        reg8(ch, OFF_SPBFCR).write(SPBFCR_INIT_32BIT);
    }
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
    unsafe {
        // Wait until TX buffer has 4 free bytes (SPTEF=1).
        while reg8(ch, OFF_SPSR).read() & SPSR_SPTEF == 0 {}
        // Reset RX buffer to prevent overflow accumulation.
        let v = reg8(ch, OFF_SPBFCR).read();
        reg8(ch, OFF_SPBFCR).write(v | SPBFCR_RX_RESET);
        // Write byte to SPDR.BYTE.LL (little-endian: lowest byte of SPDR = base+0x04).
        crate::mmio::Reg8(reg32(ch, OFF_SPDR).0).write(data);
    }
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
    unsafe { while reg8(ch, OFF_SPSR).read() & SPSR_TEND == 0 {} }
}

// ── Owned driver ──────────────────────────────────────────────────────────────

use core::marker::PhantomData;

use crate::Peri;

mod sealed {
    pub trait Sealed {}
}

/// An RSPI channel singleton (`peripherals::RSPI0`–`RSPI4`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number, 0–4.
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

instances!(RSPI0 = 0, RSPI1 = 1, RSPI2 = 2, RSPI3 = 3, RSPI4 = 4);

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
    fn rspi_channel_bases() {
        assert_eq!(base(0), 0xE800_C800);
        assert_eq!(base(1), 0xE800_D000);
        assert_eq!(base(2), 0xE800_D800);
        assert_eq!(base(3), 0xE800_E000);
        assert_eq!(base(4), 0xE800_E800);
    }

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
        assert_eq!(<crate::peripherals::RSPI4 as Instance>::INDEX, 4);
    }

    /// `Rspi::new` programs the master-mode SPI config through the seam: the key registers end
    /// up with the expected values for a 10 MHz setup.
    #[test]
    fn new_programs_master_mode_config() {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        use crate::mmio;
        mmio::test::reset();
        let _rspi = Rspi::new(unsafe { crate::peripherals::RSPI0::steal() }, 10_000_000);
        let b = base(0);

        assert_eq!(mmio::test::peek8(b + OFF_SPBR), 3, "baud divider for ~10 MHz");
        assert_eq!(mmio::test::peek8(b + OFF_SPDCR), SPDCR_32BIT, "32-bit longword frames");
        assert_eq!(mmio::test::peek8(b + OFF_SPBFCR), SPBFCR_INIT_32BIT);
        assert_eq!(mmio::test::peek16(b + OFF_SPCMD0), SPCMD0_32BIT, "32-bit mode-0 cmd");
        let spcr = mmio::test::peek8(b + OFF_SPCR);
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
        let b = base(1);
        assert_eq!(
            crate::mmio::test::writes(),
            [
                (b + OFF_SPDCR, SPDCR_8BIT as u32),
                (b + OFF_SPCMD0, SPCMD0_8BIT as u32),
                (b + OFF_SPBFCR, SPBFCR_8BIT as u32),
            ]
        );
    }
}
