//! SPI Multi-I/O Bus Controller (SPIBSC0) — manual-command NOR-flash driver.
//!
//! The Deluge stores the first-stage bootloader, the SSB, and (with this change)
//! a bootable firmware image in a single SPI NOR flash chip — a **Spansion
//! S25FL512** (512 Mbit, uniform **256 KB** sectors) on SPIBSC channel 0.
//! At reset the controller is in **external-address-space read mode**
//! (memory-mapped): the flash appears at `0x1800_0000` (cached) / `0x5800_0000`
//! (uncached mirror) and ordinary loads fetch flash bytes.  That mode is
//! read-only.
//!
//! To **erase** or **program** the flash this driver switches the controller to
//! **SPI operating mode** (`CMNCR.MD = 1`), issues raw JEDEC commands through the
//! `SM*` register block, then switches back to memory-mapped read mode (and
//! flushes the read cache) so memory-mapped reads — and the boot trampoline — see
//! fresh data.
//!
//! ## Why this is only safe in the SSB
//! Leaving memory-mapped read mode makes the `0x1800_0000` window stop responding
//! to instruction/data fetches.  The SSB executes from on-chip SRAM
//! (`0x2002_0000`, copied there by the first-stage bootloader), so it keeps
//! running while the flash bus is in manual mode.  A firmware that executed *in
//! place* from flash could not do this.
//!
//! ## Register / encoding cross-check
//! Register layout, bit positions and command sequences here are matched against
//! the Renesas `spibsc` sample BSP shipped in the Deluge bootloader
//! (`DelugeBootloader/src/spibsc_flash_api.c`, `inc/iodefines/spibsc_iodefine.h`,
//! `inc/iobitmasks/spibsc_iobitmask.h`) and the S25FL512 command set in
//! `DelugeFirmware/src/RZA1/spibsc/sflash.h`:
//!
//! | Register | Offset | Purpose |
//! |----------|--------|---------|
//! | CMNCR    | 0x000  | Common control. Bit 31 `MD`: 0 = memory-mapped read, 1 = manual SPI. |
//! | DRCR     | 0x00C  | Data-read (memory-mapped) control. Bit 9 `RCF`: flush read cache. |
//! | SMCR     | 0x020  | Manual control. `SPIE`(0) start, `SPIWE`(1) write, `SPIRE`(2) read, `SSLKP`(8) keep SSL. |
//! | SMCMR    | 0x024  | Manual command. `CMD[7:0]` in bits[23:16]. |
//! | SMADR    | 0x028  | Manual address (flash-relative, 0-based). |
//! | SMENR    | 0x030  | Manual enable. `CDE`(14) command, `ADE[11:8]` address, `SPIDE[3:0]` data width. |
//! | SMRDR0   | 0x038  | Manual read data. |
//! | SMWDR0   | 0x040  | Manual write data. |
//! | CMNSR    | 0x048  | Common status. Bit 0 `TEND`: transfer end. |
//!
//! ## Interrupts must be masked during a manual-mode operation
//! While the controller is in manual SPI mode the memory-mapped read window at
//! `0x1800_0000` stops responding.  The Deluge bootloader found the hard way that
//! taking an interrupt during this window — specifically the **OLED SPI DMA**
//! completion interrupt — **freezes the machine** (`DelugeBootloader`
//! `src/spibsc_init2.c:739`: *"if you `R_INTC_Disable(DMA_INTERRUPT_0 +
//! OLED_SPI_DMA_CHANNEL)` … that at least saves everything from freezing"*).
//! The ISR (or a task it wakes) evidently touches the now-dead flash window and
//! stalls the AHB bus.  So every public erase/program here runs the whole
//! `to_manual … to_read_mode` window inside a [`critical_section`] (interrupts
//! masked); `to_read_mode` restores the read window before interrupts come back,
//! which is also what lets the OLED work again afterwards.
//!
//! **Critical data-register alignment** (per the sample BSP): sub-32-bit manual
//! transfers are **MSB-aligned** in `SMWDR0`/`SMRDR0` — an 8-bit datum is at bits
//! `[31:24]`, a 16-bit datum at `[31:16]`; a 32-bit datum is the little-endian
//! word as-is.  [`read_id`] reports the JEDEC ID; its **density byte** is used to
//! derive the chip's true size, and every erase/program is bounded against that
//! size ([`chip_capacity`]) so an out-of-range offset can never *alias* down onto
//! a reserved sector (a too-large offset on a small part wraps modulo the chip
//! size — this is what once erased the FSB).  Callers should still read
//! programmed data back through the memory-mapped window to verify.

use core::sync::atomic::{AtomicU32, Ordering, compiler_fence};

// ---------------------------------------------------------------------------
// Flash geometry
// ---------------------------------------------------------------------------

