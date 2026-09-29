# deluge-sdk

A user-friendly SDK for building apps that run on the [Synthstrom Deluge]
(Renesas RZ/A1L, Arm Cortex-A9).

> **Crate vs. import name:** this crate is published as **`deluge-sdk`** because
> the name `deluge` is already taken on crates.io. The library name is still
> `deluge`, so all code uses `use deluge::…` unchanged.

It wraps the board support package ([`deluge-bsp`]) and the HAL
([`rza1l-hal`]) behind a single dependency, and provides the
`#[deluge::app]` attribute that absorbs the platform bring-up boilerplate
(heaps, clocks, interrupts, executor, panic handler).

## Quick start

```toml
[dependencies]
deluge = { package = "deluge-sdk", version = "0.1" }
```

```rust,ignore
#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
use deluge::prelude::*;
use embassy_time::Timer;

#[deluge::app]
async fn main(_dlg: Deluge) {
    loop {
        // heaps, clocks, interrupts and the executor are already up.
        Timer::after_millis(200).await;
    }
}
```

## Feature flags

| Feature | Description |
|---|---|
| `alloc` | Register the on-chip SRAM heap (`deluge-alloc`) as the global allocator so `alloc` collections work. Off by default — the SDK is otherwise allocator-free. Required by apps that draw with the (GPL) `deluge-ui-toolkit`. Build with `-Zbuild-std=core,alloc`. |
| `usb-log` | Route the `log` crate to a USB CDC-ACM serial port, so firmware logs appear over the USB cable with no debug probe. |
| `audio-irq` | Drive `dlg.audio()` from the per-block RX DMA interrupt (drift-free, lower latency) instead of the default poll loop. |
| `usb-serial` | Let the app take USB0 as a CDC-ACM serial device with its own VID/PID via `Deluge::usb_serial`. Device-only; mutually exclusive with `usb-log` at runtime. |
| `rtt` | Enable RTT (SEGGER Real-Time Transfer) debug logging in the `#[deluge::app]` runtime. |
| `sim` | Host backend: run the app against the desktop simulator panel (`cargo deluge sim`). Host builds only. |
| `linux` | Linux backend: run the app on the Deluge's Linux userland through `libdeluge` (`cargo deluge linux --features linux`). |

Apps usually forward the backend features as their own, e.g.
`sim = ["deluge/sim"]` and `linux = ["deluge/linux"]` (see
`examples/baremetal/oled_hello`).

`usb-log` takes precedence over `rtt` when both are enabled — only one global
logger may be registered.

## Toolchain

On the device the SDK is `no_std` and targets `armv7a-none-eabihf`. It relies on
nightly features (`impl_trait_in_assoc_type`, build-std), so a nightly toolchain
and `-Zbuild-std` are required; `cargo deluge` passes the right flags for you.
See the [advanced developer guide] for the architecture and internals.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.

[Synthstrom Deluge]: https://synthstrom.com/product/deluge/
[`deluge-bsp`]: https://crates.io/crates/deluge-bsp
[`rza1l-hal`]: https://crates.io/crates/rza1l-hal
[advanced developer guide]: https://github.com/FirestormAudio/deluge-sdk/blob/main/docs/advanced-guide.md
