//! Quadrature decoding of the front-panel encoders' two pins, with glitch rejection.
//!
//! Pure, so it unit-tests on the host; [`crate::encoder`] feeds it from the A-pin interrupt and a
//! periodic poll.
//!
//! A complete positive cycle follows `AB` = 00 → 01 → 11 → 10 → 00; the reverse is negative
//! movement. Each one-bit transition is one signed edge, four per cycle.
//!
//! An edge is only reported once the *other* pin has also changed, as the 1.x firmware's
//! `Encoder::read()` did: a single pin bouncing or hovering around its threshold never reports
//! movement, at the cost of each edge being confirmed one edge late.
//!
//! Only A has an interrupt. The poll catches B-only transitions; when both pins changed between
//! two looks, the A interrupt that is necessarily pending (or running) tells the order: B first.

/// Decoder state for one encoder.
#[derive(Clone, Copy, Debug, Default)]
pub struct QuadratureDecoder {
    /// Last state whose edge has been confirmed and reported.
    committed: u8,
    /// Most recently observed state.
    sampled: u8,
}

impl QuadratureDecoder {
    /// A decoder at the pins' current levels, which are not reported as movement.
    pub const fn new(a: bool, b: bool) -> Self {
        let state = encode(a, b);
        Self {
            committed: state,
            sampled: state,
        }
    }

    /// Process a polled sample. Returns the signed number of edges it confirms.
    ///
    /// If both pins changed since the last sample the order is unknown, so the sample is ignored:
    /// A changed, so its interrupt is pending and [`Self::observe_a_edge`] will resolve it.
    pub fn observe(&mut self, a: bool, b: bool, invert: bool) -> i8 {
        let next = encode(a, b);
        if self.sampled ^ next == 0b11 {
            return 0;
        }
        orient(self.step(next), invert)
    }

    /// Process a sample taken in response to an A-pin edge interrupt. Returns the signed number
    /// of edges it confirms.
    ///
    /// If both pins changed since the last sample, the A interrupt means the missed B transition
    /// came first, so both are replayed in that order.
    pub fn observe_a_edge(&mut self, a: bool, b: bool, invert: bool) -> i8 {
        let next = encode(a, b);
        let mut movement = 0;
        if self.sampled ^ next == 0b11 {
            movement += self.step(self.sampled ^ 0b01);
        }
        movement += self.step(next);
        orient(movement, invert)
    }

    /// Advance by one sample at most one bit away from `sampled`. Returns ±1 when the sample
    /// confirms the previous edge, otherwise 0.
    fn step(&mut self, next: u8) -> i8 {
        if self.committed ^ next != 0b11 {
            // Back where we were last confirmed (a bounce), or one pin away from it
            // (unconfirmed). Neither is movement.
            self.sampled = next;
            return 0;
        }
        // Both pins now differ from the confirmed state, so the edge to `sampled` was real.
        let movement = transition(self.committed, self.sampled);
        self.committed = self.sampled;
        self.sampled = next;
        movement
    }
}

/// Pack the pin levels into the state order `AB`.
const fn encode(a: bool, b: bool) -> u8 {
    ((a as u8) << 1) | b as u8
}

const fn orient(movement: i8, invert: bool) -> i8 {
    if invert { -movement } else { movement }
}

