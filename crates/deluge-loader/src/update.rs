//! The loader's self-update: a new loader slot image (from the card) written into the boot slot
//! that isn't running, with a sequence above both, left pending. Stage-0 then enters it; it
//! confirms itself, or the watchdog rolls back to the running one. The running slot is never
//! written, so a write that fails or tears leaves the unit booting what it booted before.

pub use deluge_flash_protect::Refusal;

use crate::boot::Form;
use deluge_image::slot::{
    self, HEADER_LEN, SlotError, SlotHeader, SlotLayout, UpdatePlan, plan_update,
};

/// Where a loader slot image's payload (the loader, under a `deluge_image::app` header) starts,
/// from the slot's start; the slot's shim, position-independent, executes in place before it, so
/// one image fits either slot.
pub const PAYLOAD: u32 = 0x1000;
/// The boot slots, as the update writes them.
pub trait SlotWriter {
    /// The slot this loader runs from (`0` = A).
    fn running_slot(&self) -> u8;
    /// Slot `n`'s check, as stage-0 would make it.
    fn read_slot(&mut self, n: u8) -> Result<SlotHeader, SlotError>;
    /// Whether the protection's lock-down is armed: no write can lift it until the flash loses
    /// power.
    fn locked(&mut self) -> bool;
    /// Write `image` (header page then body) into slot `n`: reload, lift (volatile), erase,
    /// program, verify, restore.
    fn write_slot(&mut self, n: u8, image: &[u8]) -> Result<(), WriteError>;
}

/// Why a slot write failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// The lift (or the restore) was refused.
    Refused(Refusal),
    Erase,
    Program,
    /// The slot didn't read back as written.
    Verify,
}

/// Why an update didn't happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateError {
    /// The image's variant code isn't one the loader accepts.
    Foreign,
    /// The image isn't a valid slot image.
    Invalid(SlotError),
    /// No sequence above the slots' exists.
    NoSequence,
    /// The lock-down is armed: nothing was written; the flash must lose power first.
    Locked,
    Write(WriteError),
}

/// Whether the menu offers `UPDATE LOADER`: only in the slot form (a loader the stock bootloader
/// started has no boot slots to update), and only for an image on the card that is a valid slot
/// image this loader accepts, and not one a slot already holds (`slots`: both slots' checks; the file stays on the card once it's
/// applied).
pub fn update_offered(
    form: Form,
    image: Option<&[u8]>,
    accepts: &[u16],
    layout: SlotLayout,
    slots: [Result<SlotHeader, SlotError>; 2],
) -> bool {
    if !matches!(form, Form::Slot { .. }) {
        return false;
    }
    image.is_some_and(|i| {
        let Ok(h) = slot::validate_with(i, layout, accepts) else {
            return false;
        };
        let held = slots
            .iter()
            .flatten()
            .any(|s| (s.body_len, s.body_crc) == (h.body_len, h.body_crc));
        !held
    })
}

