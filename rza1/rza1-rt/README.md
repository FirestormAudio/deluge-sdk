# rza1-rt

Runtime for the Renesas RZ/A1L (ARM Cortex-A9, R7S721020): the reset vector with the boot
loader's metadata words, the reset handler (cache/TLB invalidate, VFP/NEON enable, per-mode
stacks, BSS zero, then `main`), the fault handlers and the GIC IRQ entry stub, plus the linker
script.

A board crate depends on it, links it with `use rza1_rt as _;`, and puts a `memory.x` on the
linker search path defining the `RAM` (and optionally `SDRAM`) regions, the per-mode stack sizes
and `INTERNAL_RAM_END`; see the script's header for the full list. The application's `build.rs`
passes `-Tlink.x` (`cargo:rustc-link-arg-bins=-Tlink.x`).

| Feature | Effect |
|---|---|
| `rtt` | Fault handlers dump their registers to SEGGER RTT channel 0; the script reserves the RTT buffer (`RTT_RAM`, `NCACHE_RTT_RAM` regions) |
| `app-fault-reporter` | Faults call the application's `rza1_cpu_fault_report(fault_pc, fault_type, sys_lr, sys_sp)` instead of blinking the status LED |

Licensed under either of Apache-2.0 or MIT at your option.
