//! Per-part bottom-1 MB hardware write-protection for the Deluge's SPI-NOR flash.
//!
//! The Deluge flash is second-sourced across three pin-compatible 4 MB parts (see
//! [`super::decode_jedec`]). All three use the JEDEC two-register protection
//! model — the block-protect field in **SR1**, and `CMP` + the SRP/SRL
//! Power-Supply-Lock-Down bit in **SR2** — and differ only by a couple of bits
//! and the status-register *write method*. So they collapse to one small data
//! table ([`recipe`]); there is deliberately no module-per-chip. The generic SR
//! transfers live in [`rza1_hal::spibsc`]; which bytes to write, and the checks
//! around writing them, are the chip-neutral `deluge-flash-protect` crate's, which
//! this module re-exports with the Deluge's [`FlashId`] and 4 MB geometry.
//!
//! Protecting the bottom 1 MB (`0x000000..0x100000` = FSB + SSB, see
//! [`super::PROTECTED_LEN`]) makes the bootloaders unerasable by a running app.
//! The lock is armed **non-volatile** (the `06h`→`01h`/`31h` path) so an
//! app-issued software reset (`66h`/`99h`) reloads it unchanged and can't clear
//! it — only a real power cycle can. The exception is the S25FL132K, whose
//! lock-down note also lists software reset; it is armed anyway as
//! defense-in-depth.
//!
//! # Nothing permanent
//!
//! Some status-register bits can never be cleared once set: the `LB` security
//! register locks (SR2 bits 5:2) on all three parts, and `SRP1:SRP0 = (1,1)`, a
//! permanent status-register lock on the S25FL132K and undefined on the AT25
//! (rev I has no `(1,1)` row). So every write is composed from scratch
//! by [`plan`] rather than read-modified-written: SR1 carries only region bits,
//! SR2 only `QE` (the one bit carried over) and the lock. `SRP0`, `CMP`, `LB` and
//! the read-only bits always go out as `0`, which no part can turn into a
//! permanent state. [`apply`] adds the checks around it: the part is re-identified,
//! the registers must read the same twice, and a write that doesn't read back as
//! planned stops every further write until the next boot.
//!
//! The arming ops ([`provision`]/[`lockdown`]/[`unprotect`]) are gated behind the
//! `flash-protect` feature so a default build never writes the status register
//! until the recipe is hardware-validated; everything else stays compiled for the
//! FLASH INFO display, `flash-sr-probe` and the host tests.

use core::ops::Range;
use core::sync::atomic::AtomicBool;

pub use deluge_flash_protect::{
    Applied, Goal, ProtectionRecipe, ProtectionState, Refusal, SrBus, SrWrite, TEST_MARK_LEN,
    TOP_64K, TestBlock, WriteMethod, decode_state, plan, protection_bits, test_block_state,
};
use rza1_hal::spibsc;

use super::FlashId;

/// The bottom-1 MB recipe for `id`, or `None` for a part we don't recognize — in
/// which case nothing is ever written. Keyed on the raw JEDEC ID (the
/// authoritative protection mapping; [`super::decode_jedec`] is the parallel
/// display/capacity mapping).
pub const fn recipe(id: FlashId) -> Option<ProtectionRecipe> {
    deluge_flash_protect::recipe(id.raw)
}

/// The range an SR1 region value protects (with `CMP = 0`) on the Deluge's 4 MB parts
/// (see `deluge_flash_protect::protected_range_for`).
pub const fn protected_range(sr1: u8) -> Option<Range<u32>> {
    deluge_flash_protect::protected_range_for(sr1, super::FLASH_SIZE)
}

/// Bring the registers of `id`'s part to `goal` (see `deluge_flash_protect::apply`).
pub fn apply<B: SrBus>(
    bus: &mut B,
    faulted: &AtomicBool,
    id: FlashId,
    goal: Goal,
) -> Result<Applied, Refusal> {
    deluge_flash_protect::apply(bus, faulted, id.raw, goal)
}

/// The HAL's name for a write method.
pub const fn hal_method(m: WriteMethod) -> spibsc::WriteMethod {
    match m {
        WriteMethod::Combined => spibsc::WriteMethod::Combined,
        WriteMethod::Separate => spibsc::WriteMethod::Separate,
    }
}

/// The SPIBSC controller as an [`SrBus`].
pub struct Spibsc(());

impl Spibsc {
    /// # Safety
    /// Every write switches the flash bus out of memory-mapped read mode, so no
    /// code may run from flash while the bus is in use (true in the SSB and the
    /// J-Link-loaded tools, which run from SRAM).
    pub unsafe fn new() -> Self {
        Spibsc(())
    }
}

