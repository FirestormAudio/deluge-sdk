//! One-time forward migration of the **Deluge app's** device settings.
//!
//! The app's `FlashStorage` record was relocated out of the hardware-protected
//! bottom 1 MB — from [`flash::DELUGE_SETTINGS_LEGACY_OFFSET`] (`0x07F000`, which
//! sits between the FSB and the SSB and becomes read-only once protection is
//! armed) to [`flash::DELUGE_SETTINGS_OFFSET`] (`0x3D0000`, in the writable zone).
//! Block protection is contiguous-from-bottom, so no `BP` combination brackets
//! "FSB + SSB but not the settings between them"; the record had to move.
//!
//! **The relocation is unconditional, so this migration must be too.** The offset
//! is a device-wide layout fact, not a `flash-protect` build option: the app
//! (DelugeFirmware) is compiled separately from this loader and resolves
//! `DELUGE_SETTINGS_OFFSET` from the same `deluge-bsp` constant. Cargo features
//! unify per build graph, not across binaries, so gating the constant would let a
//! `flash-protect` SSB and a default-built app disagree about where settings live
//! — a silent split far worse than one copy-forward.
//!
//! ## The format contract is one byte
//!
//! The app writes a fixed 256-byte record at offset 0 of the region
//! (`FlashStorage::writeSettings` → `deluge_flash_program(0, buffer, 256)`) and
//! reads the same span back. It treats byte 0 (`Entries::FIRMWARE_TYPE`) equal to
//! `FirmwareVersion::Type::UNKNOWN` as "no settings saved" — and that value is
//! `255`, which is also the NOR-erased byte. So "blank sector" and "never saved"
//! are the same test, and the migration needs no other knowledge of the record's
//! interior: copy when the new location reads blank and the legacy one does not.
//!
//! Copying a *corrupt* legacy record is harmless. The app validates byte 0 itself
//! and factory-resets, exactly as it would have without the migration.
//!
//! ## It is non-destructive
//!
//! The legacy record is never erased — it is inside the protected region, so once
//! protection is armed it physically cannot be. A unit reverted to an older loader
//! or to stock firmware still finds its original settings at `0x07F000`; it simply
//! won't see changes made after the migration.

use deluge_bsp::flash;

/// Bytes the app's `FlashStorage` actually stores (`kFilenameBufferSize`). It
/// programs exactly this many at offset 0 and reads the same span back, so the
/// rest of the 64 KB sector is never written and does not need copying.
const APP_RECORD_LEN: usize = 256;

/// What [`migrate`] did, for the boot log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to do: the new location already holds a record, or the legacy one
    /// is blank (a unit that never ran the old layout).
    NotNeeded,
    /// The legacy record was copied forward and verified.
    Migrated,
    /// The erase/program was rejected, or the read-back did not match.
    Failed,
}

/// Read `out.len()` bytes at flash-relative `offset` through the **uncached**
/// memory-mapped mirror.
///
/// The cached window can return a stale line after a program (the RZ/A1L's caches
/// ignore maintenance ops), which would make the post-write verify lie — the same
/// reason [`crate::settings`] reads through the mirror. Reads are permitted
/// anywhere in the chip, protected region included, so the legacy source is
/// readable even with the lock-down armed.
fn read_at(offset: u32, out: &mut [u8]) {
    let base = rza1_hal::spibsc::SPI_FLASH_BASE + offset + rza1_hal::UNCACHED_MIRROR_OFFSET as u32;
    for (i, b) in out.iter_mut().enumerate() {
        // SAFETY: a volatile byte read from the memory-mapped flash mirror, within
        // the 4 MB chip window; reads never alter flash state.
        *b = unsafe { core::ptr::read_volatile((base + i as u32) as *const u8) };
    }
}

/// Copy the Deluge app's settings record forward if — and only if — the new
/// location is blank and the legacy one holds a record.
///
/// Idempotent: once the record is at the new offset the predicate short-circuits,
/// so this is a two-byte read on every subsequent boot.
///
/// # Safety
/// Switches the SPI-flash bus out of memory-mapped read mode for the erase and
/// program (no code may run from flash during the call — true in the SSB, which
/// runs from SRAM). Call from the boot path, like [`crate::settings::write`].
pub unsafe fn migrate() -> Outcome {
    // Peek the first byte of each location; the rule itself is
    // `flash::deluge_settings_need_migration` (host-tested).
    let mut head = [0u8; 1];
    let mut legacy_head = [0u8; 1];
    read_at(flash::DELUGE_SETTINGS_OFFSET, &mut head);
    read_at(flash::DELUGE_SETTINGS_LEGACY_OFFSET, &mut legacy_head);
    if !flash::deluge_settings_need_migration(head[0], legacy_head[0]) {
        return Outcome::NotNeeded;
    }

    let mut record = [0u8; APP_RECORD_LEN];
    read_at(flash::DELUGE_SETTINGS_LEGACY_OFFSET, &mut record);

    // SAFETY: `DELUGE_SETTINGS_OFFSET`'s 64 KB block is one of the board's
    // writable windows, so `MAP` accepts it; both calls run their manual-mode
    // window with interrupts masked and a bounded WIP-wait internally (required
    // here — taking the OLED SPI-DMA interrupt mid-erase freezes the machine).
    unsafe {
        flash::MAP.erase_range(flash::DELUGE_SETTINGS_OFFSET, APP_RECORD_LEN as u32);
        flash::MAP.program(flash::DELUGE_SETTINGS_OFFSET, &record);
    }

    // Verify through the mirror: a rejected erase/program reports nothing itself,
    // and silently losing a user's settings is the failure worth catching.
    let mut back = [0u8; APP_RECORD_LEN];
    read_at(flash::DELUGE_SETTINGS_OFFSET, &mut back);
    if back == record {
        Outcome::Migrated
    } else {
        Outcome::Failed
    }
}
