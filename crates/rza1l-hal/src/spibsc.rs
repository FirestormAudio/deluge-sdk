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
//! **Critical data-register alignment** (per the sample BSP): sub-32-bit manual
//! transfers are **MSB-aligned** in `SMWDR0`/`SMRDR0` — an 8-bit datum is at bits
//! `[31:24]`, a 16-bit datum at `[31:16]`; a 32-bit datum is the little-endian
//! word as-is.  [`read_id`] is a cheap power-on sanity check: it must return the
//! S25FL512 JEDEC ID before any erase/program is trusted, and callers should read
//! programmed data back through the memory-mapped window (byte-exact) to verify.

use core::sync::atomic::{Ordering, compiler_fence};

// ---------------------------------------------------------------------------
// Flash geometry
// ---------------------------------------------------------------------------

/// Base of the cached memory-mapped read window — flash offset 0 maps here.
pub const SPI_FLASH_BASE: u32 = 0x1800_0000;
/// Flash-relative offset of the bootable app slot.
///
/// Everything below this is **hardware-reserved** and is never erased or
/// programmed by this driver — [`erase_sector`] / [`program`] refuse any offset
/// outside the slot (see [`writable`]).  The Deluge QSPI map, by 256 KB sector:
///   * sector 0   `0x00000..0x40000`  — first-stage bootloader (FSB)
///   * sector 1   `0x40000..0x80000`  — Deluge device-settings (data at `0x7F000`;
///     DelugeFirmware erases this whole 256 KB sector when it saves settings)
///   * sectors 2-3 `0x80000..0x100000` — second-stage bootloader (SSB); the FSB
///     loads the SSB from `0x80000`
///
/// The bootable app sits **above** the SSB so storing an app can never touch the
/// FSB, the settings, or the SSB itself.  `0x100000` is 256 KB-aligned, so
/// erasing a slot sector never spills into the reserved region below.
pub const FLASH_SLOT_OFFSET: u32 = 0x0010_0000; // 1 MB
/// Memory-mapped (read-window) address of the app slot.
pub const FLASH_SLOT_ADDR: u32 = SPI_FLASH_BASE + FLASH_SLOT_OFFSET;
/// Length of the app slot (3 MB: `0x100000..0x400000`, the top of the
/// FSB-managed firmware window; 12 × 256 KB sectors).
pub const FLASH_SLOT_LEN: u32 = 0x0030_0000;

/// Uniform erase-sector size of the S25FL512 (**256 KB**).  This part has no
/// 4 KB/64 KB erase — the only granularities are this sector and full-chip.
pub const SECTOR_SIZE: u32 = 0x0004_0000;
/// Program chunk size.  The S25FL512 page buffer is 512 bytes, but the Deluge
/// bootloader programs the firmware region in 256-byte units with the note
/// "Bigger doesn't seem to work" on this board (`FLASH_WRITE_SIZE` in
/// `DelugeBootloader/src/spibsc_init2.c`), so we match that: a single program is
/// at most 256 bytes and never crosses a 256-byte boundary.
pub const PAGE: usize = 256;

