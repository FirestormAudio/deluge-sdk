# rza1-hal Phase 2: GPIO Ownership, Atomic Writes and Pin Traits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pins become owned singletons; `Input`/`Output`/`Flex` replace the raw `gpio::*` functions and write atomically; the SCIF, RSPI and SDHI drivers take their pins and set the pin functions themselves. Every downstream crate migrates in the same phase.

**Architecture:**
- Every RZ/A1L pin (Table 41.8) is a `peripherals!` singleton.
- `gpio::{Input, Output, Flex}<'d>` are built from `Peri<'d, impl Pin>` and write through the set/reset registers (PSRn, PMSRn, PMCSRn, PNOTn). Only registers without a set/reset twin are read-modify-written, inside a critical section.
- Pin-function traits (`scif::TxPin<T>` …) are generated from the manual's per-port function tables by a checked-in script. A hand-written overrides file covers the rows the PDF extraction mangled.
- `deluge-bsp`'s `Board` hands out pins by role. BSP modules, the facade, firmwares and hw-tests stop calling raw `gpio::*`.

**Tech Stack:** Rust nightly, embassy-hal-internal 0.5 `peripherals!`, embedded-hal 1.0 digital traits, python3 (generator), QEMU ARM host tests.

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md` (§4 "GPIO follows embassy", §5 phase 2). Phase 1's plan, `docs/superpowers/plans/2026-10-21-rza1-hal-phase1-rename-and-foundations.md`, is the model for layout and verification.

## Global Constraints

- Spec §4, verbatim:
  - "pin singletons (`P1_0` …) go into `peripherals!`";
  - "`Input` / `Output` / `Flex<'d>` are built from `Peri<'d, impl Pin>`";
  - "per-peripheral pin traits (`pin_trait!`-style, e.g. `TxPin<T>`, `SckPin<T>`) let each driver set the pin function itself";
  - "writes go through the PAC's masked set/clear register (`psr(n)`), so they're atomic";
  - "`Pin::steal()` and the free `set_pin_mux` are no longer public."
- No behaviour change on the Deluge:
  - every pin keeps its function number, PIPC, PIBC, direction and initial level;
  - `tools/link-layout.sh check` matches;
  - the full suite (`tools/test.sh`) passes;
  - every firmware, hw-test and example builds with `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p <pkg>`.
- **Lockstep:** each task updates every downstream crate it breaks (`deluge-bsp`, `deluge-sdk`, firmwares, hw-tests, examples). DelugeFirmware is not migrated (spec: phase 9).
- Layering rule 1 holds: nothing under `rza1/` names `deluge`. The generator and the overrides file name only chip signals.
- Only `rza1l` gets pin-function tables (spec decision on minimal `rza1h`). With `rza1h`, the pin singletons for ports 0–9 exist but no pin traits are implemented, and `rza1/tools/test.sh`'s `rza1h` build must still pass.
- **Manual handling:** the manual (`rza1/vendor/docs/rza1/rza1l-hw-manual/ch41-ports.md`) is gitignored. The generated table is checked in, and its `--check` mode skips with a notice when the manual is absent, as `gen-pac.sh` does.
- Never push or publish.

## Review Focus

1. **A pin function that the generated table gets wrong.** The PDF extraction drops empty cells in some rows; P3_15 parses TxD1 as alternative 6, but the Deluge drives it as 5 and that works. A wrong table entry silently muxes the wrong function. Pinned by Task 2's `deluge_pin_functions_match_the_known_working_muxes` test, and by the generator refusing short rows.
2. **An input-only pin used as an output.** P0_0–P0_3 and P1_8–P1_15 have no port-mode output. Pinned by `OutputCapable` being implemented only for pins whose table row has a port-mode output (Task 1's `output_capable_pins_are_the_tables_outputs` test).
3. **A glitch when a pin becomes an output.** If the direction switches before the level is written, the pin drives its stale P value for a moment; on the Deluge that pulses the CV DAC chip select or the codec reset. Pinned by Task 1's `output_new_writes_the_level_before_the_direction` test.
4. **Dropping an `Input`/`Output`.** Embassy returns a dropped pin to its reset state; doing that here would release the SDRAM bus or the codec reset when a long-lived binding is dropped. Pinned by Task 1's `dropping_an_output_leaves_the_pin_as_it_was` test: dropping changes no register.
5. **A Table 41.7 function (TIOCnx, SSITxD0/1/3, WDTOVF) muxed with PIPC=1.** The manual wants PIPC=0 and PM=0 for these. The Deluge muxes SSITxD0 with PIPC=1 today and audio works, so this phase keeps that: `Flex::set_as_alternate` sets PIPC=1, as `set_pin_mux` did. A separate `set_as_alternate_software_io` covers Table 41.7. Pinned by Task 1's two `set_as_alternate*` tests; Phase 5's SSI driver applies Table 41.7 properly.

---

## File Structure

| Path | Responsibility |
|---|---|
| `rza1/rza1-hal/src/gpio.rs` → `rza1/rza1-hal/src/gpio/mod.rs` | `Pin`, `AnyPin`, `Level`, `Alternate`, `Input`, `Output`, `Flex`, `read_port`; register helpers |
| `rza1/rza1-hal/src/gpio/pins.rs` (new) | `pins!` macro: singletons' `Pin` impls (port, bit, `OutputCapable`) |
| `rza1/rza1-hal/src/gpio/rza1l_functions.rs` (new, generated) | `pin_function!` invocations for SCIF, RSPI, SDHI signals |
| `rza1/rza1-hal/pins/rza1l-overrides.yaml` (new) | Hand-resolved rows the extraction mangled, each with its source note |
| `rza1/tools/gen-pins.py` (new) | Parse ch41's Table 41.11–41.29 rows → `rza1l_functions.rs`; `--check` |
| `rza1/rza1-hal/src/{uart,rspi,sdhi}.rs` | Pin traits; constructors take pins |
| `rza1/rza1-hal/src/lib.rs` | Pin singletons in `peripherals!` |
| `crates/deluge-bsp/src/board.rs` | `BoardPins`: pins by role |
| `crates/deluge-bsp/src/*`, `crates/deluge-sdk/src/{sync_led,plat/device,lib}.rs`, firmwares, hw-tests | Owned pins instead of `gpio::*` |

---

### Task 1: Pin singletons, `Input`/`Output`/`Flex`, atomic writes

**Files:**
- Move: `rza1/rza1-hal/src/gpio.rs` → `rza1/rza1-hal/src/gpio/mod.rs` (`git mv`)
- Create: `rza1/rza1-hal/src/gpio/pins.rs`
- Modify: `rza1/rza1-hal/src/lib.rs` (`peripherals!`)
- Test: `rza1/rza1-hal/src/gpio/mod.rs` tests

**Interfaces:**
- Produces:

```rust
// gpio/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)] #[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Level { Low, High }                       // + From<bool>, Into<bool>
#[derive(Clone, Copy, Debug, PartialEq, Eq)] #[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Alternate { Alt1 = 1, Alt2, Alt3, Alt4, Alt5, Alt6, Alt7, Alt8 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port { P0 = 0, P1, P2, P3, P4, P5, P6, P7, P8, P9 }

pub trait Pin: PeripheralType + sealed::Sealed + Into<AnyPin> + 'static {
    fn port(&self) -> Port;
    fn bit(&self) -> u8;                            // 0–15
    fn degrade(self) -> AnyPin;
}
/// Has a port-mode output (Table 41.8 / the function tables): not P0_*, not P1_8–P1_15.
pub trait OutputCapable: Pin {}
pub struct AnyPin { /* port, bit */ }               // impl Pin, PeripheralType

