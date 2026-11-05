//! The register handle the generated code uses (chiptool `--common-module crate::common`).
//!
//! chiptool's built-in handle with one change: every read and write goes through
//! [`crate::mmio`], real volatile accesses on the device and a shadow memory with an access log
//! on the host. `size_of::<T>()` is a constant, so the width dispatch compiles to one load or
//! store.

use core::marker::PhantomData;

/// Access marker: the register can be read and written.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct RW;
/// Access marker: the register is read-only.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct R;
/// Access marker: the register is write-only.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct W;

mod sealed {
    use super::*;
    pub trait Access {}
    impl Access for R {}
    impl Access for W {}
    impl Access for RW {}
}

/// A register access marker: [`R`], [`W`] or [`RW`]. Sealed.
pub trait Access: sealed::Access + Copy {}
impl Access for R {}
impl Access for W {}
impl Access for RW {}

/// An access marker that allows reads ([`R`], [`RW`]).
pub trait Read: Access {}
impl Read for RW {}
impl Read for R {}

/// An access marker that allows writes ([`W`], [`RW`]).
pub trait Write: Access {}
impl Write for RW {}
impl Write for W {}

/// One memory-mapped register of type `T` (`u8`, `u16`, `u32` or a fieldset newtype of one).
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Reg<T: Copy, A: Access> {
    ptr: *mut u8,
    phantom: PhantomData<*mut (T, A)>,
}
unsafe impl<T: Copy, A: Access> Send for Reg<T, A> {}
unsafe impl<T: Copy, A: Access> Sync for Reg<T, A> {}

impl<T: Copy, A: Access> Reg<T, A> {
    /// # Safety
    /// `ptr` is a register of type `T`.
    #[inline(always)]
    pub const unsafe fn from_ptr(ptr: *mut T) -> Self {
        Self {
            ptr: ptr as _,
            phantom: PhantomData,
        }
    }

    /// The register's address as a raw pointer.
    #[inline(always)]
    pub const fn as_ptr(&self) -> *mut T {
        self.ptr as _
    }

    /// The register's bus address.
    #[inline(always)]
    pub fn addr(&self) -> usize {
        self.ptr as usize
    }

    /// The register's width in bytes.
    #[inline(always)]
    pub const fn width(&self) -> usize {
        core::mem::size_of::<T>()
    }
}

impl<T: Copy, A: Read> Reg<T, A> {
    /// Read the register.
    #[inline(always)]
    pub fn read(&self) -> T {
        let addr = self.ptr as usize;
        // SAFETY: the handle names one register of `T`'s width, and `T` is that many bytes of
        // plain data.
        unsafe {
            match core::mem::size_of::<T>() {
                1 => core::mem::transmute_copy(&crate::mmio::read8(addr)),
                2 => core::mem::transmute_copy(&crate::mmio::read16(addr)),
                4 => core::mem::transmute_copy(&crate::mmio::read32(addr)),
                _ => unreachable!("RZ/A1L registers are 8, 16 or 32 bits wide"),
            }
        }
    }
}

impl<T: Copy, A: Write> Reg<T, A> {
    /// Write `val` to the register.
    #[inline(always)]
    pub fn write_value(&self, val: T) {
        let addr = self.ptr as usize;
        // SAFETY: as in `read`.
        unsafe {
            match core::mem::size_of::<T>() {
                1 => crate::mmio::write8(addr, core::mem::transmute_copy(&val)),
                2 => crate::mmio::write16(addr, core::mem::transmute_copy(&val)),
                4 => crate::mmio::write32(addr, core::mem::transmute_copy(&val)),
                _ => unreachable!("RZ/A1L registers are 8, 16 or 32 bits wide"),
            }
        }
    }
}

impl<T: Default + Copy, A: Write> Reg<T, A> {
    /// Write `f`'s changes to the register's zero value.
    #[inline(always)]
    pub fn write(&self, f: impl FnOnce(&mut T)) {
        let mut val = Default::default();
        f(&mut val);
        self.write_value(val);
    }
}

impl<T: Copy, A: Read + Write> Reg<T, A> {
    /// Read, apply `f`, write back.
    #[inline(always)]
    pub fn modify(&self, f: impl FnOnce(&mut T)) {
        let mut val = self.read();
        f(&mut val);
        self.write_value(val);
    }
}
