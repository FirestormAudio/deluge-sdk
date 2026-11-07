//! Status-register block protection for JEDEC SPI-NOR flash: which bytes to write to protect a
//! region, and the checks around writing them.
//!
//! The supported parts all use the JEDEC two-register protection model — the block-protect field
//! in **SR1**, and `CMP` + the SRP/SRL Power-Supply-Lock-Down bit in **SR2** — and differ only by
//! their region bits and the status-register *write method*. So they collapse to one small data
//! table ([`recipe`]); there is deliberately no module-per-chip. The bus transfers are the
//! caller's ([`SrBus`]); this crate decides *which bytes* to write and decodes the read-back. It
//! has no hardware dependency, so the host tests exercise all of it.
//!
//! Each recipe protects the bottom 1 MB (a board's boot region). The protection is armed
//! **non-volatile** (the `06h`→`01h`/`31h` path) so an app-issued software reset (`66h`/`99h`)
//! reloads it unchanged and can't clear it — only a real power cycle can.
//!
//! # Nothing permanent
//!
//! Some status-register bits can never be cleared once set: the `LB` security
//! register locks (`LB`: SR2 bits 5:2, or 5:3 on the AT25QF641B, where bit 2 is `P_SUS`), and
//! `SRP1:SRP0 = (1,1)`, a
//! permanent status-register lock on the S25FL132K and undefined on the AT25
//! parts. So every write is composed from scratch
//! by [`plan`] rather than read-modified-written: SR1 carries only region bits,
//! SR2 only `QE` (the one bit carried over) and the lock. `SRP0`, `CMP`, `LB` and
//! the read-only bits always go out as `0`, which no part can turn into a
//! permanent state. [`apply`] adds the checks around it: the part is re-identified,
//! the registers must read the same twice, and a write that doesn't read back as
//! planned stops every further write until the next boot.

#![cfg_attr(not(test), no_std)]

use core::sync::atomic::{AtomicBool, Ordering};

/// How a part accepts status-register writes. All supported parts hold the block-protect field in
/// SR1 and the SRP/SRL lock-down bit in SR2, but differ in how those two bytes are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMethod {
    /// A single `01h` writes SR1 then SR2 (two bytes). Required where a one-byte
    /// `01h` would clear CMP/QE (e.g. S25FL132K).
    Combined,
    /// `01h` writes SR1; SR2 is written separately via `31h` (e.g. AT25SF321B,
    /// W25Q32JV, AT25QF641B).
    Separate,
}

// Status-register bit positions — identical across all the supported parts.

/// SR1 block-protect field: AT25 `BP4:BP0`, W25/S25FL132K `SEC|TB|BP2:BP0` — both
/// occupy bits 6:2. (SR1 bit 7 is `SRP0`; bits 1:0 are read-only `WEL`/`BUSY`.)
const SR1_REGION_MASK: u8 = 0x7C;
/// SR1 bit 5: `TB` (W25 / S25FL132K), `BP3` (AT25) — protect from the bottom.
const SR1_TB: u8 = 1 << 5;
/// SR1 bit 4: `BP2`.
const SR1_BP2: u8 = 1 << 4;
/// SR1 bit 2: `BP0`.
const SR1_BP0: u8 = 1 << 2;
/// SR1 region value protecting `0x000000..0x100000` (a board's boot region) on the 4 MB parts:
/// from the bottom, `BP2:BP0 = 101` = lower 1/4 (AT25SF321B `BP4:BP0 = 01101`, Table 6; W25 /
/// S25FL132K `SEC=0, TB=1, BP=101`). (The 8 MB AT25QF641B's is `TB|BP2`, lower 1/8.) Checked
/// against the datasheet tables by [`protected_range_for`]'s tests.
const BOTTOM_1MB: u8 = SR1_TB | SR1_BP2 | SR1_BP0;
/// SR1 `SRP0`, bit 7. Never written `1`: with the lock bit it would form the
/// permanent `(1,1)` state.
const SR1_SRP0: u8 = 1 << 7;
/// SR1 `BUSY`/`WIP`, bit 0.
const SR1_BUSY: u8 = 1 << 0;
/// SR2 `QE` (quad enable), bit 1 — the one bit a write carries over from the read.
const SR2_QE: u8 = 1 << 1;
/// SR2 `CMP` (complement-protect), bit 6 — always written 0, so the region field
/// protects the region as written rather than its complement.
const SR2_CMP: u8 = 1 << 6;
/// SR2 lock-down bit — `SRP1` (AT25 / S25FL132K) / `SRL` (W25), bit 0. Set
/// non-volatile to enter Power-Supply-Lock-Down (status register read-only until
/// the next power cycle).
const SR2_LOCK: u8 = 1 << 0;
/// The SR2 bits a write controls. The rest — `LB` (OTP), suspend and reserved —
/// are never written `1` and are ignored when verifying.
const SR2_OWNED: u8 = SR2_QE | SR2_CMP | SR2_LOCK;

/// SR1 region value protecting only the top 1/64 of a part: the top 64 KB
/// (`0x3F0000..0x400000`) of the 4 MB parts (AT25SF321B `BP0` = upper 1/64, Table 6; W25 /
/// S25FL132K `TB=0, BP2:BP0=001`), the top 128 KB (`0x7E0000..0x800000`) of the 8 MB
/// AT25QF641B. `flash-sr-probe` proves the write path and the lock-down there before anything
/// touches the boot region.
pub const TOP_64K: u8 = SR1_BP0;

/// The range an SR1 region value protects (with `CMP = 0`) on a part of `capacity` bytes, by the
/// rule the supported parts' tables share: `BP2:BP0 = 0` protects nothing, `111` everything,
/// otherwise `capacity / 64 << (BP - 1)` from the bottom when bit 5 (`TB`; the AT25SF321B's
/// `BP3`) is set, else from the top. `None` when nothing is protected — and for bit 6 set (`SEC`;
/// the AT25SF321B's `BP4`: 4 KB-sector granularity), which no recipe uses and this does not
/// decode.
pub const fn protected_range_for(sr1: u8, capacity: u32) -> Option<core::ops::Range<u32>> {
    if sr1 & (1 << 6) != 0 {
        return None;
    }
    let bp = (sr1 >> 2) & 0b111;
    match bp {
        0 => None,
        0b111 => Some(0..capacity),
        _ => {
            let len = (capacity >> 6) << (bp - 1);
            if sr1 & (1 << 5) != 0 {
                Some(0..len)
            } else {
                Some(capacity - len..capacity)
            }
        }
    }
}

/// How many zero bytes `flash-sr-probe`'s rejection test programs at the start of
/// a test block.
pub const TEST_MARK_LEN: usize = 16;

