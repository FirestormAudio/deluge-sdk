//! Renesas Serial Peripheral Interface (RSPI) driver for the RZ/A1L.
//!
//! Implements a blocking / interrupt-driven SPI master following the
//! `R_RSPI_Create` + `R_RSPI_Start` + `R_RSPI_SendBasic32/8` sequences from the
//! C firmware.  The channel runs 32-bit frames for the MAX5136 CV DAC and is
//! switched to 8-bit frames for the OLED that shares it.
//!
//! Build a blocking one with [`Rspi::new_blocking`] (or [`Rspi::new_blocking_txonly`]) from its
//! peripheral singleton and a [`Config`]; it is ready in 32-bit frames, and is an embedded-hal
//! `SpiBus` of `u8` or `u32` words.
//!
//! ## Channel map
//! | Channel | Base address  | Use on Deluge         |
//! |---------|---------------|-----------------------|
//! | RSPI0   | `0xE800_C800` | CV DAC + OLED display |
//! | RSPI1   | `0xE800_D000` | (unused)              |
//! | RSPI2   | `0xE800_D800` | (unused)              |
//!
//! ## Clock gating
//! RSPI0–2 (the RZ/A1L has three channels) are clocked through `Config::module_clocks` (STBCR10).
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
/// SPRF  — receive buffer full (at least the RXTRG count received)
const SPSR_SPRF: u8 = 1 << 7;
/// MODF  — mode fault (SSL asserted by another master)
const SPSR_MODF: u8 = 1 << 2;
/// OVRF  — overrun (a frame arrived with the receive buffer full)
const SPSR_OVRF: u8 = 1 << 0;

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
/// Full-duplex frames: TX trigger = 4 bytes free (TXTRG = 10), RX trigger = 1 byte (8-bit frames).
const SPBFCR_DUPLEX_8BIT: u8 = 0b0010_0000;
/// Full-duplex frames: TX trigger = 4 bytes free, RX trigger = 4 bytes, one 32-bit frame.
const SPBFCR_DUPLEX_32BIT: u8 = 0b0010_0010;

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

/// SPCMD0's CPOL (bit 1) and CPHA (bit 0) for `mode`.
fn mode_bits(mode: embedded_hal::spi::Mode) -> u16 {
    use embedded_hal::spi::{Phase, Polarity};
    let cpol = match mode.polarity {
        Polarity::IdleLow => 0,
        Polarity::IdleHigh => 1 << 1,
    };
    let cpha = match mode.phase {
        Phase::CaptureOnFirstTransition => 0,
        Phase::CaptureOnSecondTransition => 1 << 0,
    };
    cpol | cpha
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
unsafe fn init(ch: u8, config: &Config) {
    let bit_rate = config.frequency;
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
    rspi(ch).spcmd0().write_value(SPCMD0_32BIT | mode_bits(config.mode));
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
    debug!(
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
        warn!(
            "rspi: ch{} send32_blocking: SPTEF timeout (STBCR10 cleared?)",
            ch
        );
    }
    debug!("rspi: ch{} SPTEF ok", ch);

    // Reset RX buffer (prevents overflow)
    let v = rspi(ch).spbfcr().read();
    rspi(ch).spbfcr().write_value(v | SPBFCR_RX_RESET);

    // Write data
    rspi(ch).spdr().write_value(data);
    debug!("rspi: ch{} SPDR written, waiting TEND", ch);

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
        warn!("rspi: ch{} send32_blocking: TEND timeout", ch);
    }
    debug!("rspi: ch{} TEND ok", ch);
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
unsafe fn configure_8bit(ch: u8, mode: u16) {
    rspi(ch).spdcr().write_value(SPDCR_8BIT);
    rspi(ch).spcmd0().write_value(SPCMD0_8BIT | mode);
    rspi(ch).spbfcr().write_value(SPBFCR_8BIT);
}

/// Restore RSPI channel `ch` to 32-bit frame mode for CV DAC transfers.
///
/// # Safety
/// Writes RSPI registers.  No concurrent SPI transfers must be in progress.
unsafe fn configure_32bit(ch: u8, mode: u16) {
    rspi(ch).spdcr().write_value(SPDCR_32BIT);
    rspi(ch).spcmd0().write_value(SPCMD0_32BIT | mode);
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
use crate::mode::{Blocking, Mode};

mod sealed {
    pub trait Sealed {}
}

pin_trait!(
    /// A pin that can carry RSPI `T`'s clock (RSPCKn).
    SckPin, Instance
);
pin_trait!(
    /// A pin that can carry RSPI `T`'s master-out data (MOSIn).
    MosiPin, Instance
);
pin_trait!(
    /// A pin that can carry RSPI `T`'s master-in data (MISOn).
    MisoPin, Instance
);
pin_trait!(
    /// A pin that can carry RSPI `T`'s slave select 0 (SSLn0).
    SslPin, Instance
);

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

/// An RSPI master's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// The highest acceptable clock rate, in Hz: the divider rounds down to the next rate the
    /// RSPI can make (`P1 / (2 × (SPBR + 1))`).
    pub frequency: u32,
    /// Clock polarity and phase.
    pub mode: embedded_hal::spi::Mode,
}

impl Default for Config {
    /// 1 MHz, SPI mode 0.
    fn default() -> Self {
        Self { frequency: 1_000_000, mode: embedded_hal::spi::MODE_0 }
    }
}

/// An RSPI transfer error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum Error {
    /// A frame arrived with the receive buffer full, and was lost.
    Overrun,
    /// Another master drove SSL while this one was enabled.
    ModeFault,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::Overrun => "receive overrun",
            Error::ModeFault => "mode fault",
        })
    }
}

