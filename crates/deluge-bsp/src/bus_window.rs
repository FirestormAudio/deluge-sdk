//! When a long RSPI0 transfer (an OLED frame) may take the bus: only if it will be done before the
//! bus's next reservation (a CV word due at a set time), or once it has waited as long as the
//! display can. Pure, so it is tested off target; [`crate::bus`] holds the reservation and
//! [`crate::oled::send_frame`] asks before each frame.

/// What a frame waiting for RSPI0 does now.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Decision {
    /// Take the bus.
    Go,
    /// Wait until this tick (the reservation), or until the reservation moves.
    WaitUntil(u64),
}

/// Whether a frame that holds the bus for `frame_hold` ticks may start at `now`, given the next
/// reservation and how long the frame has already waited (`waited`). It goes if it ends, with
/// `margin` to spare, before the reservation, or once it has waited `max_defer`. All in Embassy
/// ticks.
pub fn decide(
    now: u64,
    next_reservation: Option<u64>,
    frame_hold: u64,
    margin: u64,
    waited: u64,
    max_defer: u64,
) -> Decision {
    match next_reservation {
        Some(at) if waited < max_defer && now + frame_hold + margin > at => Decision::WaitUntil(at),
        _ => Decision::Go,
    }
}

/// The longest recent hold of the bus by a frame: a longer one replaces it at once, a shorter one
/// pulls it down by a sixteenth of the difference, so one slow frame doesn't set it for ever.
#[derive(Debug, Clone, Copy)]
pub struct FrameHold(u64);

impl FrameHold {
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Note a frame that held the bus for `held` ticks.
    pub fn record(&mut self, held: u64) {
        self.0 = if held >= self.0 {
            held
        } else {
            self.0 - ((self.0 - held) >> 4)
        };
    }

    pub fn get(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reservation_goes() {
        assert_eq!(decide(1_000, None, 2_000, 100, 0, 40_000), Decision::Go);
    }

    #[test]
    fn a_frame_that_fits_before_the_reservation_goes() {
        assert_eq!(
            decide(1_000, Some(4_000), 2_000, 100, 0, 40_000),
            Decision::Go
        );
    }

    #[test]
    fn a_frame_that_would_overrun_the_reservation_waits_for_it() {
        assert_eq!(
            decide(1_000, Some(2_500), 2_000, 100, 0, 40_000),
            Decision::WaitUntil(2_500)
        );
    }

    #[test]
    fn a_reservation_already_past_still_holds_the_frame_until_it_is_written() {
        assert_eq!(
            decide(5_000, Some(4_000), 2_000, 100, 0, 40_000),
            Decision::WaitUntil(4_000)
        );
    }

    #[test]
    fn a_frame_that_has_waited_its_longest_goes_anyway() {
        assert_eq!(
            decide(1_000, Some(2_500), 2_000, 100, 40_000, 40_000),
            Decision::Go
        );
    }

    #[test]
    fn frame_hold_takes_a_longer_hold_at_once_and_decays_slowly() {
        let mut h = FrameHold::new(1_600);
        h.record(3_200);
        assert_eq!(h.get(), 3_200);
        h.record(1_600);
        assert_eq!(h.get(), 3_100);
    }
}