/// Base of the cached memory-mapped read window — flash offset 0 maps here.
pub const SPI_FLASH_BASE: u32 = 0x1800_0000;
/// Total flash size in bytes.  **The Deluge's SPI NOR is 4 MB**, not the larger
/// part our earlier comments assumed: the bootloader places the firmware at
/// `0x80000` with a 3.5 MB maximum, ending exactly at `0x400000`, which *is the
/// end of the chip* (`DelugeBootloader/src/spibsc_init2.c`).  Nothing exists
/// above `0x400000`; an address there wraps modulo the chip size back onto low
/// sectors — writing `0x400000` aliases to `0x0` and erases the FSB.  Every
/// write is therefore bounded against this (and the JEDEC-derived
/// [`chip_capacity`]); no offset at or beyond it is ever issued.
pub const FLASH_SIZE: u32 = 0x0040_0000; // 4 MB

/// Flash-relative offset of the bootable app slot.
///
/// Everything below this is **hardware-reserved** and is never erased or
/// programmed by this driver — [`erase_sector`] / [`program`] refuse any offset
/// outside the writable windows (see [`writable`]).  The Deluge flash map (the
/// regions below are described in the original 256 KB units; the erase
/// granularity is actually a 64 KB [`SECTOR_SIZE`] block):
///   * `0x00000..0x40000`  — first-stage bootloader (FSB)
///   * `0x40000..0x80000`  — Deluge device-settings
///   * `0x80000..0x100000` — second-stage bootloader (SSB); the FSB loads the
///     SSB from `0x80000`
///
/// The app slot sits **above** the SSB so storing an app can never touch the
/// FSB, the device settings, or the SSB itself.  `0x100000` is 64 KB-aligned,
/// so erasing a slot block never spills into the reserved region below.
pub const FLASH_SLOT_OFFSET: u32 = 0x0010_0000; // 1 MB
/// Memory-mapped (read-window) address of the app slot.
pub const FLASH_SLOT_ADDR: u32 = SPI_FLASH_BASE + FLASH_SLOT_OFFSET;
/// Length of the app slot (2.75 MB: `0x100000..0x3C0000`; 11 × 256 KB sectors).
/// The slot stops one sector short of the chip end so the top sector can hold the
/// [settings record](SETTINGS_SECTOR_OFFSET) — everything stays inside the 4 MB
/// device.
pub const FLASH_SLOT_LEN: u32 = 0x002C_0000;

/// Flash-relative offset of the **settings sector** — the **top 256 KB sector of
/// the 4 MB chip** (`0x3C0000..0x400000`), immediately above the app slot.
///
/// It must live *inside* the device: the previous choice (`0x400000`) was at the
/// chip boundary and aliased to `0x0`, erasing the FSB.  This sector is the last
/// one that physically exists, sits above the app slot, and is never touched by
/// the FSB/SSB.  The app-loader stores its persistent, card-independent dev-mode
/// flag here (see `app-loader/src/settings.rs`).
pub const SETTINGS_SECTOR_OFFSET: u32 = 0x003C_0000;
/// Memory-mapped (read-window) address of the settings sector.
pub const SETTINGS_SECTOR_ADDR: u32 = SPI_FLASH_BASE + SETTINGS_SECTOR_OFFSET;

/// Uniform erase-sector size (**64 KB**) — the granularity of the `0xD8` block
/// erase ([`CMD_SECTOR_ERASE`]) on this S25FL032-class 4 MB part.  Earlier code
/// wrongly assumed 256 KB sectors (copied from an S25FL512 reference): a `0xD8`
/// clears only 64 KB, so a 256 KB stride left three quarters of every region
/// un-erased and a multi-sector store corrupted everything past the first 64 KB.
/// Every slot/settings offset below is 64 KB-aligned, so the geometry is
/// unchanged — only the erase step shrinks to match the hardware.
pub const SECTOR_SIZE: u32 = 0x0001_0000;
/// Program chunk size.  The page buffer is larger, but the Deluge bootloader
/// programs the firmware region in 256-byte units with the note "Bigger doesn't
/// seem to work" on this board (`FLASH_WRITE_SIZE` in
/// `DelugeBootloader/src/spibsc_init2.c`), so we match that: a single program is
/// at most 256 bytes and never crosses a 256-byte boundary.
pub const PAGE: usize = 256;

/// `true` if `[offset, offset+len)` lies entirely within one of the two writable
/// windows: the firmware **app slot** or the **settings sector** above it.
///
/// Belt-and-suspenders write protection: every erase and program funnels through
/// a check against this, so a caller bug can never reach the FSB, the
/// device-settings sector, or the SSB below [`FLASH_SLOT_OFFSET`].  The settings
/// sector ([`SETTINGS_SECTOR_OFFSET`]) is a second, separate allowed window — one
/// sector — that holds the app-loader's persistent dev-mode flag.
#[inline]
fn writable(offset: u32, len: u32) -> bool {
    if len == 0 {
        return false;
    }
    let end = offset as u64 + len as u64;
    let in_slot =
        offset >= FLASH_SLOT_OFFSET && end <= (FLASH_SLOT_OFFSET as u64 + FLASH_SLOT_LEN as u64);
    let in_settings = offset >= SETTINGS_SECTOR_OFFSET
        && end <= (SETTINGS_SECTOR_OFFSET as u64 + SECTOR_SIZE as u64);
    in_slot || in_settings
}

