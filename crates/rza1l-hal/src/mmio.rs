//! Memory-mapped I/O seam.
//!
//! A thin indirection over volatile register reads/writes so HAL register
//! *sequences* can be unit-tested. On the firmware target these are plain
//! `core::ptr::{read,write}_volatile`; on the host/QEMU test target they go to
//! a per-thread shadow memory + access log, so a test can drive an init routine
//! and assert exactly which registers it touched, in what order, with what
//! values — instead of dereferencing unmapped MMIO addresses (which would
//! fault) or only checking address arithmetic.
//!
//! ## Using it in a driver
//! Replace `core::ptr::write_volatile(addr as *mut u32, val)` with
//! `mmio::write32(addr, val)` (and likewise `read*`). The calls stay `unsafe`
//! (real MMIO is), so existing `unsafe` blocks are unchanged.
//!
//! ## Using it in a test (host only)
//! ```ignore
//! mmio::test::reset();
//! unsafe { stb::init(&CONFIG) };
//! assert_eq!(mmio::test::writes(), &[(STBCR2, 0x6A), /* … */]);
//! ```
//!
//! The shadow is **thread-local**, and cargo reuses test threads, so each test
//! that uses the seam must call [`test::reset`] first for a clean slate.

// ── Firmware: real volatile MMIO ───────────────────────────────────────────

/// Write a 32-bit register.
///
/// # Safety
/// `addr` must be a valid, correctly-aligned MMIO register address.
#[cfg(target_os = "none")]
#[inline(always)]
pub unsafe fn write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

/// Read a 32-bit register.
///
/// # Safety
/// `addr` must be a valid, correctly-aligned MMIO register address.
#[cfg(target_os = "none")]
#[inline(always)]
pub unsafe fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Write a 16-bit register. See [`write32`] for safety.
#[cfg(target_os = "none")]
#[inline(always)]
pub unsafe fn write16(addr: usize, val: u16) {
    unsafe { core::ptr::write_volatile(addr as *mut u16, val) }
}

/// Read a 16-bit register. See [`read32`] for safety.
#[cfg(target_os = "none")]
#[inline(always)]
pub unsafe fn read16(addr: usize) -> u16 {
    unsafe { core::ptr::read_volatile(addr as *const u16) }
}

/// Write an 8-bit register. See [`write32`] for safety.
#[cfg(target_os = "none")]
#[inline(always)]
pub unsafe fn write8(addr: usize, val: u8) {
    unsafe { core::ptr::write_volatile(addr as *mut u8, val) }
}

/// Read an 8-bit register. See [`read32`] for safety.
#[cfg(target_os = "none")]
#[inline(always)]
pub unsafe fn read8(addr: usize) -> u8 {
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

// ── Host/QEMU: shadow MMIO with an access log ──────────────────────────────

/// Write a 32-bit register (recorded in the shadow). See firmware impl for safety.
#[cfg(not(target_os = "none"))]
#[inline]
pub unsafe fn write32(addr: usize, val: u32) {
    shadow::store(addr, val, 4);
}

/// Read a 32-bit register (from the shadow). See firmware impl for safety.
#[cfg(not(target_os = "none"))]
#[inline]
pub unsafe fn read32(addr: usize) -> u32 {
    shadow::load(addr, 4)
}

#[cfg(not(target_os = "none"))]
#[inline]
pub unsafe fn write16(addr: usize, val: u16) {
    shadow::store(addr, val as u32, 2);
}

#[cfg(not(target_os = "none"))]
#[inline]
pub unsafe fn read16(addr: usize) -> u16 {
    shadow::load(addr, 2) as u16
}

#[cfg(not(target_os = "none"))]
#[inline]
pub unsafe fn write8(addr: usize, val: u8) {
    shadow::store(addr, val as u32, 1);
}

#[cfg(not(target_os = "none"))]
#[inline]
pub unsafe fn read8(addr: usize) -> u8 {
    shadow::load(addr, 1) as u8
}

#[cfg(not(target_os = "none"))]
mod shadow {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    /// A single recorded register access.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Access {
        /// `true` for a write, `false` for a read.
        pub write: bool,
        /// Access width in bytes (1, 2 or 4).
        pub width: u8,
        pub addr: usize,
        /// Value written, or value returned by the read.
        pub val: u32,
    }

    struct State {
        /// Byte-addressed backing store (little-endian), so overlapping
        /// widths at the same address stay consistent.
        mem: BTreeMap<usize, u8>,
        log: Vec<Access>,
    }

    thread_local! {
        static STATE: RefCell<State> = RefCell::new(State {
            mem: BTreeMap::new(),
            log: Vec::new(),
        });
    }

    pub fn store(addr: usize, val: u32, width: u8) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            for i in 0..width as usize {
                s.mem.insert(addr + i, (val >> (8 * i)) as u8);
            }
            s.log.push(Access { write: true, width, addr, val });
        });
    }

    pub fn load(addr: usize, width: u8) -> u32 {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let mut v = 0u32;
            for i in 0..width as usize {
                v |= (*s.mem.get(&(addr + i)).unwrap_or(&0) as u32) << (8 * i);
            }
            s.log.push(Access { write: false, width, addr, val: v });
            v
        })
    }

    pub fn reset() {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.mem.clear();
            s.log.clear();
        });
    }

    pub fn log() -> Vec<Access> {
        STATE.with(|s| s.borrow().log.clone())
    }

    /// Preload a register value (no log entry) so a subsequent read sees it.
    pub fn poke(addr: usize, val: u32, width: u8) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            for i in 0..width as usize {
                s.mem.insert(addr + i, (val >> (8 * i)) as u8);
            }
        });
    }

    /// Current backing value of a register (no log entry).
    pub fn peek(addr: usize, width: u8) -> u32 {
        STATE.with(|s| {
            let s = s.borrow();
            let mut v = 0u32;
            for i in 0..width as usize {
                v |= (*s.mem.get(&(addr + i)).unwrap_or(&0) as u32) << (8 * i);
            }
            v
        })
    }
}