pub struct Flex<'d> { /* Peri<'d, AnyPin> */ }
impl<'d> Flex<'d> {
    pub fn new(pin: Peri<'d, impl Pin>) -> Self;    // touches no register
    pub fn set_as_input(&mut self);                 // PMC=0 (PMCSR), PM=1 (PMSR), PIBC=1 (RMW)
    pub fn set_as_output(&mut self, initial: Level); // panics if the pin has no port-mode output
                                                    // (Flex holds an AnyPin, so this is a runtime check)
    pub fn set_as_alternate(&mut self, alt: Alternate);              // = old set_pin_mux: PFC/PFCE/PFCAE, PMC=1, PIPC=1
    pub fn set_as_alternate_software_io(&mut self, alt: Alternate, output: bool); // Table 41.7: PIPC=0, PM from `output`
    pub fn enable_input_buffer(&mut self);          // PIBC=1 (an alternate pin's level readable)
    pub fn is_high(&self) -> bool; pub fn is_low(&self) -> bool;     // PPR
    pub fn set_level(&mut self, level: Level);      // PSR
    pub fn set_high(&mut self); pub fn set_low(&mut self);
    pub fn toggle(&mut self);                       // PNOT
    pub fn is_set_high(&self) -> bool;              // P
}
pub struct Input<'d>(Flex<'d>);   impl Input<'d>  { pub fn new(pin: Peri<'d, impl Pin>) -> Self; is_high/is_low; get_level() -> Level }
pub struct Output<'d>(Flex<'d>);  impl Output<'d> { pub fn new(pin: Peri<'d, impl OutputCapable>, initial: Level) -> Self; set_high/set_low/set_level/toggle/is_set_high }
/// All 16 pin levels of `port` in one PPR read: reading changes nothing, so it needs no ownership
/// (the encoder ISR reads port 1 this way).
pub fn read_port(port: Port) -> u16;
```

- `Input` and `Output` implement embedded-hal 1.0 `InputPin`, and `OutputPin`/`StatefulOutputPin` respectively, with `Error = Infallible`. `Flex` implements all three.
- `Drop` changes no register (Review Focus 4).
- Write ordering for `set_as_output`: PSR (level) → PMCSR (PMC=0) → PIPC (RMW clear) → PMSR (PM=0).
- Register addresses (GPIO base `0xFCFE_3000`, port `n` at `+4n`): P `+0x0000`, PSR `+0x0100`, PPR `+0x0200`, PM `+0x0300`, PMC `+0x0400`, PFC `+0x0500`, PFCE `+0x0600`, PNOT `+0x0700`, PMSR `+0x0800`, PMCSR `+0x0900`, PFCAE `+0x0A00`, PIBC `+0x4000`, PIPC `+0x4200`. A set/reset write is `(mask << 16) | (value & mask)`.
- Singletons in `lib.rs` `peripherals!`, per Table 41.8:
  - `P0_0`–`P0_3`, `P1_0`–`P1_15`, `P2_0`–`P2_9`, `P3_0`–`P3_15`, `P4_0`–`P4_7`, `P5_0`–`P5_15`, `P6_0`–`P6_15`, `P7_0`–`P7_11`, `P8_0`–`P8_15`, `P9_0`–`P9_5`.
  - P8 and P9 exist only on the 208/233-pin packages; say so in their docs.
- `OutputCapable` covers every pin except `P0_*` and `P1_8`–`P1_15`. `P1_0`–`P1_7` are open-drain outputs; say so in the docs.
- The old free functions, `Pin<PORT, BIT, MODE>`, `Output`/`Input` markers and `steal` stay for now as `#[deprecated]`. Rename the old marker types to `OutputMode`/`InputMode` so the new `Input`/`Output` names are free. Task 5 deletes them.

