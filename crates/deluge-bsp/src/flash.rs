//! Deluge SPI-NOR flash profile: the chip geometry and board memory map.
//!
//! The SPIBSC *controller* (manual-mode transfers, memory-mapped read mode, JEDEC
//! reads, the chip-bounds anti-alias guard) is SoC-generic and lives in
//! [`rza1_hal::spibsc`].  The facts that depend on **which flash part is on the
//! Deluge and how its space is carved up** live here, so a different part or board
//! is a one-line change.
//!
//! ## The part
//! The Deluge's SPI NOR is a **4 MB** SPI-NOR part with **64 KB** uniform erase
//! blocks (the `0xD8` block erase clears 64 KB, not the 256 KB of larger parts such
//! as the S25FL512) and 256-byte program pages. The board is second-sourced across
//! three pin-compatible parts (AT25SF321B / S25FL132K / W25Q32JV); identify the
//! populated one at runtime with [`identify`].
//!
//! ## The map (flash-relative offsets)
//! | Region | Bytes | Use |
//! |--------|-------|-----|
//! | `0x000000..0x040000` | 256 KB | first-stage bootloader (FSB) — reserved, **protected** |
//! | `0x040000..0x080000` | 256 KB | reserved (was device-settings; relocated) — **protected** |
//! | `0x080000..0x100000` | 512 KB | second-stage bootloader (SSB) — reserved, **protected** |
//! | `0x100000..0x3C0000` | 2.75 MB | **app slot** ([`SLOT_OFFSET`]) — writable |
//! | `0x3C0000..0x3D0000` | 64 KB | app-loader **settings** ([`SETTINGS_OFFSET`]) — writable |
//! | `0x3D0000..0x3E0000` | 64 KB | **Deluge device-settings** ([`DELUGE_SETTINGS_OFFSET`]) — writable |
//! | `0x3E0000..0x400000` | 128 KB | free |
//!
//! The bottom **1 MB** (`0x000000..0x100000` = FSB + SSB, see [`PROTECTED_LEN`])
//! can be hardware write-protected with the chip's block-protect bits (the
//! `flash-protect` feature, [`protect`]), so a running app can't erase a bootloader. Device
//! settings were moved out of that region (was `0x07F000`) into the writable zone
//! above the slot. Only the three writable windows are in [`MAP`]; every
//! erase/program through [`MAP`] is refused outside them (and outside the chip).

use rza1_hal::spibsc::{self, FlashMap};

/// Per-part bottom-1 MB write-protection recipe + arming ops (built on the
/// generic SR transfers in `rza1_hal::spibsc`). One data table for all three
/// second-sourced parts — see [`protect`].
pub mod protect;

/// Erase-block size in bytes — the `0xD8` block-erase granularity of the part.
pub const SECTOR_SIZE: u32 = 0x0001_0000; // 64 KB
/// Page-program size in bytes, matching the Deluge bootloader, which found larger
/// program units unreliable on this board; a program never crosses a page.
pub const PAGE: usize = 256;
/// Total flash size — the Deluge SPI NOR is 4 MB; nothing exists above this and an
/// offset at/beyond it aliases modulo the chip size onto a low sector.
pub const FLASH_SIZE: u32 = 0x0040_0000; // 4 MB

/// Length of the hardware-write-protected region at the bottom of flash: the FSB
/// + the (now-vacated) device-settings region + the SSB = `0x000000..0x100000`.
/// Each supported part's block-protect recipe locks exactly this 1 MB (see
/// [`protect::recipe`]); the boundary equals [`SLOT_OFFSET`], so everything the
/// app or loader writes lies above it.
pub const PROTECTED_LEN: u32 = 0x0010_0000; // 1 MB (FSB + SSB)

/// Flash-relative offset of the bootable app slot (above the reserved FSB/SSB).
pub const SLOT_OFFSET: u32 = 0x0010_0000; // 1 MB
/// Memory-mapped (read-window) address of the app slot.
pub const SLOT_ADDR: u32 = spibsc::SPI_FLASH_BASE + SLOT_OFFSET;
/// Length of the app slot (2.75 MB: `0x100000..0x3C0000`; 44 × 64 KB blocks).  The
/// slot stops short of the chip end so the settings block can sit above it.
pub const SLOT_LEN: u32 = 0x002C_0000;

