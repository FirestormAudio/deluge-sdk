#![cfg_attr(target_os = "none", no_std)]
// `#[embassy_executor::task]` (used by `usb::host`'s supervisor and per-device
// tasks) expands to an associated type with `impl Trait`. Only needed on the
// bare-metal target, where those tasks are compiled.
#![cfg_attr(target_os = "none", feature(impl_trait_in_assoc_type))]
#![allow(dead_code)]

// The runtime (reset vector, exception entry, linker script). Nothing calls into it by name,
// so this is what links it into every image built on the BSP.
#[cfg(target_os = "none")]
use rza1_rt as _;
// The Deluge's boot header, in the `.boot_header` section rza1-rt places after the vector table.
#[cfg(target_os = "none")]
mod boot_header;
// The image's fault handler, unless the application brings its own.
#[cfg(all(target_os = "none", feature = "fault-blink"))]
mod fault_blink;
/// The image's entry point: `#[entry] fn main() -> !`, called once the reset handler has zeroed
/// BSS. (`#[deluge::app]` generates it for an SDK app.)
#[cfg(target_os = "none")]
pub use rza1_rt::entry;

pub mod audio;
#[cfg(target_os = "none")]
pub mod audio_block;
pub mod battery;
/// The Deluge's peripherals by role ([`Board`]).
pub mod board;
pub use board::Board;
pub mod bus;
pub mod controls;
#[cfg(target_os = "none")]
pub mod cpu_load;
pub mod cv_gate;
pub mod encoder;
/// Pure quadrature detent accumulation used by the bare-metal `encoder` driver;
/// non-gated so it unit-tests on the host.
pub mod encoder_detent;
/// Pure two-pin quadrature decoding with glitch rejection, used by the `encoder` driver;
/// non-gated so it unit-tests on the host.
pub mod encoder_quadrature;
// `fat` builds on `sd`, and `midi_gate` pulls in `cortex_ar` — both depend on
// items only available on the bare-metal target, so they are excluded from the
// host/QEMU test build (the pure-logic modules below still compile there).
#[cfg(all(target_os = "none", feature = "fat"))]
pub mod fat;
/// Deluge SPI-NOR flash profile (chip geometry + board memory map).  Pure
/// data + re-exports over the SoC `rza1_hal::spibsc` controller.  Opt-in via
/// the `flash` feature — only the app-loader writes to flash.
#[cfg(feature = "flash")]
pub mod flash;
/// Host builds: the hook a simulated front panel plugs into.
#[cfg(not(target_os = "none"))]
pub mod host_panel;
pub mod jacks;
#[cfg(target_os = "none")]
pub mod midi_gate;
pub mod midi_stream;
pub mod oled;
pub mod pads;
pub mod pic;
pub mod rgb;
/// Write-ahead planning for a sample ring a DMA reads; non-gated so it unit-tests on the host.
pub mod ring_follow;
/// Pure audio sample-format conversion + dither used by the bare-metal
/// `audio_block` ring driver; non-gated so it unit-tests on the host.
pub mod sample_fmt;
pub mod scux_dvu_path;
pub mod scux_src_path;
pub mod scux_usb_tx_path;
/// SD card driver: real SDHI1 hardware on the firmware target, a file-backed
/// disk image standing in for the card on host/QEMU. See the module docs.
pub mod sd;
pub mod sdram;
pub mod system;
#[cfg(target_os = "none")]
pub mod trigger_clock;
pub mod uart;
// Not gated to `target_os = "none"` as a whole: `usb::host`'s class drivers are
// generic over `UsbHostAllocator` and are unit-tested under QEMU on
// armv7-unknown-linux-gnueabihf. The device-side children that need
// `embassy-usb` are gated individually inside the module.
pub mod usb;

// RSPI0 is shared by the OLED DMA path and the CV DAC; `bus` arbitrates it as
// an owned, mutex-guarded resource. See the Advanced developer guide
// (`docs/advanced-guide.md`, §7 — *Dropping down to the BSP & HAL*).
