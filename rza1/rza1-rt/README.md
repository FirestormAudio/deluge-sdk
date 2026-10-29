# rza1-rt

Runtime for the Renesas RZ/A1L (ARM Cortex-A9, R7S721020): the vector table, the reset handler
(cache/TLB invalidate, VFP/NEON enable, per-mode stacks, BSS zero, then `main`), the fault
handlers and the GIC IRQ entry stub, plus the linker script.

A board crate depends on it, links it with `use rza1_rt as _;`, and puts a `memory.x` on the
linker search path defining the `RAM` (and optionally `SDRAM`) regions, the per-mode stack sizes
and `INTERNAL_RAM_END`; see the script's header for the full list. The application's `build.rs`
passes `-Tlink.x` (`cargo:rustc-link-arg-bins=-Tlink.x`).

Requires a nightly Rust toolchain (the workspace's `rust-toolchain.toml` pins one).

## Attributes

- `#[entry]` on `fn() -> !`: the image's entry, which the reset handler calls once BSS is zeroed,
  in System mode with interrupts masked and the MMU and caches off. It exports `main`.
- `#[fault]` on `fn(&FaultInfo) -> !`: the fault handler. Every fault vector (undefined
  instruction, SVC, prefetch abort, data abort, reserved) calls it on the abort stack, with the
  faulting instruction's address, the fault's kind and the interrupted code's LR and SP. It
  exports `rza1_cpu_fault_report(fault_pc, fault_type, sys_lr, sys_sp)`, which a C function of
  that name can define instead. With neither, the core parks (`wfi`). The expansion names
  `rza1_rt::FaultInfo`, so the crate using it depends on `rza1-rt`.
- `#[irq]` on `unsafe fn(u32)`: the IRQ dispatcher, called with each interrupt's ICCIAR value. It
  exports `gic_dispatch`, which `rza1-hal`'s `rt` feature otherwise provides; it is for an image
  that turns that off and dispatches interrupts itself.

The attributes check the signature and reject anything else with a compile error.

## Boot header

The vector table is 8 branches. A board whose first-stage loader checks words or a signature
after it emits them into the `.boot_header` section, labelled `__rza1_boot_header`: link.x places
the section straight after the vector table, at `_start + 0x20`, and pulls the label in by name.
A board with no boot header leaves both out.

| Feature | Effect |
|---|---|
| `rtt` | Fault handlers dump their registers to SEGGER RTT channel 0 before the fault handler runs; the script reserves the RTT buffer (`RTT_RAM`, `NCACHE_RTT_RAM` regions) |

Licensed under either of Apache-2.0 or MIT at your option.