/// Flash-relative offset of the app-loader settings block, directly above the
/// slot.  Only its first 64 KB block is used; the settings record occupies the
/// first page.  Must stay strictly inside the chip: an offset at the chip end
/// (`0x400000`) aliases to `0x0` and would erase the FSB.
pub const SETTINGS_OFFSET: u32 = 0x003C_0000;
/// Memory-mapped (read-window) address of the settings block.
pub const SETTINGS_ADDR: u32 = spibsc::SPI_FLASH_BASE + SETTINGS_OFFSET;

/// Flash-relative base of the **Deluge app's** device settings (the firmware's
/// `FlashStorage`), distinct from the app-loader's own [`SETTINGS_OFFSET`].
///
/// **Relocated** out of the original `0x07F000` — which sits inside the now
/// hardware-protected bottom 1 MB ([`PROTECTED_LEN`]) and would become read-only —
/// to its own 64 KB block in the writable zone above the loader settings. A
/// one-time boot migration copies the old record forward (reads of the protected
/// region are always allowed; the new target is writable). Its erase block is
/// `0x3D0000..0x3E0000`.
pub const DELUGE_SETTINGS_OFFSET: u32 = 0x003D_0000;
/// The 64 KB erase block containing [`DELUGE_SETTINGS_OFFSET`].
const DELUGE_SETTINGS_SECTOR: u32 = 0x003D_0000;
/// Legacy device-settings offset (pre-relocation), now inside the protected
/// region and read-only. Used only by the one-time forward migration.
pub const DELUGE_SETTINGS_LEGACY_OFFSET: u32 = 0x0007_F000;

/// First byte of a Deluge settings record that means "no settings saved".
///
/// The app stores `FirmwareVersion::Type` there (`Entries::FIRMWARE_TYPE`) and
/// treats `UNKNOWN` as absent; `UNKNOWN` is `255`, which is also the NOR-erased
/// byte — so "blank sector" and "never saved" are one test. That coincidence is
/// the entire format contract the migration depends on.
pub const DELUGE_SETTINGS_BLANK: u8 = 0xFF;

/// Whether the one-time forward migration should copy the app's settings record
/// from [`DELUGE_SETTINGS_LEGACY_OFFSET`] to [`DELUGE_SETTINGS_OFFSET`], given the
/// first byte found at each.
///
/// Copy only when the new location is blank *and* the legacy one holds a record: a
/// record already at the new offset is live and must never be overwritten by the
/// stale legacy copy, and a blank legacy offset means the unit never ran the old
/// layout. Both blank is a factory-fresh unit.
pub const fn deluge_settings_need_migration(current_head: u8, legacy_head: u8) -> bool {
    current_head == DELUGE_SETTINGS_BLANK && legacy_head != DELUGE_SETTINGS_BLANK
}

/// The board's app-writable windows: the app slot, the app-loader settings block,
/// and the Deluge app's device-settings block.  Erase and program through [`MAP`]
/// refuse any range outside these (and outside the chip), keeping a caller bug away
/// from the FSB / SSB.
const WRITABLE: &[core::ops::Range<u32>] = &[
    SLOT_OFFSET..SLOT_OFFSET + SLOT_LEN,
    SETTINGS_OFFSET..SETTINGS_OFFSET + SECTOR_SIZE,
    DELUGE_SETTINGS_SECTOR..DELUGE_SETTINGS_SECTOR + SECTOR_SIZE,
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
pub use rza1_hal::spibsc::{read_id, read_status_reg};

/// Put the flash's read mode on the stock Deluge bootloader's final settings: single-bit `0Bh`
/// fast read with 8 dummy cycles and the read cache on, at SPBR = 1, BRDV = 0 (66.67 Mbps at the
/// Deluge's clocks), as `DelugeBootloader/src/spibsc_ioset_userdef.c` configures channel 0. A
/// first stage that leaves the boot ROM's `03h` reads hands over a window several times slower.
///
/// # Safety
/// Nothing may execute from the flash window during the call, and no flash operation may be in
/// progress.
pub unsafe fn set_stock_read_mode() {
    // SAFETY: the caller's contract.
    unsafe { spibsc::set_fast_read(1, 0) }
}

/// Prime the SPIBSC write-bounds guard with the true capacity decoded from the
/// JEDEC ID, so writes work on parts whose density byte isn't a `log2(size)`
/// (the AT25SF321B reports `1F 87 01`, density byte `0x01`, which the controller's
/// `1<<density` fallback would read as "implausible ⇒ refuse all writes"). Call
/// once early at boot. An unrecognized part decodes to capacity `0`, which the
/// setter ignores, leaving the fail-safe "unknown ⇒ refuse writes" in place.
pub fn prime_capacity() {
    rza1_hal::spibsc::set_chip_capacity(identify().capacity);
}

/// A decoded SPI-NOR part identity from its 3-byte JEDEC `RDID` (0x9F).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashId {
    /// Raw RDID in wire order: `[manufacturer, memory type, density/device]`.
    pub raw: [u8; 3],
    /// Human-readable model, or `"unknown"` / `"no flash"` when unrecognized.
    pub model: &'static str,
    /// Usable capacity in bytes when known from the ID, else `0`.
    pub capacity: u32,
}

