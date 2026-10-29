# rza1-rt-macros

Attribute macros for [`rza1-rt`](../rza1-rt/README.md), the RZ/A1 runtime. Use them through
`rza1_rt::{entry, fault, irq}` rather than depending on this crate directly.

| Attribute | On | Exports | Called by |
|---|---|---|---|
| `#[entry]` | `fn() -> !` | `main` | the reset handler, once BSS is zeroed |
| `#[fault]` | `fn(&rza1_rt::FaultInfo) -> !` | `rza1_cpu_fault_report` | every fault vector, on the abort stack |
| `#[irq]` | `unsafe fn(u32)` | `gic_dispatch` | the IRQ entry stub, with each interrupt's ICCIAR value |

Each checks the function's signature and reports anything else as a compile error that names
the expected one. `#[fault]`'s expansion names `rza1_rt::FaultInfo`, so the crate using it
depends on `rza1-rt`. `#[irq]` is for an image that turns off `rza1-hal`'s `rt` feature, which
otherwise provides `gic_dispatch`.

Licensed under either of Apache-2.0 or MIT at your option.

Requires a nightly Rust toolchain (the workspace's `rust-toolchain.toml` pins one).