/// Update into the other slot from `image`, a slot image (`mkslot wrap`) whose header is
/// rewritten with the planned sequence. `accepts`: the loader's variant codes.
pub fn self_update<W: SlotWriter>(
    w: &mut W,
    image: &mut [u8],
    accepts: &[u16],
    layout: SlotLayout,
) -> Result<UpdatePlan, UpdateError> {
    let h = slot::validate_with(image, layout, accepts).map_err(|e| match e {
        SlotError::BadVariant => UpdateError::Foreign,
        e => UpdateError::Invalid(e),
    })?;
    let running = w.running_slot();
    let (a, b) = (w.read_slot(0), w.read_slot(1));
    let plan = plan_update(running, a, b).ok_or(UpdateError::NoSequence)?;
    if w.locked() {
        return Err(UpdateError::Locked);
    }
    let len = HEADER_LEN + h.body_len as usize;
    let header = slot::encode_header_with(
        &image[HEADER_LEN..len],
        h.variant,
        plan.sequence,
        h.entry_offset,
        layout,
    )
    .map_err(UpdateError::Invalid)?;
    image[..HEADER_LEN].copy_from_slice(&header);
    w.write_slot(plan.slot, &image[..len])
        .map_err(UpdateError::Write)?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deluge_image::slot::{DELUGE, HEADER_LEN, encode_header_with, validate_with};
    use std::vec;
    use std::vec::Vec;

    const VARIANT: u16 = 0;
    /// Both slots empty.
    const NONE: [Result<SlotHeader, SlotError>; 2] = [Err(SlotError::BadMagic); 2];

    /// Two slots in memory; `fail_after` stops a write part-way, as a reset mid-program would.
    struct FakeSlots {
        running: u8,
        slots: [Vec<u8>; 2],
        fail_after: Option<usize>,
        writes: Vec<u8>,
        locked: bool,
    }

    fn slot_image(seq: u32, fill: u8) -> Vec<u8> {
        let body = vec![fill; 64];
        let mut v = encode_header_with(&body, VARIANT, seq, 0x100, DELUGE)
            .unwrap()
            .to_vec();
        v.extend_from_slice(&body);
        v
    }

    impl SlotWriter for FakeSlots {
        fn running_slot(&self) -> u8 {
            self.running
        }
        fn read_slot(&mut self, n: u8) -> Result<SlotHeader, SlotError> {
            validate_with(&self.slots[n as usize], DELUGE, &[VARIANT])
        }
        fn locked(&mut self) -> bool {
            self.locked
        }
        fn write_slot(&mut self, n: u8, image: &[u8]) -> Result<(), WriteError> {
            self.writes.push(n);
            let s = &mut self.slots[n as usize];
            s.clear();
            s.resize(image.len(), 0xFF);
            let upto = self.fail_after.unwrap_or(image.len()).min(image.len());
            s[..upto].copy_from_slice(&image[..upto]);
            if upto < image.len() {
                return Err(WriteError::Program);
            }
            Ok(())
        }
    }

    fn slots(running: u8) -> FakeSlots {
        FakeSlots {
            running,
            slots: [slot_image(3, 0xAA), slot_image(1, 0xBB)],
            fail_after: None,
            writes: Vec::new(),
            locked: false,
        }
    }

    /// A slot image whose body reaches past the payload offset, as a loader slot image does.
    fn loader_image(seq: u32) -> Vec<u8> {
        // Patterned, not erased-looking: a torn write must not leave a body that happens to match.
        let body: Vec<u8> = (0..(PAYLOAD as usize - HEADER_LEN) + 64)
            .map(|i| i as u8)
            .collect();
        let mut v = encode_header_with(&body, VARIANT, seq, 0x100, DELUGE)
            .unwrap()
            .to_vec();
        v.extend_from_slice(&body);
        v
    }

    /// The lock-down is armed (an app ran since the flash last lost power): nothing can be
    /// written, and the unlock route (a power cycle), not a write failure, is the answer.
    #[test]
    fn locked_update_asks_to_unlock() {
        let mut w = slots(0);
        w.locked = true;
        let mut img = loader_image(1);
        assert_eq!(
            self_update(&mut w, &mut img, &[VARIANT], DELUGE),
            Err(UpdateError::Locked)
        );
        assert!(w.writes.is_empty());
    }

    /// Once applied, the file stays on the card: the update isn't offered again for an image a
    /// slot already holds.
    #[test]
    fn update_not_offered_when_a_slot_already_holds_it() {
        let slot_form = Form::Slot {
            slot: 0,
            reason: deluge_image::slot::Reason::OnlyValid,
        };
        let img = loader_image(1);
        let held = validate_with(&img, DELUGE, &[VARIANT]);
        let other = validate_with(&slot_image(3, 0xAA), DELUGE, &[VARIANT]);
        assert!(update_offered(
            slot_form,
            Some(&img),
            &[VARIANT],
            DELUGE,
            [other, other]
        ));
        assert!(!update_offered(
            slot_form,
            Some(&img),
            &[VARIANT],
            DELUGE,
            [other, held]
        ));
        assert!(!update_offered(
            slot_form,
            Some(&img),
            &[VARIANT],
            DELUGE,
            [held, other]
        ));
    }

    #[test]
    fn slot_form_offers_update_only_with_a_valid_file() {
        let slot_form = Form::Slot {
            slot: 0,
            reason: deluge_image::slot::Reason::OnlyValid,
        };
        let good = loader_image(1);
        let mut bad = good.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(update_offered(
            slot_form,
            Some(&good),
            &[VARIANT],
            DELUGE,
            NONE
        ));
        assert!(!update_offered(
            slot_form,
            Some(&bad),
            &[VARIANT],
            DELUGE,
            NONE
        ));
        assert!(!update_offered(
            slot_form,
            Some(&good),
            &[0x0100],
            DELUGE,
            NONE
        ));
        assert!(!update_offered(slot_form, None, &[VARIANT], DELUGE, NONE));
        assert!(!update_offered(
            Form::Stock,
            Some(&good),
            &[VARIANT],
            DELUGE,
            NONE
        ));
    }

    #[test]
    fn update_writes_only_the_other_slot() {
        let mut w = slots(0);
        let mut img = loader_image(1);
        let plan = self_update(&mut w, &mut img, &[VARIANT], DELUGE).unwrap();
        assert_eq!(plan.slot, 1);
        assert_eq!(w.writes, [1]);
        assert_eq!(w.read_slot(0).unwrap().sequence, 3);
    }

    #[test]
    fn update_header_carries_planned_sequence() {
        let mut w = slots(0);
        let mut img = loader_image(1);
        self_update(&mut w, &mut img, &[VARIANT], DELUGE).unwrap();
        let h = w.read_slot(1).unwrap();
        assert_eq!((h.sequence, h.confirmed), (4, false));
        assert_eq!(&w.slots[1][HEADER_LEN..], &img[HEADER_LEN..]);
    }

    #[test]
    fn update_refuses_foreign_variant() {
        let mut w = slots(0);
        let body = vec![0xCC; 64];
        let mut img = encode_header_with(&body, 0x0100, 1, 0x100, DELUGE)
            .unwrap()
            .to_vec();
        img.extend_from_slice(&body);
        assert_eq!(
            self_update(&mut w, &mut img, &[VARIANT], DELUGE),
            Err(UpdateError::Foreign)
        );
        assert!(w.writes.is_empty());
    }

    #[test]
    fn update_refuses_an_invalid_image() {
        let mut w = slots(0);
        let mut img = slot_image(1, 0xCC);
        let last = img.len() - 1;
        img[last] ^= 1;
        assert_eq!(
            self_update(&mut w, &mut img, &[VARIANT], DELUGE),
            Err(UpdateError::Invalid(SlotError::BodyCrc))
        );
        assert!(w.writes.is_empty());
    }

    #[test]
    fn torn_update_leaves_running_slot() {
        let mut w = slots(1);
        w.fail_after = Some(HEADER_LEN + 10);
        let mut img = loader_image(1);
        assert_eq!(
            self_update(&mut w, &mut img, &[VARIANT], DELUGE),
            Err(UpdateError::Write(WriteError::Program))
        );
        assert_eq!(w.read_slot(1).unwrap().sequence, 1, "the running slot");
        assert!(w.read_slot(0).is_err(), "the torn target");
    }
}
