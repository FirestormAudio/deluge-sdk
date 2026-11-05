//! Writing ahead of a free-running DMA read head in a circular sample ring.
//!
//! A DMA channel reads the ring forever (the SCUX FFD feeding the codec, say); the writer keeps
//! its write head a fixed lead ahead of the read head, topping up as the DMA consumes. Offsets
//! are in slots (one `i32` sample); a stereo frame is two slots, and every offset stays
//! frame-aligned.

/// What the writer should do this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteAhead {
    /// Write this many frames at the write head (0: the lead is already full).
    Write(usize),
    /// The DMA has passed the write head: move it to this slot (the read head) and write the
    /// whole lead from there.
    Reanchor(usize),
}

/// Plan this tick's write.
///
/// `read`: the DMA read slot. `write`: the writer's slot. `len`: ring length in slots.
/// `lead_frames`: the target distance, in frames, of the write head ahead of the read head
/// (less than half the ring).
pub fn plan(read: usize, write: usize, len: usize, lead_frames: usize) -> WriteAhead {
    let read = read & !1; // the DMA's position can be mid-frame
    let target = lead_frames * 2;
    let lead = (write + len - read) % len;
    if lead > target {
        // The writer never runs more than `target` ahead, so a longer "lead" is the DMA
        // having passed the write head and wrapped the distance.
        WriteAhead::Reanchor(read)
    } else {
        WriteAhead::Write((target - lead) / 2)
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    const LEN: usize = 4096; // 2048 stereo frames
    const LEAD: usize = 512;

    #[test]
    fn a_full_lead_needs_no_write() {
        assert_eq!(plan(100, 100 + LEAD * 2, LEN, LEAD), WriteAhead::Write(0));
    }

    #[test]
    fn the_writer_tops_up_what_the_dma_consumed() {
        // The DMA moved 48 frames since the lead was full.
        assert_eq!(
            plan(100 + 96, 100 + LEAD * 2, LEN, LEAD),
            WriteAhead::Write(48)
        );
    }

    #[test]
    fn the_lead_wraps_around_the_ring_end() {
        let read = LEN - 20; // 10 frames before the end
        let write = (read + LEAD * 2 - 64) % LEN; // 32 frames short, wrapped
        assert_eq!(plan(read, write, LEN, LEAD), WriteAhead::Write(32));
    }

    #[test]
    fn a_dma_that_passed_the_writer_reanchors_it() {
        // The DMA is 10 frames past the write head (the writer stalled).
        let write = 1000;
        let read = write + 20;
        assert_eq!(plan(read, write, LEN, LEAD), WriteAhead::Reanchor(read));
        // A mid-frame read head re-anchors to its frame.
        assert_eq!(plan(read + 1, write, LEN, LEAD), WriteAhead::Reanchor(read));
    }

    /// After a re-anchor the caller writes the whole lead from the slot it was given; the next
    /// plan must then be an ordinary top-up, not another re-anchor (the lead doubled, every
    /// tick re-anchored, and the output was mostly padding).
    #[test]
    fn a_reanchored_writer_tops_up_next_tick() {
        let read = 2000;
        let WriteAhead::Reanchor(slot) = plan(read, read - 40, LEN, LEAD) else {
            panic!("expected a re-anchor")
        };
        let written = (slot + LEAD * 2) % LEN; // the caller wrote the lead
        let read = read + 96; // the DMA moved 48 frames
        assert_eq!(plan(read, written, LEN, LEAD), WriteAhead::Write(48));
    }

    #[test]
    fn an_odd_read_slot_is_treated_frame_aligned() {
        // CRSA can be mid-frame; the plan rounds the read head down to its frame.
        assert_eq!(plan(101, 100 + LEAD * 2, LEN, LEAD), WriteAhead::Write(0));
    }
}
