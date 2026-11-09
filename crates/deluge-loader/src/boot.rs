//! What the loader does to the flash's protection, and to its own slot, at boot and before it
//! launches an app.
//!
//! At boot ([`early_boot`]):
//! 1. **reload** the status registers from their non-volatile copy (`66h`/`99h`), always, so a
//!    volatile lift an earlier boot left open (it hung mid-confirm; a watchdog reset doesn't reset
//!    the flash) never outlives this one;
//! 2. **confirm** the running slot if stage-0 entered it pending — unless the protection is
//!    locked down (an app launched since the flash last lost power), when nothing is written and
//!    the flash must be powered off to finish ([`Unlock::NeedsPowerOff`]);
//! 3. **provision** the boot region's protection if it's off (with the protection policy on and
//!    a part `deluge_flash_protect` knows). After the confirm: a provision that fails latches a
//!    fault refusing every later write this boot.
//!
//! Before an app ([`before_launch`]): arm the lock-down, so no app can change the protection.
//!
//! An unknown part is never provisioned or locked; a pending slot on one is still confirmed (the
//! firmware programs the word with no lift to make), or it would be rolled back every boot.

pub use deluge_flash_protect::{Applied, ProtectionState, Refusal};
use deluge_image::slot::{Reason, decode_boot_info};

/// The flash, as the boot sequence uses it.
pub trait BootFlash {
    /// Reload the status registers from their non-volatile copy (`66h`/`99h`).
    fn reload(&mut self);
    /// The protection as the registers read, or `None` for a part `deluge_flash_protect` doesn't
    /// know.
    fn state(&mut self) -> Option<ProtectionState>;
    /// Protect the boot region, non-volatile, lock-down off.
    fn provision(&mut self) -> Result<Applied, Refusal>;
    /// Program the running slot's confirm word: reload, lift (volatile), program, restore,
    /// verify. On an unknown part, program it with no lift.
    fn confirm_own_slot(&mut self, slot: u8) -> Result<(), ConfirmError>;
    /// Arm the power-supply lock-down.
    fn lockdown(&mut self) -> Result<Applied, Refusal>;
}

/// Why a confirm didn't happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmError {
    /// The lift (or the restore) was refused.
    Refused(Refusal),
    /// The program failed.
    Program,
    /// The word didn't read back.
    Verify,
}

/// How the loader was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// By something other than stage-0 (on the Deluge, the stock first-stage bootloader): no
    /// slot to confirm.
    Stock,
    /// By stage-0, from `slot` (`0` = A), for `reason`.
    Slot { slot: u8, reason: Reason },
}

/// The form a boot-info word says, as read from where stage-0 leaves it (before it's cleared).
pub fn form(boot_info: u32) -> Form {
    match decode_boot_info(boot_info) {
        Some((slot, reason)) => Form::Slot { slot, reason },
        None => Form::Stock,
    }
}

/// The loader's protection policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootPolicy {
    /// Provision the boot region and lock it down before apps.
    pub protect: bool,
}

/// What the firmware must do before the boot can finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unlock {
    /// Nothing.
    None,
    /// The lock-down blocks a write the boot needs: power the flash off (and back on).
    NeedsPowerOff,
}

/// What [`early_boot`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EarlyBoot {
    /// The running slot is confirmed now (always `true` in the stock form).
    pub confirmed: bool,
    pub unlock: Unlock,
    /// The provisioning write, if one was made.
    pub provision: Option<Result<Applied, Refusal>>,
    /// The confirm, if one was attempted.
    pub confirm: Option<Result<(), ConfirmError>>,
}

/// The boot's protection and confirm sequence (the module docs). `own_confirmed`: the running
/// slot's confirm word, as its header reads.
pub fn early_boot<F: BootFlash>(
    f: &mut F,
    form: Form,
    own_confirmed: bool,
    policy: BootPolicy,
) -> EarlyBoot {
    f.reload();
    let state = f.state();
    let mut boot = EarlyBoot {
        confirmed: true,
        unlock: Unlock::None,
        provision: None,
        confirm: None,
    };
    // The confirm first: a provision that fails latches a fault that refuses every later write
    // this boot, and must not cost a good slot its confirm.
    if let Form::Slot { slot, .. } = form
        && !own_confirmed
    {
        if state.is_some_and(|s| s.locked) {
            boot.confirmed = false;
            boot.unlock = Unlock::NeedsPowerOff;
            return boot;
        }
        let r = f.confirm_own_slot(slot);
        boot.confirmed = r.is_ok();
        boot.confirm = Some(r);
    }
    boot.provision = match state {
        Some(s) if policy.protect && !s.provisioned && !s.locked => Some(f.provision()),
        _ => None,
    };
    boot
}

