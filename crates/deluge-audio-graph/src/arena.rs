//! Node + output-slot lifecycle. Node slots are indexed by `NodeId`. Output
//! slots are a separate index space (engine-internal) allocated first-fit and
//! reclaimed on free.
//!
//! An explicit eval-order list records **creation order** and keeps it stable
//! across free/reuse (memory order no longer equals eval order once slots are
//! recycled — see spec §3.5). Nothing here sorts: `create` appends and `free`
//! stable-compacts, so a freed-then-recreated `NodeId` moves to the *end* of
//! eval order. Building a patch in dependency order is the author's obligation.

use crate::node::Kind;
use crate::{Input, Node, NodeId};

pub struct Arena<const NODES: usize, const OUTS: usize> {
    nodes: [Option<Node>; NODES],
    out_used: [bool; OUTS],
    order: [u16; NODES],
    order_len: usize,
}

impl<const NODES: usize, const OUTS: usize> Arena<NODES, OUTS> {
    pub fn new() -> Self {
        Arena {
            nodes: [None; NODES],
            out_used: [false; OUTS],
            order: [0; NODES],
            order_len: 0,
        }
    }

    fn alloc_out_run(&mut self, width: usize) -> Option<usize> {
        let mut start = 0;
        while start + width <= OUTS {
            if (start..start + width).all(|s| !self.out_used[s]) {
                for s in start..start + width {
                    self.out_used[s] = true;
                }
                return Some(start);
            }
            start += 1;
        }
        None
    }

    /// Create a node at `id`. Allocates its output run and appends to eval order.
    /// Returns false (node inert) if no output run fits.
    pub fn create(&mut self, id: NodeId, kind: Kind) -> bool {
        let idx = id.0 as usize;
        if idx >= NODES {
            return false;
        }
        if self.nodes[idx].is_some() {
            return false; // refuse to double-allocate a live id
        }
        let width = Node::out_width(kind);
        let base = match self.alloc_out_run(width) {
            Some(b) => b,
            None => return false,
        };
        self.nodes[idx] = Some(Node::new(kind, base as u16));
        self.order[self.order_len] = id.0;
        self.order_len += 1;
        true
    }

    pub fn free(&mut self, id: NodeId) {
        let idx = id.0 as usize;
        let taken = match self.nodes.get_mut(idx) {
            Some(slot) => slot.take(),
            None => return,
        };
        if let Some(n) = taken {
            let width = Node::out_width(n.kind);
            let base = n.out_base as usize;
            for s in base..base + width {
                self.out_used[s] = false;
            }
            // Remove from eval order (stable compaction).
            let mut w = 0;
            for r in 0..self.order_len {
                if self.order[r] != id.0 {
                    self.order[w] = self.order[r];
                    w += 1;
                }
            }
            self.order_len = w;
        }
    }