impl core::error::Error for Error {}

impl embedded_hal::spi::Error for Error {
    fn kind(&self) -> embedded_hal::spi::ErrorKind {
        match self {
            Error::Overrun => embedded_hal::spi::ErrorKind::Overrun,
            Error::ModeFault => embedded_hal::spi::ErrorKind::ModeFault,
        }
    }
}

/// The frame width the channel is set to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    /// Not known: a stolen handle, until its first switch.
    Unknown,
    Bits8,
    Bits32,
}

/// An RSPI channel as an SPI master, in 32-bit or 8-bit frames. A blocking one
/// ([`new_blocking`](Rspi::new_blocking)) is an embedded-hal [`SpiBus`](embedded_hal::spi::SpiBus)
/// of `u8` or `u32` words.
pub struct Rspi<'d, M: Mode> {
    ch: u8,
    width: Width,
    /// SPCMD0's CPOL/CPHA bits; `None` on a stolen handle until it reads them.
    mode: Option<u16>,
    _rspi: PhantomData<&'d mut ()>,
    _mode: PhantomData<M>,
}

impl<'d> Rspi<'d, Blocking> {
    /// Bring RSPI `T` up as a transmit-only SPI master with `config`, in 32-bit frames, on pins
    /// `sck` and `mosi` (put on the RSPI's function here). Mirrors `R_RSPI_Create` +
    /// `R_RSPI_Start` from the C firmware, apart from the SPI mode.
    ///
    /// SPTIE is set, as the C firmware sets it: it is the DMAC's transmit request, for a DMA
    /// channel that feeds SPDR. Reads return whatever the unconnected MISO samples.
    ///
    /// # Panics
    /// If [`crate::clocks::init`] has not been called.
    pub fn new_blocking_txonly<T: Instance>(
        _rspi: Peri<'d, T>,
        sck: Peri<'d, impl SckPin<T>>,
        mosi: Peri<'d, impl MosiPin<T>>,
        config: Config,
    ) -> Self {
        let (sck_alt, mosi_alt) = (sck.alternate(), mosi.alternate());
        crate::gpio::mux(sck, sck_alt);
        crate::gpio::mux(mosi, mosi_alt);
        // SAFETY: RSPI T is owned, so nothing else drives it.
        unsafe { init(T::INDEX, &config) };
        Self {
            ch: T::INDEX,
            width: Width::Bits32,
            mode: Some(mode_bits(config.mode)),
            _rspi: PhantomData,
            _mode: PhantomData,
        }
    }

    /// [`new_blocking_txonly`](Self::new_blocking_txonly), with `miso` for receiving too.
    ///
    /// # Panics
    /// If [`crate::clocks::init`] has not been called.
    pub fn new_blocking<T: Instance>(
        rspi: Peri<'d, T>,
        sck: Peri<'d, impl SckPin<T>>,
        mosi: Peri<'d, impl MosiPin<T>>,
        miso: Peri<'d, impl MisoPin<T>>,
        config: Config,
    ) -> Self {
        let miso_alt = miso.alternate();
        crate::gpio::mux(miso, miso_alt);
        Self::new_blocking_txonly(rspi, sck, mosi, config)
    }

