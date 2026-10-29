# rza1-hal

Embassy HAL for the **Renesas RZ/A1L** (Cortex-A9, R7S721020) and, untested on hardware, the
RZ/A1H: `no_std` Rust with async drivers over owned peripherals.

Developed for the [Synthstrom Deluge] open-source firmware port, but kept
board-agnostic where possible — board-specific configuration is passed in
at runtime rather than compiled in.

---

Requires a nightly Rust toolchain (the workspace's `rust-toolchain.toml` pins one).

## Target

| Property       | Value                              |
|----------------|------------------------------------|
| CPU            | ARM Cortex-A9, ARMv7-A             |
| Chip           | Renesas RZ/A1L (R7S721020)         |
| Rust target    | `armv7a-none-eabihf`               |
| Async runtime  | [Embassy]                          |
| `no_std`       | yes (host-target unit tests only)  |

---

## Modules

`rza1_hal::init(Config)` takes the peripheral singletons (`Peripherals`, Embassy's
`peripherals!`) and brings up the CPG, the module clocks and the GIC; drivers take their
peripheral as `Peri<'d, T>`.

### System

| Module        | Description |
|---------------|-------------|
| `mmu`         | Flat-map L1 Translation Table (VA = PA, 1 MB sections) covering the full 4 GB address space with correct cache attributes per region (SRAM cached, I/O strongly-ordered, uncached mirror at +0x4000_0000). |
| `cache`       | L1 D/I-cache + Cortex-A9 branch predictor enable; PL310 L2 cache init. Call `mmu::init_and_enable` first. |
| `clocks`      | The clock tree, from the board's EXTAL crystal: `clocks::init(ClockConfig::new(extal_hz))` once at boot (`rza1_hal::init` does it), then P0φ/P1φ (and the exact P0 ticks-per-µs ratio) are derived for every driver. |
| `Config`      | `rza1_hal::init`'s configuration: the crystal, `CpgConfig` (FRQCR), `ModuleClocks` (which STBCR2–STBCR12 module clocks run, named after the manual's MSTP bits) and the DMAC and time-driver interrupt priorities. `Config::new(clocks)` with `const` builders; no `Default`, since the chip has no internal oscillator. |
| `gic`         | ARM GIC-400 (GICD/GICC) driver. `rza1_hal::init` runs `gic::init`, which starts every interrupt at `Priority::DEFAULT` (P16). The IRQ assembly shim calls `gic_dispatch` (the `rt` feature). |
| `interrupt`   | Typed GIC interrupts (`interrupt::typelevel`), `Priority`, `Handler`/`Binding` and `bind_interrupts!`. Each interrupt type has `enable`/`disable`/`pend`/`unpend`/`set_priority`/`get_priority`. A driver's constructor takes the binding, installs its handler and enables it; its priority is the application's. |
| `bsc`         | Bus State Controller CS0/CS1 timing (NOR flash). |
| `memmap`      | Pure memory-map and cache-line arithmetic shared by `mmu` and `cache` (host-testable). |

### Memory

The SRAM and SDRAM heaps live in the separate
[`rza1-alloc`](../rza1-alloc/README.md) crate.

The RZ/A1L provides **uncached mirror aliases** of SRAM and SDRAM at
`physical_address + 0x4000_0000` (see `UNCACHED_MIRROR_OFFSET`). DMA-shared
buffers must be accessed through these aliases to avoid stale cache lines
without explicit cache maintenance.

### Timers

| Module        | Description |
|---------------|-------------|
| `ostm`        | OS Timer — two 32-bit channels clocked from P0φ = EXTAL × 2.5 (see `clocks`; 33.064 MHz on the Deluge). Owned by the time driver; `delay_ms`/`Delay` busy-wait on OSTM0's counter. |
| `time_driver`  | Embassy `time-driver` implementation backed by OSTM0 (free-running) + OSTM1 (alarm). Behind the `time-driver-ostm` feature (on by default). `init` starts it, and OSTM0/OSTM1 are then not in `Peripherals`. |
| `time_math`   | Pure tick/µs conversion and wrap arithmetic behind `time_driver` (host-testable). |
| `mtu2`        | Multi-Function Timer Pulse Unit 2: five 16-bit channels. `OneShot::new(Peri<MTU2_n>, prescaler, priority, Irqs)` for compare-match one-shots and periodic ticks. |

### DMA