impl SrBus for Spibsc {
    fn read_id(&mut self) -> [u8; 3] {
        spibsc::read_id()
    }
    fn read_regs(&mut self) -> (u8, u8) {
        spibsc::protection_regs()
    }
    fn write(&mut self, method: WriteMethod, sr1: Option<u8>, sr2: Option<u8>) -> bool {
        // SAFETY: `Spibsc::new`'s contract — nothing is running from flash.
        unsafe { spibsc::write_protection(hal_method(method), sr1, sr2) }
    }
    fn write_volatile(&mut self, method: WriteMethod, sr1: Option<u8>, sr2: Option<u8>) -> bool {
        // SAFETY: as `write`.
        unsafe { spibsc::write_protection_volatile(hal_method(method), sr1, sr2) }
    }
    fn software_reset(&mut self) {
        // SAFETY: as `write`. Memory-mapped reads use single-wire Fast Read, which
        // a reset leaves working.
        unsafe { spibsc::software_reset() }
    }
}

/// Latched by the first status-register write this boot that doesn't verify.
pub static FAULTED: AtomicBool = AtomicBool::new(false);

/// Read and decode the current protection state for `id`'s recipe, or `None` if
/// the part isn't recognized. Read-only; always compiled (used by FLASH INFO).
pub fn protection(id: FlashId) -> Option<ProtectionState> {
    let r = recipe(id)?;
    let (sr1, sr2) = spibsc::protection_regs();
    Some(decode_state(r, sr1, sr2))
}

/// Provision bottom-1 MB block protection (non-volatile, reversible), lock-down
/// off. The region bits persist across power cycles; only [`lockdown`] needs
/// re-arming each boot.
///
/// # Safety
/// Switches the flash bus out of memory-mapped read mode (no code may run from
/// flash during the call — true in the SSB, which runs from SRAM) and writes
/// non-volatile protection bits.
#[cfg(feature = "flash-protect")]
pub unsafe fn provision(id: FlashId) -> Result<Applied, Refusal> {
    let r = recipe(id).ok_or(Refusal::UnknownPart)?;
    // SAFETY: this function's own contract — nothing runs from flash.
    let mut bus = unsafe { Spibsc::new() };
    apply(&mut bus, &FAULTED, id, Goal::provision(r))
}

/// Provision the region and arm the non-volatile Power-Supply-Lock-Down
/// (`SRP1`/`SRL` = 1), so a running app cannot modify protection until the next
/// power cycle. Call right before handing off to an app: the loader itself runs
/// with the status register writable, so a J-Link can always clear `BP`.
///
/// # Safety
/// As [`provision`].
#[cfg(feature = "flash-protect")]
pub unsafe fn lockdown(id: FlashId) -> Result<Applied, Refusal> {
    let r = recipe(id).ok_or(Refusal::UnknownPart)?;
    // SAFETY: this function's own contract — nothing runs from flash.
    let mut bus = unsafe { Spibsc::new() };
    apply(&mut bus, &FAULTED, id, Goal::lockdown(r))
}

/// Clear block protection and the lock-down to arm a deliberate SSB update or
/// recover a unit. Only effective while the lock-down isn't in force (fresh after
/// a power cycle, before [`lockdown`] runs); [`Refusal::Locked`] otherwise.
///
/// # Safety
/// Removes write protection from the bootloader region — the caller must not run
/// untrusted code afterwards. Switches bus mode and writes the status register.
#[cfg(feature = "flash-protect")]
pub unsafe fn unprotect(id: FlashId) -> Result<Applied, Refusal> {
    // SAFETY: this function's own contract — nothing runs from flash.
    let mut bus = unsafe { Spibsc::new() };
    apply(&mut bus, &FAULTED, id, Goal::unprotect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: [u8; 3]) -> FlashId {
        FlashId {
            raw,
            model: "x",
            capacity: 0,
        }
    }

    /// The Deluge's parts keep their recipes through the FlashId wrapper, and each protects
    /// exactly the bootloaders' bottom 1 MB.
    #[test]
    fn the_deluge_recipes_protect_the_bootloaders() {
        for raw in [[0x1F, 0x87, 0x01], [0xEF, 0x40, 0x16], [0x01, 0x40, 0x16]] {
            let r = recipe(id(raw)).unwrap();
            assert_eq!(
                protected_range(Goal::provision(r).target(0).0),
                Some(0..super::super::PROTECTED_LEN)
            );
        }
        assert_eq!(recipe(id([0x01, 0x02, 0x15])), None);
        assert_eq!(protected_range(TOP_64K), Some(0x3F_0000..0x40_0000));
    }

    #[test]
    fn hal_methods_map_one_to_one() {
        assert_eq!(
            hal_method(WriteMethod::Combined),
            spibsc::WriteMethod::Combined
        );
        assert_eq!(
            hal_method(WriteMethod::Separate),
            spibsc::WriteMethod::Separate
        );
    }
}