/// Cached usable flash capacity in bytes from the JEDEC density byte.
/// `0` = not yet probed *or* an implausible ID — either way writes are refused
/// (fail safe).
static CHIP_CAPACITY: AtomicU32 = AtomicU32::new(0);

/// Probe (once, then cache) the usable flash size in bytes from the JEDEC density
/// byte returned by [`read_id`].  Returns `0` if the ID is missing/implausible.
///
/// **Must be called outside a manual-mode window** — it issues its own RDID
/// (which toggles `CMNCR.MD`); the public erase/program entry points prime it
/// before `to_manual`, so the per-sector check ([`write_allowed`]) only ever
/// reads the cache.
fn chip_capacity() -> u32 {
    let cached = CHIP_CAPACITY.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let id = read_id(); // [manufacturer, memory type, density]
    // For these serial NOR parts the density byte `d` encodes 2^d bytes. Reject a
    // blank/no-response ID (all 0x00 or 0xFF) and clamp to a sane window
    // (2^18 = 256 KB .. 2^26 = 64 MB).  The Deluge's part is 4 MB (density 0x16);
    // we never assume a size larger than the chip actually reports.
    let density = id[2];
    let cap = if id[0] == 0x00 || id[0] == 0xFF || !(18..=26).contains(&density) {
        0
    } else {
        1u32 << density
    };
    CHIP_CAPACITY.store(cap, Ordering::Relaxed);
    cap
}

/// `true` if `[offset, offset+len)` is inside a writable window **and** inside the
/// physically present flash (using the cached [`chip_capacity`]).  The chip-size
/// bound is what prevents an out-of-range offset from wrapping (modulo the chip
/// size) onto a reserved low sector — the failure that erased the FSB.  Reads
/// only the cache, so it is safe to call inside a manual-mode window; an unknown
/// capacity (`0`) refuses the write.
fn write_allowed(offset: u32, len: u32) -> bool {
    if !writable(offset, len) {
        return false;
    }
    in_chip(offset, len)
}

/// `true` if `[offset, offset+len)` is non-empty and lies inside the physically
/// present flash (using the cached [`chip_capacity`]) — **without** the
/// writable-window restriction of [`writable`].
///
/// This is the only guard kept on the `unlock-bootloader` forced path: it still
/// prevents an out-of-range offset from wrapping (modulo the chip size) onto a
/// low sector — the failure that once erased the FSB — but it deliberately does
/// *not* protect the FSB / device-settings / SSB regions, because the whole point
/// of the forced path is to (re)write them from a JTAG-loaded recovery tool.
#[inline]
fn in_chip(offset: u32, len: u32) -> bool {
    if len == 0 {
        return false;
    }
    let cap = CHIP_CAPACITY.load(Ordering::Relaxed);
    cap != 0 && (offset as u64 + len as u64) <= cap as u64
}

// ---------------------------------------------------------------------------
// Registers
// ---------------------------------------------------------------------------

const SPIBSC0: usize = 0x3FEF_A000;
const CMNCR: usize = SPIBSC0;
const DRCR: usize = SPIBSC0 + 0x00C;
const SMCR: usize = SPIBSC0 + 0x020;
const SMCMR: usize = SPIBSC0 + 0x024;
const SMADR: usize = SPIBSC0 + 0x028;
const SMENR: usize = SPIBSC0 + 0x030;
const SMRDR0: usize = SPIBSC0 + 0x038;
const SMWDR0: usize = SPIBSC0 + 0x040;
const CMNSR: usize = SPIBSC0 + 0x048;

// CMNCR
const CMNCR_MD: u32 = 1 << 31; // 1 = manual SPI mode, 0 = memory-mapped read mode

// DRCR
const DRCR_RCF: u32 = 1 << 9; // write 1 to flush the read cache
const DRCR_SSLN: u32 = 1 << 24; // write 1 to negate SSL after the current access

// SMCR
const SMCR_SPIE: u32 = 1 << 0; // start transfer
const SMCR_SPIWE: u32 = 1 << 1; // write-data enable
const SMCR_SPIRE: u32 = 1 << 2; // read-data enable
const SMCR_SSLKP: u32 = 1 << 8; // keep SSL asserted after this transfer

// SMENR
const SMENR_CDE: u32 = 1 << 14; // command field enable
const SMENR_ADE_3B: u32 = 0x7 << 8; // address field = ADR[23:0] (3 bytes)
const SMENR_SPIDE_8: u32 = 0x8; // transfer-data width = 8 bit
const SMENR_SPIDE_16: u32 = 0xC; // transfer-data width = 16 bit
const SMENR_SPIDE_32: u32 = 0xF; // transfer-data width = 32 bit