| Module | Description |
|--------|-------------|
| `dmac` | 16-channel DMAC (channels 0–15). Register-mode (one-shot) and link-descriptor (circular) transfers. Owned channels: `Dma::new(Peri<DMA_CHn>, Irqs)` for awaited transfers, `Dma::new_without_interrupt` for polled rings (DMAINT stays masked), `BlockDma::new(Peri<DMA_CHn>, Irqs)` with a `BlockInterruptHandler` for per-block descriptor rings; `stop_all` for hand-off code. Drivers hold their `Dma` handles. |

DMAC resource selectors (DMARS values) are peripheral-specific constants
documented in each peripheral module.

### Audio

| Module | Description |
|--------|-------------|
| `ssi`  | SSIF (I²S) driver on SSIF0/1/3: `Ssi::new` (full duplex), `new_rx_only`, `new_with_scux_tx`, each taking the channel, its pins, a `ring_buffer::RingBuf` per direction and an `ssi::Config`. The rings are `ring_buffer::{WritableRing, ReadableRing}`. |
| `scux` | Sample Rate Conversion Unit. `Scux::new` splits it into four channels; a channel becomes a `Path` (FFD ring → SRC → DVU → FFU ring, or → an SSI on the routes the hardware has, checked at compile time). `Path::split` gives its `Control` (volume, ramps, input rate) and its rings. |
| `ring_buffer` | DMA rings lent by the caller (`dma_buf::UncachedMut<u32>`): an embassy-style copy API (`write`/`read`, `Underrun`/`Overrun`) and a positional window (`position()`, the whole ring) usable from interrupt context. |

### Peripherals