    /// RSPI `T` without its singleton, for code that can't reach the owner: a panic handler
    /// drawing to a display. Touches no register; the first [`SpiBus`](embedded_hal::spi::SpiBus)
    /// call sets the frame width again.
    ///
    /// # Safety
    /// [`Rspi::new_blocking`] has run for `T`, and its owner doesn't run while the stolen handle
    /// is in use (a panic handler with interrupts masked).
    #[doc(hidden)]
    #[cfg(any(feature = "unstable-driver-steal", test))]
    pub unsafe fn steal<T: Instance>() -> Rspi<'static, Blocking> {
        Rspi {
            ch: T::INDEX,
            width: Width::Unknown,
            mode: None,
            _rspi: PhantomData,
            _mode: PhantomData,
        }
    }
}

impl<'d, M: Mode> Rspi<'d, M> {
    /// Switch to 8-bit frames: SPDCR, SPCMD0, and SPBFCR (which also holds the receive buffer in
    /// reset, for transmit-only use).
    pub fn configure_8bit(&mut self) {
        let mode = self.mode_bits();
        // SAFETY: owned channel.
        unsafe { configure_8bit(self.ch, mode) }
        self.width = Width::Bits8;
    }

    /// Switch to 32-bit frames.
    pub fn configure_32bit(&mut self) {
        let mode = self.mode_bits();
        // SAFETY: owned channel.
        unsafe { configure_32bit(self.ch, mode) }
        self.width = Width::Bits32;
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

    /// SPCMD0's CPOL/CPHA bits, read from SPCMD0 the first time on a stolen handle.
    fn mode_bits(&mut self) -> u16 {
        *self.mode.get_or_insert_with(|| rspi(self.ch).spcmd0().read() & 0b11)
    }

    /// Get ready for full-duplex frames of `width`: switch to it if needed, reset the receive
    /// buffer (so nothing a transmit-only user left in it is read back) with a one-frame receive
    /// trigger, and clear a stale overrun.
    fn begin(&mut self, width: Width) {
        if self.width != width {
            match width {
                Width::Bits8 => self.configure_8bit(),
                _ => self.configure_32bit(),
            }
        }
        let r = rspi(self.ch);
        let spbfcr = match width {
            Width::Bits8 => SPBFCR_DUPLEX_8BIT,
            _ => SPBFCR_DUPLEX_32BIT,
        };
        r.spbfcr().write_value(spbfcr | SPBFCR_RX_RESET);
        r.spbfcr().write_value(spbfcr);
        // OVRF clears by writing 0 after reading 1.
        let spsr = r.spsr().read();
        r.spsr().write_value(spsr & !(SPSR_OVRF | SPSR_MODF));
    }

    /// An error SPSR reports, clearing it.
    fn take_error(&mut self, spsr: u8) -> Option<Error> {
        let err = if spsr & SPSR_OVRF != 0 {
            Error::Overrun
        } else if spsr & SPSR_MODF != 0 {
            Error::ModeFault
        } else {
            return None;
        };
        rspi(self.ch).spsr().write_value(spsr & !(SPSR_OVRF | SPSR_MODF));
        Some(err)
    }

    /// Write one frame to SPDR (byte or word access, as the width is).
    fn write_spdr(&mut self, word: u32) {
        let r = rspi(self.ch);
        match self.width {
            Width::Bits8 => r.spdr_byte().write_value(word as u8),
            _ => r.spdr().write_value(word),
        }
    }

    /// Read one frame from SPDR.
    fn read_spdr(&mut self) -> u32 {
        let r = rspi(self.ch);
        match self.width {
            Width::Bits8 => r.spdr_byte().read() as u32,
            _ => r.spdr().read(),
        }
    }

    /// Send `word` and return the frame received with it, polling.
    fn frame(&mut self, word: u32) -> Result<u32, Error> {
        let r = rspi(self.ch);
        while r.spsr().read() & SPSR_SPTEF == 0 {}
        self.write_spdr(word);
        loop {
            let spsr = r.spsr().read();
            if let Some(e) = self.take_error(spsr) {
                return Err(e);
            }
            if spsr & SPSR_SPRF != 0 {
                return Ok(self.read_spdr());
            }
        }
    }

    /// Full-duplex `read`/`write` of `W` words: `max(len)` frames, writing 0 past `write`'s end
    /// and dropping what arrives past `read`'s.
    fn transfer_words<W: Word>(&mut self, read: &mut [W], write: &[W]) -> Result<(), Error> {
        self.begin(W::WIDTH);
        for i in 0..read.len().max(write.len()) {
            let got = self.frame(write.get(i).map_or(0, |w| w.to_u32()))?;
            if let Some(r) = read.get_mut(i) {
                *r = W::from_u32(got);
            }
        }
        Ok(())
    }

    /// [`transfer_words`](Self::transfer_words) into the words it sends.
    fn transfer_in_place_words<W: Word>(&mut self, words: &mut [W]) -> Result<(), Error> {
        self.begin(W::WIDTH);
        for w in words {
            *w = W::from_u32(self.frame(w.to_u32())?);
        }
        Ok(())
    }
}

/// An SPI word the RSPI carries in one frame.
trait Word: Copy + 'static {
    const WIDTH: Width;
    fn to_u32(self) -> u32;
    fn from_u32(v: u32) -> Self;
}

