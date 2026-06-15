//! Pure memory-system math shared by the (bare-metal-only) `cache` and `mmu`
//! modules: L1 cache-line range planning and the MMU section-descriptor table.
//!
//! `cache` and `mmu` are `#[cfg(target_os = "none")]` (they issue CP15 / asm),
//! so their arithmetic could not otherwise be unit-tested. This module has no
//! hardware dependencies, so it compiles and tests on the host/QEMU target and
//! is the single source of truth both gated modules call into.
//!
//! The MMU area table mirrors the RZ/A1L address map (HW manual ch.5,
//! "Address space"); the cache-line size is the Cortex-A9 fixed 32-byte line.

// ===========================================================================
// L1 D-cache line range planning
// ===========================================================================

/// Cortex-A9 L1 D-cache line size in bytes (fixed for this SoC).
pub const L1_LINE_BYTES: usize = 32;

const LINE_MASK: usize = L1_LINE_BYTES - 1;

/// Round `addr` down to its cache-line boundary.
#[inline]
pub const fn line_floor(addr: usize) -> usize {
    addr & !LINE_MASK
}

/// Iterator over the aligned addresses of every cache line that overlaps
/// `[floor(start), end)`, stepping by [`L1_LINE_BYTES`].
///
/// This is the loop shared by `dma_clean_range` / `dma_clean_inv_range` and the
/// full-line portion of [`dma_inv_plan`].
#[derive(Clone, Copy, Debug)]
pub struct CacheLines {
    next: usize,
    end: usize,
}

impl Iterator for CacheLines {
    type Item = usize;
    #[inline]
    fn next(&mut self) -> Option<usize> {
        if self.next < self.end {
            let a = self.next;
            self.next += L1_LINE_BYTES;
            Some(a)
        } else {
            None
        }
    }
}

/// Cache lines covering `[floor(start), end)` (the clean / clean-invalidate loop).
#[inline]
pub fn cache_lines(start: usize, end: usize) -> CacheLines {
    CacheLines {
        next: line_floor(start),
        end,
    }
}

/// Plan for an *invalidate* over `[start, end)` (DMA read).
///
/// A plain invalidate of a partial boundary line would discard dirty bytes
/// outside the range, so the partial first/last lines are **clean+invalidated**
/// instead; the fully-covered interior lines are plain-invalidated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DmaInvPlan {
    /// Clean+invalidate this line first iff `start` is not line-aligned.
    pub clean_inv_first: Option<usize>,
    /// Clean+invalidate this line iff `end` is not line-aligned.
    pub clean_inv_last: Option<usize>,
    /// Plain-invalidate the lines in `[inv_first, inv_last)` (see [`inv_lines`]).
    ///
    /// [`inv_lines`]: DmaInvPlan::inv_lines
    pub inv_first: usize,
    pub inv_last: usize,
}

impl DmaInvPlan {
    /// The interior full lines to plain-invalidate.
    #[inline]
    pub fn inv_lines(&self) -> CacheLines {
        CacheLines {
            next: self.inv_first,
            end: self.inv_last,
        }
    }
}

/// Build the invalidate plan for `[start, end)`.
#[inline]
pub fn dma_inv_plan(start: usize, end: usize) -> DmaInvPlan {
    let first = line_floor(start);
    let last = line_floor(end);
    DmaInvPlan {
        clean_inv_first: if start & LINE_MASK != 0 { Some(first) } else { None },
        clean_inv_last: if end & LINE_MASK != 0 { Some(last) } else { None },
        inv_first: first,
        inv_last: last,
    }
}

// ===========================================================================
// MMU L1 section table (1 MB short-descriptor format)
// ===========================================================================

/// Strongly-ordered memory (device I/O). TEX=000, C=0, B=0; section bits 0b10.
pub const PARA_STRONGLY_ORDERED: u32 = 0x0DE2;
/// Normal non-cached. TEX=001, C=0, B=0 → inner/outer non-cacheable.
pub const PARA_NORMAL_NOT_CACHE: u32 = 0x1DE2;
/// Normal write-back, write-allocate (fully cached). TEX=001, C=1, B=1.
pub const PARA_NORMAL_CACHE: u32 = 0x1DEE;

