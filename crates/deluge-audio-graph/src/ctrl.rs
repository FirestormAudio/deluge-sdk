//! Mono control buses: shared, named modulation values (G6).
//!
//! An audio bus ([`crate::bus`]) is a stereo accumulator, zeroed and re-summed
//! every block. A control bus is the opposite in every respect, and
//! deliberately so:
//!
//! |            | audio bus                  | control bus              |
//! |------------|----------------------------|--------------------------|
//! | shape      | `[[f32; BLOCK]; BUSES]`    | one `f32`                |
//! | lifetime   | zeroed and re-summed/block | **persists until written** |
//! | read as    | [`crate::In::A`], L+R summed | [`crate::In::K`]       |
//!
//! **Persistence is the point.** A host writes a MIDI CC to a control bus once
//! ([`crate::Cmd::SetCtrl`], scsynth's `/c_set`) and it stays there until
//! something writes it again. Summing-and-zeroing like an audio bus would wipe
//! that write on the very next block. It also means a control bus has a value
//! before anything writes one — `0.0` — rather than being undefined.
//!
//! **One writer wins, rather than summing.** Two nodes routed to the same
//! control bus is a patching mistake, not a mix; the last write in the standing
//! route list takes effect. Modulation sources are combined by patching them
//! through `Add`, where the intent is explicit.
//!
//! ## Why a control bus read is one block old
//! `render_block` evaluates nodes and only then applies the standing routes, so
//! a node reading a control bus sees the value from the previous block. That is
//! the same rule [`crate::Input::Bus`] already follows, for the same reason: it
//! is what lets a modulation source and its destination sit anywhere in eval
//! order without the graph needing a cycle. Control-bus edges are therefore not
//! dependencies for the topological sort (`arena.rs`). One block is 1.3 ms at
//! 48 kHz with `BLOCK = 64` — inaudible for modulation, which is all this
//! carries.

use crate::ids::CTRL_BUSES;

/// The control-bus value space: `CTRL_BUSES` persistent mono values.
#[derive(Clone, Copy)]
pub struct CtrlBuses {
    v: [f32; CTRL_BUSES],
}

impl CtrlBuses {
    pub fn new() -> Self {
        CtrlBuses {
            v: [0.0; CTRL_BUSES],
        }
    }

    /// Value on bus `b`. Out-of-range reads `0.0` — a dangling control read is
    /// silence, exactly as a dangling `Input::Node` is (`engine.rs`).
    #[inline]
    pub fn get(&self, b: u16) -> f32 {
        let i = b as usize;
        if i < CTRL_BUSES { self.v[i] } else { 0.0 }
    }

    /// Write bus `b`. Out-of-range writes are dropped, never panic.
    #[inline]
    pub fn set(&mut self, b: u16, value: f32) {
        let i = b as usize;
        if i < CTRL_BUSES {
            self.v[i] = value;
        }
    }

    /// Reset every bus to `0.0`.
    pub fn clear(&mut self) {
        self.v = [0.0; CTRL_BUSES];
    }
}

impl Default for CtrlBuses {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buses_start_at_zero_and_hold_what_is_written() {
        let mut c = CtrlBuses::new();
        assert_eq!(c.get(0), 0.0, "defined before anything writes it");
        c.set(0, 0.75);
        assert_eq!(c.get(0), 0.75);
        assert_eq!(c.get(1), 0.0, "writes do not bleed between buses");
    }

    #[test]
    fn out_of_range_access_is_inert() {
        let mut c = CtrlBuses::new();
        c.set(CTRL_BUSES as u16, 1.0); // dropped, must not panic
        c.set(9999, 1.0);
        assert_eq!(c.get(CTRL_BUSES as u16), 0.0);
        assert_eq!(c.get(9999), 0.0);
    }

    #[test]
    fn clear_zeroes_every_bus() {
        let mut c = CtrlBuses::new();
        for b in 0..CTRL_BUSES as u16 {
            c.set(b, 1.0);
        }
        c.clear();
        assert!((0..CTRL_BUSES as u16).all(|b| c.get(b) == 0.0));
    }
}