impl Word for u8 {
    const WIDTH: Width = Width::Bits8;
    fn to_u32(self) -> u32 {
        self as u32
    }
    fn from_u32(v: u32) -> Self {
        v as u8
    }
}

impl Word for u32 {
    const WIDTH: Width = Width::Bits32;
    fn to_u32(self) -> u32 {
        self
    }
    fn from_u32(v: u32) -> Self {
        v
    }
}

impl<M: Mode> embedded_hal::spi::ErrorType for Rspi<'_, M> {
    type Error = Error;
}

macro_rules! blocking_spi_bus {
    ($($w:ty),*) => {$(
        impl embedded_hal::spi::SpiBus<$w> for Rspi<'_, Blocking> {
            fn read(&mut self, words: &mut [$w]) -> Result<(), Error> {
                self.transfer_words(words, &[])
            }

            fn write(&mut self, words: &[$w]) -> Result<(), Error> {
                self.transfer_words::<$w>(&mut [], words)
            }

            fn transfer(&mut self, read: &mut [$w], write: &[$w]) -> Result<(), Error> {
                self.transfer_words(read, write)
            }

            fn transfer_in_place(&mut self, words: &mut [$w]) -> Result<(), Error> {
                self.transfer_in_place_words(words)
            }

            fn flush(&mut self) -> Result<(), Error> {
                self.wait_end();
                Ok(())
            }
        }
    )*};
}