/// Test-only inspection API for the shadow MMIO (host/QEMU target).
#[cfg(not(target_os = "none"))]
pub mod test {
    pub use super::shadow::Access;
    use super::shadow;
    use std::vec::Vec;

    /// Clear the shadow memory and access log. Call at the start of every test
    /// that uses the seam (test threads are reused across tests).
    pub fn reset() {
        shadow::reset();
    }

    /// The full ordered access log (reads and writes).
    pub fn log() -> Vec<Access> {
        shadow::log()
    }

    /// Just the writes, as `(addr, value)` in order.
    pub fn writes() -> Vec<(usize, u32)> {
        shadow::log()
            .into_iter()
            .filter(|a| a.write)
            .map(|a| (a.addr, a.val))
            .collect()
    }

    /// Preload a register so a later read returns `val` (does not log).
    pub fn poke32(addr: usize, val: u32) {
        shadow::poke(addr, val, 4);
    }
    /// Preload a 16-bit register (does not log).
    pub fn poke16(addr: usize, val: u16) {
        shadow::poke(addr, val as u32, 2);
    }
    /// Preload an 8-bit register (does not log).
    pub fn poke8(addr: usize, val: u8) {
        shadow::poke(addr, val as u32, 1);
    }

    /// Current backing value of a 32-bit register (does not log).
    pub fn peek32(addr: usize) -> u32 {
        shadow::peek(addr, 4)
    }
    /// Current backing value of a 16-bit register (does not log).
    pub fn peek16(addr: usize) -> u16 {
        shadow::peek(addr, 2) as u16
    }
    /// Current backing value of an 8-bit register (does not log).
    pub fn peek8(addr: usize) -> u8 {
        shadow::peek(addr, 1) as u8
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn write_read_round_trip_per_width() {
        test::reset();
        unsafe {
            write32(0x1000, 0xDEAD_BEEF);
            write16(0x2000, 0xABCD);
            write8(0x3000, 0x5A);
            assert_eq!(read32(0x1000), 0xDEAD_BEEF);
            assert_eq!(read16(0x2000), 0xABCD);
            assert_eq!(read8(0x3000), 0x5A);
        }
    }

    #[test]
    fn writes_are_little_endian_in_the_byte_store() {
        test::reset();
        unsafe {
            write32(0x40, 0x1122_3344);
            // Low byte first.
            assert_eq!(read8(0x40), 0x44);
            assert_eq!(read8(0x41), 0x33);
            assert_eq!(read8(0x42), 0x22);
            assert_eq!(read8(0x43), 0x11);
            // A 16-bit read of the low half.
            assert_eq!(read16(0x40), 0x3344);
        }
    }

    #[test]
    fn log_records_order_and_kind() {
        test::reset();
        unsafe {
            write8(0xFF00, 1);
            let _ = read8(0xFF00);
            write32(0xFF10, 0x99);
        }
        let log = test::log();
        assert_eq!(log.len(), 3);
        assert!(log[0].write && log[0].addr == 0xFF00 && log[0].width == 1);
        assert!(!log[1].write && log[1].addr == 0xFF00 && log[1].val == 1);
        assert!(log[2].write && log[2].addr == 0xFF10 && log[2].val == 0x99);

        assert_eq!(test::writes(), [(0xFF00, 1), (0xFF10, 0x99)]);
    }

    #[test]
    fn reset_clears_state() {
        unsafe { write32(0x10, 0x1) };
        test::reset();
        assert!(test::log().is_empty());
        assert_eq!(test::peek32(0x10), 0);
    }

    #[test]
    fn poke_preloads_reads_without_logging() {
        test::reset();
        test::poke32(0x2000, 0xCAFE);
        assert!(test::log().is_empty(), "poke must not log");
        assert_eq!(unsafe { read32(0x2000) }, 0xCAFE);
        assert_eq!(test::log().len(), 1, "the read is logged");
    }
}
