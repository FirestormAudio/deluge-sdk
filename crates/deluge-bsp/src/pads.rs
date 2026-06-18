// ---------------------------------------------------------------------------
// Shared pad state — atomic bit fields (no mutex, no allocation)
// ---------------------------------------------------------------------------
//
// 144 pads packed into 5 × u32 (160 bits; high 16 bits of word 4 unused).
// AtomicU32::fetch_xor provides lock-free single-bit toggle.

use core::sync::atomic::{AtomicU32, Ordering};

pub static PAD_BITS: [AtomicU32; 5] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// Return `true` if pad `id` (0–143) is currently lit.
#[inline]
pub fn pad_get(id: u8) -> bool {
    (PAD_BITS[id as usize / 32].load(Ordering::Relaxed) >> (id % 32)) & 1 != 0
}

/// Toggle pad `id`; return `true` if it is now lit.
#[inline]
pub fn pad_toggle(id: u8) -> bool {
    let old = PAD_BITS[id as usize / 32].fetch_xor(1 << (id % 32), Ordering::Relaxed);
    (old >> (id % 32)) & 1 == 0
}

/// Set every pad to `val` (true = all lit, false = all dark).
pub fn pad_set_all(val: bool) {
    let fill = if val { !0u32 } else { 0u32 };
    for slot in PAD_BITS[..4].iter() {
        slot.store(fill, Ordering::Relaxed);
    }
    PAD_BITS[4].store(fill & 0x0000_FFFF, Ordering::Relaxed);
}

/// Flip every pad's lit state.
pub fn pad_invert_all() {
    for slot in PAD_BITS[..4].iter() {
        slot.fetch_xor(!0u32, Ordering::Relaxed);
    }
    PAD_BITS[4].fetch_xor(0x0000_FFFF, Ordering::Relaxed);
}

/// Convert display grid position (x ∈ 0..18, y ∈ 0..8) to a pad ID 0–143.
///
/// `y` is in pad coordinates where y=0 is the bottom row (lower-left origin).
/// Inverse of [`crate::pic::pad_coords`].
#[inline]
pub fn pad_id_from_xy(x: u8, y: u8) -> u8 {
    if x.is_multiple_of(2) {
        y * 9 + x / 2
    } else {
        (y + 8) * 9 + (x - 1) / 2
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn xy_to_id_known_points() {
        // Even columns fill the low half (ids 0..72); odd columns the high half.
        assert_eq!(pad_id_from_xy(0, 0), 0);
        assert_eq!(pad_id_from_xy(2, 0), 1);
        assert_eq!(pad_id_from_xy(16, 0), 8);
        assert_eq!(pad_id_from_xy(1, 0), 72);
        assert_eq!(pad_id_from_xy(17, 7), 143);
    }

    #[test]
    fn xy_to_id_is_inverse_of_pic_pad_coords() {
        // Every grid cell must round-trip through both mappings.
        for y in 0..8u8 {
            for x in 0..18u8 {
                let id = pad_id_from_xy(x, y);
                assert!(id < 144, "id in range for ({x},{y})");
                assert_eq!(crate::pic::pad_coords(id), (x, y), "round-trip ({x},{y})");
            }
        }
    }

    #[test]
    fn xy_to_id_is_a_bijection_over_the_grid() {
        let mut seen = [false; 144];
        for y in 0..8u8 {
            for x in 0..18u8 {
                let id = pad_id_from_xy(x, y) as usize;
                assert!(!seen[id], "id {id} produced twice");
                seen[id] = true;
            }
        }
        assert!(seen.iter().all(|&s| s), "all 144 ids are covered");
    }

    /// Single owner of the shared `PAD_BITS` state (other tests must not touch
    /// it, so the parallel test runner can't race here).
    #[test]
    fn pad_state_set_get_invert() {
        pad_set_all(false);
        assert!(!pad_get(0) && !pad_get(143));
        assert!(pad_toggle(5), "toggle lights the pad");
        assert!(pad_get(5));
        assert!(!pad_toggle(5), "toggle again clears it");

        pad_set_all(true);
        assert!(pad_get(0) && pad_get(143));
        // The top 16 bits of the last word are unused and must stay clear.
        assert_eq!(PAD_BITS[4].load(Ordering::Relaxed) & 0xFFFF_0000, 0);

        pad_invert_all();
        assert!(!pad_get(0) && !pad_get(143));
        assert_eq!(PAD_BITS[4].load(Ordering::Relaxed) & 0xFFFF_0000, 0);

        pad_set_all(false); // leave clean
    }
}