/// Right before an app: arm the lock-down (with the policy on and a known part).
pub fn before_launch<F: BootFlash>(f: &mut F, policy: BootPolicy) -> Result<(), Refusal> {
    if !policy.protect || f.state().is_none() {
        return Ok(());
    }
    f.lockdown().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deluge_image::slot::boot_info;
    use std::vec::Vec;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Call {
        Reload,
        State,
        Provision,
        Confirm(u8),
        Lockdown,
    }

    struct FakeFlash {
        state: Option<ProtectionState>,
        calls: Vec<Call>,
    }

    impl FakeFlash {
        fn new(state: Option<ProtectionState>) -> Self {
            Self {
                state,
                calls: Vec::new(),
            }
        }
        fn writes(&self) -> Vec<Call> {
            self.calls
                .iter()
                .copied()
                .filter(|c| !matches!(c, Call::Reload | Call::State))
                .collect()
        }
    }

    impl BootFlash for FakeFlash {
        fn reload(&mut self) {
            self.calls.push(Call::Reload);
        }
        fn state(&mut self) -> Option<ProtectionState> {
            self.calls.push(Call::State);
            self.state
        }
        fn provision(&mut self) -> Result<Applied, Refusal> {
            self.calls.push(Call::Provision);
            Ok(Applied::Written)
        }
        fn confirm_own_slot(&mut self, slot: u8) -> Result<(), ConfirmError> {
            self.calls.push(Call::Confirm(slot));
            Ok(())
        }
        fn lockdown(&mut self) -> Result<Applied, Refusal> {
            self.calls.push(Call::Lockdown);
            Ok(Applied::Written)
        }
    }

    const UNPROTECTED: Option<ProtectionState> = Some(ProtectionState {
        provisioned: false,
        locked: false,
    });
    const PROTECTED: Option<ProtectionState> = Some(ProtectionState {
        provisioned: true,
        locked: false,
    });
    const LOCKED: Option<ProtectionState> = Some(ProtectionState {
        provisioned: true,
        locked: true,
    });
    const ON: BootPolicy = BootPolicy { protect: true };
    const OFF: BootPolicy = BootPolicy { protect: false };
    const PENDING_B: Form = Form::Slot {
        slot: 1,
        reason: Reason::PendingTry,
    };

    #[test]
    fn form_reads_the_boot_info_word() {
        assert_eq!(form(0), Form::Stock);
        assert_eq!(form(0xDEAD_BEEF), Form::Stock);
        assert_eq!(form(boot_info(1, Reason::PendingTry as u8)), PENDING_B);
    }

    /// What the stock bootloader can leave at the boot-info address: nothing, noise, or (after
    /// an app was launched) the Deluge loader's trampoline, whose first word is `cpsid if`.
    #[test]
    fn form_is_stock_without_a_boot_info_word() {
        for w in [0, 0xFFFF_FFFF, 0xF10C_00C0, 0x1234_5678, 0x5330_0300] {
            assert_eq!(form(w), Form::Stock, "{w:#010x}");
        }
    }

    #[test]
    fn reload_comes_first() {
        let mut f = FakeFlash::new(PROTECTED);
        early_boot(&mut f, PENDING_B, false, ON);
        assert_eq!(f.calls.first(), Some(&Call::Reload));
    }

    #[test]
    fn provisions_unprotected_known_part() {
        let mut f = FakeFlash::new(UNPROTECTED);
        early_boot(&mut f, Form::Stock, true, ON);
        assert_eq!(f.writes(), [Call::Provision]);
    }

    #[test]
    fn unknown_part_writes_nothing() {
        let mut f = FakeFlash::new(None);
        let e = early_boot(&mut f, Form::Stock, true, ON);
        assert_eq!(e.unlock, Unlock::None);
        assert!(before_launch(&mut f, ON).is_ok());
        assert_eq!(f.writes(), []);
    }

    /// An unknown part has no protection to lift, but a pending slot must still confirm, or the
    /// watchdog rolls it back on every boot: the confirm programs it unlifted.
    #[test]
    fn unknown_part_still_confirms_a_pending_slot() {
        let mut f = FakeFlash::new(None);
        assert!(early_boot(&mut f, PENDING_B, false, ON).confirmed);
        assert_eq!(f.writes(), [Call::Confirm(1)]);
    }

    /// A provisioning that fails latches the protection core's fault, refusing every later write
    /// this boot: the pending slot's confirm goes first, so a failed provision can't roll a good
    /// slot back.
    #[test]
    fn confirm_comes_before_provision() {
        let mut f = FakeFlash::new(UNPROTECTED);
        early_boot(&mut f, PENDING_B, false, ON);
        assert_eq!(f.writes(), [Call::Confirm(1), Call::Provision]);
    }

    #[test]
    fn stock_form_never_confirms() {
        let mut f = FakeFlash::new(PROTECTED);
        early_boot(&mut f, Form::Stock, false, ON);
        assert_eq!(f.writes(), []);
    }

    #[test]
    fn pending_slot_confirms_after_reload() {
        let mut f = FakeFlash::new(PROTECTED);
        let e = early_boot(&mut f, PENDING_B, false, ON);
        assert!(e.confirmed);
        let reload = f.calls.iter().position(|&c| c == Call::Reload).unwrap();
        let confirm = f.calls.iter().position(|&c| c == Call::Confirm(1)).unwrap();
        assert!(reload < confirm);
    }

    #[test]
    fn confirmed_slot_is_left_alone() {
        let mut f = FakeFlash::new(PROTECTED);
        assert!(early_boot(&mut f, PENDING_B, true, ON).confirmed);
        assert_eq!(f.writes(), []);
    }

    #[test]
    fn locked_pending_boot_asks_to_unlock() {
        let mut f = FakeFlash::new(LOCKED);
        let e = early_boot(&mut f, PENDING_B, false, ON);
        assert_eq!((e.confirmed, e.unlock), (false, Unlock::NeedsPowerOff));
        assert_eq!(f.writes(), []);
    }

    #[test]
    fn protect_off_writes_nothing_but_reload() {
        let mut f = FakeFlash::new(UNPROTECTED);
        early_boot(&mut f, Form::Stock, true, OFF);
        assert!(before_launch(&mut f, OFF).is_ok());
        assert_eq!(f.writes(), []);
        assert_eq!(f.calls.first(), Some(&Call::Reload));
    }

    #[test]
    fn before_launch_locks_down() {
        let mut f = FakeFlash::new(PROTECTED);
        assert!(before_launch(&mut f, ON).is_ok());
        assert_eq!(f.writes(), [Call::Lockdown]);
    }
}