- [ ] **Step 1: Write the failing tests** (in `gpio/mod.rs`, using `mmio::test`)

```rust
#[test] fn output_new_writes_the_level_before_the_direction() {
    test::reset();
    let _led = Output::new(unsafe { P6_7::steal() }, Level::High);
    assert_eq!(test::writes(), [
        (0xFCFE_3118, (1 << 23) | (1 << 7)),   // PSR6: P6_7 high
        (0xFCFE_3918, 1 << 23),                // PMCSR6: PMC6_7 = 0 (port mode)
        (0xFCFE_7218, 0),                      // PIPC6 RMW: bit 7 cleared
        (0xFCFE_3818, 1 << 23),                // PMSR6: PM6_7 = 0 (output)
    ]);
}
#[test] fn set_level_and_toggle_use_psr_and_pnot()          // set_low → (0xFCFE_3118, 1 << 23); toggle → (0xFCFE_3718, 1 << 7)
#[test] fn input_new_sets_pm_and_the_input_buffer()          // PMCSR6 (1<<19), PMSR6 (1<<19)|(1<<3), PIBC6 RMW sets bit 3 — for P6_3
#[test] fn is_high_reads_ppr()                               // poke32 PPR6 (0xFCFE_3218) = 1<<4 → Input(P6_4).is_high()
#[test] fn set_as_alternate_matches_set_pin_mux()            // Flex(P1_7).set_as_alternate(Alt6) leaves the same PFC/PFCE/PFCAE/PMC/PIPC bits the old set_pin_mux(1, 7, 6) test asserts
#[test] fn set_as_alternate_software_io_clears_pipc()        // Table 41.7: PIPC bit 0, PM bit from `output`
#[test] fn dropping_an_output_leaves_the_pin_as_it_was()     // build, test::reset(), drop → test::writes() is empty
#[test] fn output_capable_pins_are_the_tables_outputs()      // fn assert_out<T: OutputCapable>() {} for P1_0, P1_7, P2_0, P9_5;
                                                             // and Flex::set_as_output on AnyPin from P1_8 panics (#[should_panic])
#[test] fn read_port_reads_one_ppr()                         // poke32 PPR1 = 0xA5A5 → read_port(Port::P1) == 0xA5A5
```

