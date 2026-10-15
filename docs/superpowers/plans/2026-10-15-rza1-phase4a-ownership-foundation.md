# RZ/A1 Phase 4a: Ownership Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the RZ/A1L HAL Embassy's ownership model: peripheral singletons from one `rza1l_hal::init(Config)`, typed interrupt binding, typed DMA channels, and a Deluge `Board` that hands each peripheral to its role, so that enabling both `usb-log` and `usb-serial` fails to compile.

**Architecture:** `embassy-hal-internal`'s `peripherals!` defines the singletons. `rza1l_hal::init` takes them once, then runs the chip bring-up the BSP used to do (CPG, STB, GIC). A new `rza1l_hal::interrupt` module provides typed GIC interrupts, `Handler`/`Binding`, `bind` and the `bind_interrupts!` macro over the existing GIC handler table. `dmac` gains a `Channel` trait, an `InterruptHandler<C>`, and an owned `Dma<'d>`. Its channel-number functions move to `dmac::raw`, where the drivers keep calling them until Phase 4b migrates each driver. `deluge_bsp::Board::new(rza1l_hal::init(BOARD_CONFIG))` replaces `system::init_clocks`. The SDK runtime moves `board.usb0` into exactly one USB owner.

**Tech Stack:** Rust nightly, `embassy-hal-internal` 0.5 (`peripherals!`, `Peri`), the HAL's MMIO test seam (`rza1l_hal::mmio::test`), QEMU ARM host tests, `armv7a-none-eabihf` firmware builds.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 4). This plan is the first of two for Phase 4; Phase 4b migrates the drivers (RSPI, SCIF, SDHI, SSI+SCUX, USB, MTU2, OSTM), removes `raw`, and ends by migrating DelugeFirmware#next.

## Global Constraints

- Layering (spec §2): `rza1l-hal` never names `deluge` in code or depends on a `deluge-*` crate; `wren/` and `examples/baremetal/` reach the platform only through the `deluge` facade; `usb/`, `drivers/` stay board-agnostic; `deluge-bsp` never depends on the simulator. `tools/check-layering.sh` enforces them (it runs first in `tools/test.sh`).
- Phase 4 exit (spec): "no public `unsafe fn new()` constructors; enabling both `usb-log` and `usb-serial` fails to compile instead of panicking." This plan meets the second half; Phase 4b meets the first.
- During migration, "Old `unsafe` free functions move to `#[doc(hidden)] pub mod raw`" (spec).
- No memory-layout change: `tools/link-layout.sh check` must pass after every task.
- Host tests: `cargo test --target armv7-unknown-linux-gnueabihf -p <crate> --lib`. Firmware builds: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p <pkg>`. zsh does not word-split a variable holding a command: write commands out.
- DelugeFirmware#next pins deluge-sdk at `28bc21ce` and is not touched by this plan (user decision: it migrates at the end of Phase 4b).
- Never push; never publish.

## Decisions settled before execution

1. **Plan split.** Phase 4 is two plans (user decision). This one builds the foundation and the USB0 exclusivity; Phase 4b migrates drivers one per task.
2. **The facade drops `pub use deluge_bsp`** (user decision). Apps that need the BSP add `deluge-bsp` as a direct dependency, as they already do for `rza1l-hal`. Nothing in the repo's apps uses the re-export; only `docs/advanced-guide.md` does (lines 52, 76, 93, 380).
3. **Peripheral list follows the RZ/A1L, not the spec's list.** The spec names SCIF0–7 and SSIF0–5, the RZ/A1H's counts. The HAL drives SCIF0–4 (`uart::NUM_CHANNELS = 5`) and the RZ/A1L has SSIF0–3. RSPI0–4, SDHI0/1, USB0/1, DMA_CH0–15, MTU2_0–4, OSTM0/1, SPIBSC0, SCUX and ADC are as the spec lists.
4. **`cpg_basic_init` moves into the HAL** as `cpg::init(frqcr)`, with the board's FRQCR value carried in `Config`. Every register it writes is chip-level (the PL310 power-control register at `0x3FFF_FF80`, FRQCR, SYSCR3); only the FRQCR value is the board's choice. `rza1l_hal::init` keeps today's order: clocks → CPG → STB → GIC.
5. **One handler per interrupt.** `bind_interrupts!` takes `IRQ => Handler;` (one handler), because a GIC slot holds one function; Embassy's multi-handler form has no meaning here.
6. **The handler table becomes atomic** (`AtomicUsize` per slot), so `interrupt::bind` is a safe function a driver constructor can call at any time.
7. **Typed DMA channels are introduced, and their users move in 4b.** `Board` assigns all 16 channels to their roles now. OLED, SD, audio, PIC and MIDI keep using `dmac::raw` with the `system` numbers until their drivers migrate, and a test pins those numbers to the `Board` roles.

## Review Focus

1. **Boot clock bring-up order.** `rza1l_hal::init` must program CPG (FRQCR) before any STBCR module-clock write, as `init_clocks` did. A reordering would boot with the wrong bus clocks. Pinned by `the_pll_is_programmed_before_any_module_clock` (Task 1).
2. **A second `rza1l_hal::init`.** An app or firmware that calls it after the SDK did must get a clear panic before any hardware is touched, not a second bring-up. Pinned by `peripherals_can_be_taken_once` (Task 1), and `init` takes the singletons before its first register write.
3. **Typed DMA writes exactly what the raw calls wrote.** An owned channel's `stop()` and its interrupt setup must hit the same registers and values as `raw::stop`/`raw::register_completion_irq`. Pinned by `stopping_an_owned_channel_writes_what_raw_stop_writes` and `a_channel_is_bound_prioritised_and_enabled_when_built` (Task 3).
4. **The Board's DMA roles and the `system` channel numbers disagree**, giving two owners of one channel while drivers still use numbers. Pinned by `board_dma_roles_name_the_channels_the_drivers_use` (Task 4).
5. **`usb-log` + `usb-serial`.** Together they must fail to compile with a message naming both, and each alone must still build. Pinned by Task 5's build probes.

---

## File Structure

| Path | Responsibility |
|---|---|
| `crates/rza1l-hal/src/cpg.rs` (new) | CPG bring-up (PL310 power control, FRQCR, SYSCR3), moved from the BSP |
| `crates/rza1l-hal/src/lib.rs` | `peripherals!`, `Config`, `init`, re-export `Peri`/`PeripheralType` |
| `crates/rza1l-hal/src/interrupt.rs` (new) | `Interrupt`, `Handler`, `Binding`, `bind`, `typelevel`, `bind_interrupts!` |
| `crates/rza1l-hal/src/gic.rs` | Atomic handler table, `set_handler`/`handler`, `enable`/`set_priority` through the MMIO seam |
| `crates/rza1l-hal/src/dmac.rs`, `crates/rza1l-hal/src/dmac/raw.rs` (new) | `Channel`, `InterruptHandler<C>`, `Dma<'d>`; the channel-number functions in `raw` |
| `crates/deluge-bsp/src/board.rs` (new) | `Board`, `BoardDma` and the per-role channel type aliases |
| `crates/deluge-bsp/src/system.rs` | `BOARD_CONFIG`; loses `cpg_basic_init` and `init_clocks` |
| `crates/deluge-sdk/src/usb0.rs` (new) | The USB0 device-mode interrupt handler and its binding, shared by `usb_debug` and `usb_serial` |
| `crates/deluge-sdk/src/{lib.rs,usb_debug.rs,usb_serial.rs}` | Runtime builds the `Board`, moves `usb0` into one owner; facade drops `pub use deluge_bsp` |

---

