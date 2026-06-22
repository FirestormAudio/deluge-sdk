//! Separate heap allocators for on-chip SRAM and external SDRAM.
//!
//! Two [`CsHeap`] instances – [`SRAM`] and [`SDRAM`] – provide distinct
//! allocation arenas, each protected by a critical-section lock (IRQ-safe).
//!
//! ## Algorithm
//! Each arena is a [TLSF (Two-Level Segregated Fit)][1] allocator from the
//! [`rlsf`] crate.  TLSF gives **O(1) allocation *and* deallocation** with a
//! bounded worst case — important here because every alloc/dealloc runs with
//! interrupts disabled (see *Locking*), so the time spent inside the allocator
//! is directly added to interrupt latency.  Its good-fit segregation also keeps
//! fragmentation low over a long session, which matters for the 64 MB SDRAM
//! arena that churns audio/sample buffers.
//!
//! [1]: http://www.gii.upv.es/tlsf/
//!
//! ## Usage
//!
//! 1. Call [`CsHeap::init`] once per allocator during startup, before any
//!    allocation in that arena.
//! 2. Pass a reference to the allocator where a nightly `Allocator` is
//!    expected:
//!
//! ```rust,ignore
//! #![feature(allocator_api)]
//! use deluge_alloc::{SRAM, SDRAM};
//!
//! let buf: Box<[u8; 4096], _> = Box::new_in([0u8; 4096], &SDRAM);
//! let vec: Vec<u32, _>        = Vec::new_in(&SRAM);
//! ```
//!
//! ## Locking
//! Both allocators use [`critical_section::with`], which disables interrupts
//! for the duration of each allocation or deallocation.  This prevents
//! deadlocks caused by IRQ handlers that also allocate, at the cost of a
//! brief IRQ-latency bump.

#![cfg_attr(target_os = "none", no_std)]
#![feature(allocator_api)]

use core::alloc::{AllocError, Allocator, Layout};
use core::cell::UnsafeCell;
use core::ptr::NonNull;

use rlsf::Tlsf;

// ---------------------------------------------------------------------------
// TLSF configuration
// ---------------------------------------------------------------------------
//
// The `FLLEN`/`SLLEN` const parameters trade metadata size against the maximum
// pool size and the worst-case internal fragmentation:
//
//   * max pool size      = GRANULARITY << FLLEN
//   * worst-case waste   ≈ allocation_size / SLLEN
//
// where `GRANULARITY = size_of::<usize>() * 4` (16 bytes on the 32-bit device).
// Both arenas share one config sized for the larger SDRAM arena; the extra
// metadata for the small SRAM arena is a couple of KiB, negligible against the
// 3 MB of on-chip RAM.
//
//   * FLLEN = 23  →  max pool 16 << 23 = 128 MiB  (covers the 64 MiB SDRAM)
//   * SLLEN = 16  →  ≤ ~6 % worst-case internal fragmentation
//
// `FLBitmap` must hold ≥ FLLEN bits and `SLBitmap` ≥ SLLEN bits; `SLLEN` must be
// a power of two. Bump `SLLEN` to 32 (and `SlBitmap` to `u32`) to halve the
// worst-case fragmentation at the cost of doubling the per-arena metadata.
type FlBitmap = u32;
type SlBitmap = u16;
const FLLEN: usize = 23;
const SLLEN: usize = 16;

type Arena = Tlsf<'static, FlBitmap, SlBitmap, FLLEN, SLLEN>;

// ---------------------------------------------------------------------------
// CsHeap — critical-section-protected heap
// ---------------------------------------------------------------------------

/// Mutable state behind the critical-section lock.
struct HeapState {
    tlsf: Arena,
    /// Total bytes the arena manages (as accepted by the TLSF pool).
    size: usize,
    /// Sum of the `Layout::size()` of all live allocations.
    used: usize,
}

/// A heap allocator guarded by a `critical_section` lock.
///
/// Implements the nightly [`Allocator`] trait so it can be used with
/// `Box::new_in`, `Vec::new_in`, and similar APIs.
///
/// # Safety invariant
/// [`init`][Self::init] must be called exactly once before any allocation.
pub struct CsHeap(UnsafeCell<HeapState>);

// SAFETY: All accesses go through `critical_section::with`, which disables
// IRQs on single-core targets, providing the required mutual exclusion.
unsafe impl Sync for CsHeap {}
unsafe impl Send for CsHeap {}

impl CsHeap {
    /// Creates a new, uninitialised heap.  Must be initialised with
    /// [`init`][Self::init] before any allocation attempt.
    pub const fn empty() -> Self {
        Self(UnsafeCell::new(HeapState {
            tlsf: Tlsf::new(),
            size: 0,
            used: 0,
        }))
    }

