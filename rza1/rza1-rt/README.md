# rza1-rt

Runtime for the Renesas RZ/A1L and RZ/A1H (ARM Cortex-A9 with NEON): the vector table, the reset handler
(cache/TLB invalidate, VFP/NEON enable, per-mode stacks, BSS zero, then `main`), the fault
handlers and the GIC IRQ entry stub, plus the linker script.

A board crate depends on it, links it with `use rza1_rt as _;`, and puts a `memory.x` on the
linker search path defining the `RAM` (and optionally `SDRAM`) regions, the per-mode stack sizes
and `INTERNAL_RAM_END`; see the script's header for the full list. The application's `build.rs`
passes `-Tlink.x` (`cargo:rustc-link-arg-bins=-Tlink.x`).

`link.x` lays the image out from the start of `RAM`: the vector table (with a board's boot
header after it), code, read-only data, `.data` and the unwind tables, then `__image_end`, then
the 16 KB MMU translation table and the zero-init `.bss`, then the SRAM heap
(`__sram_heap_start..__sram_heap_end`) up to the per-mode stacks at the top. `__image_end` is the
last byte a loader has to copy; a board whose first-stage loader wants a rounded copy length
derives it from that symbol in its `memory.x`. With the `zero-init-below-image` feature the
translation table and `.bss` come first, at the start of `RAM`, and the image follows them, so an
image that defines `__sram_heap_base` inside itself (to give a load copy it no longer needs to the
heap) has no zero-init data in its heap.

Builds on stable Rust 1.97 or later. (The workspace pins a nightly only for its own `-Zbuild-std` builds.)

## Attributes

- `#[entry]` on `fn() -> !`: the image's entry, which the reset handler calls once BSS is zeroed,
  in System mode with interrupts masked and the MMU and caches off. It exports `main`.
- `#[fault]` on `fn(&FaultInfo) -> !`: the fault handler. Every fault vector (undefined
  instruction, SVC, prefetch abort, data abort, reserved) calls it on the abort stack, with the
  faulting instruction's address, the fault's kind and the interrupted code's LR and SP. It
  exports `rza1_cpu_fault_report(fault_pc, fault_type, sys_lr, sys_sp)`, which a C function of
  that name can define instead. With neither, the core parks (`wfi`). The expansion names
  `::rza1_rt::FaultInfo`; a crate that depends on `rza1-rt` under another name passes the path,
  `#[fault(crate = my_rt)]`.
- `#[irq]` on `unsafe fn(u32)`: the IRQ dispatcher, called with each interrupt's ICCIAR value. It
  exports `gic_dispatch`, which `rza1-hal`'s `rt` feature otherwise provides; it is for an image
  that turns that off and dispatches interrupts itself.

The attributes check the signature and reject anything else with a compile error.

## Execute in place

An image that runs straight from the memory-mapped SPI flash links with `-Tlink_xip.x` instead,
and puts a `memory_xip.x` on the search path defining `FLASH` (where it executes, e.g.
`0x1800_0000`) and `RAM` regions, the per-mode stack sizes and `INTERNAL_RAM_END`. Code and
read-only data stay in flash; the reset handler copies `.data` from flash (`__data_load__`) to
RAM before zeroing BSS. In a RAM-linked image the two addresses are equal and the copy is
skipped. The XIP script reserves the MMU table in RAM as `link.x` does, but has no SRAM heap or
RTT variant.

## Boot header

The vector table is 8 branches. A board whose first-stage loader checks words or a signature
after it emits them into the `.boot_header` section, labelled `__rza1_boot_header`: link.x places
the section straight after the vector table, at `_start + 0x20`, and pulls the label in by name.
A board with no boot header leaves both out.

| Feature | Effect |
|---|---|
| `rtt` | Fault handlers dump their registers to SEGGER RTT channel 0 before the fault handler runs; the script reserves the RTT buffer (`RTT_RAM`, `NCACHE_RTT_RAM` regions) |
| `zero-init-below-image` | The translation table and `.bss` at the start of `RAM`, below the image, for an image that gives part of itself to the heap (`__sram_heap_base`) |

Licensed under either of Apache-2.0 or MIT at your option.