/// What a bench test block (one 64 KB erase block) holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestBlock {
    /// All `FF`.
    Blank,
    /// The probe's mark — [`TEST_MARK_LEN`] zero bytes — and otherwise blank: the
    /// only contents the probe may erase.
    ProbeMark,
    /// Anything else: not the probe's to touch.
    Other,
}

/// Classify a test block's contents (see [`TestBlock`]).
pub fn test_block_state(block: &[u8]) -> TestBlock {
    let rest_blank = block
        .get(TEST_MARK_LEN..)
        .is_some_and(|r| r.iter().all(|&b| b == 0xFF));
    let head = &block[..TEST_MARK_LEN.min(block.len())];
    if rest_blank && head.iter().all(|&b| b == 0xFF) {
        TestBlock::Blank
    } else if rest_blank && head.iter().all(|&b| b == 0) {
        TestBlock::ProbeMark
    } else {
        TestBlock::Other
    }
}

/// The bottom-1 MB protection recipe for one part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtectionRecipe {
    /// SR1 value (region bits 6:2) protecting `0x0..0x100000`.
    sr1_region: u8,
    /// How this part accepts status-register writes.
    method: WriteMethod,
    /// SR2 bits outside [`SR2_OWNED`] that the part ships with set — the
    /// S25FL132K's `LB0`. Any other such bit stops arming.
    factory_sr2: u8,
}

impl ProtectionRecipe {
    /// How this part accepts status-register writes.
    pub const fn method(self) -> WriteMethod {
        self.method
    }
}

/// The bottom-1 MB recipe for `id`, or `None` for a part we don't recognize — in
/// which case nothing is ever written. Keyed on the raw JEDEC ID (the
/// authoritative protection mapping).
pub const fn recipe(id: [u8; 3]) -> Option<ProtectionRecipe> {
    match id {
        // AT25SF321B: CMP=0, BP4:BP0=01101. Separate writes (SR1 via 01h, SR2 via
        // 31h); a combined 01h isn't its model.
        [0x1F, 0x87, 0x01] => Some(ProtectionRecipe {
            sr1_region: BOTTOM_1MB,
            method: WriteMethod::Separate,
            factory_sr2: 0,
        }),
        // W25Q32JV: CMP=0, SEC=0, TB=1, BP2:BP0=101. Separate writes.
        [0xEF, 0x40, 0x16] => Some(ProtectionRecipe {
            sr1_region: BOTTOM_1MB,
            method: WriteMethod::Separate,
            factory_sr2: 0,
        }),
        // S25FL132K: the W25's region bits, but a one-byte 01h clears CMP/QE, so
        // SR1+SR2 must go in one combined 01h. LB0 is factory-set (SFDP).
        [0x01, 0x40, 0x16] => Some(ProtectionRecipe {
            sr1_region: BOTTOM_1MB,
            method: WriteMethod::Combined,
            factory_sr2: 1 << 2,
        }),
        // AT25QF641B (8 MB): CMP=0, SEC=0, TB=1, BP2:BP0=100 ("lower 1/8", datasheet Table 6).
        // Separate writes (01h SR1, 31h SR2). Keyed on the datasheet's 9Fh bytes (Table 16).
        [0x1F, 0x88, 0x01] => Some(ProtectionRecipe {
            sr1_region: SR1_TB | SR1_BP2,
            method: WriteMethod::Separate,
            factory_sr2: 0,
        }),
        _ => None,
    }
}

/// Decoded protection state: what a boot path needs to decide whether to provision or lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtectionState {
    /// The recipe's region is block-protected (SR1 region field matches it).
    pub provisioned: bool,
    /// Power-Supply-Lock-Down is armed (status register read-only until a power cycle).
    pub locked: bool,
}

/// Decode `(sr1, sr2)` as read against recipe `r`.
pub const fn decode_state(r: ProtectionRecipe, sr1: u8, sr2: u8) -> ProtectionState {
    ProtectionState {
        provisioned: (sr1 & SR1_REGION_MASK) == (r.sr1_region & SR1_REGION_MASK),
        locked: (sr2 & SR2_LOCK) != 0,
    }
}

/// The protection state a write aims for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Goal {
    /// SR1 region bits (6:2).
    region: u8,
    /// Arm the Power-Supply-Lock-Down.
    lock: bool,
    /// Refuse when the registers hold a bit we don't expect. Arming is strict;
    /// [`Goal::unprotect`], the recovery path, only ever writes zeros and so may
    /// clear whatever it finds.
    strict: bool,
}

impl Goal {
    /// The part's bottom-1 MB region, lock-down off.
    pub const fn provision(r: ProtectionRecipe) -> Self {
        Goal {
            region: r.sr1_region,
            lock: false,
            strict: true,
        }
    }

    /// The part's bottom-1 MB region with the lock-down armed.
    pub const fn lockdown(r: ProtectionRecipe) -> Self {
        Goal {
            region: r.sr1_region,
            lock: true,
            strict: true,
        }
    }

    /// No region, no lock-down.
    pub const fn unprotect() -> Self {
        Goal {
            region: 0,
            lock: false,
            strict: false,
        }
    }

    /// The top 1/64 of the part ([`TOP_64K`]), optionally locked: `flash-sr-probe`'s harmless
    /// stand-in for the bootloader region.
    pub const fn top_test(lock: bool) -> Self {
        Goal {
            region: TOP_64K,
            lock,
            strict: true,
        }
    }

    /// The `(SR1, SR2)` this goal leaves, given the SR2 read before writing (for
    /// `QE`) — also the exact bytes a write for it sends. Only region bits, `QE`
    /// and the lock can be `1`.
    pub const fn target(self, sr2_read: u8) -> (u8, u8) {
        let lock = if self.lock { SR2_LOCK } else { 0 };
        (self.region & SR1_REGION_MASK, (sr2_read & SR2_QE) | lock)
    }
}

/// The protection bits of a register read — the region and `SRP0` in SR1, the
/// bits a write controls in SR2 — with the status bits (`WEL`, `BUSY`, suspend)
/// and the OTP `LB` bits masked out. Compare these, not raw reads: a write the
/// locked chip ignores leaves `WEL` set.
pub const fn protection_bits(sr1: u8, sr2: u8) -> (u8, u8) {
    (sr1 & (SR1_REGION_MASK | SR1_SRP0), sr2 & SR2_OWNED)
}

/// The bytes to send: `None` leaves that register unwritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SrWrite {
    pub sr1: Option<u8>,
    pub sr2: Option<u8>,
}

