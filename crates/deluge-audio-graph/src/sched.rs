//! Time-stamped commands: the deferred half of the [`crate::cmd::Host`] seam.
//!
//! [`crate::Engine::apply`] mutates the graph the moment it is called, which
//! ties every edit to whenever the control thread happened to run.
//! [`crate::Engine::apply_at`] instead files the command against a sample
//! position on the engine's own clock, and `render_block` applies it at the
//! right block. That is what makes a sequencer's timing independent of control
//! -thread jitter — scsynth gets the same property from OSC bundle timetags.
//!
//! ## Block accurate, not sample accurate
//! A command fires at the start of the block **containing** its timestamp, not
//! at the exact sample. Sample accuracy would mean splitting a block's render
//! around the command, which costs far more than it buys for graph edits;
//! scsynth draws the line in the same place. Resolution is therefore `BLOCK`
//! samples — 1.3 ms at 48 kHz with `BLOCK = 64`.
//!
//! An **overdue** command (one whose block has already gone by, because the
//! host filed it late) fires at the next block rather than being dropped. Late
//! is recoverable; silently vanishing is not.
//!
//! ## Ordering
//! Entries sort by timestamp, and among equal timestamps by arrival — a
//! scheduled `NewNode` and the `SetInput` that wires it up must not invert.
//! [`SchedQueue::push`] inserts *after* every entry with a timestamp `<=` the
//! new one, which gets that for free with no sequence counter to overflow.
//!
//! ## Capacity
//! Fixed [`SCHED_QUEUE`] entries, no allocation: the queue is drained on the
//! audio thread, and this crate does not allocate or free there (see the
//! real-time safety note in `docs/audio-graph-vs-glicol.md`). A full queue
//! refuses the command and reports [`crate::event::CmdError::ScheduleFull`]
//! rather than dropping something already accepted. A host that wants to file a
//! whole bar ahead should keep its own unbounded queue on the control side and
//! feed this one just in time.

use crate::cmd::Cmd;

/// Capacity of the engine's scheduled-command queue, in commands.
pub const SCHED_QUEUE: usize = 64;

/// A fixed-capacity queue of `(timestamp, command)`, ordered by timestamp then
/// arrival.
pub struct SchedQueue {
    at: [u64; SCHED_QUEUE],
    cmd: [Cmd; SCHED_QUEUE],
    len: usize,
}

impl SchedQueue {
    pub fn new() -> Self {
        SchedQueue {
            at: [0; SCHED_QUEUE],
            cmd: [Cmd::Nop; SCHED_QUEUE],
            len: 0,
        }
    }

    /// File `cmd` to run at sample `at`. `false` if the queue is full (the
    /// command is not stored, and nothing already queued is disturbed).
    ///
    /// Inserting after every entry with a timestamp `<= at` keeps arrival order
    /// among equal timestamps.
    pub fn push(&mut self, at: u64, cmd: Cmd) -> bool {
        if self.len == SCHED_QUEUE {
            return false;
        }
        let mut i = self.len;
        while i > 0 && self.at[i - 1] > at {
            self.at[i] = self.at[i - 1];
            self.cmd[i] = self.cmd[i - 1];
            i -= 1;
        }
        self.at[i] = at;
        self.cmd[i] = cmd;
        self.len += 1;
        true
    }

    /// Take the earliest command due strictly before `now`, if any.
    ///
    /// `render_block` passes the clock *after* its `+= BLOCK`, which names the
    /// first sample of the next block — so `at < now` is exactly "this block
    /// contains sample `at`", and an overdue entry is due as well.
    pub fn pop_due(&mut self, now: u64) -> Option<Cmd> {
        if self.len == 0 || self.at[0] >= now {
            return None;
        }
        let cmd = self.cmd[0];
        for i in 1..self.len {
            self.at[i - 1] = self.at[i];
            self.cmd[i - 1] = self.cmd[i];
        }
        self.len -= 1;
        Some(cmd)
    }

    /// Timestamp of the earliest queued command, if any.
    pub fn next_at(&self) -> Option<u64> {
        (self.len > 0).then(|| self.at[0])
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl Default for SchedQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeId;

    fn gate(n: u16) -> Cmd {
        Cmd::Trigger { node: NodeId(n) }
    }

    fn node_of(c: Cmd) -> u16 {
        match c {
            Cmd::Trigger { node } => node.0,
            _ => u16::MAX,
        }
    }

    #[test]
    fn pops_in_timestamp_order_regardless_of_push_order() {
        let mut q = SchedQueue::new();
        assert!(q.push(300, gate(3)));
        assert!(q.push(100, gate(1)));
        assert!(q.push(200, gate(2)));
        assert_eq!(q.next_at(), Some(100));
        assert_eq!(q.pop_due(1000).map(node_of), Some(1));
        assert_eq!(q.pop_due(1000).map(node_of), Some(2));
        assert_eq!(q.pop_due(1000).map(node_of), Some(3));
        assert_eq!(q.pop_due(1000), None);
    }

    #[test]
    fn equal_timestamps_keep_arrival_order() {
        // A scheduled NewNode and the SetInput wiring it up share a timestamp;
        // inverting them would build the patch wrong.
        let mut q = SchedQueue::new();
        for n in 0..5 {
            assert!(q.push(500, gate(n)));
        }
        q.push(499, gate(99)); // earlier, must come first
        assert_eq!(q.pop_due(1000).map(node_of), Some(99));
        for n in 0..5 {
            assert_eq!(q.pop_due(1000).map(node_of), Some(n));
        }
    }

    #[test]
    fn nothing_is_due_before_its_block() {
        let mut q = SchedQueue::new();
        q.push(100, gate(1));
        assert_eq!(q.pop_due(100), None, "at == now is the *next* block");
        assert_eq!(q.pop_due(64), None);
        assert_eq!(q.pop_due(101).map(node_of), Some(1));
    }

    #[test]
    fn overdue_entries_are_due() {
        let mut q = SchedQueue::new();
        q.push(10, gate(1));
        assert_eq!(q.pop_due(10_000).map(node_of), Some(1), "late, not lost");
    }

    #[test]
    fn full_queue_refuses_without_disturbing_its_contents() {
        let mut q = SchedQueue::new();
        for n in 0..SCHED_QUEUE {
            assert!(q.push(100 + n as u64, gate(n as u16)));
        }
        assert!(!q.push(50, gate(999)), "full → refused");
        assert_eq!(q.len(), SCHED_QUEUE);
        assert_eq!(q.next_at(), Some(100), "existing head untouched");
        assert_eq!(q.pop_due(u64::MAX).map(node_of), Some(0));
    }

    #[test]
    fn clear_empties_the_queue() {
        let mut q = SchedQueue::new();
        q.push(100, gate(1));
        q.push(200, gate(2));
        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.next_at(), None);
        assert_eq!(q.pop_due(u64::MAX), None);
    }
}