    /// Initialises the heap to cover `start..start+size`.
    ///
    /// # Safety
    /// - `start..start+size` must be a valid, exclusively-owned, writable
    ///   memory region for the lifetime of the program.
    /// - Must be called exactly once per instance, before the first
    ///   allocation.
    pub unsafe fn init(&self, start: *mut u8, size: usize) {
        critical_section::with(|_| {
            // SAFETY: exclusive via critical section; caller upholds the rest.
            let state = unsafe { &mut *self.0.get() };
            // SAFETY: caller guarantees `start` is non-null and the region is
            // valid, exclusively owned, and lives for the whole program — so a
            // `'static` block is sound.
            let block =
                NonNull::slice_from_raw_parts(unsafe { NonNull::new_unchecked(start) }, size);
            // SAFETY: the block is exclusively owned for `'static` per the above.
            let accepted = unsafe { state.tlsf.insert_free_block_ptr(block) };
            state.size = accepted.map_or(0, |n| n.get());
        });
    }

    /// Returns the number of bytes currently in use (sum of live allocation
    /// sizes). Useful as a high-water diagnostic.
    pub fn used(&self) -> usize {
        critical_section::with(|_| unsafe { (*self.0.get()).used })
    }

    /// Returns the total number of bytes managed by this allocator.
    pub fn size(&self) -> usize {
        critical_section::with(|_| unsafe { (*self.0.get()).size })
    }

    /// Returns the number of free bytes remaining (`size - used`).
    ///
    /// Note: this is the *accounting* free total, not the largest allocatable
    /// block — it does not reflect fragmentation.
    pub fn free(&self) -> usize {
        critical_section::with(|_| unsafe {
            let s = &*self.0.get();
            s.size - s.used
        })
    }
}

unsafe impl Allocator for CsHeap {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        critical_section::with(|_| {
            // SAFETY: exclusive via critical section.
            let state = unsafe { &mut *self.0.get() };
            match state.tlsf.allocate(layout) {
                Some(ptr) => {
                    state.used += layout.size();
                    Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
                }
                None => Err(AllocError),
            }
        })
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        critical_section::with(|_| {
            // SAFETY: exclusive via critical section.
            let state = unsafe { &mut *self.0.get() };
            // SAFETY: caller guarantees `ptr` came from this allocator with a
            // matching layout, so `layout.align()` is the original alignment.
            unsafe { state.tlsf.deallocate(ptr, layout.align()) };
            state.used = state.used.saturating_sub(layout.size());
        });
    }
}

// ---------------------------------------------------------------------------
// Public allocator instances
// ---------------------------------------------------------------------------

/// Allocator backed by on-chip SRAM.
///
/// Must be initialised with
/// `unsafe { SRAM.init(heap_start, heap_size) }` before first use.
/// The canonical heap window is from the end of the firmware image
/// (`__sram_heap_start`) to just below the RTT/stack reservation
/// (`__sram_heap_end`).
pub static SRAM: CsHeap = CsHeap::empty();

/// Allocator backed by the external 64 MB SDRAM (CS3, 0x0C00_0000–0x0FFF_FFFF).
///
/// Must be initialised *after* the SDRAM controller has been brought up and the
/// SDRAM window is accessible:
/// ```rust,ignore
/// unsafe { SDRAM.init(0x0C00_0000 as *mut u8, 64 * 1024 * 1024) }
/// ```
///
/// **Calling `allocate` before `init` will return `AllocError`** (which causes
/// `Box::new_in` / `Vec::try_reserve` to panic). There is no silent UB, but
/// any allocation attempt before `init` will fail at runtime.
pub static SDRAM: CsHeap = CsHeap::empty();

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use core::alloc::Layout;

    #[test]
    fn empty_heap_reports_zero() {
        let h = CsHeap::empty();
        assert_eq!(h.size(), 0);
        assert_eq!(h.used(), 0);
        assert_eq!(h.free(), 0);
    }

    #[test]
    fn init_then_allocate_and_free_tracks_usage() {
        // Back the arena with an owned, sufficiently-aligned buffer.
        let mut buf = vec![0u8; 8192];
        let start = buf.as_mut_ptr();
        let h = CsHeap::empty();
        unsafe { h.init(start, buf.len()) };

        // TLSF may trim a few bytes for alignment / its sentinel, so the
        // managed size is close to but not necessarily equal to the request.
        let size = h.size();
        assert!(size > 0 && size <= 8192, "managed size within the arena");
        assert_eq!(h.used(), 0);
        assert_eq!(h.free(), size);

        let layout = Layout::from_size_align(256, 8).unwrap();
        let p = h.allocate(layout).expect("allocation within an 8 KB arena");
        assert_eq!(h.used(), 256, "used grows by the requested size");
        assert_eq!(h.free(), size - 256);

        // Returned region is inside the arena.
        let addr = p.as_ptr() as *mut u8 as usize;
        let base = start as usize;
        assert!(addr >= base && addr < base + 8192);

        unsafe { h.deallocate(p.cast::<u8>(), layout) };
        assert_eq!(h.used(), 0, "freeing returns the arena to empty");
    }

    #[test]
    fn allocation_failure_when_arena_exhausted() {
        let mut buf = vec![0u8; 1024];
        let h = CsHeap::empty();
        unsafe { h.init(buf.as_mut_ptr(), buf.len()) };
        // Requesting far more than the arena holds must fail, not panic/UB.
        let too_big = Layout::from_size_align(64 * 1024, 8).unwrap();
        assert!(h.allocate(too_big).is_err());
    }
}