/// Why nothing was written (or, for [`Refusal::VerifyFailed`], why nothing more
/// will be this boot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No recipe for this part.
    UnknownPart,
    /// The JEDEC ID read now differs from the one the caller identified.
    PartChanged,
    /// Two reads of the status registers disagree.
    UnstableRead,
    /// An erase, program or status-register write is still running.
    Busy,
    /// A bit we don't expect is set (`SRP0`, an `LB` beyond the factory one, a
    /// suspend flag). Arming stops rather than guess.
    UnexpectedBits { sr1: u8, sr2: u8 },
    /// The lock-down is in force; only a power cycle clears it.
    Locked,
    /// An earlier write this boot didn't verify; no more are attempted.
    Faulted,
    /// The volatile rehearsal of a combined write read back wrong (`sr1`/`sr2` as
    /// read), so the bytes did not arrive as sent. The registers were reset and
    /// nothing non-volatile was written. Latches [`Refusal::Faulted`].
    PreflightFailed { sr1: u8, sr2: u8 },
    /// The write didn't read back as planned. Latches [`Refusal::Faulted`].
    VerifyFailed { sr1: u8, sr2: u8 },
}

/// What [`apply`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The registers already held the goal.
    AlreadySet,
    /// The write was sent and read back as planned.
    Written,
}

/// Decide what to write to reach `goal` from the registers as read (`sr1`,
/// `sr2`), or why not to. Pure; the composition rules are in the module docs.
pub const fn plan(
    r: ProtectionRecipe,
    goal: Goal,
    sr1: u8,
    sr2: u8,
) -> Result<Option<SrWrite>, Refusal> {
    if sr1 & SR1_BUSY != 0 {
        return Err(Refusal::Busy);
    }
    if goal.strict {
        let odd_sr2 = sr2 & !SR2_OWNED & !r.factory_sr2;
        if sr1 & SR1_SRP0 != 0 || odd_sr2 != 0 {
            return Err(Refusal::UnexpectedBits { sr1, sr2 });
        }
    }
    let (want1, want2) = goal.target(sr2);
    let (have1, have2) = protection_bits(sr1, sr2);
    if have1 == want1 && have2 == want2 {
        return Ok(None);
    }
    if sr2 & SR2_LOCK != 0 {
        return Err(Refusal::Locked);
    }
    let w = match r.method {
        WriteMethod::Combined => SrWrite {
            sr1: Some(want1),
            sr2: Some(want2),
        },
        WriteMethod::Separate => SrWrite {
            sr1: if have1 == want1 { None } else { Some(want1) },
            sr2: if have2 == want2 { None } else { Some(want2) },
        },
    };
    Ok(Some(w))
}

/// The status-register I/O [`apply`] needs: the real controller on the device,
/// a model of the chip in the host tests.
pub trait SrBus {
    /// JEDEC `RDID`.
    fn read_id(&mut self) -> [u8; 3];
    /// `(SR1, SR2)`.
    fn read_regs(&mut self) -> (u8, u8);
    /// Send a non-volatile status-register write. `false` if the transfer failed.
    fn write(&mut self, method: WriteMethod, sr1: Option<u8>, sr2: Option<u8>) -> bool;
    /// Send the same write to the volatile copy (`50h`), lost at the next power
    /// cycle or software reset. `false` if the transfer failed.
    fn write_volatile(&mut self, method: WriteMethod, sr1: Option<u8>, sr2: Option<u8>) -> bool;
    /// Software reset (`66h 99h`): reload the volatile copy from the non-volatile bits.
    fn software_reset(&mut self);
}

/// Read both registers twice; they must agree.
fn read_stable<B: SrBus>(bus: &mut B) -> Result<(u8, u8), Refusal> {
    let a = bus.read_regs();
    if bus.read_regs() != a {
        return Err(Refusal::UnstableRead);
    }
    Ok(a)
}

/// Bring the registers of `id`'s part to `goal`: re-identify the part, read the
/// registers twice, [`plan`] the write, send it, and read it back twice. A
/// read-back that doesn't match sets `faulted`, which refuses every later call
/// — a write is never retried.
pub fn apply<B: SrBus>(
    bus: &mut B,
    faulted: &AtomicBool,
    id: [u8; 3],
    goal: Goal,
) -> Result<Applied, Refusal> {
    if faulted.load(Ordering::Acquire) {
        return Err(Refusal::Faulted);
    }
    let r = recipe(id).ok_or(Refusal::UnknownPart)?;
    if bus.read_id() != id {
        return Err(Refusal::PartChanged);
    }
    let (sr1, sr2) = read_stable(bus)?;
    let Some(w) = plan(r, goal, sr1, sr2)? else {
        return Ok(Applied::AlreadySet);
    };
    let (want1, want2) = goal.target(sr2);
    if r.method == WriteMethod::Combined {
        if let Err(e) = preflight(bus, w, (sr1, sr2), (want1, want2)) {
            faulted.store(true, Ordering::Release);
            return Err(e);
        }
    }
    let sent = bus.write(r.method, w.sr1, w.sr2);
    match read_stable(bus) {
        Ok((got1, got2)) if sent && protection_bits(got1, got2) == (want1, want2) => {
            Ok(Applied::Written)
        }
        Ok((got1, got2)) => {
            faulted.store(true, Ordering::Release);
            Err(Refusal::VerifyFailed {
                sr1: got1,
                sr2: got2,
            })
        }
        Err(e) => {
            faulted.store(true, Ordering::Release);
            Err(e)
        }
    }
}

/// A protected region opened for a program, volatile: what [`restore_volatile`] puts back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifted {
    region: u8,
    method: WriteMethod,
    qe: u8,
}

/// The volatile write that sets SR1's region field to `region`, composed from scratch: SR1 carries
/// only region bits, and SR2 (sent only where the part writes both bytes at once) only `QE`.
fn volatile_region<B: SrBus>(bus: &mut B, method: WriteMethod, region: u8, qe: u8) -> bool {
    let sr2 = match method {
        WriteMethod::Combined => Some(qe & SR2_QE),
        WriteMethod::Separate => None,
    };
    bus.write_volatile(method, Some(region & SR1_REGION_MASK), sr2)
}