    pub fn reset(&mut self) {
        self.nodes = [None; NODES];
        self.out_used = [false; OUTS];
        self.order_len = 0;
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id.0 as usize)?.as_mut()
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0 as usize)?.as_ref()
    }

    pub fn out_base(&self, id: NodeId) -> Option<usize> {
        self.node(id).map(|n| n.out_base as usize)
    }

    /// Read-only accessor for a node's `Kind` (e.g. so a consumer can check a
    /// source's `out_width` before deciding how to resolve a poly input edge).
    pub fn kind_of(&self, id: NodeId) -> Option<Kind> {
        self.node(id).map(|n| n.kind)
    }

    /// Read-only accessor for a node's `(out_base, kind)` in one lookup — lets a
    /// consumer branch on a source's `out_width` (via `kind`) and its base row
    /// without a second table lookup or an infallible-relookup `expect`.
    pub fn out_base_and_kind(&self, id: NodeId) -> Option<(usize, Kind)> {
        self.node(id).map(|n| (n.out_base as usize, n.kind))
    }

    pub fn eval_order(&self) -> &[u16] {
        &self.order[..self.order_len]
    }

    /// Number of edges from `src` into the node in slot `idx`.
    ///
    /// Self-edges do not count: a node reading its own previous block is legal
    /// feedback, not a dependency it could ever satisfy.
    fn edges_from(&self, idx: usize, src: u16) -> u16 {
        if idx == src as usize {
            return 0;
        }
        let Some(n) = self.nodes[idx].as_ref() else {
            return 0;
        };
        let mut count = 0;
        for inp in n.inputs_snapshot() {
            if let Input::Node { node, .. } = inp {
                if node.0 == src {
                    count += 1;
                }
            }
        }
        count
    }

    /// Reorder eval order so every node evaluates after the nodes it reads
    /// (G11). Kahn's algorithm, in place, no allocation.
    ///
    /// **Stable:** among nodes that are ready at the same time, the current
    /// order is preserved — parallel chains are never scrambled, and a
    /// `move_before` / `move_after` that does not contradict a dependency
    /// survives sorting.
    ///
    /// **Cycles:** a feedback loop has no topological order. Its members never
    /// reach in-degree zero and are appended at the end in their existing
    /// relative order, because that order is what decides where the loop's
    /// one-block delay falls — the author's choice, not the sort's. `Input::Bus`
    /// edges are deliberately not dependencies (a bus read is one block
    /// delayed by design), so they never constrain the sort.
    ///
    /// Cost is O(V²·MAX_INPUTS) with no adjacency list to build or store. It
    /// runs at most once per block (see `Engine::render_block`), not once per
    /// command, so a burst of edits during one block pays for one sort.
    pub fn sort(&mut self) {
        let len = self.order_len;
        let mut indeg = [0u16; NODES];
        for k in 0..len {
            let idx = self.order[k] as usize;
            let Some(n) = self.nodes[idx].as_ref() else {
                continue;
            };
            let mut d = 0;
            for inp in n.inputs_snapshot() {
                if let Input::Node { node: src, .. } = inp {
                    let s = src.0 as usize;
                    // A dangling edge renders silence; it must not also stall
                    // the sort with an in-degree nothing can discharge.
                    if s != idx && s < NODES && self.nodes[s].is_some() {
                        d += 1;
                    }
                }
            }
            indeg[idx] = d;
        }

        let mut emitted = [false; NODES];
        let mut out = [0u16; NODES];
        let mut w = 0;
        for _ in 0..len {
            // Always take the EARLIEST ready node in the current order, then
            // rescan from the start. Emitting every ready node in one sweep
            // would be a valid topological order but not a stable one: a node
            // that became ready mid-sweep would jump ahead of one that was
            // already waiting.
            let mut pick = None;
            for k in 0..len {
                let idx = self.order[k] as usize;
                if !emitted[idx] && indeg[idx] == 0 {
                    pick = Some(self.order[k]);
                    break;
                }
            }
            let Some(id) = pick else {
                break; // only cycle members left
            };
            emitted[id as usize] = true;
            out[w] = id;
            w += 1;
            for k in 0..len {
                let c = self.order[k] as usize;
                if !emitted[c] {
                    indeg[c] = indeg[c].saturating_sub(self.edges_from(c, id));
                }
            }
        }

        for k in 0..len {
            let id = self.order[k];
            if !emitted[id as usize] {
                out[w] = id;
                w += 1;
            }
        }
        self.order = out;
        self.order_len = w;
    }

    fn order_pos(&self, id: u16) -> Option<usize> {
        self.order[..self.order_len].iter().position(|&x| x == id)
    }

    /// Move `node` so it evaluates immediately before `target`.
    ///
    /// `false` (and no change) if either id is not live or the two are equal.
    /// Every other node keeps its relative order. Output slots, bus writes,
    /// stream cursors and envelope history are all keyed by `NodeId`, not by
    /// eval position, so nothing else has to move with it.
    pub fn move_before(&mut self, node: NodeId, target: NodeId) -> bool {
        self.reorder(node, target, false)
    }

    /// Move `node` so it evaluates immediately after `target`. This is the
    /// insert-into-an-existing-chain primitive: create the new node (it lands
    /// at the end of eval order), then `move_after` it onto its upstream.
    pub fn move_after(&mut self, node: NodeId, target: NodeId) -> bool {
        self.reorder(node, target, true)
    }

    /// Remove `node` from the eval-order list and re-insert it adjacent to
    /// `target`. Takes effect on the next `render_block`.
    fn reorder(&mut self, node: NodeId, target: NodeId, after: bool) -> bool {
        if node.0 == target.0 {
            return false;
        }
        let (Some(from), Some(tpos0)) = (self.order_pos(node.0), self.order_pos(target.0)) else {
            return false;
        };

        // Remove `node`, shifting the tail left.
        let v = self.order[from];
        for i in from..self.order_len - 1 {
            self.order[i] = self.order[i + 1];
        }
        self.order_len -= 1;

        // The removal shifts `target` left by one iff it sat after `node`.
        // Computing this is exact, so no re-search and no infallible lookup.
        let tpos = if tpos0 > from { tpos0 - 1 } else { tpos0 };
        let dest = if after { tpos + 1 } else { tpos };

        // Insert at `dest`, shifting the tail right. `order_len` was just
        // decremented, so the write at `order_len` is always in bounds.
        let mut i = self.order_len;
        while i > dest {
            self.order[i] = self.order[i - 1];
            i -= 1;
        }
        self.order[dest] = v;
        self.order_len += 1;
        true
    }
}

