# deluge-alloc

Critical-section-guarded heap allocators for the Synthstrom Deluge (Renesas
RZ/A1L, ARM Cortex-A9).

Provides two independent allocation arenas as nightly `Allocator`-trait heaps:

- `SRAM` — backed by on-chip SRAM; the default global heap.
- `SDRAM` — backed by the external 64 MB SDRAM, for bulk audio buffers.

Each is a `CsHeap`: a `linked_list_allocator::Heap` wrapped in
`critical_section::with`, so allocation is safe from interrupt handlers.

```rust,ignore
#![feature(allocator_api)]
use deluge_alloc::{SRAM, SDRAM};

// Initialise once at startup, before any allocation in that arena.
unsafe { SRAM.init(heap_start, heap_size) };

let buf: Box<[u8; 4096], _> = Box::new_in([0u8; 4096], &SDRAM);
```

Part of the [`deluge-sdk`](https://github.com/FirestormAudio/deluge-sdk)
workspace.