/// Open the protected region for a program without touching the non-volatile protection: clear
/// the block-protect bits through the volatile path (`50h`), which a power cycle or software
/// reset undoes. Pair with [`restore_volatile`].
///
/// As [`apply`]: refused with nothing sent for an unknown part, a part other than `id`, after a
/// write that failed this boot (`faulted`), while the lock-down is armed, or with a bit this crate
/// doesn't expect set (`SRP0`, `CMP`, an `LB` beyond the factory one, a suspend flag). If the
/// write goes out but doesn't read back as planned, a software reset reloads the non-volatile
/// protection, `faulted` latches, and the error says so: the region is never left open behind an
/// `Err`.
///
/// While lifted (until [`restore_volatile`], a power cycle, `66h`/`99h`, or an [`apply`] of a
/// provisioning or lock-down goal, which rewrites the region) the boot region is writable:
/// run nothing untrusted in between.
pub fn lift_volatile<B: SrBus>(
    bus: &mut B,
    faulted: &AtomicBool,
    id: [u8; 3],
) -> Result<Lifted, Refusal> {
    if faulted.load(Ordering::Acquire) {
        return Err(Refusal::Faulted);
    }
    let r = recipe(id).ok_or(Refusal::UnknownPart)?;
    if bus.read_id() != id {
        return Err(Refusal::PartChanged);
    }
    let (sr1, sr2) = read_stable(bus)?;
    if sr1 & SR1_BUSY != 0 {
        return Err(Refusal::Busy);
    }
    if sr2 & SR2_LOCK != 0 {
        return Err(Refusal::Locked);
    }
    let odd_sr2 = sr2 & !(SR2_QE | SR2_LOCK) & !r.factory_sr2;
    if sr1 & SR1_SRP0 != 0 || odd_sr2 != 0 {
        return Err(Refusal::UnexpectedBits { sr1, sr2 });
    }
    let lifted = Lifted {
        region: sr1 & SR1_REGION_MASK,
        method: r.method,
        qe: sr2 & SR2_QE,
    };
    if lifted.region == 0 {
        return Ok(lifted);
    }
    let sent = volatile_region(bus, r.method, 0, lifted.qe);
    checked(bus, faulted, sent, 0).map(|()| lifted)
}

/// Put back the block-protect bits [`lift_volatile`] cleared, volatile. If the write doesn't read
/// back as planned, a software reset reloads the non-volatile protection instead (and `faulted`
/// latches): the region ends protected either way.
pub fn restore_volatile<B: SrBus>(
    bus: &mut B,
    faulted: &AtomicBool,
    lifted: Lifted,
) -> Result<(), Refusal> {
    if lifted.region == 0 {
        return Ok(());
    }
    let sent = volatile_region(bus, lifted.method, lifted.region, lifted.qe);
    checked(bus, faulted, sent, lifted.region)
}

/// Check a volatile region write read back as `region`; if not, software-reset to the
/// non-volatile bits and latch `faulted`.
fn checked<B: SrBus>(
    bus: &mut B,
    faulted: &AtomicBool,
    sent: bool,
    region: u8,
) -> Result<(), Refusal> {
    let read = read_stable(bus);
    if let Ok((got1, _)) = read
        && sent
        && got1 & SR1_REGION_MASK == region
    {
        return Ok(());
    }
    bus.software_reset();
    faulted.store(true, Ordering::Release);
    match read {
        Ok((got1, got2)) => Err(Refusal::VerifyFailed {
            sr1: got1,
            sr2: got2,
        }),
        Err(e) => Err(e),
    }
}