impl<const NODES: usize, const OUTS: usize> Default for Arena<NODES, OUTS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeId;
    use crate::node::Kind;

    type A = Arena<8, 8>;

    #[test]
    fn create_assigns_contiguous_output_runs() {
        let mut a = A::new();
        assert!(a.create(NodeId(0), Kind::Saw)); // width 1 → slot 0
        assert!(a.create(NodeId(1), Kind::Split2)); // width 2 → slots 1,2
        assert!(a.create(NodeId(2), Kind::Saw)); // width 1 → slot 3
        assert_eq!(a.out_base(NodeId(0)), Some(0));
        assert_eq!(a.out_base(NodeId(1)), Some(1));
        assert_eq!(a.out_base(NodeId(2)), Some(3));
        assert_eq!(a.eval_order(), &[0, 1, 2]);
    }

    #[test]
    fn free_reclaims_slots_and_eval_order() {
        let mut a = A::new();
        a.create(NodeId(0), Kind::Split2); // slots 0,1
        a.create(NodeId(1), Kind::Saw); // slot 2
        a.free(NodeId(0)); // slots 0,1 free again
        assert_eq!(a.eval_order(), &[1]);
        a.create(NodeId(2), Kind::Split2); // reuses slots 0,1
        assert_eq!(a.out_base(NodeId(2)), Some(0));
        assert_eq!(a.eval_order(), &[1, 2]);
    }

    #[test]
    fn output_exhaustion_reports_false() {
        let mut a = A::new(); // 8 output slots
        assert!(a.create(NodeId(0), Kind::Split2)); // 2
        assert!(a.create(NodeId(1), Kind::Split2)); // 4
        assert!(a.create(NodeId(2), Kind::Split2)); // 6
        assert!(a.create(NodeId(3), Kind::Split2)); // 8 (full)
        assert!(!a.create(NodeId(4), Kind::Saw)); // no slot → false
    }

    #[test]
    fn free_out_of_range_id_is_noop() {
        let mut a = A::new();
        assert!(a.create(NodeId(0), Kind::Saw));
        a.free(NodeId(99)); // out of range: must not panic, must be a no-op
        assert_eq!(a.eval_order(), &[0]);
        assert_eq!(a.out_base(NodeId(0)), Some(0));
    }

    // ── G4b: eval-order reordering ────────────────────────────────────────

    fn chain(n: u16) -> A {
        let mut a = A::new();
        for i in 0..n {
            assert!(a.create(NodeId(i), Kind::Saw));
        }
        a
    }

    #[test]
    fn move_after_inserts_into_a_chain() {
        // The motivating case: a node created last (so it evaluates last) is
        // dropped in immediately after its upstream, without touching anything
        // downstream of it.
        let mut a = chain(4); // [0,1,2,3]
        assert!(a.move_after(NodeId(3), NodeId(0)));
        assert_eq!(a.eval_order(), &[0, 3, 1, 2]);
    }

    #[test]
    fn move_before_inserts_into_a_chain() {
        let mut a = chain(4);
        assert!(a.move_before(NodeId(3), NodeId(1)));
        assert_eq!(a.eval_order(), &[0, 3, 1, 2]);
    }

    #[test]
    fn moves_work_in_both_directions() {
        // Backward (later → earlier) and forward (earlier → later) hit
        // different shift paths in `reorder`.
        let mut a = chain(5);
        assert!(a.move_after(NodeId(4), NodeId(1))); // backward
        assert_eq!(a.eval_order(), &[0, 1, 4, 2, 3]);
        assert!(a.move_after(NodeId(0), NodeId(3))); // forward
        assert_eq!(a.eval_order(), &[1, 4, 2, 3, 0]);
        assert!(a.move_before(NodeId(3), NodeId(1))); // backward to head
        assert_eq!(a.eval_order(), &[3, 1, 4, 2, 0]);
    }

    #[test]
    fn move_to_head_and_tail() {
        let mut a = chain(4);
        assert!(a.move_before(NodeId(2), NodeId(0)));
        assert_eq!(a.eval_order(), &[2, 0, 1, 3], "to head");
        assert!(a.move_after(NodeId(0), NodeId(3)));
        assert_eq!(a.eval_order(), &[2, 1, 3, 0], "to tail");
    }

    #[test]
    fn adjacent_move_is_stable() {
        let mut a = chain(3);
        assert!(a.move_after(NodeId(1), NodeId(0))); // already there
        assert_eq!(a.eval_order(), &[0, 1, 2]);
        assert!(a.move_before(NodeId(1), NodeId(2))); // also already there
        assert_eq!(a.eval_order(), &[0, 1, 2]);
    }

    #[test]
    fn invalid_moves_are_refused_and_change_nothing() {
        let mut a = chain(3);
        assert!(!a.move_after(NodeId(0), NodeId(0)), "self-move");
        assert!(!a.move_after(NodeId(9), NodeId(0)), "dead node");
        assert!(!a.move_after(NodeId(0), NodeId(9)), "dead target");
        a.free(NodeId(1));
        assert!(!a.move_before(NodeId(1), NodeId(0)), "freed node");
        assert!(!a.move_before(NodeId(0), NodeId(1)), "freed target");
        assert_eq!(a.eval_order(), &[0, 2]);
    }

    #[test]
    fn reordering_does_not_disturb_output_slots() {
        // Output runs are keyed by NodeId, not by eval position: a move must
        // not renumber them, or every existing `Input::Node` edge would break.
        let mut a = A::new();
        a.create(NodeId(0), Kind::Saw);
        a.create(NodeId(1), Kind::Split2);
        a.create(NodeId(2), Kind::Saw);
        let bases = [
            a.out_base(NodeId(0)),
            a.out_base(NodeId(1)),
            a.out_base(NodeId(2)),
        ];
        assert!(a.move_before(NodeId(2), NodeId(0)));
        assert_eq!(a.eval_order(), &[2, 0, 1]);
        assert_eq!(
            [
                a.out_base(NodeId(0)),
                a.out_base(NodeId(1)),
                a.out_base(NodeId(2))
            ],
            bases
        );
    }

    #[test]
    fn free_after_move_compacts_the_moved_order() {
        let mut a = chain(4);
        a.move_after(NodeId(3), NodeId(0)); // [0,3,1,2]
        a.free(NodeId(3));
        assert_eq!(a.eval_order(), &[0, 1, 2]);
        // A recreated id still lands at the end — move is what places it.
        assert!(a.create(NodeId(3), Kind::Saw));
        assert_eq!(a.eval_order(), &[0, 1, 2, 3]);
    }

    #[test]
    fn create_on_live_id_is_refused() {
        let mut a = A::new();
        assert!(a.create(NodeId(0), Kind::Saw));
        assert!(!a.create(NodeId(0), Kind::Split2)); // refused: id already live
        assert_eq!(a.eval_order(), &[0]);
        assert_eq!(a.out_base(NodeId(0)), Some(0)); // unchanged: still Saw's slot, width 1
    }

    // ── Topological ordering (G11) ───────────────────────────────────────

    /// Wire `node`'s port 0 to `src`'s port 0.
    fn wire(a: &mut A, node: u16, src: u16) {
        *a.node_mut(NodeId(node)).unwrap().input_mut(0).unwrap() = crate::Input::Node {
            node: NodeId(src),
            port: 0,
        };
    }

    #[test]
    fn sort_puts_a_source_before_the_node_that_reads_it() {
        let mut a = A::new();
        a.create(NodeId(0), Kind::Add); // consumer created FIRST
        a.create(NodeId(1), Kind::Saw); // its source, created second
        wire(&mut a, 0, 1);
        assert_eq!(a.eval_order(), &[0, 1], "creation order, before sorting");
        a.sort();
        assert_eq!(a.eval_order(), &[1, 0], "source now evaluates first");
    }

    #[test]
    fn sort_keeps_independent_nodes_in_creation_order() {
        // Nothing constrains these three, so the author's order must survive:
        // an unstable sort would scramble parallel chains for no reason.
        let mut a = A::new();
        a.create(NodeId(2), Kind::Saw);
        a.create(NodeId(0), Kind::Saw);
        a.create(NodeId(1), Kind::Saw);
        a.sort();
        assert_eq!(a.eval_order(), &[2, 0, 1]);
    }

    #[test]
    fn sort_orders_a_three_node_chain_built_backwards() {
        let mut a = A::new();
        a.create(NodeId(0), Kind::Add); // sink
        a.create(NodeId(1), Kind::Add); // middle
        a.create(NodeId(2), Kind::Saw); // source
        wire(&mut a, 0, 1);
        wire(&mut a, 1, 2);
        a.sort();
        assert_eq!(a.eval_order(), &[2, 1, 0]);
    }

    #[test]
    fn sort_leaves_a_cycle_in_its_existing_relative_order() {
        // A feedback loop has no topological order. The sort must terminate,
        // keep every node, and leave the cycle's members in the order the
        // author put them — that order is what decides where the one-block
        // delay falls, and it is the author's to choose.
        let mut a = A::new();
        a.create(NodeId(0), Kind::Saw); // outside the cycle
        a.create(NodeId(1), Kind::Add);
        a.create(NodeId(2), Kind::Add);
        wire(&mut a, 1, 2);
        wire(&mut a, 2, 1);
        a.sort();
        assert_eq!(a.eval_order(), &[0, 1, 2]);
    }

    #[test]
    fn sort_ignores_edges_from_dead_nodes() {
        // A dangling `Input::Node` renders silence; it must not also stall the
        // sort by contributing an in-degree that can never be discharged.
        let mut a = A::new();
        a.create(NodeId(0), Kind::Add);
        a.create(NodeId(1), Kind::Saw);
        wire(&mut a, 1, 7); // node 7 was never created
        a.sort();
        assert_eq!(a.eval_order(), &[0, 1]);
    }

    #[test]
    fn sort_handles_a_node_reading_one_source_on_two_ports() {
        // Two edges from the same source: the in-degree bookkeeping must
        // discharge both, or the consumer never becomes ready and falls
        // through to the cycle path.
        let mut a = A::new();
        a.create(NodeId(0), Kind::Add);
        a.create(NodeId(1), Kind::Saw);
        wire(&mut a, 0, 1);
        *a.node_mut(NodeId(0)).unwrap().input_mut(1).unwrap() = crate::Input::Node {
            node: NodeId(1),
            port: 0,
        };
        a.sort();
        assert_eq!(a.eval_order(), &[1, 0]);
    }
}
