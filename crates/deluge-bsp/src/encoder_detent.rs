//! Pure quadrature detent accumulation, shared by the (bare-metal-only)
//! [`crate::encoder`] IRQ driver.
//!
//! The encoder module reads edge deltas from ISR-updated atomics (hardware), so
//! it is `#[cfg(target_os = "none")]`. The edge→detent accumulation is pure, so
//! it lives here and unit-tests on the host.

/// Fold an edge `delta` into `acc` and emit whole detents.
///
/// Each encoder IRQ contributes ±1 per edge; two accumulated edges in the same
/// direction make one detent (matching the Deluge's physical click spacing).
/// The leftover (0 or ±1) stays in `acc` for the next call.
#[inline]
pub fn accumulate_detents(delta: i8, acc: &mut i8) -> i8 {
    *acc = acc.saturating_add(delta);

    let mut detents = 0;
    while *acc > 1 {
        *acc -= 2;
        detents += 1;
    }
    while *acc < -1 {
        *acc += 2;
        detents -= 1;
    }
    detents
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn two_edges_make_one_detent() {
        let mut acc = 0;
        assert_eq!(accumulate_detents(1, &mut acc), 0, "one edge: no detent yet");
        assert_eq!(acc, 1, "leftover edge retained");
        assert_eq!(accumulate_detents(1, &mut acc), 1, "second edge completes a detent");
        assert_eq!(acc, 0);
    }

    #[test]
    fn negative_direction_symmetric() {
        let mut acc = 0;
        assert_eq!(accumulate_detents(-1, &mut acc), 0);
        assert_eq!(accumulate_detents(-1, &mut acc), -1);
        assert_eq!(acc, 0);
    }

    #[test]
    fn batch_of_edges_yields_multiple_detents_with_remainder() {
        let mut acc = 0;
        // 5 edges in one go → 2 detents, 1 edge left over.
        assert_eq!(accumulate_detents(5, &mut acc), 2);
        assert_eq!(acc, 1);
    }

    #[test]
    fn zero_delta_is_no_op() {
        let mut acc = 1;
        assert_eq!(accumulate_detents(0, &mut acc), 0);
        assert_eq!(acc, 1, "leftover preserved across an empty poll");
    }

    #[test]
    fn direction_reversal_cancels_leftover() {
        let mut acc = 0;
        accumulate_detents(1, &mut acc); // acc = 1
        assert_eq!(accumulate_detents(-1, &mut acc), 0, "reversal nets to zero");
        assert_eq!(acc, 0);
    }

    #[test]
    fn saturating_add_does_not_panic_on_extreme_delta() {
        let mut acc = i8::MAX;
        // Must not overflow-panic; just drains as many detents as possible.
        let d = accumulate_detents(i8::MAX, &mut acc);
        assert!(d > 0);
        assert!((-1..=1).contains(&acc));
    }
}
