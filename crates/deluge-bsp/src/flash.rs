//! Deluge SPI-NOR flash profile: the chip geometry and board memory map.
//!
//! The SPIBSC *controller* (manual-mode transfers, memory-mapped read mode, JEDEC
//! reads, the chip-bounds anti-alias guard) is SoC-generic and lives in
//! [`rza1l_hal::spibsc`].  The facts that depend on **which flash part is on the
//! Deluge and how its space is carved up** live here, so a different part or board
//! is a one-line change.
//!
//! ## The part
//! The Deluge's SPI NOR is a **4 MB** S25FL032-class part with **64 KB** uniform
//! erase blocks (the `0xD8` block erase clears 64 KB — an earlier 256 KB
//! assumption, copied from an S25FL512 reference, left three quarters of every
//! region un-erased and corrupted multi-block stores) and 256-byte program pages.
//!
//! ## The map (flash-relative offsets)
//! | Region | Bytes | Use |
//! |--------|-------|-----|
//! | `0x000000..0x040000` | 256 KB | first-stage bootloader (FSB) — reserved |
//! | `0x040000..0x080000` | 256 KB | Deluge device-settings — reserved |
//! | `0x080000..0x100000` | 512 KB | second-stage bootloader (SSB) — reserved |
//! | `0x100000..0x3C0000` | 2.75 MB | **app slot** ([`SLOT_OFFSET`]) — writable |
//! | `0x3C0000..0x3D0000` | 64 KB | app-loader **settings** ([`SETTINGS_OFFSET`]) — writable |
//!
//! The app slot sits above the SSB so storing an app can never touch the FSB,
//! device settings, or SSB.  Only the two writable windows above are listed in
//! [`MAP`]; every erase/program through [`MAP`] is refused outside them (and
//! outside the physically present chip).

use rza1l_hal::spibsc::{self, FlashMap};

/// Erase-block size in bytes — the `0xD8` block-erase granularity of the part.
pub const SECTOR_SIZE: u32 = 0x0001_0000; // 64 KB
/// Page-program size in bytes.  The Deluge bootloader programs in 256-byte units
/// ("Bigger doesn't seem to work" on this board); a program never crosses a page.
pub const PAGE: usize = 256;
/// Total flash size — the Deluge SPI NOR is 4 MB; nothing exists above this and an
/// offset at/beyond it aliases modulo the chip size onto a low sector.
pub const FLASH_SIZE: u32 = 0x0040_0000; // 4 MB

/// Flash-relative offset of the bootable app slot (above the reserved FSB/SSB).
pub const SLOT_OFFSET: u32 = 0x0010_0000; // 1 MB
/// Memory-mapped (read-window) address of the app slot.
pub const SLOT_ADDR: u32 = spibsc::SPI_FLASH_BASE + SLOT_OFFSET;
/// Length of the app slot (2.75 MB: `0x100000..0x3C0000`; 44 × 64 KB blocks).  The
/// slot stops short of the chip end so the settings block can sit above it.
pub const SLOT_LEN: u32 = 0x002C_0000;

/// Flash-relative offset of the app-loader settings block, directly above the
/// slot.  Only its first 64 KB block is used; the settings record occupies the
/// first page.  Must stay strictly inside the chip (a previous `0x400000` choice
/// aliased to `0x0` and erased the FSB).
pub const SETTINGS_OFFSET: u32 = 0x003C_0000;
/// Memory-mapped (read-window) address of the settings block.
pub const SETTINGS_ADDR: u32 = spibsc::SPI_FLASH_BASE + SETTINGS_OFFSET;

/// The board's app-writable windows: the app slot and the settings block.  Erase
/// and program through [`MAP`] refuse any range outside these (and outside the
/// chip), keeping a caller bug away from the FSB / settings / SSB.
const WRITABLE: &[core::ops::Range<u32>] = &[
    SLOT_OFFSET..SLOT_OFFSET + SLOT_LEN,
    SETTINGS_OFFSET..SETTINGS_OFFSET + SECTOR_SIZE,
];

/// The Deluge flash profile: geometry + writable-window policy.  Hand this to the
/// SPIBSC controller for every guarded erase/program, e.g.
/// `unsafe { flash::MAP.erase_range(flash::SLOT_OFFSET, len) }`.
pub const MAP: FlashMap = FlashMap {
    sector_size: SECTOR_SIZE,
    page: PAGE,
    writable: WRITABLE,
};

// Controller-level reads need no map; re-export them so flash access funnels
// through this module.
pub use rza1l_hal::spibsc::{read_id, read_status_reg};

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot must be a whole number of 64 KB erase blocks, block-aligned, so
    /// erasing a slot block never spills into the reserved region (FSB/SSB) below.
    #[test]
    fn slot_is_block_aligned() {
        assert_eq!(
            SLOT_OFFSET % SECTOR_SIZE,
            0,
            "slot base must be block-aligned"
        );
        assert_eq!(SLOT_LEN % SECTOR_SIZE, 0, "slot must be whole blocks");
        assert_eq!(SLOT_LEN / SECTOR_SIZE, 44, "2.75 MB / 64 KB = 44 blocks");
        assert_eq!(SLOT_ADDR, 0x1810_0000);
    }

    /// The settings block is block-aligned, sits directly above the slot, and
    /// stays strictly inside the chip (its erase block must not reach/pass the
    /// chip end — the alias that once erased the FSB).
    #[test]
    fn settings_block_is_in_chip_and_above_slot() {
        assert_eq!(
            SETTINGS_OFFSET % SECTOR_SIZE,
            0,
            "settings must be block-aligned"
        );
        assert_eq!(
            SETTINGS_OFFSET,
            SLOT_OFFSET + SLOT_LEN,
            "settings sits above the slot"
        );
        assert!(
            SETTINGS_OFFSET + SECTOR_SIZE <= FLASH_SIZE,
            "settings stays inside the chip"
        );
    }

    /// Both writable windows stay strictly inside the chip, so no erase can alias
    /// past the end onto a reserved low sector.
    #[test]
    fn writable_windows_stay_inside_the_chip() {
        for w in MAP.writable {
            assert!(w.start < w.end);
            assert!(w.end <= FLASH_SIZE);
        }
        assert_eq!(
            MAP.writable.len(),
            2,
            "exactly the slot and the settings block"
        );
        assert_eq!(MAP.sector_size, SECTOR_SIZE);
        assert_eq!(MAP.page, PAGE);
    }
}