/// The movement from one state to an adjacent one: +1, −1, or 0 for no change or a two-bit jump.
const fn transition(from: u8, to: u8) -> i8 {
    const TABLE: [i8; 16] = [0, 1, -1, 0, -1, 0, 0, 1, 1, 0, 0, -1, 0, -1, 1, 0];
    TABLE[((from << 2) | to) as usize]
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// Pin levels `(a, b)`.
    type Pins = (bool, bool);

    const S00: Pins = (false, false);
    const S01: Pins = (false, true);
    const S11: Pins = (true, true);
    const S10: Pins = (true, false);

    /// Feeds `states` (the first is the initial level) as the hardware would: an A change through
    /// the interrupt path, a B-only change through the poll when `poll_b`. Returns the edges.
    fn run(states: &[Pins], poll_b: bool, invert: bool) -> i32 {
        let mut decoder = QuadratureDecoder::new(states[0].0, states[0].1);
        let mut total = 0i32;
        for pair in states.windows(2) {
            let (prev, next) = (pair[0], pair[1]);
            if next.0 != prev.0 {
                total += i32::from(decoder.observe_a_edge(next.0, next.1, invert));
            } else if poll_b {
                total += i32::from(decoder.observe(next.0, next.1, invert));
            }
        }
        total
    }

    #[test]
    fn complete_positive_cycle_confirms_all_but_last_edge() {
        assert_eq!(run(&[S00, S01, S11, S10, S00], true, false), 3);
    }

    #[test]
    fn complete_negative_cycle_confirms_all_but_last_edge() {
        assert_eq!(run(&[S00, S10, S11, S01, S00], true, false), -3);
    }

    #[test]
    fn next_edge_confirms_fourth_edge_of_cycle() {
        assert_eq!(run(&[S00, S01, S11, S10, S00, S01], true, false), 4);
    }

    #[test]
    fn inverted_wiring_reverses_direction() {
        assert_eq!(run(&[S00, S01, S11, S10, S00], true, true), -3);
    }

    #[test]
    fn poll_confirms_slow_b_transition() {
        // 00 -> 01 is a B-only edge (unconfirmed); A rising to 11 confirms it, then B falling to 10
        // is seen only by the poll and confirms the A edge.
        assert_eq!(run(&[S00, S01, S11, S10], true, false), 2);
        assert_eq!(run(&[S00, S01, S11, S10], false, false), 1);
    }

    #[test]
    fn single_edge_is_not_reported_until_confirmed() {
        let mut decoder = QuadratureDecoder::new(false, false);
        assert_eq!(decoder.observe(false, true, false), 0);
        assert_eq!(decoder.observe_a_edge(true, true, false), 1);
    }

    #[test]
    fn b_pin_chatter_does_not_report_movement() {
        assert_eq!(run(&[S00, S01, S00, S01, S00, S01, S00], true, false), 0);
    }

    #[test]
    fn a_pin_chatter_does_not_report_movement() {
        assert_eq!(run(&[S00, S10, S00, S10, S00, S10, S00], true, false), 0);
    }

    #[test]
    fn chatter_after_movement_does_not_change_count() {
        assert_eq!(
            run(&[S00, S01, S11, S10, S11, S10, S11, S10], true, false),
            2
        );
    }

    #[test]
    fn reversal_nets_to_zero_once_confirmed() {
        assert_eq!(run(&[S00, S01, S11, S01, S00, S10], true, false), 0);
    }

    #[test]
    fn delayed_a_interrupt_recovers_unpolled_b_edge() {
        // From 00, B and then A rose before anything sampled: the A interrupt replays B first.
        let mut decoder = QuadratureDecoder::new(false, false);
        assert_eq!(decoder.observe_a_edge(true, true, false), 1);
    }

    #[test]
    fn poll_ignores_a_sample_where_both_pins_changed() {
        let mut decoder = QuadratureDecoder::new(false, false);
        assert_eq!(decoder.observe(true, true, false), 0);
        // The pending A interrupt then resolves it.
        assert_eq!(decoder.observe_a_edge(true, true, false), 1);
    }

    #[test]
    fn fast_complete_cycle_matches_polled_cycle() {
        // A spin too fast for the poll: only A interrupts, each seeing B already moved.
        let mut decoder = QuadratureDecoder::new(false, false);
        let mut total = 0;
        for _ in 0..4 {
            total += i32::from(decoder.observe_a_edge(true, true, false)); // 00 → (01) → 11
            total += i32::from(decoder.observe_a_edge(false, false, false)); // 11 → (10) → 00
        }
        assert_eq!(
            total,
            run(
                &[
                    S00, S01, S11, S10, S00, S01, S11, S10, S00, S01, S11, S10, S00, S01, S11, S10,
                    S00
                ],
                true,
                false
            )
        );
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        /// One step of a real turn: forward or back by one state, with some bounce on the way.
        fn walk() -> impl Strategy<Value = Vec<(bool, u8)>> {
            // (forward, bounces): each step moves one quadrature state; `bounces` toggles the
            // moving pin back and forth that many extra times before it settles.
            proptest::collection::vec((any::<bool>(), 0u8..3), 1..200)
        }

        /// The quadrature sequence `AB`, indexed by position mod 4.
        const SEQUENCE: [Pins; 4] = [S00, S01, S11, S10];

        fn states_for(steps: &[(bool, u8)]) -> (Vec<Pins>, i32) {
            let mut position: i32 = 0;
            let mut states = vec![SEQUENCE[0]];
            for &(forward, bounces) in steps {
                let from = SEQUENCE[position.rem_euclid(4) as usize];
                position += if forward { 1 } else { -1 };
                let to = SEQUENCE[position.rem_euclid(4) as usize];
                for _ in 0..bounces {
                    states.push(to);
                    states.push(from);
                }
                states.push(to);
            }
            (states, position)
        }

        proptest! {
            #[test]
            fn polled_decoding_tracks_the_position_within_one_edge(steps in walk()) {
                let (states, position) = states_for(&steps);
                let decoded = run(&states, true, false);
                prop_assert!((decoded - position).abs() <= 1, "decoded {decoded}, moved {position}");
            }

            #[test]
            fn interrupt_only_decoding_tracks_the_position_within_one_cycle(steps in walk()) {
                // Without the poll, B-only edges wait for the next A edge, which can be up to
                // a cycle later.
                let (states, position) = states_for(&steps);
                let decoded = run(&states, false, false);
                prop_assert!((decoded - position).abs() <= 3, "decoded {decoded}, moved {position}");
            }
        }
    }
}