- [ ] **Step 2: Run to see them fail**

Run: `cd rza1 && cargo test --target armv7-unknown-linux-gnueabihf -p rza1-hal --features rza1l --lib gpio`
Expected: FAIL to compile, because `Level`, `Output::new` and `P6_7` are not defined.

- [ ] **Step 3: Implement**

1. A `pins!` macro in `pins.rs` takes `(Name, port, bit, output_capable)` rows and emits `sealed::Sealed`, `Pin`, `From<Name> for AnyPin`, and `OutputCapable` where flagged.
2. Add the singletons to `peripherals!`.
3. The PFC/PFCE/PFCAE/PIPC/PIBC read-modify-writes run in `critical_section::with`.

- [ ] **Step 4: Verify**

Run:
- the test command;
- `rza1/tools/test.sh`;
- `tools/test.sh`;
- the firmware builds, where deprecation warnings are expected but there must be no errors.

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: pin singletons; gpio::{Input, Output, Flex} with set/reset-register writes"
```

---

### Task 2: Generated pin-function traits

**Files:**
- Create: `rza1/tools/gen-pins.py`, `rza1/rza1-hal/pins/rza1l-overrides.yaml`, `rza1/rza1-hal/src/gpio/rza1l_functions.rs` (generated)
- Modify: `rza1/rza1-hal/src/{uart,rspi,sdhi}.rs` (trait declarations), `rza1/rza1-hal/src/gpio/mod.rs` (`pin_trait!`, `pin_function!`), `rza1/tools/test.sh`

**Interfaces:**
- Produces:

```rust
// gpio/mod.rs
macro_rules! pin_trait { ($name:ident, $instance:path) => {
    pub trait $name<T: $instance>: crate::gpio::Pin { #[doc(hidden)] fn alternate(&self) -> crate::gpio::Alternate; }
} }
// rza1l_functions.rs (generated, one line per (pin, signal)):
pin_function!(crate::uart::TxPin, SCIF0, P6_15, Alt5);
```

- Traits:
  - `uart::{TxPin<T>, RxPin<T>}` for SCIF0–4 (signals `TxDn`, `RxDn`).
  - `rspi::{SckPin<T>, MosiPin<T>, MisoPin<T>, SslPin<T>}` for RSPI0–2 (`RSPCKn`, `MOSIn`, `MISOn`, `SSLn0`).
  - `sdhi::{ClkPin<T>, CmdPin<T>, D0Pin<T>, D1Pin<T>, D2Pin<T>, D3Pin<T>, CdPin<T>, WpPin<T>}` for SDHI0/1 (`SD_CLK_n`, `SD_CMD_n`, `SD_Dk_n`, `SD_CD_n`, `SD_WP_n`).
- `gen-pins.py`:
  - **Reading rows:** read every `|Pn_m|…|` row of Tables 41.11–41.29 in ch41.
  - **Cleaning cells:** join `<br>` splits (`SD_<br>CLK_1` → `SD_CLK_1`) and drop footnote marks (`*1`).
  - **Cell positions:** cells 0–1 are the port-mode input/output; alternative k is cells 2k and 2k+1, and its signal is whichever cell is non-empty.
  - **Rejecting rows:** a row whose cell count is not 2 + 2 × (the port's alternative count: 4, or 8 for tables with a "5th Alt" header) is rejected unless the overrides file lists it. A rejected row that names a wanted signal fails the run, naming the row.
  - **Overrides:** `rza1l-overrides.yaml` maps `P3_15: {5: TxD1}`-style entries, each with a `source:` note. P3_15 is the first entry, sourced from the Deluge's working `set_pin_mux(3, 15, 5)` and the PDF text (`rza1/vendor/docs/rza1/r01uh0437ej0700_rz_a1l.txt`).
  - **Output:** sorted, deterministic, with a "generated by rza1/tools/gen-pins.py — do not edit" header.
  - **`--check`:** regenerate and diff; skip with a notice when the manual is absent.
  - It also emits the `OutputCapable` flags Task 1 hand-wrote. Task 1's rows must match, and `--check` compares them.
- `rza1-hal` includes `rza1l_functions.rs` under `#[cfg(feature = "rza1l")]`.

- [ ] **Step 1: Write the failing test** (in `gpio/mod.rs`, `#[cfg(feature = "rza1l")]`)

```rust
#[test] fn deluge_pin_functions_match_the_known_working_muxes() {
    // Every mux the Deluge drives today through set_pin_mux, with the function number it uses.
    // One helper per trait, e.g.:
    fn tx_alt<T: crate::uart::Instance>(p: &impl crate::uart::TxPin<T>) -> Alternate { p.alternate() }
    assert_eq!(tx_alt::<SCIF0>(&P6_15), Alternate::Alt5);   // MIDI TxD0
    assert_eq!(rx_alt::<SCIF0>(&P6_14), Alternate::Alt5);   // MIDI RxD0
    assert_eq!(tx_alt::<SCIF1>(&P3_15), Alternate::Alt5);   // PIC TxD1 (the override)
    assert_eq!(rx_alt::<SCIF1>(&P1_9),  Alternate::Alt3);   // PIC RxD1
    assert_eq!(sck_alt::<RSPI0>(&P6_0), Alternate::Alt3);   // OLED/CV RSPCK0
    assert_eq!(mosi_alt::<RSPI0>(&P6_2), Alternate::Alt3);  // OLED/CV MOSI0
    // SD port 1, all Alt3: CD P7_0, WP P7_1, D1 P7_2, D0 P7_3, CLK P7_4, CMD P7_5, D3 P7_6, D2 P7_7
}
```

The `*_alt` helpers are one-line generic fns, one per trait. Construct the pins with `steal()` inside the test.

- [ ] **Step 2: Run to see it fail**

Run: `cd rza1 && cargo test --target armv7-unknown-linux-gnueabihf -p rza1-hal --features rza1l --lib deluge_pin_functions`
Expected: FAIL to compile, because `uart::TxPin` is not defined.

- [ ] **Step 3: Write `gen-pins.py` and the overrides, generate, and wire it in**

Then add to `rza1/tools/test.sh`, beside the PAC check: `python3 tools/gen-pins.py --check` (skips without the manual).

- [ ] **Step 4: Verify**

Run:
- the test;
- `python3 rza1/tools/gen-pins.py --check`;
- `rza1/tools/test.sh`.

Expected:
- PASS.
- `--check` reports no diff.
- A hand edit to `rza1l_functions.rs` makes `--check` fail. Try it, then revert.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: SCIF, RSPI and SDHI pin-function traits generated from the RZ/A1L manual"
```

---

### Task 3: Drivers take their pins

**Files:**
- Modify:
  - `rza1/rza1-hal/src/uart.rs` (`Uart::new`, docs at lines 11, 21, 214);
  - `rza1/rza1-hal/src/rspi.rs` (`Rspi::new`);
  - `rza1/rza1-hal/src/sdhi.rs` (`Sdhi::new`);
  - `crates/deluge-bsp/src/{uart.rs:57-82,bus.rs:41-42,157-158,sd.rs:157-164,board.rs}`.
- Test: the drivers' existing constructor tests.

**Interfaces:**
- Consumes the Task 2 traits and Task 1 `Flex::set_as_alternate`.
- Produces:
  - `Uart::new(scif, tx: Peri<'d, impl TxPin<T>>, rx: Peri<'d, impl RxPin<T>>, rx_dma, tx_dma, irq, baud)`. The pins go first, after the instance, as embassy orders them.
  - `Rspi::new_txonly(rspi, sck: Peri<'d, impl SckPin<T>>, mosi: Peri<'d, impl MosiPin<T>>, bit_rate)` and `Rspi::new(rspi, sck, mosi, miso: Peri<'d, impl MisoPin<T>>, bit_rate)`. The Deluge uses `new_txonly`.
  - `Sdhi::new(sdhi, clk, cmd, d0, d1, d2, d3, cd: Peri<'d, impl CdPin<T>>, wp: Peri<'d, impl WpPin<T>>, tx_dma, rx_dma, irq, sd_option)`.
  - Each constructor muxes its pins with `set_as_alternate(pin.alternate())` before touching the peripheral.
  - `deluge-bsp`'s `Board` gains the pins these constructors need, as `BoardPins` fields (Task 4 adds the rest): `midi_tx`, `midi_rx`, `pic_tx`, `pic_rx`, `spi_sck`, `spi_mosi`, `sd_clk`, `sd_cmd`, `sd_d0`–`sd_d3`, `sd_cd`, `sd_wp`.

- [ ] **Step 1: Extend each constructor test to assert the pin mux**

For example, `Uart::new` for SCIF0 with P6_15/P6_14 now also writes PFCAE6 bits 14 and 15 (Alt5), PMC6 and PIPC6, before its first SCIF register write.

- [ ] **Step 2: Run to see them fail**

Run: `cd rza1 && cargo test --target armv7-unknown-linux-gnueabihf -p rza1-hal --features rza1l --lib uart rspi sdhi`
Expected: FAIL to compile, because the constructors have the wrong arity.

- [ ] **Step 3: Implement**

Change the constructors, delete `deluge-bsp`'s `set_pin_mux` calls for these pins, and pass the board pins in.

- [ ] **Step 4: Verify**

Run: the test command, `tools/test.sh`, the firmware builds, and `tools/link-layout.sh check`.
Expected: PASS. `grep -n "set_pin_mux" crates/deluge-bsp/src/{uart,bus,sd}.rs` prints nothing.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: Uart, Rspi and Sdhi take their pins and mux them"
```

---

### Task 4: `deluge-bsp` and the facade on owned pins

**Files:**
- Modify:
  - `crates/deluge-bsp/src/board.rs` (`BoardPins`);
  - `crates/deluge-bsp/src/{audio.rs:84-132,cv_gate.rs,encoder.rs:240-250,jacks.rs,sdram.rs:41-83,trigger_clock.rs:70-80}`;
  - `crates/deluge-sdk/src/{sync_led.rs,plat/device.rs,lib.rs}`.
- Test: `crates/deluge-bsp/src/board.rs` tests.

**Interfaces:**
- Consumes Task 1's `Input`/`Output`/`Flex`/`read_port`.
- Produces the `BoardPins` role fields, each a `Peri<'static, Pxx_y>`:

  | Field | Pin(s) |
  |---|---|
  | `sync_led` | `P6_7` |
  | `jack_detect` | `P6_3`, `P6_4`, `P6_5`, `P6_6`, `P7_9`: name each by role from `jacks.rs` |
  | `speaker_enable` | `P4_1` |
  | `codec_power` | `CODEC_PORT`/`CODEC_PIN` from `audio.rs` |
  | `cv_cs` | `CS_PORT`/`CS_PIN` from `cv_gate.rs` |
  | `audio_xout` | `P7_11` |
  | `ssi_tx`, `ssi_sck`, `ssi_ws`, `ssi_rx` | `P6_8`–`P6_11` |
  | `encoder_irq` | the six IRQ pins on port 1 |
  | `encoder_comp` | the six direction pins on port 1 |
  | `trigger` | `P1_14` |
  | `sdram` | the port 2/3/5 bus pins `sdram.rs` muxes, as one `SdramPins` struct |

- BSP modules take these as `Output`/`Input`/`Flex`:
  - `sdram.rs`, `audio.rs`, `encoder.rs` and `trigger_clock.rs` keep their exact Alt numbers and PIBC settings through `Flex::set_as_alternate` and `enable_input_buffer`;
  - the encoder ISR reads port 1 with `gpio::read_port(Port::P1)`.
- The facade's `sync_led` holds an `Output<'static>` from `BoardPins::sync_led`.

- [ ] **Step 1: Write the failing test**

```rust
#[test] fn board_pins_name_every_pin_the_bsp_drives() {
    // Every (port, bit) the BSP touched through gpio::* before this phase, from the call sites
    // listed in Interfaces, appears exactly once in BoardPins (by Pin::port/bit), and no two
    // roles share a pin.
}
```

Build the expected list from today's call sites before editing them.

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host-uac,usb-host-overbridge --lib board_pins`
Expected: FAIL to compile, because `BoardPins` is not defined.

- [ ] **Step 3: Implement**

Migrate module by module, keeping each pin's register sequence.

- [ ] **Step 4: Verify**

Run: the test, `tools/test.sh`, the firmware builds, and `tools/link-layout.sh check`.
Expected: PASS. `grep -rn "gpio::\(set_as_\|write\|read_pin\|set_pin_mux\|enable_input\)" crates/` prints nothing.

- [ ] **Step 5: Commit**

```bash
git commit -am "deluge-bsp, deluge: pins by role from Board; no raw gpio calls"
```

---

### Task 5: Firmwares and hw-tests, and the old API goes

**Files:**
- Modify:
  - `firmwares/{controller-firmware,demo-firmware,msc-firmware}/src/**`;
  - `hw-tests/{overbridge-device-firmware,overbridge-host-firmware,sd-bench,uac-host-firmware,vbus-probe,wp-probe}/src/**`;
  - examples that use `gpio::*`;
  - `rza1/rza1-hal/src/gpio/mod.rs` (deletions).

**Interfaces:**
- Consumes `BoardPins` from Task 4.
- Produces:
  - The blink tasks and jack-detect tasks take `Output<'static>`/`Input<'static>` from `board.pins`.
  - The `gpio` free functions `set_as_output`, `set_as_input`, `enable_input_buffer`, `read_pin`, `write` and `set_pin_mux`, the type `Pin<PORT, BIT, MODE>`, the `OutputMode`/`InputMode` markers and `steal` are deleted.
  - Register helpers the HAL still needs become private. `read_port` stays public.

- [ ] **Step 1: Delete the deprecated API**

Build to see every remaining call site fail.
Run: the firmware builds.
Expected: FAIL in each firmware and hw-test that still calls `gpio::*`.

- [ ] **Step 2: Migrate each one onto board pins**

- [ ] **Step 3: Verify**

Run:
- `tools/test.sh`;
- `tools/check-layering.sh`;
- `tools/link-layout.sh check`;
- every firmware, hw-test and example build.

Expected:
- PASS.
- `grep -rn "gpio::\(set_as_\|write\|read_pin\|set_pin_mux\|enable_input\)\|gpio::Pin<" --include='*.rs' crates firmwares hw-tests examples usb rza1` prints nothing.

- [ ] **Step 4: Commit**

```bash
git commit -am "firmwares, hw-tests: owned pins; rza1-hal drops the raw gpio functions and Pin<PORT, BIT>"
```

---

### Task 6: Phase close and bench

- [ ] **Step 1: Full checks**

Run:
- `tools/test.sh`;
- `tools/check-layering.sh`;
- `tools/link-layout.sh check`;
- every package build;
- `cd rza1 && cargo doc --no-deps -p rza1-hal --features rza1l`.

Expected: all pass, with no more doc warnings than the 19 phase 1 ended with.

- [ ] **Step 2: Bench** (pi-top; start `JLinkRemoteServerCLExe -Port 19020` there if `deluge_jlink.py` reports "GDB server never connected")

Load `demo-firmware --features rtt --release` and confirm:
- the OLED draws (RSPI pins);
- the CV DAC initialises (RSPI and its chip select);
- MIDI and the PIC come up (SCIF0/1 pins), and PIC fw is reported;
- the SD card goes High-Speed (SDHI pins);
- audio plays and records over USB: `speaker-test` and a 3-second `arecord` of exactly 793,844 bytes. These are the SSI and AUDIO_XOUT pins, migrated in Task 4.
- the sync LED blinks.

Encoders, pads and the trigger input need someone at the bench. List them as open if nobody is there.

- [ ] **Step 3: CHANGELOG**

Under `[Unreleased] / Changed`, cover:
- the pin singletons;
- `Input`/`Output`/`Flex` and the atomic writes;
- the generated pin traits;
- the new `Uart::new`, `Rspi::new_txonly`/`new` and `Sdhi::new` signatures;
- that `set_pin_mux`, the raw `gpio` functions and `Pin<PORT, BIT, MODE>` are gone;
- `read_port`;
- `BoardPins`.

- [ ] **Step 4: Commit**

```bash
git commit -am "CHANGELOG: rza1-hal phase 2"
```