// CMNSR
const CMNSR_TEND: u32 = 1 << 0; // transfer end
const CMNSR_SSLF: u32 = 1 << 1; // SSL flag: 1 = SSL (chip select) currently asserted

// JEDEC / S25FL512 opcodes
const CMD_WREN: u32 = 0x06;
const CMD_RDSR: u32 = 0x05;
const CMD_RDID: u32 = 0x9F;
const CMD_PP: u32 = 0x02; // page program (3-byte address)
const CMD_SECTOR_ERASE: u32 = 0xD8; // 64 KB uniform block erase (3-byte address)
const CMD_CLEAR_STATUS: u32 = 0x30; // clear latched P_ERR/E_ERR (and stuck WIP)
const SR_WIP: u32 = 1 << 0; // status register: write-in-progress
const SR_E_ERR: u32 = 1 << 5; // erase error (S25FL-S): set on a rejected erase
const SR_P_ERR: u32 = 1 << 6; // program error (S25FL-S): set on a rejected program

#[inline(always)]
unsafe fn wr32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

#[inline(always)]
unsafe fn rd32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Iteration cap for [`wait_tend`].  A manual transfer completes in microseconds;
/// this is only a ceiling so a misbehaving controller can never hard-freeze the
/// machine (the panic handler and any stuck poll are indistinguishable to a user
/// — both look frozen).
const TEND_TIMEOUT: u32 = 2_000_000;
/// Iteration cap for [`wait_wip`].  Each iteration issues a full RDSR transfer,
/// so this comfortably covers a 256 KB sector erase (max ~2.6 s) while still
/// bounding a stuck WIP (e.g. a protected-sector erase that sets E_ERR and never
/// clears WIP until a Clear-Status) to a few seconds instead of forever.
const WIP_TIMEOUT: u32 = 8_000_000;

/// Spin until the current manual-mode transfer completes, or [`TEND_TIMEOUT`]
/// iterations elapse.  Returns `true` if the transfer ended, `false` on timeout.
#[inline]
#[must_use]
unsafe fn wait_tend() -> bool {
    unsafe {
        let mut n = TEND_TIMEOUT;
        while rd32(CMNSR) & CMNSR_TEND == 0 {
            if n == 0 {
                return false;
            }
            n -= 1;
            compiler_fence(Ordering::SeqCst);
        }
        true
    }
}

/// Quiesce the bus before changing `CMNCR.MD`: negate SSL (`DRCR.SSLN`) and wait
/// for `CMNSR.SSLF` to clear so the chip select left asserted by the previous
/// (memory-mapped) access is released.
///
/// **This is mandatory** — mirrors the reference `spibsc_stop()`
/// (`DelugeBootloader/src/spibsc_ioset_drv.c`).  Flipping `MD` while SSL is still
/// asserted wedges the controller: manual transfers never complete, `TEND` never
/// sets, and reads come back as `0x00` (observed as a JEDEC ID of `00 00 00`).
/// Bounded so a stuck `SSLF` can't hang.
#[inline]
unsafe fn stop() {
    unsafe {
        wr32(DRCR, rd32(DRCR) | DRCR_SSLN);
        let mut n = TEND_TIMEOUT;
        while rd32(CMNSR) & CMNSR_SSLF != 0 {
            if n == 0 {
                break;
            }
            n -= 1;
            compiler_fence(Ordering::SeqCst);
        }
    }
}

/// Enter manual SPI operating mode (sets `CMNCR.MD`).  Stops the bus first (see
/// [`stop`]); the `DR*` (memory-mapped read) configuration left by the first-
/// stage bootloader is otherwise preserved.
#[inline]
unsafe fn to_manual() {
    unsafe {
        if rd32(CMNCR) & CMNCR_MD == 0 {
            stop();
            wr32(CMNCR, rd32(CMNCR) | CMNCR_MD);
        }
    }
}

/// Return to memory-mapped read mode and flush the read cache so subsequent
/// memory-mapped reads see freshly programmed data.  Stops the bus before
/// clearing `MD` (see [`stop`]), matching the reference `spibsc_exmode()`.
#[inline]
unsafe fn to_read_mode() {
    unsafe {
        if rd32(CMNCR) & CMNCR_MD != 0 {
            stop();
            wr32(CMNCR, rd32(CMNCR) & !CMNCR_MD);
        }
        // Drop any cached flash reads captured before the program/erase.
        wr32(DRCR, rd32(DRCR) | DRCR_RCF);
        // Read back to order the flush before the caller dereferences flash.
        let _ = rd32(DRCR);
    }
}

/// Issue WREN (0x06): one command, no address, no data.
#[inline]
unsafe fn write_enable() {
    unsafe {
        wr32(SMCMR, CMD_WREN << 16);
        wr32(SMADR, 0);
        wr32(SMENR, SMENR_CDE);
        wr32(SMCR, SMCR_SPIE);
        let _ = wait_tend();
    }
}