/// `true` if `[offset, offset+len)` lies entirely within the writable app slot.
///
/// Belt-and-suspenders write protection: every erase and program funnels through
/// a check against this, so a caller bug can never reach the FSB, the
/// device-settings sector, or the SSB below [`FLASH_SLOT_OFFSET`].
#[inline]
fn writable(offset: u32, len: u32) -> bool {
    len > 0
        && offset >= FLASH_SLOT_OFFSET
        && (offset as u64 + len as u64) <= (FLASH_SLOT_OFFSET as u64 + FLASH_SLOT_LEN as u64)
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

// JEDEC / S25FL512 opcodes
const CMD_WREN: u32 = 0x06;
const CMD_RDSR: u32 = 0x05;
const CMD_RDID: u32 = 0x9F;
const CMD_PP: u32 = 0x02; // page program (3-byte address)
const CMD_SECTOR_ERASE: u32 = 0xD8; // 256 KB uniform sector erase (3-byte address)
const SR_WIP: u32 = 1 << 0; // status register: write-in-progress

#[inline(always)]
unsafe fn wr32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

#[inline(always)]
unsafe fn rd32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Spin until the current manual-mode transfer completes.
#[inline]
unsafe fn wait_tend() {
    unsafe {
        while rd32(CMNSR) & CMNSR_TEND == 0 {
            compiler_fence(Ordering::SeqCst);
        }
    }
}

/// Enter manual SPI operating mode (sets `CMNCR.MD`).  The `DR*` (memory-mapped
/// read) configuration left by the first-stage bootloader is preserved, so
/// [`to_read_mode`] only has to clear `MD` to restore reads.
#[inline]
unsafe fn to_manual() {
    unsafe { wr32(CMNCR, rd32(CMNCR) | CMNCR_MD) }
}

/// Return to memory-mapped read mode and flush the read cache so subsequent
/// memory-mapped reads see freshly programmed data.
#[inline]
unsafe fn to_read_mode() {
    unsafe {
        wr32(CMNCR, rd32(CMNCR) & !CMNCR_MD);
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
        wait_tend();
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
        wait_tend();
        (rd32(SMRDR0) >> 24) & 0xFF
    }
}

/// Spin until the flash clears its write-in-progress bit.
#[inline]
unsafe fn wait_wip() {
    unsafe {
        while read_status() & SR_WIP != 0 {
            compiler_fence(Ordering::SeqCst);
        }
    }
}

/// Erase the 256 KB sector at sector-aligned `offset`.  Assumes manual mode is
/// already active.  Refuses anything outside the app slot — the single choke
/// point for every erase path.
unsafe fn erase_sector_at(offset: u32) {
    if !writable(offset, SECTOR_SIZE) {
        debug_assert!(false, "spibsc: refused erase outside app slot");
        return;
    }
    unsafe {
        write_enable();
        wr32(SMCMR, CMD_SECTOR_ERASE << 16);
        wr32(SMADR, offset);
        wr32(SMENR, SMENR_CDE | SMENR_ADE_3B);
        wr32(SMCR, SMCR_SPIE);
        wait_tend();
        wait_wip();
    }
}

/// Page-program helper that assumes manual mode is already active (so multi-page
/// [`program`] does not toggle read mode per page).  Refuses anything outside the
/// app slot — the single choke point for every program path.
///
/// Mirrors the Renesas `R_SFLASH_ByteProgram` sequence: a command+address
/// transfer with SSL kept, then data transfers in 32/16/8-bit units with the
/// datum MSB-aligned in `SMWDR0`, SSL negated on the last unit.
unsafe fn program_page_manual(offset: u32, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    if !writable(offset, data.len() as u32) {
        debug_assert!(false, "spibsc: refused program outside app slot");
        return;
    }
    unsafe {
        write_enable();

        // Transfer 1: PP command + 3-byte address, SSL kept, no data.
        wr32(SMCMR, CMD_PP << 16);
        wr32(SMADR, offset);
        wr32(SMENR, SMENR_CDE | SMENR_ADE_3B);
        wr32(SMCR, SMCR_SPIE | SMCR_SSLKP);
        wait_tend();

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
            wait_tend();
            i += unit;
        }
        wait_wip();
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read the 3-byte JEDEC ID (`RDID`, 0x9F): manufacturer, memory type, capacity.
/// For the S25FL512S this is `01 02 20`.
///
/// Use this as a power-on sanity check before trusting any erase/program — a
/// plausible non-`00`/`FF` ID confirms the manual-mode register plumbing.
pub fn read_id() -> [u8; 3] {
    unsafe {
        to_manual();
        // Command + a single 32-bit read in one SSL assertion; the four received
        // bytes land MSB-first in SMRDR0 (`[31:24]` = first byte).
        wr32(SMCMR, CMD_RDID << 16);
        wr32(SMADR, 0);
        wr32(SMENR, SMENR_CDE | SMENR_SPIDE_32);
        wr32(SMWDR0, 0);
        wr32(SMCR, SMCR_SPIE | SMCR_SPIRE);
        wait_tend();
        let v = rd32(SMRDR0);
        to_read_mode();
        [(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8]
    }
}

/// Erase the 256 KB sector containing `offset` (flash-relative).
///
/// # Safety
/// Switches the flash bus out of memory-mapped read mode, so no code may execute
/// from flash during the call.  Offsets outside the app slot are refused.
pub unsafe fn erase_sector(offset: u32) {
    unsafe {
        to_manual();
        erase_sector_at(offset & !(SECTOR_SIZE - 1));
        to_read_mode();
    }
}

/// Erase every 256 KB sector touched by `[offset, offset+len)` (flash-relative).
///
/// # Safety
/// See [`erase_sector`].
pub unsafe fn erase_range(offset: u32, len: u32) {
    unsafe {
        let mut addr = offset & !(SECTOR_SIZE - 1);
        let end = offset + len;
        to_manual();
        while addr < end {
            erase_sector_at(addr);
            addr += SECTOR_SIZE;
        }
        to_read_mode();
    }
}

/// Program up to [`PAGE`] bytes at `offset` (flash-relative).  The byte range
/// must not cross a page boundary and the sector must already be erased.
///
/// # Safety
/// See [`erase_sector`].
pub unsafe fn program_page(offset: u32, data: &[u8]) {
    unsafe {
        to_manual();
        program_page_manual(offset, data);
        to_read_mode();
    }
}

/// Program an arbitrary-length buffer at `offset` (flash-relative), splitting it
/// into [`PAGE`]-bounded writes.  The target range must already be erased.
///
/// # Safety
/// See [`erase_sector`].
pub unsafe fn program(offset: u32, data: &[u8]) {
    unsafe {
        to_manual();
        let mut addr = offset;
        let mut rest = data;
        while !rest.is_empty() {
            let page_room = PAGE - (addr as usize & (PAGE - 1));
            let chunk = core::cmp::min(page_room, rest.len());
            program_page_manual(addr, &rest[..chunk]);
            addr += chunk as u32;
            rest = &rest[chunk..];
        }
        to_read_mode();
    }
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
        assert_eq!(FLASH_SLOT_OFFSET, 0x0010_0000);
        assert_eq!(FLASH_SLOT_ADDR, 0x1810_0000);
        assert_eq!(FLASH_SLOT_LEN, 0x0030_0000);
        assert_eq!(SECTOR_SIZE, 0x0004_0000);
    }

    /// The slot must be a whole number of erase sectors, and the slot base must
    /// be sector-aligned so erasing a slot sector never spills into the
    /// hardware-reserved region (FSB / settings / SSB) below it.
    #[test]
    fn slot_is_sector_aligned() {
        assert_eq!(FLASH_SLOT_OFFSET % SECTOR_SIZE, 0, "slot base must be sector-aligned");
        assert_eq!(FLASH_SLOT_LEN % SECTOR_SIZE, 0, "slot must be whole sectors");
        assert_eq!(FLASH_SLOT_LEN / SECTOR_SIZE, 12, "3 MB / 256 KB = 12 sectors");
    }
}