### Task 1: Peripheral singletons and `rza1l_hal::init`

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`), `crates/rza1l-hal/Cargo.toml`
- Create: `crates/rza1l-hal/src/cpg.rs`
- Modify: `crates/rza1l-hal/src/lib.rs`

**Interfaces:**
- Produces: `rza1l_hal::peripherals::{OSTM0, OSTM1, RSPI0..RSPI4, SCIF0..SCIF4, SSIF0..SSIF3, SDHI0, SDHI1, USB0, USB1, DMA_CH0..DMA_CH15, MTU2_0..MTU2_4, SPIBSC0, SCUX, ADC}`; `rza1l_hal::Peripherals` (one `pub` field per name); `rza1l_hal::{Peri, PeripheralType}` (re-exported from `embassy_hal_internal`); `pub struct rza1l_hal::Config { pub clocks: clocks::ClockConfig, pub frqcr: u16, pub stb: stb::StbConfig }`; `pub fn rza1l_hal::init(config: Config) -> Peripherals`.

- [ ] **Step 1: Branch and dependency**

```bash
git checkout -b refactor/rza1-ownership
```

In the workspace `Cargo.toml` `[workspace.dependencies]`, after `embassy-futures`, add:

```toml
embassy-hal-internal = "0.5.0"
```

In `crates/rza1l-hal/Cargo.toml` `[dependencies]`, after `embassy-sync`, add:

```toml
embassy-hal-internal = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Append to `crates/rza1l-hal/src/lib.rs`:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod init_tests {
    use super::*;
    use crate::mmio::test;

    const STB: stb::StbConfig = stb::StbConfig {
        stbcr2: 0x6A,
        stbcr3: 0x80,
        stbcr4: 0x00,
        stbcr5: 0x00,
        stbcr6: 0x00,
        stbcr7: 0x24,
        stbcr8: 0x05,
        stbcr9: 0x00,
        stbcr10: 0x00,
        stbcr11: 0xDF,
        stbcr12: 0xFB,
    };

    #[test]
    fn the_pll_is_programmed_before_any_module_clock() {
        test::reset();
        unsafe { stb::init(&STB) };
        let stb_writes = test::writes();

        test::reset();
        unsafe { cpg_then_stb(0x1035, &STB) };
        let writes = test::writes();

        // PL310 power control, then FRQCR, then SYSCR3 (retention-RAM write enable), exactly
        // as the BSP's cpg_basic_init wrote them; then the module clocks, unchanged.
        assert_eq!(
            writes[..3],
            [(0x3FFF_FF80, 1), (0xFCFE_0010, 0x1035), (0xFCFE_0408, 0x0F)]
        );
        assert_eq!(writes[3..], stb_writes[..]);
    }

    #[test]
    #[should_panic(expected = "init called more than once!")]
    fn peripherals_can_be_taken_once() {
        let _first = Peripherals::take();
        let _second = Peripherals::take();
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib init_tests 2>&1 | grep -E "^error|cannot find" | head`
Expected: compile errors — `cpg_then_stb` and `Peripherals` not found.

- [ ] **Step 4: Implement**

`crates/rza1l-hal/src/cpg.rs`:

```rust
//! CPG (clock pulse generator) bring-up: the L2 cache controller's power control, the PLL and
//! divider ratios (FRQCR), and write access to the on-chip data-retention RAM (SYSCR3).

use crate::mmio;

/// PL310 L2 cache controller power-control register (`0x3FFF_F000` + `0xF80`).
const L2_POWER_CTRL: usize = 0x3FFF_FF80;
/// CPG frequency control register: PLL multiplier and the I:G:B:P1:P0 dividers.
const FRQCR: usize = 0xFCFE_0010;
/// System control register 3: write enable for the data-retention RAM banks
/// (`0x2000_0000`–`0x2001_FFFF`).
const SYSCR3: usize = 0xFCFE_0408;

/// Program the clock pulse generator with the board's FRQCR value (the Deluge's `0x1035` is
/// PLL ×30, I:G:B:P1:P0 = 30:20:10:5:5/2). Each write is read back, as the hardware manual
/// requires before the next access. FRQCR2 is not written: it exists only on the RZ/A1H.
///
/// # Safety
/// Reprograms the CPU and bus clocks: call once at boot, before any peripheral is used.
pub(crate) unsafe fn init(frqcr: u16) {
    unsafe {
        mmio::write32(L2_POWER_CTRL, 1);
        let _ = mmio::read32(L2_POWER_CTRL);
        mmio::write16(FRQCR, frqcr);
        let _ = mmio::read16(FRQCR);
        mmio::write8(SYSCR3, 0x0F);
        let _ = mmio::read8(SYSCR3);
    }
}
```

In `crates/rza1l-hal/src/lib.rs`, add `mod cpg;` beside `pub mod clocks;`, and after the `pub const UNCACHED_MIRROR_OFFSET` item add:

```rust
pub use embassy_hal_internal::{Peri, PeripheralType};

embassy_hal_internal::peripherals! {
    OSTM0, OSTM1,
    RSPI0, RSPI1, RSPI2, RSPI3, RSPI4,
    SCIF0, SCIF1, SCIF2, SCIF3, SCIF4,
    SSIF0, SSIF1, SSIF2, SSIF3,
    SDHI0, SDHI1,
    USB0, USB1,
    DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, DMA_CH4, DMA_CH5, DMA_CH6, DMA_CH7,
    DMA_CH8, DMA_CH9, DMA_CH10, DMA_CH11, DMA_CH12, DMA_CH13, DMA_CH14, DMA_CH15,
    MTU2_0, MTU2_1, MTU2_2, MTU2_3, MTU2_4,
    SPIBSC0, SCUX, ADC,
}

/// The board's chip configuration for [`init`].
pub struct Config {
    /// The board's crystal, from which every peripheral clock is derived.
    pub clocks: clocks::ClockConfig,
    /// The CPG frequency-control (FRQCR) value: PLL multiplier and divider ratios.
    pub frqcr: u16,
    /// Which module clocks to run (STBCR2–STBCR12).
    pub stb: stb::StbConfig,
}

/// Bring up the chip and hand out its peripherals: record the clock tree, program the CPG,
/// start the module clocks, and initialise the GIC, in that order.
///
/// # Panics
/// If called more than once (the peripherals are taken before any register is written).
pub fn init(config: Config) -> Peripherals {
    let peripherals = Peripherals::take();
    // SAFETY: `take` succeeded, so this is the one bring-up.
    unsafe {
        clocks::init(config.clocks);
        cpg_then_stb(config.frqcr, &config.stb);
        gic::init();
    }
    peripherals
}

/// CPG, then the module clocks: a module clock must not start before the bus clocks it runs from
/// are programmed.
unsafe fn cpg_then_stb(frqcr: u16, stb: &stb::StbConfig) {
    unsafe {
        cpg::init(frqcr);
        stb::init(stb);
    }
}
```

If the build warns `unexpected cfg condition value: defmt` (the `peripherals!` macro carries a `cfg_attr(feature = "defmt", …)`), add to `crates/rza1l-hal/Cargo.toml`:

```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(feature, values("defmt"))'] }
```

- [ ] **Step 5: Run the tests and the HAL suite**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|FAILED|^error|^warning: unexpected"`
Expected: `test result: ok.` with 158 passed (156 + 2); no `unexpected` warning.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/rza1l-hal
git commit -m "feat(rza1l-hal): peripheral singletons and init(Config), which owns CPG, STB and GIC bring-up"
```

---

### Task 2: Typed interrupts and `bind_interrupts!`

**Files:**
- Create: `crates/rza1l-hal/src/interrupt.rs`
- Modify: `crates/rza1l-hal/src/gic.rs` (handler table `:84-103`, `register` `:370-379`, `enable` `:388-396`, `set_priority` `:432-444`), `crates/rza1l-hal/src/lib.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `rza1l_hal::interrupt::Interrupt` (`const ID: u16`);
  - `rza1l_hal::interrupt::Handler<I>` (`unsafe fn on_interrupt()`);
  - `unsafe trait rza1l_hal::interrupt::Binding<I, H>`;
  - `pub fn rza1l_hal::interrupt::bind<I, H>(binding: impl Binding<I, H>)`;
  - `rza1l_hal::interrupt::typelevel::{DMAINT0..DMAINT15, USB0, USB1}`;
  - the exported macro `rza1l_hal::bind_interrupts!`, used as `bind_interrupts!(pub struct Irqs { USB0 => MyHandler; });`;
  - `pub fn rza1l_hal::gic::set_handler(id: u16, handler: Handler)` and `pub(crate) fn rza1l_hal::gic::handler(id: u16) -> Option<Handler>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rza1l-hal/src/interrupt.rs` holding only the tests for now:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::mmio::test;
    use core::sync::atomic::{AtomicU32, Ordering};

    static CALLS: AtomicU32 = AtomicU32::new(0);

    struct CountingHandler;
    impl Handler<typelevel::USB1> for CountingHandler {
        unsafe fn on_interrupt() {
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
    }

    crate::bind_interrupts!(struct TestIrqs { USB1 => CountingHandler; });

    #[test]
    fn a_bound_handler_runs_for_its_interrupt() {
        bind(TestIrqs);
        let handler = crate::gic::handler(typelevel::USB1::ID).expect("USB1 has a handler");
        handler();
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn interrupt_ids_match_the_hardware_manual() {
        assert_eq!(typelevel::DMAINT0::ID, 41);
        assert_eq!(typelevel::DMAINT15::ID, 56);
        assert_eq!(typelevel::USB0::ID, crate::usb::USB0_IRQ);
        assert_eq!(typelevel::USB1::ID, crate::usb::USB1_IRQ);
    }

    #[test]
    fn enabling_an_interrupt_sets_its_distributor_bit() {
        test::reset();
        unsafe { crate::gic::enable(73) };
        // GICD_ISENABLER2 (0xE820_1100 + 2*4), bit 73 - 64 = 9.
        assert_eq!(test::writes(), [(0xE820_1108, 1 << 9)]);
    }

    #[test]
    fn setting_a_priority_writes_its_byte_lane() {
        test::reset();
        unsafe { crate::gic::set_priority(45, 13) };
        // GICD_IPRIORITYR11 (0xE820_1400 + 11*4), byte lane 45 % 4 = 1, value 13 << 3.
        assert_eq!(test::writes(), [(0xE820_142C, (13 << 3) << 8)]);
    }
}
```

and in `crates/rza1l-hal/src/lib.rs` add `pub mod interrupt;` beside `pub mod gic;`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib interrupt 2>&1 | grep -E "^error" | head`
Expected: compile errors (`Handler`, `typelevel`, `bind`, `bind_interrupts`, `gic::handler` not found).

- [ ] **Step 3: Make the handler table atomic and route enable/priority through the seam**

In `crates/rza1l-hal/src/gic.rs`, replace `HandlerCell` (`:84-103`) with:

```rust
/// One dispatch-table slot: the handler's address, 0 when empty. Atomic, so a handler can be
/// installed while interrupts run (a word store; `gic_dispatch` reads it with one load).
struct HandlerCell(core::sync::atomic::AtomicUsize);

impl HandlerCell {
    const fn new() -> Self {
        HandlerCell(core::sync::atomic::AtomicUsize::new(0))
    }
    fn set(&self, h: Handler) {
        self.0.store(h as usize, core::sync::atomic::Ordering::Release);
    }
    fn get(&self) -> Option<Handler> {
        match self.0.load(core::sync::atomic::Ordering::Acquire) {
            0 => None,
            // SAFETY: every non-zero value was stored by `set` from a `Handler`.
            addr => Some(unsafe { core::mem::transmute::<usize, Handler>(addr) }),
        }
    }
}
```

Remove every `unsafe { … }` wrapper the compiler now reports as unnecessary around `HANDLERS[..].get()` / `.set(..)` calls (in `gic_dispatch` and its damage count), keeping the surrounding logic unchanged.

Replace `register` (`:370-379`) with:

```rust
/// Install `handler` for interrupt `id`. Out-of-range IDs are ignored.
pub fn set_handler(id: u16, handler: Handler) {
    log::trace!("gic: register IRQ {}", id);
    if (id as usize) < INT_ID_TOTAL {
        HANDLERS[id as usize].set(handler);
    }
}

/// The handler installed for `id`, if any.
pub(crate) fn handler(id: u16) -> Option<Handler> {
    HANDLERS.get(id as usize).and_then(HandlerCell::get)
}

/// Register a Rust function as the handler for interrupt `id`.
///
/// # Safety
/// None beyond [`set_handler`]'s; kept `unsafe` for the callers Phase 4b migrates to
/// [`crate::interrupt::bind`].
pub unsafe fn register(id: u16, handler: Handler) {
    set_handler(id, handler);
}
```

In `enable` (`:388-396`) replace the two pointer lines with

```rust
        crate::mmio::write32(GICD_ISENABLER0 + 4 * (id >> 5) as usize, 1u32 << (id & 31));
```

and in `set_priority` (`:432-444`) replace its body after the range check with

```rust
        let addr = GICD_IPRIORITYR0 + 4 * (id / 4) as usize;
        let shift = ((id % 4) * 8) as u32;
        let mut val = crate::mmio::read32(addr);
        val &= !(0xFFu32 << shift);
        val |= ((priority as u32) << GIC_PRIORITY_SHIFT) << shift; // bits [7:3] are the valid field
        crate::mmio::write32(addr, val);
```

(`GICD_ISENABLER0` and `GICD_IPRIORITYR0` are `usize` constants in `gic.rs`; if either is typed otherwise, cast it with `as usize`.)

- [ ] **Step 4: Write the interrupt module**

Prepend to `crates/rza1l-hal/src/interrupt.rs`, above the tests:

```rust
//! Typed interrupts. Each GIC interrupt the HAL drives is a type in [`typelevel`]; a driver's
//! interrupt handler is a [`Handler`] for it; [`bind_interrupts!`](crate::bind_interrupts)
//! declares which handler serves which interrupt. A driver's constructor takes that binding and
//! installs the handler with [`bind`], so a driver can't be built without its interrupt wired.

use crate::gic;

/// A GIC interrupt.
pub trait Interrupt: 'static {
    /// Its GIC interrupt ID.
    const ID: u16;
}

/// An interrupt handler for `I`.
pub trait Handler<I: Interrupt> {
    /// Runs when `I` fires: in IRQ context (SYS mode, with IRQs re-enabled for nesting).
    ///
    /// # Safety
    /// Only the GIC dispatcher calls it, for `I`.
    unsafe fn on_interrupt();
}

/// Proof that `H` is bound to `I`.
///
/// # Safety
/// Implemented only by [`bind_interrupts!`](crate::bind_interrupts).
pub unsafe trait Binding<I: Interrupt, H: Handler<I>> {}

/// Install `H` as the handler for `I`. A driver's constructor calls this with the binding it
/// takes.
pub fn bind<I: Interrupt, H: Handler<I>>(_binding: impl Binding<I, H>) {
    gic::set_handler(I::ID, dispatch_to::<I, H>);
}

fn dispatch_to<I: Interrupt, H: Handler<I>>() {
    // SAFETY: installed only for `I::ID`, so the dispatcher calls this for `I`.
    unsafe { H::on_interrupt() }
}

/// The interrupt types, by name.
pub mod typelevel {
    macro_rules! interrupts {
        ($($name:ident = $id:expr),* $(,)?) => {$(
            #[allow(non_camel_case_types)]
            #[doc = concat!("GIC interrupt ", stringify!($id), ".")]
            pub enum $name {}
            impl super::Interrupt for $name {
                const ID: u16 = $id;
            }
        )*};
    }

    interrupts! {
        // DMAC channel n's completion interrupt is GIC ID 41 + n.
        DMAINT0 = 41, DMAINT1 = 42, DMAINT2 = 43, DMAINT3 = 44,
        DMAINT4 = 45, DMAINT5 = 46, DMAINT6 = 47, DMAINT7 = 48,
        DMAINT8 = 49, DMAINT9 = 50, DMAINT10 = 51, DMAINT11 = 52,
        DMAINT12 = 53, DMAINT13 = 54, DMAINT14 = 55, DMAINT15 = 56,
        USB0 = 73, USB1 = 74,
    }
}

/// Declare which handler serves which interrupt:
///
/// ```ignore
/// rza1l_hal::bind_interrupts!(struct Irqs {
///     USB0 => MyUsbHandler;
/// });
/// ```
///
/// One handler per interrupt: a GIC slot holds one function.
#[macro_export]
macro_rules! bind_interrupts {
    ($vis:vis struct $name:ident { $($irq:ident => $handler:ty;)* }) => {
        #[derive(Copy, Clone)]
        $vis struct $name;
        $(
            // SAFETY: this macro implements `Binding` only for the pairs it is given.
            unsafe impl $crate::interrupt::Binding<$crate::interrupt::typelevel::$irq, $handler>
                for $name {}
        )*
    };
}
```

- [ ] **Step 5: Run the tests and the HAL suite**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|FAILED|^error"`
Expected: `test result: ok.` with 162 passed.

Run: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p deluge-controller 2>&1 | grep -E "^error|^warning: unused unsafe|Finished"`
Expected: `Finished`, no errors (callers of `gic::register` still compile; `unused unsafe` warnings in the HAL are fixed by removing the redundant blocks).

- [ ] **Step 6: Commit**

```bash
git add crates/rza1l-hal
git commit -m "feat(rza1l-hal): typed GIC interrupts and bind_interrupts!; the handler table is atomic"
```

---

### Task 3: Typed DMA channels

**Files:**
- Create: `crates/rza1l-hal/src/dmac/raw.rs`
- Modify: `crates/rza1l-hal/src/dmac.rs`, and every caller of the moved functions in `crates/rza1l-hal/src`, `crates/deluge-bsp/src`, `firmwares/`, `hw-tests/` (Step 4 lists them)

**Interfaces:**
- Consumes: `rza1l_hal::peripherals::DMA_CH0..15`, `Peri` (Task 1); `interrupt::{Interrupt, Handler, Binding, bind, typelevel::DMAINTn}`, `gic::handler` (Task 2).
- Produces:
  - `rza1l_hal::dmac::Channel` (sealed; `const NUMBER: u8`, `type Interrupt: Interrupt`), implemented for `DMA_CH0`–`DMA_CH15`;
  - `rza1l_hal::dmac::InterruptHandler<C: Channel>`;
  - `pub struct rza1l_hal::dmac::Dma<'d>`, with:
    - `fn new<C: Channel>(ch: Peri<'d, C>, irq: impl Binding<C::Interrupt, InterruptHandler<C>> + 'd) -> Self`;
    - `fn number(&self) -> u8`;
    - `unsafe fn init_register_mode(&mut self, chcfg: u32, dst: u32, dmars: u32)`;
    - `unsafe fn init_register_mode_rx(&mut self, chcfg: u32, src: u32, dmars: u32)`;
    - `unsafe fn start_transfer(&mut self, src: u32, count: u32)`;
    - `unsafe fn start_transfer_rx(&mut self, dst: u32, count: u32)`;
    - `async fn wait_transfer_complete(&mut self)`;
    - `fn stop(&mut self)`;
    - `fn transfer_in_flight(&self) -> bool`;
    - `fn current_src(&self) -> u32` and `fn current_dst(&self) -> u32`.
  - `#[doc(hidden)] pub mod rza1l_hal::dmac::raw`, holding every former public `dmac` function, with unchanged signatures.

- [ ] **Step 1: Write the failing tests**

Append to `crates/rza1l-hal/src/dmac.rs` (if it already has a `#[cfg(all(test, not(target_os = "none")))] mod tests`, add these inside it instead and merge the `use` lines):

```rust
#[cfg(all(test, not(target_os = "none")))]
mod typed_tests {
    use super::*;
    use crate::interrupt::{Handler, Interrupt};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH0, DMA_CH4, DMA_CH7, DMA_CH9, DMA_CH15};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::task::{Wake, Waker};

    crate::bind_interrupts!(struct Irqs {
        DMAINT4 => InterruptHandler<DMA_CH4>;
        DMAINT7 => InterruptHandler<DMA_CH7>;
    });

    #[test]
    fn channels_know_their_number_and_interrupt() {
        assert_eq!(<DMA_CH0 as Channel>::NUMBER, 0);
        assert_eq!(<DMA_CH15 as Channel>::NUMBER, 15);
        assert_eq!(<<DMA_CH9 as Channel>::Interrupt as Interrupt>::ID, 41 + 9);
    }

    #[test]
    fn a_channel_is_bound_prioritised_and_enabled_when_built() {
        test::reset();
        let dma = Dma::new(unsafe { DMA_CH4::steal() }, Irqs);
        assert_eq!(dma.number(), 4);
        assert!(crate::gic::handler(45).is_some(), "DMAINT4 has a handler");
        // Priority 13 into IPRIORITYR11 lane 1, then ISENABLER1 bit 13: what
        // raw::register_completion_irq(4) writes.
        assert_eq!(
            test::writes(),
            [(0xE820_142C, (13 << 3) << 8), (0xE820_1104, 1 << 13)]
        );
    }

    #[test]
    fn stopping_an_owned_channel_writes_what_raw_stop_writes() {
        test::reset();
        unsafe { raw::stop(7) };
        let raw_writes = test::writes();

        let mut dma = Dma::new(unsafe { DMA_CH7::steal() }, Irqs);
        test::reset();
        dma.stop();
        assert_eq!(test::writes(), raw_writes);
    }

    struct CountWakes(AtomicU32);
    impl Wake for CountWakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn the_completion_handler_wakes_its_channel() {
        let wakes = Arc::new(CountWakes(AtomicU32::new(0)));
        DMAC_WAKERS[4].register(&Waker::from(wakes.clone()));
        unsafe { <InterruptHandler<DMA_CH4> as Handler<_>>::on_interrupt() };
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib typed_tests 2>&1 | grep -E "^error" | head`
Expected: compile errors (`Channel`, `Dma`, `InterruptHandler`, `raw` not found).

- [ ] **Step 3: Move the channel-number functions into `dmac::raw`**

Run this from the repo root. It cuts each listed public function, with its doc comment and attributes, out of `dmac.rs` into `dmac/raw.rs`, and adds the `raw` module declaration:

```bash
python3 - <<'PYEOF'
import re
src = open('crates/rza1l-hal/src/dmac.rs').read()
names = ['init_with_link_descriptor', 'channel_start', 'stop', 'transfer_in_flight', 'current_src',
         'current_dst', 'init_register_mode', 'register_completion_irq', 'on_dma_int',
         'init_register_mode_rx', 'start_transfer_rx', 'start_transfer', 'wait_transfer_complete',
         'register_block_irq', 'wait_block']
lines = src.split('\n')
moved = []
for name in names:
    pat = re.compile(r'^pub (unsafe |async )?fn ' + re.escape(name) + r'\b')
    i = next(k for k, l in enumerate(lines) if pat.match(l))
    start = i
    while start > 0 and (lines[start-1].startswith('///') or lines[start-1].startswith('#[')):
        start -= 1
    depth, end, seen = 0, i, False
    for k in range(i, len(lines)):
        depth += lines[k].count('{') - lines[k].count('}')
        seen = seen or '{' in lines[k]
        if seen and depth == 0:
            end = k
            break
    moved.append('\n'.join(lines[start:end+1]))
    del lines[start:end+1]
    if start < len(lines) and lines[start] == '' and start > 0 and lines[start-1] == '':
        del lines[start]
out = '\n'.join(lines)
out = out.replace('\nuse ', '\n#[doc(hidden)]\npub mod raw;\n\nuse ', 1)
open('crates/rza1l-hal/src/dmac.rs', 'w').write(out)
header = ('//! The DMAC by channel number: what drivers call until Phase 4b migrates them to\n'
          '//! [`super::Dma`]. Not part of the public API.\n\nuse super::*;\n\n')
import os
os.makedirs('crates/rza1l-hal/src/dmac', exist_ok=True)
open('crates/rza1l-hal/src/dmac/raw.rs', 'w').write(header + '\n\n'.join(moved) + '\n')
PYEOF
grep -c "^pub \(unsafe \|async \)\?fn" crates/rza1l-hal/src/dmac/raw.rs crates/rza1l-hal/src/dmac.rs
```

Expected: `raw.rs:15`, `dmac.rs:0`. (If the `pub mod raw;` line landed inside a doc block rather than before the first `use`, move it to directly after the module doc.)

Inside `dmac.rs`, any remaining non-public code that called a moved function (the handler tables `DMA_INT_HANDLERS`/`DMA_BLOCK_HANDLERS` call `on_dma_int` and the block handler) now calls `raw::on_dma_int(..)` etc.; the compiler names each. In `raw.rs`, private items of `dmac.rs` are reachable through `use super::*;`; anything `super::*` doesn't bring in (a private `const` used only by moved code is fine) gets a `super::` path.

- [ ] **Step 4: Point every caller at `raw`**

```bash
grep -rlE "dmac::(init_with_link_descriptor|channel_start|stop|transfer_in_flight|current_src|current_dst|init_register_mode|register_completion_irq|on_dma_int|init_register_mode_rx|start_transfer_rx|start_transfer|wait_transfer_complete|register_block_irq|wait_block)\b" \
  crates/rza1l-hal/src crates/deluge-bsp/src crates/deluge-sdk/src firmwares hw-tests examples \
  | xargs sed -i -E 's/dmac::(init_with_link_descriptor|channel_start|stop|transfer_in_flight|current_src|current_dst|init_register_mode_rx|init_register_mode|register_completion_irq|on_dma_int|start_transfer_rx|start_transfer|wait_transfer_complete|register_block_irq|wait_block)\b/dmac::raw::\1/g'
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p deluge-controller 2>&1 | grep -E "^error" -A4 | head -30
```

Expected: no errors, or only errors from callers that import the functions by name (`use crate::dmac::{start_transfer, …}` / `use rza1l_hal::dmac::{…}`); rewrite each such import to `…::dmac::raw::{…}` and rebuild until clean.

- [ ] **Step 5: Add the typed layer**

In `crates/rza1l-hal/src/dmac.rs`, after the `pub mod raw;` line's surrounding `use` block, add:

```rust
use core::marker::PhantomData;

use crate::Peri;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

/// A DMAC channel singleton (`peripherals::DMA_CH0`–`DMA_CH15`).
pub trait Channel: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number, 0–15.
    const NUMBER: u8;
    /// Its completion interrupt (DMAINT<NUMBER>).
    type Interrupt: Interrupt;
}

macro_rules! channels {
    ($($ch:ident = $n:literal, $irq:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$ch {}
        impl Channel for crate::peripherals::$ch {
            const NUMBER: u8 = $n;
            type Interrupt = crate::interrupt::typelevel::$irq;
        }
    )*};
}

channels! {
    DMA_CH0 = 0, DMAINT0; DMA_CH1 = 1, DMAINT1; DMA_CH2 = 2, DMAINT2; DMA_CH3 = 3, DMAINT3;
    DMA_CH4 = 4, DMAINT4; DMA_CH5 = 5, DMAINT5; DMA_CH6 = 6, DMAINT6; DMA_CH7 = 7, DMAINT7;
    DMA_CH8 = 8, DMAINT8; DMA_CH9 = 9, DMAINT9; DMA_CH10 = 10, DMAINT10; DMA_CH11 = 11, DMAINT11;
    DMA_CH12 = 12, DMAINT12; DMA_CH13 = 13, DMAINT13; DMA_CH14 = 14, DMAINT14; DMA_CH15 = 15, DMAINT15;
}

/// Channel `C`'s completion interrupt handler: wakes [`Dma::wait_transfer_complete`].
pub struct InterruptHandler<C: Channel> {
    _channel: PhantomData<C>,
}

impl<C: Channel> Handler<C::Interrupt> for InterruptHandler<C> {
    unsafe fn on_interrupt() {
        raw::on_dma_int(C::NUMBER);
    }
}

/// An owned DMAC channel.
///
/// The methods that take bus addresses stay `unsafe`: the DMAC writes wherever it is told.
/// Owning the channel is what the caller no longer has to promise.
pub struct Dma<'d> {
    ch: u8,
    _channel: PhantomData<&'d mut ()>,
}

impl<'d> Dma<'d> {
    /// Take channel `C`, install its completion handler, and enable its interrupt.
    pub fn new<C: Channel>(
        _channel: Peri<'d, C>,
        irq: impl Binding<C::Interrupt, InterruptHandler<C>> + 'd,
    ) -> Self {
        interrupt::bind(irq);
        // SAFETY: this channel's own interrupt, at the DMAC priority raw::register_completion_irq uses.
        unsafe {
            crate::gic::set_priority(C::Interrupt::ID, DMAC_IRQ_PRIORITY);
            crate::gic::enable(C::Interrupt::ID);
        }
        Self { ch: C::NUMBER, _channel: PhantomData }
    }

    /// The channel number.
    pub fn number(&self) -> u8 {
        self.ch
    }

    /// See [`raw::init_register_mode`].
    ///
    /// # Safety
    /// `dst` is the bus address every transfer writes to.
    pub unsafe fn init_register_mode(&mut self, chcfg: u32, dst: u32, dmars: u32) {
        unsafe { raw::init_register_mode(self.ch, chcfg, dst, dmars) }
    }

    /// See [`raw::init_register_mode_rx`].
    ///
    /// # Safety
    /// `src` is the bus address every transfer reads from.
    pub unsafe fn init_register_mode_rx(&mut self, chcfg: u32, src: u32, dmars: u32) {
        unsafe { raw::init_register_mode_rx(self.ch, chcfg, src, dmars) }
    }

    /// See [`raw::start_transfer`].
    ///
    /// # Safety
    /// `src..src + count` must stay valid and unaliased until the transfer completes.
    pub unsafe fn start_transfer(&mut self, src: u32, count: u32) {
        unsafe { raw::start_transfer(self.ch, src, count) }
    }

    /// See [`raw::start_transfer_rx`].
    ///
    /// # Safety
    /// `dst..dst + count` must stay valid and unaliased until the transfer completes.
    pub unsafe fn start_transfer_rx(&mut self, dst: u32, count: u32) {
        unsafe { raw::start_transfer_rx(self.ch, dst, count) }
    }

    /// Wait for the transfer in flight to complete.
    pub async fn wait_transfer_complete(&mut self) {
        raw::wait_transfer_complete(self.ch).await
    }

    /// Stop the channel and release its peripheral-request route.
    pub fn stop(&mut self) {
        // SAFETY: this channel is owned, so no other code drives it.
        unsafe { raw::stop(self.ch) }
    }

    /// Whether a transfer is in flight.
    pub fn transfer_in_flight(&self) -> bool {
        // SAFETY: a read of this owned channel's status.
        unsafe { raw::transfer_in_flight(self.ch) }
    }

    /// The current source address.
    pub fn current_src(&self) -> u32 {
        // SAFETY: a read of this owned channel's register.
        unsafe { raw::current_src(self.ch) }
    }

    /// The current destination address.
    pub fn current_dst(&self) -> u32 {
        // SAFETY: a read of this owned channel's register.
        unsafe { raw::current_dst(self.ch) }
    }
}
```

(If `wait_transfer_complete` in `raw` is not `pub async fn wait_transfer_complete(ch: u8)`, match its real signature; the move in Step 3 kept every signature.)

- [ ] **Step 6: Run the tests, the HAL suite and the builds**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|FAILED|^error"`
Expected: `test result: ok.` with 166 passed.

Run: `tools/link-layout.sh check`
Expected: `==> Link layout matches tools/link-layout.baseline.` (builds every firmware against the moved functions).

- [ ] **Step 7: Commit**

```bash
git add -A crates/rza1l-hal crates/deluge-bsp crates/deluge-sdk firmwares hw-tests examples
git commit -m "feat(rza1l-hal): typed DMA channels; the channel-number functions move to dmac::raw"
```

---

### Task 4: The Deluge `Board`

**Files:**
- Create: `crates/deluge-bsp/src/board.rs`
- Modify: `crates/deluge-bsp/src/lib.rs`, `crates/deluge-bsp/src/system.rs:32-63,228-242`, `crates/deluge-bsp/src/oled.rs:60` (visibility)
- Modify (callers of `init_clocks`): `crates/deluge-sdk/src/lib.rs:591-603`, `firmwares/controller-firmware/src/main.rs:136`, `firmwares/msc-firmware/src/main.rs:103`, `firmwares/app-loader/src/main.rs:105`, `hw-tests/sd-bench/src/main.rs:73`, `hw-tests/uac-host-firmware/src/main.rs:57`, `hw-tests/vbus-probe/src/main.rs:75`, `hw-tests/wp-probe/src/main.rs:73`

**Interfaces:**
- Consumes: `rza1l_hal::{init, Config, Peripherals, Peri, peripherals::*}` (Task 1); `rza1l_hal::dmac::Channel` (Task 3).
- Produces:
  - `deluge_bsp::system::BOARD_CONFIG: rza1l_hal::Config`;
  - `pub struct deluge_bsp::Board { pub usb0: Peri<'static, USB0>, pub usb1: Peri<'static, USB1>, pub dma: BoardDma }`;
  - `pub struct deluge_bsp::board::BoardDma`, with one `Peri<'static, …>` field per role: `scux_ffd0, scux_ffu0, scux_ffd2, scux_ffd1, oled, scux_ffu1, ssi_tx, ssi_rx, scux_ffu2, scux_ffu3, pic_tx, sd_tx, pic_rx, midi_rx, sd_rx, scux_ffd3`;
  - one type alias per role in `deluge_bsp::board` (`OledDma = DMA_CH4`, …);
  - `#[cfg(target_os = "none")] pub fn deluge_bsp::Board::new(p: rza1l_hal::Peripherals) -> Board`.

  `system::init_clocks` is removed.

- [ ] **Step 1: Write the failing test**

Create `crates/deluge-bsp/src/board.rs` with only the test:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::system::*;
    use rza1l_hal::dmac::Channel;

    #[test]
    fn board_dma_roles_name_the_channels_the_drivers_use() {
        // The drivers that still take channel numbers use these constants; each role's
        // singleton must be the same channel, or the channel has two owners.
        assert_eq!(ScuxFfd0Dma::NUMBER, SCUX_FFD0_DMA_CH);
        assert_eq!(ScuxFfd1Dma::NUMBER, SCUX_FFD1_DMA_CH);
        assert_eq!(ScuxFfd2Dma::NUMBER, SCUX_FFD2_DMA_CH);
        assert_eq!(ScuxFfd3Dma::NUMBER, SCUX_FFD3_DMA_CH);
        assert_eq!(ScuxFfu0Dma::NUMBER, SCUX_FFU0_DMA_CH);
        assert_eq!(ScuxFfu1Dma::NUMBER, SCUX_FFU1_DMA_CH);
        assert_eq!(ScuxFfu2Dma::NUMBER, SCUX_FFU2_DMA_CH);
        assert_eq!(ScuxFfu3Dma::NUMBER, SCUX_FFU3_DMA_CH);
        assert_eq!(SsiTxDma::NUMBER, SSI_CONFIG.tx_dma_ch);
        assert_eq!(SsiRxDma::NUMBER, SSI_CONFIG.rx_dma_ch);
        assert_eq!(OledDma::NUMBER, crate::oled::OLED_DMA_CH);
        assert_eq!(PicTxDma::NUMBER, PIC_DMA_TX_CH);
        assert_eq!(PicRxDma::NUMBER, PIC_DMA_RX_CH);
        assert_eq!(MidiRxDma::NUMBER, MIDI_DMA_RX_CH);
        assert_eq!(SdTxDma::NUMBER, SD_DMA_TX_CH);
        assert_eq!(SdRxDma::NUMBER, SD_DMA_RX_CH);
    }
}
```

In `crates/deluge-bsp/src/lib.rs` add `pub mod board;` (alphabetically, after `pub mod battery;`) and `pub use board::Board;` near the top-level items. In `crates/deluge-bsp/src/oled.rs:60`, change `const OLED_DMA_CH: u8 = 4;` to `pub(crate) const OLED_DMA_CH: u8 = 4;`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib board 2>&1 | grep -E "^error" | head`
Expected: compile errors (the `*Dma` aliases not found).

- [ ] **Step 3: Write the Board**

Prepend to `crates/deluge-bsp/src/board.rs`:

```rust
//! The Deluge's peripherals, each in the role the board wires it to. [`Board::new`] finishes the
//! bring-up `rza1l_hal::init` starts, then hands the peripherals out by role.

use rza1l_hal::Peri;
use rza1l_hal::peripherals::*;

/// DMA channel 0: SCUX FFD0 (DVU playback path).
pub type ScuxFfd0Dma = DMA_CH0;
/// DMA channel 1: SCUX FFU0.
pub type ScuxFfu0Dma = DMA_CH1;
/// DMA channel 2: SCUX FFD2 (USB TX SRC path).
pub type ScuxFfd2Dma = DMA_CH2;
/// DMA channel 3: SCUX FFD1 (USB RX SRC path).
pub type ScuxFfd1Dma = DMA_CH3;
/// DMA channel 4: the OLED's RSPI0 transmit.
pub type OledDma = DMA_CH4;
/// DMA channel 5: SCUX FFU1.
pub type ScuxFfu1Dma = DMA_CH5;
/// DMA channel 6: SSIF0 transmit (codec out).
pub type SsiTxDma = DMA_CH6;
/// DMA channel 7: SSIF0 receive (codec in).
pub type SsiRxDma = DMA_CH7;
/// DMA channel 8: SCUX FFU2.
pub type ScuxFfu2Dma = DMA_CH8;
/// DMA channel 9: SCUX FFU3.
pub type ScuxFfu3Dma = DMA_CH9;
/// DMA channel 10: PIC UART transmit.
pub type PicTxDma = DMA_CH10;
/// DMA channel 11: SD card transmit.
pub type SdTxDma = DMA_CH11;
/// DMA channel 12: PIC UART receive.
pub type PicRxDma = DMA_CH12;
/// DMA channel 13: MIDI UART receive.
pub type MidiRxDma = DMA_CH13;
/// DMA channel 14: SD card receive.
pub type SdRxDma = DMA_CH14;
/// DMA channel 15: SCUX FFD3.
pub type ScuxFfd3Dma = DMA_CH15;

/// Every DMA channel, by the role the Deluge gives it.
pub struct BoardDma {
    pub scux_ffd0: Peri<'static, ScuxFfd0Dma>,
    pub scux_ffu0: Peri<'static, ScuxFfu0Dma>,
    pub scux_ffd2: Peri<'static, ScuxFfd2Dma>,
    pub scux_ffd1: Peri<'static, ScuxFfd1Dma>,
    pub oled: Peri<'static, OledDma>,
    pub scux_ffu1: Peri<'static, ScuxFfu1Dma>,
    pub ssi_tx: Peri<'static, SsiTxDma>,
    pub ssi_rx: Peri<'static, SsiRxDma>,
    pub scux_ffu2: Peri<'static, ScuxFfu2Dma>,
    pub scux_ffu3: Peri<'static, ScuxFfu3Dma>,
    pub pic_tx: Peri<'static, PicTxDma>,
    pub sd_tx: Peri<'static, SdTxDma>,
    pub pic_rx: Peri<'static, PicRxDma>,
    pub midi_rx: Peri<'static, MidiRxDma>,
    pub sd_rx: Peri<'static, SdRxDma>,
    pub scux_ffd3: Peri<'static, ScuxFfd3Dma>,
}

/// The Deluge, brought up, with its peripherals by role.
pub struct Board {
    /// USB0: the device/host port on the back panel.
    pub usb0: Peri<'static, USB0>,
    /// USB1: not wired on the Deluge.
    pub usb1: Peri<'static, USB1>,
    pub dma: BoardDma,
}

impl Board {
    /// Finish the bring-up `rza1l_hal::init(system::BOARD_CONFIG)` started (the GIC's SDRAM code
    /// window, MMU, L1/L2 caches, SDRAM controller, OSTM clock and the Embassy time driver),
    /// then hand out the peripherals.
    ///
    /// Safe: only the one `Peripherals` from `rza1l_hal::init` can reach it.
    #[cfg(target_os = "none")]
    pub fn new(p: rza1l_hal::Peripherals) -> Self {
        // SAFETY: once, straight after `rza1l_hal::init`, before any peripheral is used.
        unsafe {
            rza1l_hal::gic::set_sdram_code_window(crate::sdram::WINDOW);
            rza1l_hal::mmu::init_and_enable();
            rza1l_hal::cache::l1_enable();
            rza1l_hal::cache::l2_init();
            crate::sdram::init();
            rza1l_hal::ostm::enable_clock();
            rza1l_hal::time_driver::init();
        }
        Self {
            usb0: p.USB0,
            usb1: p.USB1,
            dma: BoardDma {
                scux_ffd0: p.DMA_CH0,
                scux_ffu0: p.DMA_CH1,
                scux_ffd2: p.DMA_CH2,
                scux_ffd1: p.DMA_CH3,
                oled: p.DMA_CH4,
                scux_ffu1: p.DMA_CH5,
                ssi_tx: p.DMA_CH6,
                ssi_rx: p.DMA_CH7,
                scux_ffu2: p.DMA_CH8,
                scux_ffu3: p.DMA_CH9,
                pic_tx: p.DMA_CH10,
                sd_tx: p.DMA_CH11,
                pic_rx: p.DMA_CH12,
                midi_rx: p.DMA_CH13,
                sd_rx: p.DMA_CH14,
                scux_ffd3: p.DMA_CH15,
            },
        }
    }
}
```

(If `gic::set_sdram_code_window` is a safe function, the compiler's `unused_unsafe` lint does not fire inside a block with other unsafe calls; leave it inside.)

- [ ] **Step 4: Replace `init_clocks` with `BOARD_CONFIG`**

In `crates/deluge-bsp/src/system.rs`, delete `cpg_basic_init` (the `#[inline(always)] unsafe fn cpg_basic_init() { … }` item, `:32-63` plus its remaining lines through its closing brace) and `init_clocks` (`:228-242`), and add after `STB_CONFIG`:

```rust
/// The Deluge's chip configuration for `rza1l_hal::init`: its 13.2256 MHz crystal, FRQCR
/// `0x1035` (PLL ×30, I:G:B:P1:P0 = 30:20:10:5:5/2, as the known-working Deluge firmware and
/// bootloader set it), and the module clocks in [`STB_CONFIG`].
pub const BOARD_CONFIG: rza1l_hal::Config = rza1l_hal::Config {
    clocks: CLOCKS,
    frqcr: 0x1035,
    stb: STB_CONFIG,
};
```

Update the module doc's bring-up list (it names `init_clocks`) to say `Board::new(rza1l_hal::init(BOARD_CONFIG))`.

The SDK, `crates/deluge-sdk/src/lib.rs` `init_platform` (`:591-603`): it becomes

```rust
        unsafe fn init_platform() -> deluge_bsp::Board {
            unsafe {
                // SRAM heap (internal RAM) — initialise before any allocation.
                let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
                let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
                rza1_alloc::SRAM.init(start, size);

                // The chip (CPG, module clocks, GIC), then the board (MMU, caches, SDRAM, OSTM
                // time driver).
                let board =
                    deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG));

                // SDRAM heap — now that the SDRAM window is accessible.
                rza1_alloc::SDRAM.init(SDRAM_BASE as *mut u8, SDRAM_SIZE);
                board
            }
        }
```

with its doc comment's "clocks/MMU/caches/SDRAM/GIC/OSTM time driver" line kept and "Returns the [`deluge_bsp::Board`]." appended; in `run`, `unsafe { init_platform() };` becomes `let board = unsafe { init_platform() };` and, until Task 5 uses it, `let _ = &board;` on the next line.

The seven firmware/hw-test call sites: replace each `unsafe { deluge_bsp::system::init_clocks() };` with

```rust
    let _board = deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG));
```

- [ ] **Step 5: Run the test, the suites and the builds**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib 2>&1 | grep -E "test result|FAILED|^error"`
Expected: `test result: ok.` with 118 passed.

Run: `grep -rn "init_clocks\|cpg_basic_init" crates firmwares hw-tests examples wren --include='*.rs' --include='*.md'`
Expected: no output.

Run: `tools/link-layout.sh check && tools/test.sh > /tmp/claude-1000/p4a-t4.log 2>&1; echo "exit $?"`
Expected: layout matches; `exit 0`.

- [ ] **Step 6: Commit**

```bash
git add -A crates/deluge-bsp crates/deluge-sdk/src/lib.rs firmwares hw-tests
git commit -m "feat(bsp): Board::new(rza1l_hal::init(BOARD_CONFIG)) replaces init_clocks and hands out peripherals by role"
```

---

### Task 5: USB0 has one owner; the facade drops `pub use deluge_bsp`

**Files:**
- Create: `crates/deluge-sdk/src/usb0.rs`
- Modify: `crates/deluge-sdk/src/lib.rs` (module list `:100-104`, re-export `:193-195`, `Deluge::usb_serial` `:376-391`, `run` `:617-656`), `crates/deluge-sdk/src/usb_debug.rs:110-150`, `crates/deluge-sdk/src/usb_serial.rs:50-75`
- Modify: `docs/advanced-guide.md:52,76,93,380`

**Interfaces:**
- Consumes: `deluge_bsp::Board.usb0` (Task 4); `rza1l_hal::interrupt::{Handler, bind, typelevel::USB0}`, `bind_interrupts!` (Task 2).
- Produces:
  - `crate::usb0::{DeviceHandler, Irqs}` (SDK-internal);
  - `pub fn usb_debug::build(usb0: Peri<'static, USB0>) -> (UsbDevice<…>, Sender<…>)`, now safe;
  - `pub(crate) fn usb_serial::stash(usb0: Peri<'static, USB0>)`;
  - `Deluge::usb_serial` takes the stashed port, and panics only on a second call.

  The facade no longer re-exports `deluge_bsp`.

- [ ] **Step 1: Watch the conflict build today**

Run: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p usb_log --features deluge/usb-serial 2>&1 | tail -1`
Expected: `Finished` — the failing state (`usb_log` already enables `usb-log`; both features compile, and the conflict is a runtime panic).

- [ ] **Step 2: The shared USB0 handler**

`crates/deluge-sdk/src/usb0.rs`:

```rust
//! USB0 in device mode: its interrupt handler, bound for whichever SDK feature owns the port
//! (`usb-log` or `usb-serial`, never both).

use rza1l_hal::interrupt::{Handler, typelevel};

/// USB0's device-mode interrupt: the RUSB1 device driver's handler for port 0.
pub(crate) struct DeviceHandler;

impl Handler<typelevel::USB0> for DeviceHandler {
    unsafe fn on_interrupt() {
        rza1l_hal::usb::dcd_int_handler(0);
    }
}

rza1l_hal::bind_interrupts!(pub(crate) struct Irqs { USB0 => DeviceHandler; });
```

In `crates/deluge-sdk/src/lib.rs`, next to the `usb_debug`/`usb_serial` module declarations (`:100-104`), add

```rust
#[cfg(all(target_os = "none", any(feature = "usb-log", feature = "usb-serial")))]
mod usb0;
```

(If `dcd_int_handler` is `unsafe fn`, it is already inside `on_interrupt`'s unsafe context; if it is safe, the call needs no block either way.)

- [ ] **Step 3: `usb_debug::build` takes the port**

In `crates/deluge-sdk/src/usb_debug.rs`, change

```rust
pub unsafe fn build() -> (
```

to

```rust
pub fn build(_usb0: rza1l_hal::Peri<'static, rza1l_hal::peripherals::USB0>) -> (
```

replace its `# Safety` doc section with

```rust
/// Takes USB0: only one of `usb-log` and `usb-serial` can own it. Call before interrupts are
/// enabled (the runtime does).
```

and replace `unsafe { gic::register(USB0_IRQ, || dcd_int_handler(0)) };` with

```rust
    rza1l_hal::interrupt::bind(crate::usb0::Irqs);
```

Keep the remaining `unsafe` blocks inside (`init_device_mode`, the `static mut` buffers): owning USB0 is what makes `build` a once-only call, so their safety comments now cite the `Peri<USB0>` argument. Remove imports the compiler reports unused (`gic`, `USB0_IRQ`, `dcd_int_handler`).

- [ ] **Step 4: `usb_serial` stashes the port for `Deluge::usb_serial`**

In `crates/deluge-sdk/src/usb_serial.rs`, replace `register_irq` with:

```rust
use core::cell::RefCell;
use critical_section::Mutex;
use rza1l_hal::Peri;
use rza1l_hal::peripherals::USB0;

/// USB0, held from the runtime until [`Deluge::usb_serial`](crate::Deluge::usb_serial) takes it.
static PORT: Mutex<RefCell<Option<Peri<'static, USB0>>>> = Mutex::new(RefCell::new(None));

/// Hold USB0 for [`Deluge::usb_serial`](crate::Deluge::usb_serial) and bind its interrupt. The
/// runtime calls this before interrupts are enabled.
pub(crate) fn stash(usb0: Peri<'static, USB0>) {
    rza1l_hal::interrupt::bind(crate::usb0::Irqs);
    critical_section::with(|cs| *PORT.borrow_ref_mut(cs) = Some(usb0));
}

/// Take the stashed USB0, once.
pub(crate) fn take_port() -> Option<Peri<'static, USB0>> {
    critical_section::with(|cs| PORT.borrow_ref_mut(cs).take())
}
```

and make `start` take the port: `pub(crate) fn start(spawner: Spawner, id: UsbIdentity, _usb0: Peri<'static, USB0>) -> UsbSerial`, with its safety comment citing the owned `Peri<USB0>` instead of the take-once guard.

In `crates/deluge-sdk/src/lib.rs`, `Deluge::usb_serial` (`:376-391`) becomes:

```rust
    /// Bring up USB0 as a CDC-ACM serial port and return its halves.
    ///
    /// Device-only, behind the `usb-serial` feature (which can't be enabled together with
    /// `usb-log`: both drive USB0). Takeable once: a second call panics.
    #[cfg(all(target_os = "none", feature = "usb-serial"))]
    pub fn usb_serial(&self, id: usb_serial::UsbIdentity) -> usb_serial::UsbSerial {
        let usb0 = usb_serial::take_port()
            .expect("Deluge::usb_serial() called twice: USB0 is already in use");
        usb_serial::start(self.spawner, id, usb0)
    }
```

(Keep any doc lines of the original that describe `UsbIdentity` or the returned halves.)

- [ ] **Step 5: The runtime moves `usb0` into one owner**

In `run` (`:617-656`): delete the `#[cfg(feature = "usb-serial")] unsafe { crate::usb_serial::register_irq() };` statement and its comment, delete Task 4's `let _ = &board;`, and after `let board = unsafe { init_platform() };` add:

```rust
            // USB0 has one owner: the `usb-log` device or the app's `usb-serial` port.
            #[cfg(all(feature = "usb-log", feature = "usb-serial"))]
            compile_error!(
                "the `usb-log` and `usb-serial` features both drive USB0: enable one of them"
            );
            #[allow(unused_variables)]
            let usb0 = board.usb0;
            #[cfg(feature = "usb-serial")]
            crate::usb_serial::stash(usb0);
```

and change `let usb = unsafe { crate::usb_debug::build() };` to `let usb = crate::usb_debug::build(usb0);`. (With both features, the `compile_error!` reports first; the second move of `usb0` would fail to compile on its own too.)

- [ ] **Step 6: The conflict fails, each feature alone builds**

Run: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p usb_log --features deluge/usb-serial 2>&1 | grep -m1 -o "the \`usb-log\` and \`usb-serial\` features both drive USB0: enable one of them"`
Expected: the message, and the build fails.

Run: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p usb_log 2>&1 | tail -1; cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p blinky --features deluge/usb-serial 2>&1 | tail -1`
Expected: `Finished` twice.

- [ ] **Step 7: Drop the facade's `pub use deluge_bsp`**

In `crates/deluge-sdk/src/lib.rs` (`:186-195`), delete `pub use deluge_bsp;`, and reword the comment above the re-exports to:

```rust
// The allocator is re-exported for apps that place data in SDRAM explicitly; it is a device-only
// escape hatch (no host equivalent). The board and chip crates are not re-exported: an app that
// needs them depends on `deluge-bsp` or `rza1l-hal` directly.
```

`docs/advanced-guide.md`:
- Line 52: delete `use deluge::deluge_bsp;                // re-exported`.
- The sentence after that block becomes: `\`deluge_alloc\` is re-exported only on the device (\`target_os = "none"\`), so an app that uses it directly won't build for the desktop simulator.`
- The paragraph about the HAL not being re-exported becomes: "Neither the board crate nor the chip HAL is re-exported. To use `deluge_bsp` or `rza1l_hal` directly (the bus lock, the GIC, raw MMIO, …), add it as a dependency of your app alongside `deluge`." Its TOML block gains a line `deluge-bsp = "0.1"` above `rza1l-hal = "0.1"`.
- Line 76's "drop straight to `deluge_bsp::*`, or to `rza1l_hal::*` with the direct dependency above" becomes "drop to `deluge_bsp::*` or `rza1l_hal::*` with the direct dependencies above".
- Line 93's `deluge_bsp::system::init_clocks()` becomes `deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG))`.
- Line 380's `use deluge::deluge_bsp::bus;` becomes `use deluge_bsp::bus; // deluge-bsp as a direct dependency`.

Run: `grep -rn "deluge::deluge_bsp" --exclude-dir=target --exclude-dir=superpowers . ; cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p wren-firmware 2>&1 | tail -1`
Expected: no grep output; `Finished`.

- [ ] **Step 8: Suite and commit**

Run: `tools/test.sh > /tmp/claude-1000/p4a-t5.log 2>&1; echo "exit $?"`
Expected: `exit 0`.

```bash
git add -A crates/deluge-sdk docs/advanced-guide.md
git commit -m "feat(sdk): USB0 has one owner, so usb-log + usb-serial fails to compile; the facade drops pub use deluge_bsp"
```

---

### Task 6: Docs, CHANGELOG and the final check

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Changed`), `crates/rza1l-hal/README.md`, `crates/deluge-bsp/README.md`, `crates/deluge-sdk/README.md`, `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 4 section)

- [ ] **Step 1: Docs**

`CHANGELOG.md`, top of `### Changed`:

```markdown
- `rza1l-hal` has Embassy-style ownership. `rza1l_hal::init(Config) -> Peripherals` takes the
  peripheral singletons once and runs the chip bring-up (CPG, module clocks, GIC);
  `rza1l_hal::bind_interrupts!` binds typed interrupt handlers; `dmac::Dma::new(Peri<DMA_CHn>, Irqs)`
  owns a DMA channel. **Breaking:** the channel-number DMA functions moved to the transitional
  `rza1l_hal::dmac::raw`, and `deluge_bsp::system::init_clocks()` is replaced by
  `deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG))`, which returns
  the peripherals by role.
- **Breaking:** enabling both `usb-log` and `usb-serial` is a compile error (both drive USB0)
  instead of a panic at `Deluge::usb_serial()`.
- **Breaking:** the `deluge` facade no longer re-exports `deluge_bsp`; depend on `deluge-bsp`
  directly to use it.
```

`crates/rza1l-hal/README.md`: add rows to the module table for `interrupt` ("Typed GIC interrupts, `Handler`/`Binding` and `bind_interrupts!`") and `cpg` is private (no row); add a short "Bring-up" paragraph above the table: "`rza1l_hal::init(Config)` takes the peripheral singletons (`Peripherals`, Embassy's `peripherals!`) and brings up the CPG, the module clocks and the GIC; drivers take their peripheral as `Peri<'d, T>`." In the `dmac` row, append: "Owned channels: `Dma::new(Peri<DMA_CHn>, Irqs)`; channel-number functions in `dmac::raw` until Phase 4b."

`crates/deluge-bsp/README.md`: add a `board` row to the module table: "`Board::new(rza1l_hal::init(system::BOARD_CONFIG))`: the rest of the bring-up, then the Deluge's peripherals by role (USB0/1, the 16 DMA channels)".

`crates/deluge-sdk/README.md`: in its feature table, the `usb-log` and `usb-serial` rows each gain "(not with `usb-serial`)" / "(not with `usb-log`)".

Spec, directly after the Phase 4 **Exit:** line:

```markdown
**4a done (2026-10-15 plan):** `peripherals!` singletons (RZ/A1L counts: SCIF0–4, SSIF0–3),
`rza1l_hal::init(Config)` with CPG/STB/GIC (CPG moved from the BSP, FRQCR in `Config`), typed GIC
interrupts with one handler per `bind_interrupts!` entry over an atomic handler table, typed DMA
channels with the number-based functions in `dmac::raw`, and `deluge_bsp::Board` holding USB0/1
and the 16 DMA channels by role. USB0 has one owner in the SDK runtime, so `usb-log` + `usb-serial`
fails to compile. The facade dropped `pub use deluge_bsp`. Phase 4b migrates the drivers, removes
`raw` and the `unsafe fn new()` constructors, and migrates DelugeFirmware#next.
```

- [ ] **Step 2: Final verification**

Run: `tools/test.sh > /tmp/claude-1000/p4a-final.log 2>&1; echo "exit $?"; grep -E "test result" /tmp/claude-1000/p4a-final.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'`
Expected: `exit 0`, `failed 0`.

Run: `tools/link-layout.sh check && tools/check-manifests.sh; echo "exit $?"`
Expected: layout matches; `exit 0`.

Run: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p uac-host-firmware 2>&1 | tail -1`
Expected: `Finished`.

Simulator unchanged:

```bash
W=/tmp/claude-1000/p4a-sim; rm -rf $W; mkdir -p $W
(cd examples/baremetal/oled_hello && timeout 300 cargo run -q --manifest-path ../../../tools/cargo-deluge/Cargo.toml \
   --target x86_64-unknown-linux-gnu -- deluge sim --headless --out $W/now > $W/now.log 2>&1; echo "exit $?")
cat $W/now/frame.state
```

Expected: `exit 0`; `cv 0 0 0 0` / `gate 0 0 0 0` / `synced 0`.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md crates/rza1l-hal/README.md crates/deluge-bsp/README.md crates/deluge-sdk/README.md \
  docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md
git commit -m "docs: Phase 4a — peripheral singletons, typed interrupts and DMA, the Deluge Board"
```

- [ ] **Step 4: Hardware smoke test (the human partner's merge gate)**

List these in the final message; the executor cannot run them:

1. `blinky` boots and blinks: the moved CPG/STB/GIC bring-up, in its old order.
2. `deluge-controller` boots, plays audio, and USB MIDI works: firmware-side bring-up through `Board::new`, and the DMA drivers through `dmac::raw`.
3. `usb_log` enumerates and logs over USB: USB0's interrupt bound through `bind_interrupts!`.
4. An app built with `usb-serial` (for example `blinky --features deluge/usb-serial` plus a `dlg.usb_serial(..)` call) enumerates as CDC-ACM.
5. The SD card mounts in `sd_demo` (SD DMA through `dmac::raw`, completion IRQ unchanged).