/// Read the status register (0x05) once and return its low byte.  The 8-bit read
/// datum is MSB-aligned in `SMRDR0` (bits `[31:24]`).
#[inline]
unsafe fn read_status() -> u32 {
    unsafe {
        wr32(SMCMR, CMD_RDSR << 16);
        wr32(SMADR, 0);
        wr32(SMENR, SMENR_CDE | SMENR_SPIDE_8);
        wr32(SMWDR0, 0);
        wr32(SMCR, SMCR_SPIE | SMCR_SPIRE);
        let _ = wait_tend();
        (rd32(SMRDR0) >> 24) & 0xFF
    }
}

/// Spin until the flash clears its write-in-progress bit, or [`WIP_TIMEOUT`]
/// iterations elapse.  Returns `true` if WIP cleared, `false` on timeout (e.g. a
/// rejected erase/program that latched E_ERR/P_ERR and left WIP stuck).
#[inline]
#[must_use]
unsafe fn wait_wip() -> bool {
    unsafe {
        let mut n = WIP_TIMEOUT;
        loop {
            let sr = read_status();
            // A latched erase/program error means the op was rejected (e.g. a
            // protected sector) and WIP will never clear on its own — fail fast.
            if sr & (SR_E_ERR | SR_P_ERR) != 0 {
                return false;
            }
            if sr & SR_WIP == 0 {
                return true;
            }
            if n == 0 {
                return false;
            }
            n -= 1;
            compiler_fence(Ordering::SeqCst);
        }
    }
}

/// Issue CLEAR STATUS REGISTER (0x30): clears latched program/erase error bits
/// (and the stuck WIP they hold) so the part accepts new commands again.  Used to
/// recover after a [`wait_wip`] timeout.
#[inline]
unsafe fn clear_status() {
    unsafe {
        wr32(SMCMR, CMD_CLEAR_STATUS << 16);
        wr32(SMADR, 0);
        wr32(SMENR, SMENR_CDE);
        wr32(SMCR, SMCR_SPIE);
        let _ = wait_tend();
    }
}

/// Erase the 256 KB sector at sector-aligned `offset`.  Assumes manual mode is
/// already active.  Refuses anything outside the app slot — the single choke
/// point for every erase path.
///
/// `allow_protected` (only ever `true` on the feature-gated forced path) relaxes
/// the writable-window check to a chip-bounds check, so a recovery tool can erase
/// the FSB / SSB; the anti-aliasing chip-size bound is kept either way.
unsafe fn erase_sector_at(offset: u32, allow_protected: bool) {
    let ok = if allow_protected {
        in_chip(offset, SECTOR_SIZE)
    } else {
        write_allowed(offset, SECTOR_SIZE)
    };
    if !ok {
        debug_assert!(false, "spibsc: refused erase outside writable/in-chip range");
        return;
    }
    unsafe {
        write_enable();
        wr32(SMCMR, CMD_SECTOR_ERASE << 16);
        wr32(SMADR, offset);
        wr32(SMENR, SMENR_CDE | SMENR_ADE_3B);
        wr32(SMCR, SMCR_SPIE);
        let _ = wait_tend();
        // On timeout the erase was rejected (e.g. a protected sector latches
        // E_ERR and never clears WIP); recover the part so it accepts the next
        // command instead of spinning here forever.
        if !wait_wip() {
            clear_status();
        }
    }
}