blocking_spi_bus!(u8, u32);

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
        let _rspi = unsafe {
            Rspi::new_blocking_txonly(
                crate::peripherals::RSPI0::steal(),
                crate::peripherals::P6_0::steal(),
                crate::peripherals::P6_2::steal(),
                config(10_000_000, embedded_hal::spi::MODE_0),
            )
        };
        let b = 0xE800_C800usize; // RSPI0

        // RSPCK0 (P6_0) and MOSI0 (P6_2) on alternative 3: PFCE6 set, PFC6/PFCAE6 clear.
        let pins = (1u16 << 0) | (1 << 2);
        assert_eq!(mmio::test::peek16(0xFCFE_3618) & pins, pins, "PFCE6");
        assert_eq!(mmio::test::peek16(0xFCFE_3518) & pins, 0, "PFC6");
        assert_eq!(mmio::test::peek16(0xFCFE_3A18) & pins, 0, "PFCAE6");

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

    fn config(frequency: u32, mode: embedded_hal::spi::Mode) -> Config {
        let mut c = Config::default();
        c.frequency = frequency;
        c.mode = mode;
        c
    }

    const B0: usize = 0xE800_C800; // RSPI0
    const SPSR: usize = B0 + 0x03;
    const SPDR: usize = B0 + 0x04;
    const SPBFCR: usize = B0 + 0x20;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn rspi0(c: Config) -> Rspi<'static, crate::mode::Blocking> {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        crate::mmio::test::reset();
        unsafe {
            Rspi::new_blocking(
                crate::peripherals::RSPI0::steal(),
                crate::peripherals::P6_0::steal(),
                crate::peripherals::P6_2::steal(),
                crate::peripherals::P6_3::steal(),
                c,
            )
        }
    }

    #[test]
    fn config_default_is_1mhz_mode_0() {
        let c = Config::default();
        assert_eq!(c.frequency, 1_000_000);
        assert_eq!(c.mode, embedded_hal::spi::MODE_0);
    }

    #[test]
    fn config_mode_sets_cpol_cpha() {
        let _serial = serial();
        let _r = rspi0(config(1_000_000, embedded_hal::spi::MODE_3));
        assert_eq!(crate::mmio::test::peek16(B0 + 0x10) & 0b11, 0b11);
        let mut r = rspi0(config(1_000_000, embedded_hal::spi::MODE_1));
        assert_eq!(crate::mmio::test::peek16(B0 + 0x10) & 0b11, 0b01);
        // A frame-width switch keeps the mode.
        r.configure_8bit();
        assert_eq!(crate::mmio::test::peek16(B0 + 0x10) & 0b11, 0b01);
    }

    #[test]
    fn config_frequency_sets_spbr() {
        let _serial = serial();
        let _r = rspi0(config(4_000_000, embedded_hal::spi::MODE_0));
        assert_eq!(crate::mmio::test::peek8(B0 + 0x0A), 8);
    }

    #[test]
    fn transfer_resets_the_rx_buffer_first() {
        use embedded_hal::spi::SpiBus;
        let _serial = serial();
        let mut r = rspi0(Config::default());
        crate::mmio::test::poke8(SPSR, SPSR_SPRF | SPSR_TEND | SPSR_SPTEF);
        let start = crate::mmio::test::log().len();
        let mut read = [0u8; 1];
        SpiBus::<u8>::transfer(&mut r, &mut read, &[0x3C]).unwrap();
        let log = crate::mmio::test::log()[start..].to_vec();
        let reset = log
            .iter()
            .position(|a| a.write && a.addr == SPBFCR && a.val & SPBFCR_RX_RESET as u32 != 0)
            .expect("RX buffer reset");
        let write = log
            .iter()
            .position(|a| a.write && a.addr == SPDR)
            .expect("SPDR written");
        let read_at = log
            .iter()
            .rposition(|a| !a.write && a.addr == SPDR)
            .expect("SPDR read");
        assert!(reset < write && write < read_at);
        // The shadow has one SPDR cell, so the byte read back is the byte written.
        assert_eq!(read, [0x3C]);
    }

    #[test]
    fn word_size_switches_the_frame_width() {
        use embedded_hal::spi::SpiBus;
        let _serial = serial();
        let mut r = rspi0(Config::default());
        crate::mmio::test::poke8(SPSR, SPSR_SPRF | SPSR_TEND | SPSR_SPTEF);
        SpiBus::<u8>::write(&mut r, &[1, 2]).unwrap();
        assert_eq!(crate::mmio::test::peek8(B0 + 0x0B), SPDCR_8BIT);
        SpiBus::<u32>::write(&mut r, &[0x1234_5678]).unwrap();
        assert_eq!(crate::mmio::test::peek8(B0 + 0x0B), SPDCR_32BIT);
        assert_eq!(crate::mmio::test::peek32(SPDR), 0x1234_5678);
    }

    #[test]
    fn overrun_is_reported() {
        use embedded_hal::spi::{Error as _, ErrorKind};
        let _serial = serial();
        let mut r = rspi0(Config::default());
        r.begin(Width::Bits8);
        crate::mmio::test::poke8(SPSR, SPSR_SPRF | SPSR_TEND | SPSR_SPTEF | SPSR_OVRF);
        assert_eq!(r.frame(0), Err(Error::Overrun));
        assert_eq!(crate::mmio::test::peek8(SPSR) & SPSR_OVRF, 0, "OVRF cleared");
        assert_eq!(Error::Overrun.kind(), ErrorKind::Overrun);
        assert_eq!(Error::ModeFault.kind(), ErrorKind::ModeFault);
    }

    #[test]
    fn a_stale_overrun_is_cleared_before_a_transfer() {
        use embedded_hal::spi::SpiBus;
        let _serial = serial();
        let mut r = rspi0(Config::default());
        crate::mmio::test::poke8(SPSR, SPSR_SPRF | SPSR_TEND | SPSR_SPTEF | SPSR_OVRF);
        let mut read = [0u8; 1];
        assert_eq!(SpiBus::<u8>::read(&mut r, &mut read), Ok(()));
    }

    #[test]
    fn a_stolen_rspi_touches_no_register() {
        crate::mmio::test::reset();
        let _rspi: Rspi<'_, crate::mode::Blocking> = unsafe { Rspi::steal::<crate::peripherals::RSPI0>() };
        assert!(crate::mmio::test::log().is_empty());
    }

    #[test]
    fn switching_to_8_bit_frames_rewrites_length_command_and_buffer_control() {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        let mut rspi = unsafe {
            Rspi::new_blocking(
                crate::peripherals::RSPI1::steal(),
                crate::peripherals::P6_12::steal(),
                crate::peripherals::P6_14::steal(),
                crate::peripherals::P6_15::steal(),
                config(4_000_000, embedded_hal::spi::MODE_0),
            )
        };
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
