# deluge-sdk-macros

Procedural macros for the [Deluge SDK](https://crates.io/crates/deluge-sdk).

The only macro today is `#[deluge::app]`, which turns a plain `async fn main`
into a complete firmware entry point — absorbing the platform bring-up (heaps,
clocks, interrupts, executor) and the panic handler that an app author would
otherwise hand-write.

> You normally do **not** depend on this crate directly. It is re-exported by
> `deluge-sdk` as `deluge::app`.

## Usage

```rust,ignore
#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
use deluge::prelude::*;

#[deluge::app]
async fn main(dlg: Deluge) {
    // your async code; the platform is already initialised.
}
```

### Optional `setup`

Pass `#[deluge::app(setup = path::to::fn)]` to run a synchronous function
*after* clocks are up but *before* interrupts are enabled — for peripheral or
GIC bring-up that must happen with IRQs masked:

```rust,ignore
#[deluge::app(setup = setup)]
async fn main(dlg: Deluge) { /* interrupts on, executor running */ }

fn setup() { /* interrupts masked; register ISRs, configure GIC sources */ }
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
