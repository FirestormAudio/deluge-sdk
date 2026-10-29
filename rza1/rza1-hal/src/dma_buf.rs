//! DMA buffers: memory the DMAC and the CPU see the same way, with no cache maintenance.
//!
//! [`Uncached`] (a source) and [`UncachedMut`] (a destination) wrap a slice of [`Word`]s (bytes
//! by default) that lies wholly
//! in a normal non-cacheable section of the HAL's MMU table: the `+0x4000_0000` mirrors of SRAM
//! and SDRAM ([`memmap::is_uncached_range`]). A DMA driver that takes one can't be handed cached
//! memory, whose dirty lines would overwrite or hide what the DMAC moves.
//!
//! A board gets such memory from a `static` buffer it touches only through its
//! [`UNCACHED_MIRROR_OFFSET`](crate::UNCACHED_MIRROR_OFFSET) alias, cleaning and invalidating the
//! cached alias's lines once at boot so no stale line is ever written back over it.
//!
//! On the host, where tests run and memory has no mirror, any non-empty slice is accepted.

use crate::memmap;

/// The buffer is empty or not wholly in uncached (DMA-coherent) memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct NotUncached;

fn check(addr: usize, len: usize) -> Result<(), NotUncached> {
    #[cfg(target_os = "none")]
    let ok = memmap::is_uncached_range(addr, len);
    #[cfg(not(target_os = "none"))]
    let ok = {
        let _ = (addr, memmap::is_uncached_range);
        len != 0
    };
    if ok { Ok(()) } else { Err(NotUncached) }
}

mod sealed {
    pub trait Sealed {}
}

/// A DMA transfer unit a buffer can hold: `u8`, `u16`, `u32` or `i32`.
pub trait Word: sealed::Sealed + Copy + 'static {}

macro_rules! words {
    ($($t:ty),*) => {$(
        impl sealed::Sealed for $t {}
        impl Word for $t {}
    )*};
}
words!(u8, u16, u32, i32);

#[cfg(all(test, not(target_os = "none")))]
std::thread_local! {
    /// The byte length the last [`check`] saw, for tests.
    pub(crate) static LAST_CHECKED_LEN: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

/// Check `buf`'s whole byte range.
fn check_words<W: Word>(buf: &[W]) -> Result<(), NotUncached> {
    let bytes = core::mem::size_of_val(buf);
    #[cfg(all(test, not(target_os = "none")))]
    LAST_CHECKED_LEN.with(|l| l.set(bytes));
    check(buf.as_ptr() as usize, bytes)
}

/// A DMA source: words (bytes by default) in uncached memory.
pub struct Uncached<'a, W: Word = u8>(&'a [W]);

impl<'a, W: Word> Uncached<'a, W> {
    /// `buf`, if it is non-empty and wholly in uncached memory.
    pub fn new(buf: &'a [W]) -> Result<Self, NotUncached> {
        check_words(buf)?;
        Ok(Self(buf))
    }

    /// The words.
    pub fn as_slice(&self) -> &[W] {
        self.0
    }

    /// Its length in words.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// A DMA destination: words (bytes by default) in uncached memory.
pub struct UncachedMut<'a, W: Word = u8>(&'a mut [W]);

impl<'a, W: Word> UncachedMut<'a, W> {
    /// `buf`, if it is non-empty and wholly in uncached memory.
    pub fn new(buf: &'a mut [W]) -> Result<Self, NotUncached> {
        check_words(buf)?;
        Ok(Self(buf))
    }

    /// The words.
    pub fn as_slice(&self) -> &[W] {
        self.0
    }

    /// The words, writable.
    pub fn as_mut_slice(&mut self) -> &mut [W] {
        self.0
    }

    /// Its length in words.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The same buffer for a shorter borrow, so one buffer serves transfer after transfer.
    pub fn reborrow(&mut self) -> UncachedMut<'_, W> {
        UncachedMut(self.0)
    }

    /// Its first `len` words.
    ///
    /// # Panics
    /// If `len` is larger than the buffer: a transfer sized for more than the buffer holds would
    /// wait for data that never arrives.
    pub fn prefix(&mut self, len: usize) -> UncachedMut<'_, W> {
        assert!(len <= self.0.len(), "prefix of {} words from a {}-word buffer", len, self.0.len());
        UncachedMut(&mut self.0[..len])
    }

    /// The same words as a DMA source.
    pub fn as_source(&self) -> Uncached<'_, W> {
        Uncached(self.0)
    }

    /// The DMAC address of its first word (the mirror is a real bus alias).
    pub(crate) fn bus_addr(&self) -> u32 {
        self.0.as_ptr() as u32
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn a_word_buffer_checks_its_byte_range() {
        let mut words = [0u32; 4];
        let buf = UncachedMut::<u32>::new(&mut words).unwrap();
        assert_eq!(buf.len(), 4);
        assert_eq!(LAST_CHECKED_LEN.with(|l| l.get()), 16, "4 words checked as 16 bytes");
        assert_eq!(UncachedMut::<u32>::new(&mut []).err(), Some(NotUncached));
        assert_eq!(Uncached::<u32>::new(&[7u32; 2]).unwrap().as_slice(), &[7, 7]);
    }

    #[test]
    fn the_byte_default_still_names_sdhi_buffers() {
        let mut storage = [0u8; 512];
        let buf: UncachedMut<'_> = UncachedMut::new(&mut storage[..]).unwrap();
        assert_eq!(buf.len(), 512);
    }

    #[test]
    fn an_empty_buffer_is_refused() {
        assert_eq!(UncachedMut::<u8>::new(&mut []).err(), Some(NotUncached));
        assert_eq!(Uncached::<u8>::new(&[]).err(), Some(NotUncached));
    }

    #[test]
    #[should_panic(expected = "prefix")]
    fn a_prefix_longer_than_the_buffer_panics() {
        let mut storage = [0u8; 512];
        let mut buf = UncachedMut::new(&mut storage).unwrap();
        let _ = buf.prefix(1024);
    }

    #[test]
    fn reborrow_keeps_the_same_memory() {
        let mut storage = [0u8; 1024];
        let mut buf = UncachedMut::new(&mut storage).unwrap();
        let before = buf.as_slice().as_ptr();
        let again = buf.reborrow();
        assert_eq!(again.as_slice().as_ptr(), before);
        assert_eq!(again.len(), 1024);
    }
}
