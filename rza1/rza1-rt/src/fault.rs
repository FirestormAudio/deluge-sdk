//! What the fault vectors hand to the `#[fault]` handler.

/// The exception that brought the core to the fault handler.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// Undefined instruction.
    Undefined,
    /// Prefetch abort: an instruction fetch faulted.
    PrefetchAbort,
    /// Data abort: a load or store faulted.
    DataAbort,
    /// The reserved vector (0x14), which the core never takes on its own.
    Reserved,
    /// Supervisor call (`svc`); nothing in this runtime handles one.
    Svc,
    /// A code the vectors do not produce.
    Unknown(u32),
}

impl FaultKind {
    /// The kind for the vectors' fault-type code: 0 undefined, 1 prefetch abort, 2 data abort,
    /// 3 reserved, 0xF SVC.
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            0 => Self::Undefined,
            1 => Self::PrefetchAbort,
            2 => Self::DataAbort,
            3 => Self::Reserved,
            0xF => Self::Svc,
            other => Self::Unknown(other),
        }
    }
}

/// A CPU fault, as the `#[fault]` handler receives it. The handler runs on the abort stack in
/// abort mode, with IRQ and FIQ masked; it must not return.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultInfo {
    /// Address of the faulting instruction.
    pub pc: u32,
    /// Which vector was taken.
    pub kind: FaultKind,
    /// The interrupted mode's (SYS mode's) link register.
    pub lr: u32,
    /// The interrupted mode's (SYS mode's) stack pointer.
    pub sp: u32,
}

impl FaultInfo {
    /// Built by the code `#[fault]` generates, from `rza1_cpu_fault_report`'s arguments.
    #[doc(hidden)]
    pub fn __from_raw(pc: u32, kind: u32, lr: u32, sp: u32) -> Self {
        Self { pc, kind: FaultKind::from_raw(kind), lr, sp }
    }

    /// The faulting address: DFAR for a data abort, IFAR for a prefetch abort, `None` for the
    /// other kinds. Read it before anything else can fault: the registers hold the latest abort.
    pub fn fault_address(&self) -> Option<u32> {
        match self.kind {
            FaultKind::DataAbort => read_dfar(),
            FaultKind::PrefetchAbort => read_ifar(),
            _ => None,
        }
    }
}

#[cfg(target_os = "none")]
fn read_dfar() -> Option<u32> {
    let v: u32;
    // SAFETY: a CP15 read with no side effects.
    unsafe { core::arch::asm!("mrc p15, 0, {}, c6, c0, 0", out(reg) v, options(nomem, nostack)) };
    Some(v)
}

#[cfg(target_os = "none")]
fn read_ifar() -> Option<u32> {
    let v: u32;
    // SAFETY: a CP15 read with no side effects.
    unsafe { core::arch::asm!("mrc p15, 0, {}, c6, c0, 2", out(reg) v, options(nomem, nostack)) };
    Some(v)
}

// Hosts (the unit tests) have no fault address registers.
#[cfg(not(target_os = "none"))]
fn read_dfar() -> Option<u32> {
    None
}

#[cfg(not(target_os = "none"))]
fn read_ifar() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_kind_from_raw() {
        assert_eq!(FaultKind::from_raw(0), FaultKind::Undefined);
        assert_eq!(FaultKind::from_raw(1), FaultKind::PrefetchAbort);
        assert_eq!(FaultKind::from_raw(2), FaultKind::DataAbort);
        assert_eq!(FaultKind::from_raw(3), FaultKind::Reserved);
        assert_eq!(FaultKind::from_raw(0xF), FaultKind::Svc);
        assert_eq!(FaultKind::from_raw(7), FaultKind::Unknown(7));
    }

    #[test]
    fn fault_info_from_the_reporter_arguments() {
        let info = FaultInfo::__from_raw(0x2002_0040, 2, 0x2002_1000, 0x2030_0000);
        assert_eq!(info.pc, 0x2002_0040);
        assert_eq!(info.kind, FaultKind::DataAbort);
        assert_eq!(info.lr, 0x2002_1000);
        assert_eq!(info.sp, 0x2030_0000);
        // Host: there is no fault address register to read.
        assert_eq!(info.fault_address(), None);
    }
}