| Module  | Description |
|---------|-------------|
| `sdhi`  | SD Host Interface — two ports (SDHI0/SDHI1). `Sdhi::new(Peri<SDHIn>, clk, cmd, d0, d1, d2, d3, cd, wp, tx: Dma, rx: Dma, Irqs, sd_option)` takes its eight pins (`sdhi::{ClkPin, CmdPin, D0Pin…D3Pin, CdPin, WpPin}`), muxes them and binds the status and operation interrupts; its methods give async `send_cmd`, `read_blocks_sw` / `write_blocks_sw` (PIO with `AtomicWaker`) and `read_blocks_dma` / `write_blocks_dma` (DMAC-backed); `reset` returns it to the identification clock before a card is re-identified. |
| `uart`  | SCIF async serial, five channels (SCIF0–4). `Uart::new(Peri<SCIFn>, tx: TxPin, rx: RxPin, rx_dma, tx_dma, Irqs, baud)` muxes its pins and receives into a DMA ring (SCIF0/1) and transmits by DMA or through the FIFO; `split()` gives `UartRx`/`UartTx`. |
| `rspi`  | SPI master (RSPI0–2, the RZ/A1L's three): `Rspi::new_txonly(Peri<RSPIn>, sck, mosi, bit_rate)` or `Rspi::new(.., sck, mosi, miso, bit_rate)` (pins by `SckPin`/`MosiPin`/`MisoPin`), 32-bit or 8-bit frames, blocking sends. Used for CV DAC and OLED. |
| `gpio`  | Owned pins: every pin is a `Peripherals` singleton (`P0_0`…`P9_5`), taken into `Input`, `Output` (`OutputCapable` pins only) or `Flex`, which implement the `embedded-hal` digital traits. Levels go through PSRn/PNOTn; `Flex::set_as_alternate` selects a function for pins no driver takes. Pin-function traits (`uart::TxPin<T>` …) are generated from the manual by `rza1/tools/gen-pins.py`. `read_port(Port)` reads a whole port. |
| `adc`   | 12-bit A/D converter, single-scan polled. |
| `spibsc` | SPI Multi-I/O Bus Controller — manual-command NOR-flash erase/program/read, with erase and program refused outside the board's writable windows. |
| `usb` | RUSB1 USB 2.0 dual-role driver: chip plumbing (bases, IRQs, STBCR7 clock gates) plus full `embassy-usb-driver` **device** (`usb::driver`) and **host** (`usb::host`) implementations over crate-private pipe/FIFO/register layers. `usb::init_device_mode(Peri<USBn>, Irqs)` / `usb::init_host_mode(Peri<USBn>, Irqs)` are the entry points, bound to a `DeviceInterruptHandler`, `HostInterruptHandler` or `DualRoleInterruptHandler`; the drivers borrow the port (`Rusb1Driver<'d>`). Isochronous endpoints can become `usb::iso` streams (`into_iso_stream`), whose consumer runs in the USB interrupt; the host's isochronous IN rings are lent with `Rusb1HostDriver::lend_iso_in_buffers`. USB *class* code (UAC2/MIDI/MSC/BOT) lives in `deluge_bsp::usb`. |

---

## Getting started

Add to `Cargo.toml`:

```toml
[dependencies]
rza1-hal = { path = "../rza1-hal" }
embassy-executor = { version = "0.10", features = ["platform-cortex-ar", "executor-thread"] }
embassy-time    = "0.5"
```

Minimal bare-metal init sequence:

```rust,ignore
use rza1_hal::{Config, ModuleClock, ModuleClocks, clocks::ClockConfig};

const MY_CONFIG: Config = Config::new(ClockConfig::new(13_333_333))
    .with_module_clocks(ModuleClocks::new().with(ModuleClock::Rspi0));

fn main() -> ! {
    // The crystal's clock tree, the CPG, module clocks, the GIC and (with `time-driver-ostm`)
    // the Embassy time driver on OSTM0/OSTM1; the singletons, once.
    let p = rza1_hal::init(MY_CONFIG);
    unsafe {
        rza1_hal::mmu::init_and_enable();
        rza1_hal::cache::l1_enable();
        rza1_hal::cache::l2_init();
    }
    // Drivers leave interrupt priorities to you; every interrupt starts at Priority::DEFAULT.
    // Hand the rest of `p` to your drivers, e.g. `rspi::Rspi::new_txonly(p.RSPI0, p.P6_0, p.P6_2, 4_000_000)`.
    // …
}
```

Drivers take their peripheral singleton (`Peri<'d, T>`) and the interrupt binding they need, so
`Rspi::new`, `Uart::new` and `OneShot::new` are safe. Every driver takes its
singleton and binding. The SSI and SCUX drivers hold no buffers: their rings are the caller's.

### Registers

Register addresses come from [`rza1-pac`](../rza1-pac/README.md), re-exported as
`rza1_hal::pac` and built with its `rza1l` feature: chiptool output from the vendor SVD (an RZ/A1H
SVD) minus what the RZ/A1L lacks, generated and checked against the RZ/A1L manual's register list
by `tools/gen-pac.sh`. Every access goes through the PAC's MMIO seam (`rza1_pac::mmio`), so
register sequences are testable on the host. Every module takes its registers from
it, and `tools/check-register-tables.sh` checks the whole crate for hand-written addresses (only
`memmap.rs` and `mmu.rs`, which describe memory rather than registers, are exempt).

---

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `rza1l` | | The RZ/A1L (R7S721020 and family). Enable exactly one chip feature. |
| `rza1h` | | The RZ/A1H: its extra driver instances (RSPI3/RSPI4, the larger interrupt table). Experimental: compiles, not validated on hardware; its pin tables are not filled in, so drivers that take pins are RZ/A1L-only for now. |
| `rt` | yes | Export `gic_dispatch`, which `rza1-rt`'s IRQ entry calls. Turn off only to dispatch interrupts yourself (`#[rza1_rt::irq]`). |
| `critical-section-impl` | yes | The `critical-section` implementation (single core: IRQ masking; FIQ stays enabled). Turn off to bring your own. |
| `time-driver-ostm` | yes | The Embassy time driver on OSTM0 and OSTM1 (`time_driver`). Turn off to bring your own driver; OSTM0/OSTM1 are then in `Peripherals`. |
| `log` | | Log through the `log` crate. |
| `defmt` | | Log through `defmt`, and derive `defmt::Format` on the public types. Not with `log`. |
| `unstable-pac` | | Re-export the register-level PAC as `rza1_hal::pac`. Not covered by semver. |

`unstable-driver-steal` (hidden, not covered by semver) adds `Dma::steal`, `Rspi::steal` and
`UartRx::steal`, for panic and fault paths that must reach an initialised driver without its
owner.

---

## Safety

Drivers are safe to construct and use: each takes its peripheral singleton and its interrupt
binding, so two owners of one peripheral can't exist. The `unsafe fn`s left are system-level
(`mmu`, `cache`, `gic` set-up, `bsc`, `spibsc` flash writes, `dmac::stop_all`), and each says in
its `# Safety` section what the caller upholds. DMA buffers are `dma_buf::{Uncached,
UncachedMut}`, checked to lie in uncached memory.

---

## Crate structure policy

This crate is board-agnostic — it does not hard-code DMA channel numbers,
GPIO pin assignments, or clock configurations. Those belong in a board
support layer (BSP). Peripheral drivers accept configuration by
value (e.g. an SSICR value, `ModuleClocks`) so the same HAL binary can target
different board layouts.

[Embassy]: https://embassy.dev