/// Read and decode the flash's JEDEC ID. Issues `RDID` (0x9F) via [`read_id`] and
/// maps it to a [`FlashId`]; see [`decode_jedec`].
pub fn identify() -> FlashId {
    let raw = read_id();
    let (model, capacity) = decode_jedec(raw);
    FlashId {
        raw,
        model,
        capacity,
    }
}

/// Map a 3-byte JEDEC `RDID` to `(model, capacity-in-bytes)`.
///
/// The Deluge's flash is second-sourced across three 4 MB / 64 KB-sector parts
/// (datasheets from Synthstrom): **S25FL132K** (`01 40 16`), Winbond **W25Q32JV**
/// (`EF 40 16`) and Adesto/Renesas **AT25SF321B** (`1F 87 01`). The legacy
/// Cypress/Spansion **S25FL032P** (`01 02 15`) is *not* a known Deluge part — it was an
/// assumption inherited from the original Deluge C firmware — but it is still recognized
/// here in case one ever turns up. A `00`/`FF` manufacturer byte means the manual-mode
/// bus isn't communicating (see `rza1_hal::spibsc`). Capacity is `0` when the exact
/// part isn't recognized — callers must treat unknown capacity as "do not write" (the
/// controller's own `chip_capacity` guard does).
pub const fn decode_jedec(id: [u8; 3]) -> (&'static str, u32) {
    match id {
        // Cypress / Spansion / Infineon (manufacturer 0x01)
        [0x01, 0x02, 0x15] => ("S25FL032P", 0x0040_0000), // not a known Deluge part (see above)
        [0x01, 0x02, _] => ("S25FL0xxP", 0),              // other FL-P family
        [0x01, 0x40, 0x16] => ("S25FL132K", 0x0040_0000),
        [0x01, 0x40, _] => ("S25FL1xxK", 0),
        // Winbond (manufacturer 0xEF)
        [0xEF, 0x40, 0x16] => ("W25Q32JV", 0x0040_0000),
        [0xEF, _, _] => ("Winbond 25Qxx", 0),
        // Adesto / Renesas (manufacturer 0x1F)
        [0x1F, 0x87, 0x01] => ("AT25SF321B", 0x0040_0000),
        [0x1F, 0x88, 0x01] => ("AT25QF641B", 0x0080_0000), // 8 MB: not a Deluge part
        [0x1F, _, _] => ("Adesto AT25xx", 0),
        // Blank bus / no response.
        [0x00, 0x00, 0x00] | [0xFF, 0xFF, 0xFF] => ("no flash", 0),
        _ => ("unknown", 0),
    }
}

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

    /// The JEDEC IDs of every supported part decode to the right model, the
    /// 4 MB Deluge-class parts report their capacity, and a blank/unknown bus is
    /// reported as such with capacity 0 (so the write guard refuses it).
    #[test]
    fn jedec_ids_decode_to_known_parts() {
        assert_eq!(decode_jedec([0x01, 0x02, 0x15]), ("S25FL032P", 0x0040_0000));
        assert_eq!(decode_jedec([0x01, 0x40, 0x16]), ("S25FL132K", 0x0040_0000));
        assert_eq!(decode_jedec([0xEF, 0x40, 0x16]), ("W25Q32JV", 0x0040_0000));
        assert_eq!(
            decode_jedec([0x1F, 0x87, 0x01]),
            ("AT25SF321B", 0x0040_0000)
        );
        // Right family, unrecognized member → named but capacity unknown (0).
        assert_eq!(decode_jedec([0x01, 0x02, 0x16]).1, 0);
        assert_eq!(decode_jedec([0xEF, 0x60, 0x17]), ("Winbond 25Qxx", 0));
        // No comms / unknown → capacity 0 (writes refused).
        assert_eq!(decode_jedec([0x00, 0x00, 0x00]), ("no flash", 0));
        assert_eq!(decode_jedec([0xFF, 0xFF, 0xFF]), ("no flash", 0));
        assert_eq!(decode_jedec([0x12, 0x34, 0x56]), ("unknown", 0));
    }

    /// The settings block is block-aligned, sits directly above the slot, and
    /// stays strictly inside the chip (an erase block reaching the chip end
    /// would alias onto the FSB).
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
            3,
            "exactly the slot, the loader settings block, and the Deluge device-settings block"
        );
        assert_eq!(MAP.sector_size, SECTOR_SIZE);
        assert_eq!(MAP.page, PAGE);
    }

    /// The hardware-protected region is exactly the bottom 1 MB and ends at the
    /// app slot, so every writable window (slot, loader settings, device
    /// settings) is above it — a `BP=101` lock can never refuse a legitimate
    /// write, and an app can never reach a bootloader.
    /// The migration copies exactly when the new home is blank and the legacy one
    /// is not — never over a live record, never from a blank source.
    #[test]
    fn migration_copies_only_a_real_record_into_a_blank_home() {
        const BLANK: u8 = DELUGE_SETTINGS_BLANK;
        // Legacy record present, new home blank: the one case that copies.
        assert!(deluge_settings_need_migration(BLANK, 0xFE));
        assert!(deluge_settings_need_migration(BLANK, 0x00));
        // A record already at the new offset is live — never clobber it, even when
        // the legacy copy still exists (it always will; it cannot be erased).
        assert!(!deluge_settings_need_migration(0xFE, 0xFE));
        assert!(!deluge_settings_need_migration(0x00, 0xFE));
        // Nothing to bring forward.
        assert!(!deluge_settings_need_migration(BLANK, BLANK));
        assert!(!deluge_settings_need_migration(0xFE, BLANK));
    }

    /// The legacy record sits inside the protected region (so it is readable but
    /// unerasable, making the migration non-destructive) and the new one does not.
    #[test]
    fn migration_endpoints_straddle_the_protected_boundary() {
        assert!(DELUGE_SETTINGS_LEGACY_OFFSET < PROTECTED_LEN);
        assert!(DELUGE_SETTINGS_OFFSET >= PROTECTED_LEN);
        // The destination must be a writable window, or the copy would be refused.
        assert!(
            WRITABLE
                .iter()
                .any(|w| w.start <= DELUGE_SETTINGS_OFFSET
                    && DELUGE_SETTINGS_OFFSET + 256 <= w.end),
            "the new settings block must be writable"
        );
    }

    #[test]
    fn protected_region_covers_only_the_bootloaders() {
        assert_eq!(
            PROTECTED_LEN, SLOT_OFFSET,
            "BP=101 boundary must equal the app-slot base"
        );
        for w in MAP.writable {
            assert!(
                w.start >= PROTECTED_LEN,
                "writable window {w:?} must be above the protected region"
            );
        }
        assert!(
            DELUGE_SETTINGS_OFFSET >= PROTECTED_LEN,
            "device settings were relocated out of the protected region"
        );
        assert!(
            DELUGE_SETTINGS_LEGACY_OFFSET < PROTECTED_LEN,
            "the legacy device-settings offset was inside the protected region"
        );
        // The relocated device-settings block sits directly above the loader's and
        // stays inside the chip.
        assert_eq!(DELUGE_SETTINGS_SECTOR, SETTINGS_OFFSET + SECTOR_SIZE);
        assert!(DELUGE_SETTINGS_SECTOR + SECTOR_SIZE <= FLASH_SIZE);
    }
}
