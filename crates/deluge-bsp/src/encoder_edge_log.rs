//! Pure per-encoder log of timed edge runs, shared by the (bare-metal) [`crate::encoder`] interrupts and
//! host-tested here.
//!
//! An encoder's interrupt decodes a run of quadrature edges (usually one) and records it with the time it was
//! decoded. A consumer that acts on the turn later, in batches, can still see each run's own time: an encoder's
//! acceleration, which grows with how fast the edges come, then does not depend on when the consumer ran.

/// One run of quadrature edges and the time it was decoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeEvent {
    /// When the run was decoded, in microseconds on the clock the recorder passed (the device uses
    /// `embassy_time::Instant::now()`).
    pub time_us: u64,
    /// The signed edge count, four per quadrature cycle.
    pub edges: i8,
}

/// How many runs an encoder's log keeps before the newest run absorbs the next.
pub const EDGE_LOG_CAPACITY: usize = 16;

/// An encoder's timed runs since the consumer last took them, oldest first.
///
/// Bounded: when full, a new run folds into the newest one (its edges added, saturating, and its time taken), so
/// no edge is lost, only the time of the runs that came while the consumer was away.
#[derive(Clone, Copy, Debug)]
pub struct EdgeLog {
    events: [EdgeEvent; EDGE_LOG_CAPACITY],
    len: usize,
    /// The runs' edges added up as a saturating `i8` count adds them, in the order they came.
    counted: i8,
}

impl Default for EdgeLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Folds `event` into `into`: the edges add, saturating, and the later time is kept.
fn fold(into: &mut EdgeEvent, event: EdgeEvent) {
    into.edges = into.edges.saturating_add(event.edges);
    into.time_us = into.time_us.max(event.time_us);
}

impl EdgeLog {
    /// An empty log.
    pub const fn new() -> Self {
        Self {
            events: [EdgeEvent {
                time_us: 0,
                edges: 0,
            }; EDGE_LOG_CAPACITY],
            len: 0,
            counted: 0,
        }
    }

    /// Records `edges` decoded at `time_us`. A zero run is not recorded.
    pub fn push(&mut self, edges: i8, time_us: u64) {
        if edges == 0 {
            return;
        }
        self.counted = self.counted.saturating_add(edges);
        let event = EdgeEvent { time_us, edges };
        if self.len < EDGE_LOG_CAPACITY {
            self.events[self.len] = event;
            self.len += 1;
        } else {
            fold(&mut self.events[EDGE_LOG_CAPACITY - 1], event);
        }
    }

    /// The runs recorded and not yet taken, oldest first.
    pub fn events(&self) -> &[EdgeEvent] {
        &self.events[..self.len]
    }

    /// The sum of the recorded runs' edges.
    pub fn edges(&self) -> i32 {
        self.events().iter().map(|e| i32::from(e.edges)).sum()
    }

    /// The recorded runs' edges added up as a saturating `i8` count adds them, in the order they came: what an `i8`
    /// accumulator fed the same runs holds, saturation included.
    pub fn counted(&self) -> i8 {
        self.counted
    }

    /// Forgets every run.
    pub fn clear(&mut self) {
        self.len = 0;
        self.counted = 0;
    }

    /// Moves every run into `out`, oldest first, and empties the log; returns how many slots it wrote.
    ///
    /// When `out` is shorter than the log, the runs past its end fold into its last slot. An empty `out` takes
    /// nothing and empties the log.
    pub fn drain_into(&mut self, out: &mut [EdgeEvent]) -> usize {
        let mut written = 0;
        for &event in self.events() {
            if written < out.len() {
                out[written] = event;
                written += 1;
            } else if let Some(last) = out.last_mut() {
                fold(last, event);
            }
        }
        self.clear();
        written
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn runs_come_back_oldest_first_with_their_own_times() {
        let mut log = EdgeLog::new();
        log.push(1, 3_000);
        log.push(0, 4_000);
        log.push(-2, 6_000);
        let mut out = [EdgeEvent::default(); 4];
        assert_eq!(log.drain_into(&mut out), 2);
        assert_eq!(
            out[..2],
            [
                EdgeEvent {
                    time_us: 3_000,
                    edges: 1
                },
                EdgeEvent {
                    time_us: 6_000,
                    edges: -2
                }
            ]
        );
        assert!(log.events().is_empty());
    }

    #[test]
    fn a_full_log_folds_the_next_run_into_the_newest() {
        let mut log = EdgeLog::new();
        for i in 0..EDGE_LOG_CAPACITY as u64 + 2 {
            log.push(1, i * 1_000);
        }
        assert_eq!(log.events().len(), EDGE_LOG_CAPACITY);
        assert_eq!(log.edges(), EDGE_LOG_CAPACITY as i32 + 2);
        let newest = log.events()[EDGE_LOG_CAPACITY - 1];
        assert_eq!(newest.edges, 3);
        assert_eq!(newest.time_us, (EDGE_LOG_CAPACITY as u64 + 1) * 1_000);
    }

    #[test]
    fn a_short_out_folds_the_rest_into_its_last_slot() {
        let mut log = EdgeLog::new();
        log.push(1, 1_000);
        log.push(1, 2_000);
        log.push(1, 3_000);
        let mut out = [EdgeEvent::default(); 2];
        assert_eq!(log.drain_into(&mut out), 2);
        assert_eq!(out[0].edges, 1);
        assert_eq!(
            out[1],
            EdgeEvent {
                time_us: 3_000,
                edges: 2
            }
        );
        assert!(log.events().is_empty());
    }

    #[test]
    fn folding_saturates_rather_than_wrapping() {
        let mut log = EdgeLog::new();
        for _ in 0..EDGE_LOG_CAPACITY {
            log.push(1, 0);
        }
        log.push(127, 1);
        log.push(127, 2);
        assert_eq!(log.events()[EDGE_LOG_CAPACITY - 1].edges, 127);
    }
}