/// Page-program helper that assumes manual mode is already active (so multi-page
/// [`program`] does not toggle read mode per page).  Refuses anything outside the
/// app slot — the single choke point for every program path.
///
/// Mirrors the Renesas `R_SFLASH_ByteProgram` sequence: a command+address
/// transfer with SSL kept, then data transfers in 32/16/8-bit units with the
/// datum MSB-aligned in `SMWDR0`, SSL negated on the last unit.
///
/// `allow_protected` (only ever `true` on the feature-gated forced path) relaxes
/// the writable-window check to a chip-bounds check; see [`erase_sector_at`].
unsafe fn program_page_manual(offset: u32, data: &[u8], allow_protected: bool) {
    if data.is_empty() {
        return;
    }
    let ok = if allow_protected {
        in_chip(offset, data.len() as u32)
    } else {
        write_allowed(offset, data.len() as u32)
    };
    if !ok {
        debug_assert!(false, "spibsc: refused program outside writable/in-chip range");
        return;
    }
    unsafe {
        write_enable();

        // Transfer 1: PP command + 3-byte address, SSL kept, no data.
        wr32(SMCMR, CMD_PP << 16);
        wr32(SMADR, offset);
        wr32(SMENR, SMENR_CDE | SMENR_ADE_3B);
        wr32(SMCR, SMCR_SPIE | SMCR_SSLKP);
        let _ = wait_tend();

        // Data transfers: command/address disabled, write enabled.
        let n = data.len();
        let mut i = 0usize;
        while i < n {
            let rem = n - i;
            let (unit, spide, val) = if rem >= 4 {
                (
                    4usize,
                    SMENR_SPIDE_32,
                    u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]),
                )
            } else if rem >= 2 {
                (
                    2usize,
                    SMENR_SPIDE_16,
                    (u16::from_le_bytes([data[i], data[i + 1]]) as u32) << 16,
                )
            } else {
                (1usize, SMENR_SPIDE_8, (data[i] as u32) << 24)
            };
            let last = i + unit >= n;
            wr32(SMCMR, 0);
            wr32(SMWDR0, val);
            wr32(SMENR, spide);
            let mut smcr = SMCR_SPIE | SMCR_SPIWE;
            if !last {
                smcr |= SMCR_SSLKP;
            }
            wr32(SMCR, smcr);
            let _ = wait_tend();
            i += unit;
        }
        // Recover if the program was rejected and left WIP stuck (see
        // `erase_sector_at`), so we never spin here forever.
        if !wait_wip() {
            clear_status();
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read the 3-byte JEDEC ID (`RDID`, 0x9F): manufacturer, memory type, and
/// **density**.  The density byte gives the chip's true size (`1 << density`
/// bytes), which [`chip_capacity`] uses to bound every write.  A `00 00 00`
/// result means manual mode isn't communicating (see [`stop`]); a plausible
/// non-`00`/`FF` ID confirms the register plumbing.  Runs its manual-mode window
/// with interrupts masked.
pub fn read_id() -> [u8; 3] {
    critical_section::with(|_| unsafe {
        to_manual();
        // Command + a single 32-bit read in one SSL assertion. For a 32-bit
        // SPIDE unit the bytes are little-endian within SMRDR0 — the first byte
        // received off the wire is in `[7:0]` — mirroring the `from_le_bytes`
        // packing the program path uses for a 32-bit write unit. (This differs
        // from an 8-bit SPIDE read, where the lone byte is MSB-aligned at
        // `[31:24]`; see `read_status`.)
        wr32(SMCMR, CMD_RDID << 16);
        wr32(SMADR, 0);
        wr32(SMENR, SMENR_CDE | SMENR_SPIDE_32);
        wr32(SMWDR0, 0);
        wr32(SMCR, SMCR_SPIE | SMCR_SPIRE);
        let _ = wait_tend();
        let v = rd32(SMRDR0);
        to_read_mode();
        // [manufacturer, memory type, density] in wire order = the three lowest
        // bytes of the little-endian word.
        [v as u8, (v >> 8) as u8, (v >> 16) as u8]
    })
}

/// Read the flash status register (`RDSR`, 0x05) once.  Diagnostic helper: the
/// low bits report write-in-progress (bit 0) and write-enable-latch (bit 1), and
/// `BP[2:0]` (bits 2-4) report block protection — useful for explaining why an
/// erase was rejected.  Runs its manual-mode window with interrupts masked.
pub fn read_status_reg() -> u8 {
    critical_section::with(|_| unsafe {
        to_manual();
        let s = read_status() as u8;
        to_read_mode();
        s
    })
}

/// Erase the 256 KB sector containing `offset` (flash-relative).
///
/// # Safety
/// Switches the flash bus out of memory-mapped read mode, so no code may execute
/// from flash during the call.  Offsets outside the app slot are refused.
pub unsafe fn erase_sector(offset: u32) {
    // Probe the chip size *before* entering manual mode (it issues its own RDID);
    // the per-sector guard then reads the cached value.
    chip_capacity();
    // Interrupts masked for the whole manual-mode window (see module docs).
    critical_section::with(|_| unsafe {
        to_manual();
        erase_sector_at(offset & !(SECTOR_SIZE - 1), false);
        to_read_mode();
    });
}

/// Erase every 256 KB sector touched by `[offset, offset+len)` (flash-relative).
///
/// # Safety
/// See [`erase_sector`].
pub unsafe fn erase_range(offset: u32, len: u32) {
    chip_capacity(); // probe chip size before manual mode (see `erase_sector`)
    // Interrupts masked for the whole manual-mode window (see module docs).
    critical_section::with(|_| unsafe {
        let mut addr = offset & !(SECTOR_SIZE - 1);
        let end = offset + len;
        to_manual();
        while addr < end {
            erase_sector_at(addr, false);
            addr += SECTOR_SIZE;
        }
        to_read_mode();
    });
}

/// Program up to [`PAGE`] bytes at `offset` (flash-relative).  The byte range
/// must not cross a page boundary and the sector must already be erased.
///
/// # Safety
/// See [`erase_sector`].
pub unsafe fn program_page(offset: u32, data: &[u8]) {
    chip_capacity(); // probe chip size before manual mode (see `erase_sector`)
    // Interrupts masked for the whole manual-mode window (see module docs).
    critical_section::with(|_| unsafe {
        to_manual();
        program_page_manual(offset, data, false);
        to_read_mode();
    });
}

/// Program an arbitrary-length buffer at `offset` (flash-relative), splitting it
/// into [`PAGE`]-bounded writes.  The target range must already be erased.
///
/// # Safety
/// See [`erase_sector`].
pub unsafe fn program(offset: u32, data: &[u8]) {
    chip_capacity(); // probe chip size before manual mode (see `erase_sector`)
    // Interrupts masked for the whole manual-mode window (see module docs).
    critical_section::with(|_| unsafe {
        to_manual();
        let mut addr = offset;
        let mut rest = data;
        while !rest.is_empty() {
            let page_room = PAGE - (addr as usize & (PAGE - 1));
            let chunk = core::cmp::min(page_room, rest.len());
            program_page_manual(addr, &rest[..chunk], false);
            addr += chunk as u32;
            rest = &rest[chunk..];
        }
        to_read_mode();
    });
}

// ---------------------------------------------------------------------------
// Forced (unlocked) writes — recovery tooling only
// ---------------------------------------------------------------------------
//
// These bypass the writable-window guard ([`writable`]) so a JTAG-loaded recovery
// program can (re)write the protected low regions — the FSB, the device-settings
// sector, and the SSB — which the normal API refuses.  They are gated behind the
// `unlock-bootloader` cargo feature so the capability cannot be linked into normal
// firmware (or the app-loader) by accident.  The anti-aliasing chip-bounds check
// ([`in_chip`]) is retained: that is what prevents an out-of-range offset from
// wrapping onto a low sector, and dropping it is what once erased the FSB.

/// Erase every 256 KB sector touched by `[offset, offset+len)` (flash-relative),
/// **including the protected FSB / settings / SSB regions**.
///
/// # Safety
/// See [`erase_sector`].  In addition, this can erase the first-stage bootloader
/// and brick the device until reflashed over JTAG/SPI — only call from a recovery
/// tool that is itself running from SRAM (never from flash).
#[cfg(feature = "unlock-bootloader")]
pub unsafe fn force_erase_range(offset: u32, len: u32) {
    chip_capacity(); // probe chip size before manual mode (see `erase_sector`)
    critical_section::with(|_| unsafe {
        let mut addr = offset & !(SECTOR_SIZE - 1);
        let end = offset + len;
        to_manual();
        while addr < end {
            erase_sector_at(addr, true);
            addr += SECTOR_SIZE;
        }
        to_read_mode();
    });
}

/// Program an arbitrary-length buffer at `offset` (flash-relative), splitting it
/// into [`PAGE`]-bounded writes, **including the protected FSB / settings / SSB
/// regions**.  The target range must already be erased.
///
/// # Safety
/// See [`force_erase_range`].
#[cfg(feature = "unlock-bootloader")]
pub unsafe fn force_program(offset: u32, data: &[u8]) {
    chip_capacity(); // probe chip size before manual mode (see `erase_sector`)
    critical_section::with(|_| unsafe {
        to_manual();
        let mut addr = offset;
        let mut rest = data;
        while !rest.is_empty() {
            let page_room = PAGE - (addr as usize & (PAGE - 1));
            let chunk = core::cmp::min(page_room, rest.len());
            program_page_manual(addr, &rest[..chunk], true);
            addr += chunk as u32;
            rest = &rest[chunk..];
        }
        to_read_mode();
    });
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// The app-slot flash geometry is the contract the app-loader relies on when
    /// it erases/programs a stored image into the slot and when the FSB copies it
    /// back into SRAM. If these literals change, the flash-store path and the
    /// boot-from-flash path must agree — so pin them here.
    #[test]
    fn app_slot_geometry_is_pinned() {
        assert_eq!(SPI_FLASH_BASE, 0x1800_0000);
        assert_eq!(FLASH_SIZE, 0x0040_0000, "the Deluge SPI NOR is 4 MB");
        assert_eq!(FLASH_SLOT_OFFSET, 0x0010_0000);
        assert_eq!(FLASH_SLOT_ADDR, 0x1810_0000);
        assert_eq!(FLASH_SLOT_LEN, 0x002C_0000);
        assert_eq!(SECTOR_SIZE, 0x0001_0000, "0xD8 erases 64 KB on this part");
    }

    /// The slot must be a whole number of erase sectors, and the slot base must
    /// be sector-aligned so erasing a slot sector never spills into the
    /// hardware-reserved region (FSB / settings / SSB) below it.
    #[test]
    fn slot_is_sector_aligned() {
        assert_eq!(FLASH_SLOT_OFFSET % SECTOR_SIZE, 0, "slot base must be sector-aligned");
        assert_eq!(FLASH_SLOT_LEN % SECTOR_SIZE, 0, "slot must be whole sectors");
        assert_eq!(FLASH_SLOT_LEN / SECTOR_SIZE, 44, "2.75 MB / 64 KB = 44 sectors");
    }

    /// The settings sector must be sector-aligned, sit directly above the app
    /// slot, and stay strictly inside the chip — its erase block must not reach
    /// or pass the chip end (placing it *at* `0x400000` aliased to `0x0` and
    /// erased the FSB).  It need not be the literal top sector: with 64 KB
    /// sectors the 64 KB at `0x3C0000` leaves three unused blocks above it, which
    /// is harmless — the settings record only occupies the first page.
    #[test]
    fn settings_sector_is_in_chip_and_above_slot() {
        assert_eq!(SETTINGS_SECTOR_OFFSET, 0x003C_0000);
        assert_eq!(SETTINGS_SECTOR_ADDR, SPI_FLASH_BASE + 0x003C_0000);
        assert_eq!(
            SETTINGS_SECTOR_OFFSET % SECTOR_SIZE,
            0,
            "settings sector must be sector-aligned"
        );
        assert_eq!(
            SETTINGS_SECTOR_OFFSET, FLASH_SLOT_OFFSET + FLASH_SLOT_LEN,
            "settings sector sits directly above the app slot"
        );
        assert!(
            SETTINGS_SECTOR_OFFSET + SECTOR_SIZE <= FLASH_SIZE,
            "settings erase block must stay inside the chip (never at/over the end)"
        );
    }

    /// Every writable offset must stay strictly inside the chip, so no erase can
    /// alias past the end onto a reserved low sector (the FSB-erasing bug).
    #[test]
    fn writable_windows_stay_inside_the_chip() {
        assert!(FLASH_SLOT_OFFSET + FLASH_SLOT_LEN <= FLASH_SIZE);
        assert!(SETTINGS_SECTOR_OFFSET + SECTOR_SIZE <= FLASH_SIZE);
    }

    /// The two writable windows — app slot and settings sector — accept exactly
    /// their own ranges and nothing below the slot (FSB / settings / SSB).
    #[test]
    fn writable_guards_both_windows() {
        // App slot: whole slot and a sub-range are writable; one byte past the
        // top is not, and the reserved region below it is not.
        assert!(writable(FLASH_SLOT_OFFSET, FLASH_SLOT_LEN));
        assert!(writable(FLASH_SLOT_OFFSET, SECTOR_SIZE));
        assert!(!writable(FLASH_SLOT_OFFSET, FLASH_SLOT_LEN + 1));
        assert!(!writable(0, SECTOR_SIZE), "FSB sector must stay protected");

        // Settings sector: exactly its one sector is writable; not one byte more,
        // and not the gap between the slot top and the settings sector.
        assert!(writable(SETTINGS_SECTOR_OFFSET, SECTOR_SIZE));
        assert!(writable(SETTINGS_SECTOR_OFFSET, PAGE as u32));
        assert!(!writable(SETTINGS_SECTOR_OFFSET, SECTOR_SIZE + 1));
        assert!(!writable(SETTINGS_SECTOR_OFFSET + SECTOR_SIZE, PAGE as u32));
        assert!(!writable(0, 0), "zero-length is never writable");
    }

    /// The forced (`unlock-bootloader`) path drops the writable-window guard but
    /// keeps the chip-bounds guard.  `in_chip` must therefore accept the FSB
    /// sector (which `writable` rejects) yet still reject anything that would
    /// reach past the physical chip end — the anti-aliasing protection that must
    /// never be lost, even on the recovery path.
    #[test]
    fn in_chip_allows_fsb_but_keeps_chip_bounds() {
        // Prime the cached capacity to the real 4 MB part (RDID is unavailable on
        // the host); restore it afterwards so test ordering can't leak state.
        let saved = CHIP_CAPACITY.load(Ordering::Relaxed);
        CHIP_CAPACITY.store(FLASH_SIZE, Ordering::Relaxed);

        // FSB sector: blocked by the normal guard, allowed by the forced guard.
        assert!(!writable(0, SECTOR_SIZE), "FSB stays protected on the normal path");
        assert!(in_chip(0, SECTOR_SIZE), "forced path may erase the FSB sector");
        // SSB region too.
        assert!(in_chip(0x0008_0000, SECTOR_SIZE));

        // Chip bounds are still enforced: the last in-chip byte is fine, one past
        // the end is not, and a wrap-prone offset at the chip end is refused.
        assert!(in_chip(FLASH_SIZE - SECTOR_SIZE, SECTOR_SIZE));
        assert!(!in_chip(FLASH_SIZE - SECTOR_SIZE, SECTOR_SIZE + 1));
        assert!(!in_chip(FLASH_SIZE, PAGE as u32));
        assert!(!in_chip(0, 0), "zero-length is never writable");

        // An unprobed/implausible capacity (0) refuses every forced write.
        CHIP_CAPACITY.store(0, Ordering::Relaxed);
        assert!(!in_chip(0, SECTOR_SIZE), "unknown capacity refuses forced writes");

        CHIP_CAPACITY.store(saved, Ordering::Relaxed);
    }
}
