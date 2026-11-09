//! The loader's own boot slots on the Deluge: confirming the running one and writing a new
//! loader into the other, both inside the protected bottom 1 MB, through a volatile lift of the
//! protection (`deluge_bsp::flash::protect`), which a status-register reload (`66h`/`99h`) always
//! precedes. The protection, the provisioning and the lock-down are `deluge_loader::boot`'s
//! decisions; these are the hands.
//!
//! Every write is verified by reading it back through the uncached mirror of the flash window:
//! the raw programming path (`rza1_hal::spibsc::force_*`) reports nothing itself, and the cached
//! window could still hold the old bytes.

use deluge_bsp::flash::{self, FlashId, protect};
use deluge_image::slot::{self, CONFIRMED, DELUGE, HEADER_LEN, SlotError, SlotHeader};
use deluge_loader::boot::{Applied, BootFlash, ConfirmError, ProtectionState, Refusal};
use deluge_loader::update::{SlotWriter, WriteError};
use rza1_hal::spibsc::{self, SPI_FLASH_BASE};

/// The confirm word's offset in a slot's header page (`deluge_image::slot`'s table).
const CONFIRM_WORD: u32 = 0xFC;

/// `len` bytes of the flash at `offset`, through the uncached mirror.
fn uncached(offset: u32, len: usize) -> &'static [u8] {
    let at = SPI_FLASH_BASE as usize + rza1_hal::UNCACHED_MIRROR_OFFSET + offset as usize;
    // SAFETY: inside the 4 MB window, read only while the controller is in read mode (every
    // `spibsc` call returns to it), and the loader is the flash's only writer.
    unsafe { core::slice::from_raw_parts(at as *const u8, len) }
}

/// Slot `n`'s check, read as stage-0 reads it.
pub fn read_slot(n: u8) -> Result<SlotHeader, SlotError> {
    slot::validate(uncached(
        DELUGE.slot_offsets[n as usize],
        DELUGE.slot_len as usize,
    ))
}

/// Whether slot `n` is pending: its header page parses and its confirm word isn't [`CONFIRMED`].
/// A page that doesn't parse (not a slot at all) is never taken for a pending one.
pub fn pending(n: u8) -> bool {
    slot::parse(uncached(DELUGE.slot_offsets[n as usize], HEADER_LEN)).is_ok_and(|h| !h.confirmed)
}

/// Restart the watchdog's count, if it's running (writing the counter of a stopped one is
/// harmless).
fn kick() {
    // SAFETY: the watchdog is the loader's alone.
    rza1_hal::wdt::Watchdog::new(unsafe { rza1_hal::peripherals::WDT::steal() }).kick()
}

/// Open the boot region for one write: `Some` lift to restore, `None` for a part with no recipe
/// (no protection to lift; the write goes ahead unlifted).
fn lift(id: FlashId) -> Result<Option<protect::Lifted>, Refusal> {
    // SAFETY: the loader runs from SRAM; nothing executes from the flash window.
    match unsafe { protect::lift_volatile(id) } {
        Ok(l) => Ok(Some(l)),
        Err(Refusal::UnknownPart) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Close what [`lift`] opened.
fn restore(lifted: Option<protect::Lifted>) -> Result<(), Refusal> {
    match lifted {
        // SAFETY: as `lift`.
        Some(l) => unsafe { protect::restore_volatile(l) },
        None => Ok(()),
    }
}

/// The Deluge's flash, as the boot sequence uses it.
pub struct DelugeFlash {
    id: FlashId,
}

impl DelugeFlash {
    pub fn new() -> Self {
        Self {
            id: flash::identify(),
        }
    }
}

impl BootFlash for DelugeFlash {
    fn reload(&mut self) {
        // SAFETY: as `lift`.
        unsafe { protect::reload() }
    }

    fn state(&mut self) -> Option<ProtectionState> {
        protect::protection(self.id)
    }

    fn provision(&mut self) -> Result<Applied, Refusal> {
        #[cfg(feature = "flash-protect")]
        // SAFETY: as `lift`; writes the non-volatile region bits only, as planned.
        return unsafe { protect::provision(self.id) };
        #[cfg(not(feature = "flash-protect"))]
        Err(Refusal::UnknownPart)
    }

    fn confirm_own_slot(&mut self, n: u8) -> Result<(), ConfirmError> {
        let at = DELUGE.slot_offsets[n as usize] + CONFIRM_WORD;
        // The lift reloads the registers (a software reset and its wait): kick on both sides.
        kick();
        let lifted = lift(self.id).map_err(ConfirmError::Refused)?;
        kick();
        // SAFETY: as `lift`; the confirm word is erased (pending, all ones) and outside both CRCs.
        unsafe { spibsc::force_program(at, &CONFIRMED.to_le_bytes(), flash::PAGE) };
        restore(lifted).map_err(ConfirmError::Refused)?;
        if uncached(at, 4) != CONFIRMED.to_le_bytes() {
            return Err(ConfirmError::Verify);
        }
        Ok(())
    }

    fn lockdown(&mut self) -> Result<Applied, Refusal> {
        #[cfg(feature = "flash-protect")]
        // SAFETY: as `lift`; arms the power-supply lock-down, released by a power cycle.
        return unsafe { protect::lockdown(self.id) };
        #[cfg(not(feature = "flash-protect"))]
        Err(Refusal::UnknownPart)
    }
}

/// The Deluge's boot slots, as a self-update writes them.
pub struct DelugeSlots {
    running: u8,
    id: FlashId,
}

impl DelugeSlots {
    pub fn new(running: u8) -> Self {
        Self {
            running,
            id: flash::identify(),
        }
    }
}

impl SlotWriter for DelugeSlots {
    fn running_slot(&self) -> u8 {
        self.running
    }

    fn locked(&mut self) -> bool {
        protect::protection(self.id).is_some_and(|s| s.locked)
    }

    fn read_slot(&mut self, n: u8) -> Result<SlotHeader, SlotError> {
        read_slot(n)
    }

    fn write_slot(&mut self, n: u8, image: &[u8]) -> Result<(), WriteError> {
        assert_ne!(
            n, self.running,
            "a self-update never writes the running slot"
        );
        let at = DELUGE.slot_offsets[n as usize];
        let lifted = lift(self.id).map_err(WriteError::Refused)?;
        // SAFETY: as `lift`; the range is the other slot, wholly inside the boot region.
        unsafe {
            spibsc::force_erase_range(at, DELUGE.slot_len, flash::SECTOR_SIZE);
            spibsc::force_program(at, image, flash::PAGE);
        }
        restore(lifted).map_err(WriteError::Refused)?;
        if uncached(at, image.len()) != image {
            return Err(WriteError::Verify);
        }
        Ok(())
    }
}
