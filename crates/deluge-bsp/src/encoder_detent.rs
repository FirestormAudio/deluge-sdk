//! Pure quadrature detent accumulation, shared by the (bare-metal-only)
//! [`crate::encoder`] IRQ driver.
//!
//! The encoder module reads edge deltas from ISR-updated atomics (hardware), so
//! it is `#[cfg(target_os = "none")]`. The edge→detent accumulation is pure, so
//! it lives here and unit-tests on the host.

/// Fold an edge `delta` into `acc` and emit whole detents.
///
/// A detent is one quadrature cycle, four edges. The decoder confirms each edge one edge late, so
/// a knob resting on a click has only three of its four edges in: a detent is emitted once more
/// than two edges have accumulated, i.e. at the nearest click. The leftover (−2..=2) stays in
/// `acc` for the next call.
#[inline]
pub fn accumulate_detents(delta: i8, acc: &mut i8) -> i8 {
    *acc = acc.saturating_add(delta);

    let mut detents = 0;
    while *acc > 2 {
        *acc -= 4;
        detents += 1;
    }
    while *acc < -2 {
        *acc += 4;
        detents -= 1;
    }
    detents
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn three_confirmed_edges_make_a_detent() {
        let mut acc = 0;
        assert_eq!(accumulate_detents(2, &mut acc), 0, "half a click: no detent yet");
        assert_eq!(acc, 2);
        assert_eq!(
            accumulate_detents(1, &mut acc),
            1,
            "the third edge is the nearest click"
        );
        assert_eq!(acc, -1, "the fourth edge, confirmed later, completes it");
        assert_eq!(accumulate_detents(1, &mut acc), 0);
        assert_eq!(acc, 0);
    }

    #[test]
    fn negative_direction_symmetric() {
        let mut acc = 0;
        assert_eq!(accumulate_detents(-3, &mut acc), -1);
        assert_eq!(accumulate_detents(-1, &mut acc), 0);
        assert_eq!(acc, 0);
    }

    #[test]
    fn batch_of_edges_yields_multiple_detents_with_remainder() {
        let mut acc = 0;
        // 9 edges in one go: 2 detents (8 edges), 1 edge left over.
        assert_eq!(accumulate_detents(9, &mut acc), 2);
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
        accumulate_detents(2, &mut acc);
        assert_eq!(accumulate_detents(-2, &mut acc), 0, "reversal nets to zero");
        assert_eq!(acc, 0);
    }

    #[test]
    fn saturating_add_does_not_panic_on_extreme_delta() {
        let mut acc = i8::MAX;
        let d = accumulate_detents(i8::MAX, &mut acc);
        assert!(d > 0);
        assert!((-2..=2).contains(&acc));
    }
}