/// Area table — `(size_in_mb, attribute)` low → high address, mirroring the
/// RZ/A1L address map (HW manual ch.5). The sizes sum to 4096 (a full 4 GB of
/// 1 MB sections); see [`AREAS_FILL_4GB`] in the tests.
pub const AREAS: &[(u32, u32)] = &[
    (128, PARA_NORMAL_CACHE),      // area  0  CS0/CS1 NOR flash      0x0000_0000
    (128, PARA_NORMAL_CACHE),      // area  1  CS2/CS3 SDRAM          0x0800_0000
    (128, PARA_STRONGLY_ORDERED),  // area  2  CS4/CS5                0x1000_0000
    (128, PARA_NORMAL_CACHE),      // area  3  SPI / SPI2 serial      0x1800_0000
    (10, PARA_NORMAL_CACHE),       // area  4  internal SRAM          0x2000_0000
    (502, PARA_STRONGLY_ORDERED),  // area  5  I/O area 1             0x20A0_0000
    (128, PARA_NORMAL_NOT_CACHE),  // area  6  CS0/CS1 mirror         0x4000_0000
    (128, PARA_NORMAL_NOT_CACHE),  // area  7  CS2/CS3 mirror (SDRAM) 0x4800_0000
    (128, PARA_STRONGLY_ORDERED),  // area  8  CS4/CS5 mirror         0x5000_0000
    (128, PARA_NORMAL_NOT_CACHE),  // area  9  SPI mirror             0x5800_0000
    (10, PARA_NORMAL_NOT_CACHE),   // area 10  SRAM mirror            0x6000_0000
    (2550, PARA_STRONGLY_ORDERED), // area 11  I/O area 2             0x60A0_0000
];

