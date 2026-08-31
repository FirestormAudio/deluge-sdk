//! Engine → host events. The return path for [`crate::cmd::Host`].
//!
//! `Cmd`s flow host → engine; `Event`s flow engine → host. The engine owns a
//! fixed-capacity FIFO that [`crate::Engine::render_block`] fills and the host
//! drains, mirroring how the host owns the `Cmd` ring the engine reads.
//!
//! ## Why edges, not levels
//! An envelope kernel is `Idle` *before its first gate* as well as after its
//! release decays, so "is idle" cannot be reported as a level — a freshly
//! created node would announce completion it never performed. The engine keeps
//! a per-node previous-idle mask and emits only on the rising edge (not-idle →
//! idle). `Engine::create` seeds that mask from the node's current state so a
//! new node never fires on its first render.
//!
//! ## Overflow
//! `push` on a full queue drops the *newest* event and bumps a saturating
//! `dropped` counter rather than panicking or blocking the audio path. A
//! dropped `VoiceDone` means a voice lane is not reclaimed that cycle — the
//! allocator degrades to its pre-event stealing behaviour rather than
//! misbehaving. Hosts should surface a non-zero `dropped()` as a capacity bug.

use crate::NodeId;

/// Capacity of the engine's event FIFO, in events.
///
/// One block can produce at most one event per envelope lane, so a patch with
/// `n` poly envelope nodes can emit up to `n * VOICES` in a single block. 64
/// covers any realistic instrument (8 poly envelope nodes at `VOICES = 8`);
/// beyond that, see the overflow note above.
pub const EVENT_QUEUE: usize = 64;

/// Something the engine observed and the control plane may care about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A mono envelope node (`Env`/`Adsr`) finished its release.
    Done { node: NodeId },
    /// Lane `voice` of a poly envelope node (`PolyAr`/`PolyAdsr`) finished its
    /// release. Consumed by [`crate::VoiceAllocator::on_event`] to return the
    /// lane to `Free`.
    VoiceDone { node: NodeId, voice: u8 },
}

/// Fixed-capacity, allocation-free FIFO of [`Event`]s.
pub struct EventQueue {
    buf: [Event; EVENT_QUEUE],
    head: usize, // next index to pop
    len: usize,
    dropped: u16,
}

impl EventQueue {
    pub fn new() -> Self {
        EventQueue {
            buf: [Event::Done { node: NodeId(0) }; EVENT_QUEUE],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    /// Enqueue, or drop and count if full. Never panics.
    pub fn push(&mut self, ev: Event) {
        if self.len == EVENT_QUEUE {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.buf[(self.head + self.len) % EVENT_QUEUE] = ev;
        self.len += 1;
    }

    /// Dequeue the oldest event, if any.
    pub fn pop(&mut self) -> Option<Event> {
        if self.len == 0 {
            return None;
        }
        let ev = self.buf[self.head];
        self.head = (self.head + 1) % EVENT_QUEUE;
        self.len -= 1;
        Some(ev)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Events lost to overflow since construction (saturating). Non-zero means
    /// the host is not draining fast enough, or `EVENT_QUEUE` is too small.
    pub fn dropped(&self) -> u16 {
        self.dropped
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order_is_preserved() {
        let mut q = EventQueue::new();
        q.push(Event::Done { node: NodeId(1) });
        q.push(Event::VoiceDone {
            node: NodeId(2),
            voice: 3,
        });
        assert_eq!(q.len(), 2);
        assert_eq!(q.pop(), Some(Event::Done { node: NodeId(1) }));
        assert_eq!(
            q.pop(),
            Some(Event::VoiceDone {
                node: NodeId(2),
                voice: 3
            })
        );
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn overflow_drops_newest_and_counts() {
        let mut q = EventQueue::new();
        for i in 0..EVENT_QUEUE {
            q.push(Event::Done {
                node: NodeId(i as u16),
            });
        }
        assert_eq!(q.dropped(), 0);
        q.push(Event::Done { node: NodeId(999) }); // full → dropped
        assert_eq!(q.dropped(), 1);
        assert_eq!(q.len(), EVENT_QUEUE);
        // The oldest survives; the overflowing newest is gone.
        assert_eq!(q.pop(), Some(Event::Done { node: NodeId(0) }));
    }

    #[test]
    fn wraps_around_without_losing_events() {
        let mut q = EventQueue::new();
        // Fill, drain half, refill past the wrap point.
        for i in 0..EVENT_QUEUE {
            q.push(Event::Done {
                node: NodeId(i as u16),
            });
        }
        for i in 0..EVENT_QUEUE / 2 {
            assert_eq!(
                q.pop(),
                Some(Event::Done {
                    node: NodeId(i as u16)
                })
            );
        }
        for i in 0..EVENT_QUEUE / 2 {
            q.push(Event::Done {
                node: NodeId(1000 + i as u16),
            });
        }
        assert_eq!(q.len(), EVENT_QUEUE);
        assert_eq!(q.dropped(), 0);
        // Remaining originals come out before the newly pushed ones.
        assert_eq!(
            q.pop(),
            Some(Event::Done {
                node: NodeId((EVENT_QUEUE / 2) as u16)
            })
        );
    }
}