/// Rehearse a combined write on the volatile copy, check it read back as
/// planned, then software-reset back to the non-volatile bits (so the real
/// write's verify sees the real write). A swapped or mangled two-byte transfer
/// shows up here, where it can do no harm: the volatile write can't touch the
/// OTP `LB` bits or `SRP1` (S25FL132K note 37), so the lock bit isn't checked.
fn preflight<B: SrBus>(
    bus: &mut B,
    w: SrWrite,
    before: (u8, u8),
    want: (u8, u8),
) -> Result<(), Refusal> {
    let checked = SR2_QE | SR2_CMP;
    let sent = bus.write_volatile(WriteMethod::Combined, w.sr1, w.sr2);
    let rehearsed = read_stable(bus);
    bus.software_reset();
    let (b1, b2) = before;
    match read_stable(bus) {
        Ok((r1, r2)) if protection_bits(r1, r2) == protection_bits(b1, b2) => {}
        Ok((r1, r2)) => return Err(Refusal::VerifyFailed { sr1: r1, sr2: r2 }),
        Err(e) => return Err(e),
    }
    match rehearsed {
        Ok((p1, p2))
            if sent && protection_bits(p1, p2).0 == want.0 && p2 & checked == want.1 & checked =>
        {
            Ok(())
        }
        Ok((p1, p2)) => Err(Refusal::PreflightFailed { sr1: p1, sr2: p2 }),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 4 MB parts' rule, as the Deluge reads it.
    fn protected_range_4m(sr1: u8) -> Option<core::ops::Range<u32>> {
        protected_range_for(sr1, 4 << 20)
    }
    use core::sync::atomic::AtomicBool;

    const AT25: [u8; 3] = [0x1F, 0x87, 0x01];
    const W25: [u8; 3] = [0xEF, 0x40, 0x16];
    const S25: [u8; 3] = [0x01, 0x40, 0x16];
    const AT25QF641B: [u8; 3] = [0x1F, 0x88, 0x01];
    const PARTS: [[u8; 3]; 4] = [AT25, W25, S25, AT25QF641B];

    fn id(raw: [u8; 3]) -> [u8; 3] {
        raw
    }

    fn rcp(raw: [u8; 3]) -> ProtectionRecipe {
        recipe(id(raw)).unwrap()
    }

    /// Every goal the firmware and `flash-sr-probe` can ask for.
    fn goals(r: ProtectionRecipe) -> [Goal; 5] {
        [
            Goal::provision(r),
            Goal::lockdown(r),
            Goal::unprotect(),
            Goal::top_test(false),
            Goal::top_test(true),
        ]
    }

    /// Each supported part maps to the right bottom-1 MB recipe, and unrecognized
    /// IDs (incl. the legacy S25FL032P) yield no recipe so nothing is ever written.
    #[test]
    fn recipes_match_the_supported_parts() {
        assert_eq!(rcp(AT25).sr1_region, 0x34);
        assert_eq!(rcp(AT25).method, WriteMethod::Separate);
        assert_eq!(rcp(W25).sr1_region, 0x34);
        assert_eq!(rcp(W25).method, WriteMethod::Separate);
        assert_eq!(rcp(S25).sr1_region, 0x34);
        assert_eq!(rcp(S25).method, WriteMethod::Combined);
        // The S25FL132K ships with LB0 set (security register 0 holds SFDP).
        assert_eq!(rcp(S25).factory_sr2, 0x04);
        assert_eq!(rcp(AT25).factory_sr2, 0);
        assert_eq!(rcp(W25).factory_sr2, 0);
        assert_eq!(recipe(id([0x01, 0x02, 0x15])), None);
        assert_eq!(recipe(id([0x00, 0x00, 0x00])), None);
    }

    /// Every region byte sits entirely inside the writable SR1 field (bits 6:2)
    /// with `SRP0` (bit 7) clear.
    #[test]
    fn region_bytes_stay_in_the_block_protect_field() {
        for raw in PARTS {
            let r = rcp(raw);
            assert_eq!(r.sr1_region & !SR1_REGION_MASK, 0, "only bits 6:2 set");
        }
        assert_eq!(TOP_64K & !SR1_REGION_MASK, 0);
    }

    /// The permanent-state guarantee: whatever the status registers read back as
    /// (including garbage like `FF`), no planned write ever carries a `1` in
    /// `SRP0` (SR1 bit 7) or in any SR2 bit other than `QE` and the lock — so
    /// neither the `(1,1)` OTP lock nor an `LB` OTP bit can be set on purpose.
    #[test]
    fn no_planned_write_sets_a_permanent_bit() {
        for raw in PARTS {
            let r = rcp(raw);
            for goal in goals(r) {
                for sr1 in 0..=255u8 {
                    for sr2 in 0..=255u8 {
                        if let Ok(Some(w)) = plan(r, goal, sr1, sr2) {
                            if let Some(b) = w.sr1 {
                                assert_eq!(b & !SR1_REGION_MASK, 0, "SR1 {b:#04x}");
                            }
                            if let Some(b) = w.sr2 {
                                assert_eq!(b & !(SR2_QE | SR2_LOCK), 0, "SR2 {b:#04x}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// If the combined two-byte write clocked its bytes out swapped, the SR2 byte
    /// would land in SR1 and the region byte in SR2. Neither may then hold `SRP0`.
    #[test]
    fn a_swapped_combined_write_cannot_set_srp0() {
        let r = rcp(S25);
        for goal in goals(r) {
            for sr2 in 0..=255u8 {
                if let Ok(Some(w)) = plan(r, goal, 0, sr2) {
                    assert_eq!(w.sr2.unwrap() & 0x80, 0);
                    assert_eq!(w.sr1.unwrap() & 0x80, 0);
                }
            }
        }
    }

    /// Arming refuses to write anything when the registers hold a bit it doesn't
    /// expect: `SRP0`, a lock bit beyond the factory one, a suspend flag, or BUSY.
    #[test]
    fn arming_refuses_unexpected_bits() {
        let (w25, s25, at25) = (rcp(W25), rcp(S25), rcp(AT25));
        let lock = Goal::lockdown(w25);
        assert_eq!(
            plan(w25, lock, 0x80, 0),
            Err(Refusal::UnexpectedBits { sr1: 0x80, sr2: 0 })
        );
        assert_eq!(
            plan(w25, lock, 0, 0x08),
            Err(Refusal::UnexpectedBits { sr1: 0, sr2: 0x08 })
        );
        assert_eq!(
            plan(w25, lock, 0, 0x80),
            Err(Refusal::UnexpectedBits { sr1: 0, sr2: 0x80 })
        );
        assert_eq!(plan(w25, lock, 0x01, 0), Err(Refusal::Busy));
        // Bit 2 is the S25's factory LB0 — expected there, unexpected on the AT25.
        assert!(plan(s25, Goal::lockdown(s25), 0, 0x04).is_ok());
        assert_eq!(
            plan(at25, Goal::lockdown(at25), 0, 0x04),
            Err(Refusal::UnexpectedBits { sr1: 0, sr2: 0x04 })
        );
    }

    /// Unprotect is the recovery path: it may clear odd bits (writing `0` is never
    /// permanent), so it doesn't refuse them — but it still won't write while busy.
    #[test]
    fn unprotect_clears_what_arming_would_refuse() {
        let r = rcp(W25);
        let w = plan(r, Goal::unprotect(), 0x80 | 0x24, 0x40 | 0x02)
            .unwrap()
            .unwrap();
        assert_eq!(w.sr1, Some(0));
        assert_eq!(w.sr2, Some(SR2_QE));
        assert_eq!(plan(r, Goal::unprotect(), 0x01, 0), Err(Refusal::Busy));
    }

    /// `QE` is the one bit carried over from the read; everything else is composed.
    #[test]
    fn qe_is_preserved() {
        let r = rcp(W25);
        let w = plan(r, Goal::lockdown(r), 0, SR2_QE).unwrap().unwrap();
        assert_eq!(w.sr2, Some(SR2_QE | SR2_LOCK));
    }

    /// With the lock-down in force nothing can be written: a goal already met is a
    /// no-op, anything else is refused (only a power cycle clears it).
    #[test]
    fn a_locked_register_is_never_written() {
        let r = rcp(W25);
        assert_eq!(plan(r, Goal::lockdown(r), 0x34, SR2_LOCK), Ok(None));
        assert_eq!(
            plan(r, Goal::unprotect(), 0x34, SR2_LOCK),
            Err(Refusal::Locked)
        );
        assert_eq!(
            plan(r, Goal::lockdown(r), 0x00, SR2_LOCK),
            Err(Refusal::Locked)
        );
    }

    /// A normal boot on a provisioned part does one non-volatile write: SR2 only,
    /// where the part can take SR2 on its own.
    #[test]
    fn a_provisioned_part_only_writes_the_lock() {
        let r = rcp(W25);
        let w = plan(r, Goal::lockdown(r), 0x34, 0).unwrap().unwrap();
        assert_eq!(
            w,
            SrWrite {
                sr1: None,
                sr2: Some(SR2_LOCK)
            }
        );
        let r = rcp(AT25);
        let w = plan(r, Goal::lockdown(r), 0x34, 0).unwrap().unwrap();
        assert_eq!(
            w,
            SrWrite {
                sr1: None,
                sr2: Some(SR2_LOCK)
            }
        );
    }

    /// The combined method has no SR2-only write: it always sends both bytes.
    #[test]
    fn the_combined_method_writes_both_registers() {
        let r = rcp(S25);
        let w = plan(r, Goal::lockdown(r), 0x34, 0x04).unwrap().unwrap();
        assert_eq!(
            w,
            SrWrite {
                sr1: Some(0x34),
                sr2: Some(SR2_LOCK)
            }
        );
    }

    /// Rows copied from the three datasheets' CMP=0 tables (AT25SF321B Table 6,
    /// W25Q32JV "Status Register Memory Protection", S25FL132K Table 20), as
    /// (TB-or-BP3, BP2:BP0) → protected range. All three agree on these rows.
    #[test]
    fn the_decoder_matches_the_datasheet_rows() {
        let row = |tb: u8, bp: u8| (tb << 5) | (bp << 2);
        assert_eq!(protected_range_4m(row(0, 0b000)), None);
        assert_eq!(protected_range_4m(row(1, 0b000)), None);
        assert_eq!(
            protected_range_4m(row(0, 0b001)),
            Some(0x3F_0000..0x40_0000)
        );
        assert_eq!(
            protected_range_4m(row(0, 0b101)),
            Some(0x30_0000..0x40_0000)
        );
        assert_eq!(
            protected_range_4m(row(0, 0b110)),
            Some(0x20_0000..0x40_0000)
        );
        assert_eq!(
            protected_range_4m(row(1, 0b001)),
            Some(0x00_0000..0x01_0000)
        );
        assert_eq!(
            protected_range_4m(row(1, 0b100)),
            Some(0x00_0000..0x08_0000)
        );
        assert_eq!(
            protected_range_4m(row(1, 0b101)),
            Some(0x00_0000..0x10_0000)
        );
        assert_eq!(
            protected_range_4m(row(1, 0b110)),
            Some(0x00_0000..0x20_0000)
        );
        assert_eq!(
            protected_range_4m(row(0, 0b111)),
            Some(0x00_0000..0x40_0000)
        );
    }

    /// Each recipe protects exactly the FSB + SSB, and the probe's stand-in
    /// region exactly the top 64 KB. (A W25/S25 recipe of `0x24` — `TB` with
    /// `BP0`, the lower 64 KB — passes a literal-for-literal test, so the
    /// range itself is checked.)
    #[test]
    fn each_recipe_protects_exactly_the_bootloaders() {
        for raw in PARTS {
            let capacity = if raw == AT25QF641B { 8 << 20 } else { 4 << 20 };
            assert_eq!(
                protected_range_for(rcp(raw).sr1_region, capacity),
                Some(0..0x10_0000),
                "{raw:02x?}"
            );
        }
        assert_eq!(protected_range_4m(TOP_64K), Some(0x3F_0000..0x40_0000));
    }

    /// `flash-sr-probe` may erase a test block only when it holds exactly what its
    /// rejection test can leave behind: the zero mark, then blank.
    #[test]
    fn only_the_probes_own_mark_is_restorable() {
        let mut block = [0xFFu8; 0x1_0000];
        assert_eq!(test_block_state(&block), TestBlock::Blank);
        block[..TEST_MARK_LEN].fill(0);
        assert_eq!(test_block_state(&block), TestBlock::ProbeMark);
        block[TEST_MARK_LEN] = 0;
        assert_eq!(test_block_state(&block), TestBlock::Other);
        let mut block = [0xFFu8; 0x1_0000];
        block[3] = 0;
        assert_eq!(test_block_state(&block), TestBlock::Other);
        let mut block = [0xFFu8; 0x1_0000];
        block[..TEST_MARK_LEN].fill(0);
        block[0xFFFF] = 0x7F;
        assert_eq!(test_block_state(&block), TestBlock::Other);
    }

    /// A goal that already holds writes nothing.
    #[test]
    fn a_met_goal_writes_nothing() {
        let r = rcp(W25);
        assert_eq!(plan(r, Goal::provision(r), 0x34, 0), Ok(None));
        assert_eq!(plan(r, Goal::unprotect(), 0x00, SR2_QE), Ok(None));
        let r = rcp(S25);
        assert_eq!(plan(r, Goal::unprotect(), 0x00, 0x04), Ok(None));
    }

    /// A write the locked chip ignores leaves `WEL` set (seen on a W25Q32JV:
    /// SR1 `04` → `06`). Status bits are not protection: only the region, `SRP0`
    /// and SR2's owned bits count as a change.
    #[test]
    fn status_bits_are_not_a_protection_change() {
        assert_eq!(protection_bits(0x04, 0x03), protection_bits(0x06, 0x03));
        assert_eq!(protection_bits(0x04, 0x03), protection_bits(0x05, 0x83));
        assert_ne!(protection_bits(0x04, 0x03), protection_bits(0x00, 0x03));
        assert_ne!(protection_bits(0x04, 0x03), protection_bits(0x04, 0x02));
        assert_ne!(protection_bits(0x04, 0x03), protection_bits(0x84, 0x03));
    }

    /// A model of the chip for the I/O path: SR writes obey the lock-down and the
    /// OTP `LB` bits, and every write is counted.
    ///
    /// `sr1`/`sr2` are the active (volatile) copies the reads return; `nv1`/`nv2`
    /// the non-volatile bits a power-up or software reset loads them from. As on
    /// the S25FL132K (note 37), a volatile write can't change `SRP1` (the lock) or
    /// the `LB` bits.
    struct Chip {
        id: [u8; 3],
        sr1: u8,
        sr2: u8,
        nv1: u8,
        nv2: u8,
        /// Return a different SR1 on every other read (a flaky bus).
        flaky: bool,
        /// Accept writes but change nothing (a write that never lands).
        deaf: bool,
        /// A combined write's two data bytes arrive in the opposite order.
        swap: bool,
        reads: u32,
        writes: u32,
        volatile_writes: u32,
        resets: u32,
    }

    impl Chip {
        fn new(raw: [u8; 3], sr2: u8) -> Self {
            Chip {
                id: raw,
                sr1: 0,
                sr2,
                nv1: 0,
                nv2: sr2,
                flaky: false,
                deaf: false,
                swap: false,
                reads: 0,
                writes: 0,
                volatile_writes: 0,
                resets: 0,
            }
        }

        /// The bytes the chip receives for a write sent as `sr1`/`sr2`.
        fn received(
            &self,
            method: WriteMethod,
            sr1: Option<u8>,
            sr2: Option<u8>,
        ) -> (Option<u8>, Option<u8>) {
            if self.swap && method == WriteMethod::Combined {
                (sr2, sr1)
            } else {
                (sr1, sr2)
            }
        }
    }

    impl SrBus for Chip {
        fn read_id(&mut self) -> [u8; 3] {
            self.id
        }
        fn read_regs(&mut self) -> (u8, u8) {
            self.reads += 1;
            let noise = if self.flaky && self.reads % 2 == 0 {
                0x10
            } else {
                0
            };
            (self.sr1 ^ noise, self.sr2)
        }
        fn write(&mut self, method: WriteMethod, sr1: Option<u8>, sr2: Option<u8>) -> bool {
            self.writes += 1;
            if self.deaf || self.sr2 & SR2_LOCK != 0 {
                return true;
            }
            let (sr1, sr2) = self.received(method, sr1, sr2);
            if let Some(b) = sr1 {
                self.nv1 = b & 0xFC;
                self.sr1 = (self.sr1 & 0x03) | (b & 0xFC);
            }
            if let Some(b) = sr2 {
                let otp = self.nv2 & 0x3C; // LB bits only ever go 0 -> 1
                self.nv2 = (b & 0x7F) | otp;
                self.sr2 = (self.sr2 & 0x80) | self.nv2;
            }
            true
        }
        fn write_volatile(
            &mut self,
            method: WriteMethod,
            sr1: Option<u8>,
            sr2: Option<u8>,
        ) -> bool {
            self.volatile_writes += 1;
            if self.deaf || self.sr2 & SR2_LOCK != 0 {
                return true;
            }
            let (sr1, sr2) = self.received(method, sr1, sr2);
            if let Some(b) = sr1 {
                self.sr1 = (self.sr1 & 0x03) | (b & 0xFC);
            }
            if let Some(b) = sr2 {
                // Not SRP1 (the lock) and not LB: those have no volatile copy.
                let kept = self.sr2 & (0x80 | 0x3C | SR2_LOCK);
                self.sr2 = kept | (b & !(0x80 | 0x3C | SR2_LOCK));
            }
            true
        }
        fn software_reset(&mut self) {
            self.resets += 1;
            self.sr1 = (self.sr1 & 0x03) | self.nv1;
            self.sr2 = (self.sr2 & 0x80) | self.nv2;
        }
    }

    #[test]
    fn at25qf641b_recipe_by_id() {
        let r = recipe(AT25QF641B).unwrap();
        assert_eq!(r.sr1_region, 0x30, "TB=1, BP2:0=100");
        assert_eq!(r.method, WriteMethod::Separate);
        assert_eq!(r.factory_sr2, 0);
        // Keyed on the datasheet's 9Fh order (Table 16) until the bench settles what the part sends.
        assert_eq!(recipe([0x1F, 0x01, 0x88]), None);
    }

    #[test]
    fn the_bottom_1mb_of_8mb_is_lower_eighth() {
        let eight = 8 << 20;
        assert_eq!(protected_range_for(0x30, eight), Some(0..0x10_0000));
        // Table 6's "lower 1/64" (TB | BP0): 128 KB, not the table's 000000h-1FFFFFh.
        assert_eq!(protected_range_for(0x24, eight), Some(0..0x2_0000));
        assert_eq!(protected_range_for(0x04, eight), Some(0x7E_0000..0x80_0000));
    }

    fn at25qf_protected() -> Chip {
        let mut c = Chip::new(AT25QF641B, 0x02);
        c.nv1 = 0x30;
        c.sr1 = 0x30;
        c
    }

    #[test]
    fn lift_clears_bp_volatile_and_restore_puts_it_back() {
        let faulted = AtomicBool::new(false);
        let mut chip = at25qf_protected();
        let lifted = lift_volatile(&mut chip, &faulted, AT25QF641B).unwrap();
        assert_eq!(
            chip.sr1 & SR1_REGION_MASK,
            0,
            "the region is open, volatile"
        );
        assert_eq!(chip.nv1, 0x30, "the non-volatile protection is untouched");
        assert_eq!((chip.writes, chip.volatile_writes), (0, 1));
        restore_volatile(&mut chip, &faulted, lifted).unwrap();
        assert_eq!(chip.sr1 & SR1_REGION_MASK, 0x30);
        assert_eq!((chip.writes, chip.volatile_writes, chip.resets), (0, 2, 0));
        assert_eq!(chip.sr2 & SR2_QE, SR2_QE, "QE as found");
    }

    #[test]
    fn lift_refused_while_locked() {
        let faulted = AtomicBool::new(false);
        let mut chip = at25qf_protected();
        chip.sr2 |= SR2_LOCK;
        chip.nv2 |= SR2_LOCK;
        assert_eq!(
            lift_volatile(&mut chip, &faulted, AT25QF641B),
            Err(Refusal::Locked)
        );
        assert_eq!((chip.writes, chip.volatile_writes), (0, 0));
    }

    #[test]
    fn a_failed_lift_reloads_the_protection() {
        let faulted = AtomicBool::new(false);
        let mut chip = at25qf_protected();
        chip.deaf = true;
        assert!(matches!(
            lift_volatile(&mut chip, &faulted, AT25QF641B),
            Err(Refusal::VerifyFailed { .. })
        ));
        assert_eq!(
            chip.resets, 1,
            "a software reset reloaded the non-volatile bits"
        );
        assert_eq!(chip.sr1 & SR1_REGION_MASK, 0x30, "protected again");
        assert!(faulted.load(Ordering::Acquire), "no more writes this boot");
        assert_eq!(
            lift_volatile(&mut at25qf_protected(), &faulted, AT25QF641B),
            Err(Refusal::Faulted)
        );
    }

    #[test]
    fn a_failed_restore_reloads_the_protection() {
        let faulted = AtomicBool::new(false);
        let mut chip = at25qf_protected();
        let lifted = lift_volatile(&mut chip, &faulted, AT25QF641B).unwrap();
        chip.deaf = true;
        assert!(restore_volatile(&mut chip, &faulted, lifted).is_err());
        assert_eq!(chip.resets, 1);
        assert_eq!(chip.sr1 & SR1_REGION_MASK, 0x30, "the reset restored it");
        assert!(faulted.load(Ordering::Acquire));
    }

    #[test]
    fn lift_refuses_cmp() {
        let faulted = AtomicBool::new(false);
        let mut chip = at25qf_protected();
        chip.sr2 |= SR2_CMP;
        chip.nv2 |= SR2_CMP;
        assert!(matches!(
            lift_volatile(&mut chip, &faulted, AT25QF641B),
            Err(Refusal::UnexpectedBits { .. })
        ));
        assert_eq!(chip.volatile_writes, 0);
    }

    #[test]
    fn lift_reidentifies_the_part() {
        let faulted = AtomicBool::new(false);
        let mut chip = at25qf_protected();
        chip.id = W25;
        assert_eq!(
            lift_volatile(&mut chip, &faulted, AT25QF641B),
            Err(Refusal::PartChanged)
        );
        assert_eq!(
            lift_volatile(&mut chip, &faulted, [0x01, 0x02, 0x15]),
            Err(Refusal::UnknownPart)
        );
        assert_eq!(chip.volatile_writes, 0);
    }

    /// Records what is sent, for every register state.
    struct Recorder {
        regs: (u8, u8),
        sent: std::vec::Vec<(Option<u8>, Option<u8>, bool)>,
    }

    impl SrBus for Recorder {
        fn read_id(&mut self) -> [u8; 3] {
            S25
        }
        fn read_regs(&mut self) -> (u8, u8) {
            self.regs
        }
        fn write(&mut self, _: WriteMethod, sr1: Option<u8>, sr2: Option<u8>) -> bool {
            self.sent.push((sr1, sr2, false));
            true
        }
        fn write_volatile(&mut self, _: WriteMethod, sr1: Option<u8>, sr2: Option<u8>) -> bool {
            self.sent.push((sr1, sr2, true));
            true
        }
        fn software_reset(&mut self) {}
    }

    #[test]
    fn lift_never_sends_srp_or_lb() {
        // The S25's combined write sends SR2 too: the widest case.
        for sr1 in 0..=255u8 {
            for sr2 in 0..=255u8 {
                let mut bus = Recorder {
                    regs: (sr1, sr2),
                    sent: std::vec::Vec::new(),
                };
                let faulted = AtomicBool::new(false);
                if let Ok(lifted) = lift_volatile(&mut bus, &faulted, S25) {
                    let _ = restore_volatile(&mut bus, &faulted, lifted);
                }
                for &(a, b, volatile) in &bus.sent {
                    assert!(volatile, "only volatile writes");
                    assert_eq!(a.unwrap_or(0) & 0x83, 0, "no SRP0 or read-only bits in SR1");
                    assert_eq!(b.unwrap_or(0) & !SR2_QE, 0, "SR2 carries only QE");
                }
            }
        }
    }

    /// An S25 whose combined write swaps the bytes: the volatile pre-flight reads
    /// back wrong, the reset puts the registers back, and nothing non-volatile is
    /// written — no `LB` bit set, nothing locked, no further writes this boot.
    #[test]
    fn the_preflight_catches_a_swapped_combined_write() {
        let mut chip = Chip::new(S25, 0x04);
        chip.swap = true;
        let faulted = AtomicBool::new(false);
        let r = rcp(S25);
        assert!(matches!(
            apply(&mut chip, &faulted, id(S25), Goal::lockdown(r)),
            Err(Refusal::PreflightFailed { .. })
        ));
        assert_eq!(chip.writes, 0, "no non-volatile write");
        assert_eq!((chip.nv1, chip.nv2), (0x00, 0x04), "LB0 only, as shipped");
        assert_eq!(
            (chip.sr1 & 0xFC, chip.sr2),
            (0x00, 0x04),
            "registers restored"
        );
        assert_eq!(chip.resets, 1);
        assert!(faulted.load(core::sync::atomic::Ordering::Acquire));
    }

    /// A good pre-flight is undone by a reset before the real write, so the
    /// non-volatile write is what the verify sees.
    #[test]
    fn a_good_preflight_leads_to_the_nonvolatile_write() {
        let mut chip = Chip::new(S25, 0x04);
        let faulted = AtomicBool::new(false);
        let r = rcp(S25);
        assert_eq!(
            apply(&mut chip, &faulted, id(S25), Goal::lockdown(r)),
            Ok(Applied::Written)
        );
        assert_eq!((chip.volatile_writes, chip.resets, chip.writes), (1, 1, 1));
        assert_eq!((chip.nv1, chip.nv2), (0x34, 0x04 | SR2_LOCK));
    }

    /// Parts with separate one-byte writes have no byte order to get wrong.
    #[test]
    fn separate_write_parts_skip_the_preflight() {
        let mut chip = Chip::new(W25, 0);
        let faulted = AtomicBool::new(false);
        let r = rcp(W25);
        assert_eq!(
            apply(&mut chip, &faulted, id(W25), Goal::lockdown(r)),
            Ok(Applied::Written)
        );
        assert_eq!((chip.volatile_writes, chip.resets), (0, 0));
    }

    #[test]
    fn apply_arms_and_verifies() {
        let mut chip = Chip::new(W25, 0);
        let faulted = AtomicBool::new(false);
        let r = rcp(W25);
        assert_eq!(
            apply(&mut chip, &faulted, id(W25), Goal::lockdown(r)),
            Ok(Applied::Written)
        );
        assert_eq!((chip.sr1, chip.sr2), (0x34, SR2_LOCK));
        // Armed again (same boot): nothing to write.
        assert_eq!(
            apply(&mut chip, &faulted, id(W25), Goal::lockdown(r)),
            Ok(Applied::AlreadySet)
        );
        assert_eq!(chip.writes, 1);
    }

    #[test]
    fn apply_refuses_another_part() {
        let mut chip = Chip::new(AT25, 0);
        let faulted = AtomicBool::new(false);
        let r = rcp(W25);
        assert_eq!(
            apply(&mut chip, &faulted, id(W25), Goal::lockdown(r)),
            Err(Refusal::PartChanged)
        );
        assert_eq!(chip.writes, 0);
    }

    #[test]
    fn apply_refuses_an_unknown_part() {
        let mut chip = Chip::new([0x01, 0x02, 0x15], 0);
        let faulted = AtomicBool::new(false);
        assert_eq!(
            apply(
                &mut chip,
                &faulted,
                id([0x01, 0x02, 0x15]),
                Goal::unprotect()
            ),
            Err(Refusal::UnknownPart)
        );
        assert_eq!(chip.writes, 0);
    }

    #[test]
    fn apply_refuses_unstable_reads() {
        let mut chip = Chip::new(W25, 0);
        chip.flaky = true;
        let faulted = AtomicBool::new(false);
        assert_eq!(
            apply(&mut chip, &faulted, id(W25), Goal::lockdown(rcp(W25))),
            Err(Refusal::UnstableRead)
        );
        assert_eq!(chip.writes, 0);
    }

    /// A write that doesn't read back as planned latches the fault: it is not
    /// retried, and no further write is attempted until the next boot.
    #[test]
    fn a_failed_verify_stops_all_further_writes() {
        let mut chip = Chip::new(W25, 0);
        chip.deaf = true;
        let faulted = AtomicBool::new(false);
        let r = rcp(W25);
        assert!(matches!(
            apply(&mut chip, &faulted, id(W25), Goal::lockdown(r)),
            Err(Refusal::VerifyFailed { .. })
        ));
        assert_eq!(chip.writes, 1);
        chip.deaf = false;
        assert_eq!(
            apply(&mut chip, &faulted, id(W25), Goal::unprotect()),
            Err(Refusal::Faulted)
        );
        assert_eq!(chip.writes, 1);
    }

    /// The S25's factory LB0 survives every write and doesn't fail verification.
    #[test]
    fn the_s25_factory_lock_bit_does_not_fail_verification() {
        let mut chip = Chip::new(S25, 0x04);
        let faulted = AtomicBool::new(false);
        let r = rcp(S25);
        assert_eq!(
            apply(&mut chip, &faulted, id(S25), Goal::lockdown(r)),
            Ok(Applied::Written)
        );
        assert_eq!(chip.sr2, 0x04 | SR2_LOCK);
    }
}