/// Encode a 1 MB section descriptor for L1-section index `index`
/// (`index == physical_address >> 20`) with attribute bits `attr`.
///
/// `(index << 20) | (attr & 0xF_FFFF)` — a flat VA=PA section entry.
#[inline]
pub const fn ttb_section_descriptor(index: u32, attr: u32) -> u32 {
    (index << 20) | (attr & 0x000F_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    // ---- cache-line planning ----------------------------------------------

    #[test]
    fn line_floor_rounds_down() {
        assert_eq!(line_floor(0), 0);
        assert_eq!(line_floor(31), 0);
        assert_eq!(line_floor(32), 32);
        assert_eq!(line_floor(33), 32);
        assert_eq!(line_floor(0x1007), 0x1000);
    }

    #[test]
    fn cache_lines_aligned_range() {
        let v: Vec<usize> = cache_lines(0, 64).collect();
        assert_eq!(v, [0, 32]);
    }

    #[test]
    fn cache_lines_covers_line_containing_end_minus_one() {
        // [0, 65) touches lines 0, 32, 64 (byte 64 is in the third line).
        let v: Vec<usize> = cache_lines(0, 65).collect();
        assert_eq!(v, [0, 32, 64]);
    }

    #[test]
    fn cache_lines_unaligned_start_floors() {
        // Starting at byte 40 still begins at line 32.
        let v: Vec<usize> = cache_lines(40, 96).collect();
        assert_eq!(v, [32, 64]);
    }

    #[test]
    fn cache_lines_single_line() {
        let v: Vec<usize> = cache_lines(0, 1).collect();
        assert_eq!(v, [0]);
    }

    // ---- invalidate plan ---------------------------------------------------

    #[test]
    fn inv_plan_fully_aligned_has_no_boundary_cleans() {
        let p = dma_inv_plan(0, 96);
        assert_eq!(p.clean_inv_first, None);
        assert_eq!(p.clean_inv_last, None);
        let v: Vec<usize> = p.inv_lines().collect();
        assert_eq!(v, [0, 32, 64]); // [0, 96) = lines 0,32,64
    }

    #[test]
    fn inv_plan_unaligned_start_cleans_first_partial_line() {
        // [40, 96): line 32 is partially in-range → clean+inv, not plain inv-only.
        let p = dma_inv_plan(40, 96);
        assert_eq!(p.clean_inv_first, Some(32));
        assert_eq!(p.clean_inv_last, None);
        // The interior plain-invalidate loop still starts at floor(start).
        assert_eq!((p.inv_first, p.inv_last), (32, 96));
    }

    #[test]
    fn inv_plan_unaligned_end_cleans_last_partial_line() {
        // [0, 100): floor(end)=96; byte 96..100 partial → clean+inv line 96,
        // which the interior loop (stops at < 96) does not re-invalidate.
        let p = dma_inv_plan(0, 100);
        assert_eq!(p.clean_inv_first, None);
        assert_eq!(p.clean_inv_last, Some(96));
        assert_eq!((p.inv_first, p.inv_last), (0, 96));
        let v: Vec<usize> = p.inv_lines().collect();
        assert_eq!(v, [0, 32, 64]);
    }

    #[test]
    fn inv_plan_both_unaligned() {
        let p = dma_inv_plan(40, 100);
        assert_eq!(p.clean_inv_first, Some(32));
        assert_eq!(p.clean_inv_last, Some(96));
        assert_eq!((p.inv_first, p.inv_last), (32, 96));
    }

    #[test]
    fn inv_plan_sub_line_range() {
        // [40, 50): both ends in line 32; clean+inv that one line, no interior.
        let p = dma_inv_plan(40, 50);
        assert_eq!(p.clean_inv_first, Some(32));
        assert_eq!(p.clean_inv_last, Some(32));
        assert_eq!((p.inv_first, p.inv_last), (32, 32));
        assert_eq!(p.inv_lines().count(), 0);
    }

    // ---- MMU section table -------------------------------------------------

    #[test]
    fn ttb_descriptor_encoding() {
        // Flat VA=PA: section 0x0C0 (=0x0C00_0000) cached.
        assert_eq!(ttb_section_descriptor(0xC0, PARA_NORMAL_CACHE), 0x0C00_0000 | 0x1DEE);
        // Index is shifted by 20; attr is masked to the low 20 bits.
        assert_eq!(ttb_section_descriptor(1, 0) >> 20, 1);
        assert_eq!(ttb_section_descriptor(0, 0xFFF0_0000), 0, "high attr bits dropped");
    }

    #[test]
    fn all_para_attrs_mark_a_section_entry() {
        // Short-descriptor section entries carry bits[1:0] = 0b10.
        for attr in [PARA_STRONGLY_ORDERED, PARA_NORMAL_NOT_CACHE, PARA_NORMAL_CACHE] {
            assert_eq!(attr & 0b11, 0b10, "{attr:#x} must be a section descriptor");
        }
    }

    #[test]
    fn areas_fill_exactly_4gb() {
        // 4096 sections × 1 MB = the full 32-bit address space.
        let total: u32 = AREAS.iter().map(|&(mb, _)| mb).sum();
        assert_eq!(total, 4096, "AREAS must map every 1 MB section");
    }

    /// Walk the area table to find the attribute assigned to a physical address.
    fn attr_at(pa: u32) -> u32 {
        let section = pa >> 20;
        let mut idx = 0u32;
        for &(mb, attr) in AREAS {
            if section < idx + mb {
                return attr;
            }
            idx += mb;
        }
        unreachable!("address {pa:#x} is outside the 4 GB map");
    }

    #[test]
    fn area_attributes_match_the_rza1l_address_map() {
        // Boundaries and cacheability per HW manual ch.5 (validated against the
        // TRM address-space table).
        assert_eq!(attr_at(0x0000_0000), PARA_NORMAL_CACHE, "CS0 NOR");
        assert_eq!(attr_at(0x0C00_0000), PARA_NORMAL_CACHE, "SDRAM (CS3) cached");
        assert_eq!(attr_at(0x1000_0000), PARA_STRONGLY_ORDERED, "CS4/CS5 device");
        assert_eq!(attr_at(0x1800_0000), PARA_NORMAL_CACHE, "SPI flash cached");
        assert_eq!(attr_at(0x2000_0000), PARA_NORMAL_CACHE, "on-chip SRAM cached");
        // Mirror windows at +0x4000_0000 are normal-non-cacheable (or device).
        assert_eq!(attr_at(0x4C00_0000), PARA_NORMAL_NOT_CACHE, "SDRAM mirror uncached");
        assert_eq!(attr_at(0x6002_0000), PARA_NORMAL_NOT_CACHE, "SRAM mirror uncached");
        // Peripherals (e.g. DMAC @ 0xE820_0000) are strongly-ordered.
        assert_eq!(attr_at(0xE820_0000), PARA_STRONGLY_ORDERED, "peripheral device");
    }
}
