# rza1-hal Phase 4: UART and RSPI Modes, Traits and Typed Errors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rza1-hal`'s UART and RSPI get `Blocking`/`Async` typestate, typed `Config`s, typed errors, embedded-hal 1.0, embedded-io and embedded-hal-nb impls, and an async UART receiver that waits on an interrupt instead of polling. GPIO gets `Wait` on IRQ0–7 pins.

**Architecture:**
- A shared `crate::mode::{Blocking, Async}` sealed typestate.
- **UART:** the ERI handler latches receive errors instead of dropping them. A blocking `Uart` polls the FIFO.
  - The new default async `Uart::new` receives on RXI into a caller-lent buffer, so it works on every SCIF.
  - The existing DMA ring becomes `Uart::new_with_rx_dma`. It is still polled, because its receive request belongs to the DMAC.
- **RSPI:** a blocking full-duplex `SpiBus<u8>`/`SpiBus<u32>`, plus an interrupt-driven async one on SPRI/SPTI/SPEI.
- **BSP:** migrates in lockstep and stays on the blocking RSPI and the DMA-ring UART, so firmware behaviour doesn't change.

**Tech Stack:** Rust `no_std`, embassy-hal-internal, embedded-hal 1.0, embedded-hal-async 1.0, embedded-hal-nb 1.0, embedded-io 0.7, embedded-io-async 0.7, the `rza1_pac::mmio::test` shadow seam.

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md` (§4 Modes, Traits, Errors; §5 item 4)

## Global Constraints

- **Modes:** "drivers that support both get `Blocking`/`Async` typestate with `new_blocking`/`new`. `Rspi` gains async. `Uart` gains blocking and stops polling RX every 100 µs, where the hardware allows it."
- **Traits:**
  - "embedded-hal 1.0 `SpiBus`, plus the async version, on RSPI";
  - "embedded-hal 1.0 `Wait` on GPIO, if IRQ0–7 pin interrupts are wired" (they are: `interrupt::typelevel::IRQ0..IRQ7` = GIC 32–39, ICR1/IRQRR in `gic.rs`);
  - "blocking `embedded-io` on UART";
  - "embedded-hal-nb serial where it's natural".
- **Errors:** "every error enum is `#[non_exhaustive]`, documented, and derives `Debug, Clone, Copy, PartialEq, Eq`; `defmt::Format` behind the `defmt` feature."
- **Deviation from the spec, for review:**
  - The spec names `uart::Error { Framing, Noise, Overrun, Parity }`, but the RZ/A1L SCIF has no noise flag (SCFSR: ER/BRK/FER/PER; SCLSR: ORER). This plan ships `uart::Error { Overrun, Framing, Parity, Break }`.
  - A variant the hardware can never produce would be dead API, and `#[non_exhaustive]` lets one be added later.
- **Configs:** every driver `Config` has `Default` and `#[non_exhaustive]`, and no constructor takes raw register words or a bare baud/bit rate.
- **Interrupts:**
  - Drivers never set interrupt priorities (Phase 1 rule); the BSP's `board::apply_irq_priorities` does.
  - New interrupt IDs come from manual Table 7.3 (`~/Downloads/r01uh0437ej0700_rz_a1l.txt` line ~38600 and ~39240): RXIn = 223 + 4n; SPEIn/SPRIn/SPTIn = 270/271/272 + 3n (RSPI3/4 under `rza1h` only: 279–284).
- **Layering:** nothing under `rza1/` names "deluge" (`tools/check-layering.sh`).
- **Steal paths:** driver `steal` stays `#[doc(hidden)]` behind `unstable-driver-steal`.
- **Lockstep migration:** this repo's BSP, firmwares, hw-tests and examples move in the same commits. DelugeFirmware#next is not touched (phase 9).
- **Done when** these are green, and every firmware, hw-test and example builds:
  - `tools/test.sh`;
  - `rza1/tools/test.sh`;
  - `tools/check-layering.sh`;
  - `tools/link-layout.sh check`.
  The verify script `/tmp/claude-1000/-home-kate-GitHub-deluge-sdk/c1820c3c-843c-48c7-935a-6aa619b4acfc/scratchpad/verify.sh` runs all of these and prints PASS/FAIL per item.
- The target is 32-bit (`usize` == `u32`): ring index math wraps by modulo with no overflowing adds.

## Review Focus

1. **A lent UART receive buffer outliving its driver.** If `UartRx<'d, Async>` (RXI mode) is dropped or `mem::forget`-free-dropped, the RXI ISR must never write the buffer again. Expected: `Drop` clears RIE, disables RXI at the GIC and nulls the ring pointer in a critical section. Test `irq_rx_drop_stops_the_isr_writing` (Task 4).
2. **The receive ring filling while nobody reads.** A full lent ring must not overwrite unread bytes. Expected: the ISR drops the new byte and latches `Error::Overrun`, and the reader sees the error, then the bytes it already had. Test `irq_rx_full_ring_reports_overrun_and_keeps_old_bytes` (Task 4).
3. **Several errors latched before one read.** Each is reported once, highest priority first (Overrun, Framing, Parity, Break), and then the bytes resume. Test `errors_are_reported_one_per_read_in_priority_order` (Task 2).
4. **A full-duplex RSPI transfer after transmit-only traffic.** Stale bytes left in the receive buffer (the OLED/CV path never reads it) must not be returned as the first reads of a `transfer`. Expected: every `read`/`transfer` resets the RX buffer and clears OVRF first. Test `transfer_resets_the_rx_buffer_first` (Task 5).
5. **An async RSPI transfer dropped mid-flight.** The next transfer on the same bus starts clean. Expected: an `OnDrop` masks SPRIE/SPTIE, waits for TEND and resets the RX buffer. Test `dropped_async_transfer_leaves_the_bus_idle` (Task 6).

---

### Task 1: Mode typestate, `uart::Config` and `uart::Error`

**Files:**
- Create: `rza1/rza1-hal/src/mode.rs`
- Modify:
  - `rza1/rza1-hal/src/lib.rs` (`pub mod mode;`)
  - `rza1/rza1-hal/src/uart.rs` (`configure`, `set_baud_regs`, `Uart`/`UartRx`/`UartTx` generics, tests)
  - `rza1/rza1-hal/Cargo.toml` (add `embedded-io = "0.7"`, `embedded-hal-nb = "1"`)
  - `crates/deluge-bsp/src/uart.rs`, `crates/deluge-bsp/src/pic.rs` (`set_pic_baud`)
  - `crates/deluge-sdk/src/plat/device.rs` (only if a signature it names changes)

**Interfaces:**
- Produces:
  - `pub mod mode`: `pub trait Mode: sealed::Sealed {}`, `pub struct Blocking;` and `pub struct Async;`, both implementing `Mode`.
  - `uart::Parity { None, Even, Odd }` and `uart::StopBits { One, Two }`.
  - `#[non_exhaustive] pub struct uart::Config { pub baudrate: u32, pub parity: Parity, pub stop_bits: StopBits }`, with `Default` = 115 200, `Parity::None`, `StopBits::One`.
  - `#[non_exhaustive] pub enum uart::Error { Overrun, Framing, Parity, Break }`, implementing `embedded_io::Error` (kinds: Overrun → `ErrorKind::Other`, the rest → `ErrorKind::InvalidData`) and `core::fmt::Display`.
  - `Uart<'d, M: Mode>`, `UartRx<'d, M: Mode>`, `UartTx<'d, M: Mode>`. Everything that exists today becomes the `Async` impl.
  - The existing constructor takes `config: Config` in place of `baud: u32`.
  - `UartTx::set_baud(&self, baud)` becomes `UartTx::set_config(&self, config: &Config)`.

- [ ] **Step 1: Write the failing tests** in `uart.rs`'s `tests` module:
  - `config_default_is_115200_8n1`: `Config::default()` has `baudrate == 115_200`, `Parity::None` and `StopBits::One`.
  - `configure_programs_smr_for_parity_and_stop_bits`:
    - `mmio::test::reset()`, then call `configure(2, &cfg)` with `Parity::Odd` and `StopBits::Two`.
    - `peek16(SCIF2 SCSMR)` has PE (bit 5), O/E (bit 4) and STOP (bit 3) set, and CHR (bit 6) clear.
    - With `Parity::Even`, O/E is clear and PE is set.
    - Read the SCIF register map from `crate::pac::scif` for the address.
  - `uart_error_kinds`: `embedded_io::Error::kind(&Error::Overrun) == ErrorKind::Other`, and `Error::Framing.kind() == ErrorKind::InvalidData`.
  - Keep the existing SCBRR tests, migrated to `Config`.

- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && cargo test -p rza1-hal --features rza1l uart:: 2>&1 | tail -20`
  Expected: compile errors: `Config`, `Parity` and `Error` are not found in `uart`.

- [ ] **Step 3: Implement**
  - `mode.rs`, `Config`, `Parity`, `StopBits` and `Error` as in Interfaces. Error derives `Debug, Clone, Copy, PartialEq, Eq`, plus `#[cfg_attr(feature = "defmt", derive(defmt::Format))]`, and each variant has a doc line.
  - `configure(ch, &Config)` and `set_baud_regs(ch, &Config)` write SMR from the config: CHR = 0 (8 data bits), PE/O/E from parity, STOP from stop bits.
  - Add the `M` type parameter, holding `PhantomData<M>`, to the three structs.
  - Migrate `crates/deluge-bsp/src/uart.rs`:
    - `init_midi` and `init_pic` build a `uart::Config` from their baud.
    - `set_pic_baud(baud)` calls `set_config`.
    - The statics become `UartRx<'static, Async>` / `UartTx<'static, Async>`.

- [ ] **Step 4: Run the tests and the full verify**
  Run:
  - `cd rza1 && cargo test -p rza1-hal --features rza1l uart:: 2>&1 | tail -5`
  - then `bash /tmp/claude-1000/-home-kate-GitHub-deluge-sdk/c1820c3c-843c-48c7-935a-6aa619b4acfc/scratchpad/verify.sh`
  Expected: the uart tests PASS, and every verify line PASS.

- [ ] **Step 5: Commit**
  `git commit -am "rza1-hal: uart Config/Error and the Blocking/Async mode typestate"`

### Task 2: Latch receive errors and return them from reads

**Files:**
- Modify:
  - `rza1/rza1-hal/src/uart.rs` (`on_eri`, `clear_rx_errors`, a new `RX_ERROR` static, `UartRx<Async>::try_read`/`read_byte`, the `embedded_io_async` impls, tests)
  - `crates/deluge-bsp/src/uart.rs` (`read_byte`, `try_read_pic`, `try_read_midi`)

**Interfaces:**
- Consumes: `uart::Error` (Task 1).
- Produces:
  - `static RX_ERROR: [AtomicU8; NUM_CHANNELS]`: latched error bits.
  - `fn take_error(ch: usize) -> Option<Error>`: clears and returns the highest-priority latched bit.
  - `UartRx<'d, Async>::try_read(&mut self) -> Result<Option<u8>, Error>`.
  - `UartRx<'d, Async>::read_byte(&mut self) -> Result<u8, Error>` (async).
  - `embedded_io_async::ErrorType for UartRx<'_, Async>` has `Error = uart::Error`.
  - The BSP's public functions keep their signatures (`read_midi_byte() -> u8` etc.). They `warn!` on an error and keep reading.

- [ ] **Step 1: Write the failing tests**
  - `eri_latches_framing_and_overrun`:
    - `mmio::test::reset()`; `poke16(SCFSR(0), FER | ER)` and `poke16(SCLSR(0), 1)`.
    - Call `on_eri(0)`.
    - The SCFSR and SCLSR error bits are written back clear (existing behaviour).
    - `take_error(0) == Some(Error::Overrun)`, then `Some(Error::Framing)`, then `None`.
  - `errors_are_reported_one_per_read_in_priority_order`:
    - Latch PER and BRK through `on_eri`.
    - A stolen `UartRx` (as the existing steal tests build it) gets `try_read() == Err(Parity)`, then `Err(Break)`, then `Ok(None)` on an empty ring.
  - `try_read_returns_ring_bytes_after_the_error`:
    - Latch FER and put one byte in the ring (the existing ring tests show how CRDA is shadowed).
    - Expect `Err(Framing)`, then `Ok(Some(byte))`.

- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && cargo test -p rza1-hal --features rza1l uart:: 2>&1 | tail -20`
  Expected: `take_error` not found; `try_read` returns `Option`, not `Result`.

- [ ] **Step 3: Implement**
  - `on_eri` reads SCFSR and SCLSR, ORs the matching bits into `RX_ERROR[ch]`, then clears them as today.
  - Bit order: bit 0 Overrun, 1 Framing, 2 Parity, 3 Break. `take_error` clears the lowest set bit with `fetch_and`.
  - `try_read` calls `take_error` first.
  - `embedded_io_async::Read::read`:
    - If an error is pending before any byte, return `Err`.
    - Otherwise return the bytes read so far. A pending error waits for the next call.
  - BSP: `read_byte` loops on `Err(e) => warn!("{name}: receive error {:?}", e)`. `try_read_*` treat `Err` the same way and try once more.

- [ ] **Step 4: Run the tests and verify**
  Run: the Task 1 Step 4 commands.
  Expected: PASS, with every verify line PASS.

- [ ] **Step 5: Commit**
  `git commit -am "rza1-hal: uart receive errors are latched by ERI and returned by reads"`

### Task 3: Blocking UART with embedded-io and embedded-hal-nb

**Files:**
- Modify: `rza1/rza1-hal/src/uart.rs` (a new blocking section and tests)

**Interfaces:**
- Consumes: `mode::Blocking`, `Config`, `Error`, `configure` (Task 1).
- Produces:
  - `Uart::new_blocking<T: Instance>(scif: Peri<'d, T>, tx: Peri<'d, impl TxPin<T>>, rx: Peri<'d, impl RxPin<T>>, config: Config) -> Uart<'d, Blocking>`, for any SCIF. No interrupts bound and no DMA. SCSCR = TE|RE only.
  - `Uart<'d, Blocking>::split(self) -> (UartRx<'d, Blocking>, UartTx<'d, Blocking>)`.
  - `UartRx<'d, Blocking>`:
    - `fn nb_read(&mut self) -> nb::Result<u8, Error>`;
    - `fn blocking_read(&mut self, buf: &mut [u8]) -> Result<usize, Error>` (waits for at least one byte).
  - `UartTx<'d, Blocking>`:
    - `fn nb_write(&mut self, byte: u8) -> nb::Result<(), Infallible>`;
    - `fn blocking_write(&mut self, buf: &[u8]) -> Result<(), Infallible>`;
    - `fn blocking_flush(&mut self)`.
  - Trait impls on the blocking halves:
    - `embedded_io::{ErrorType, Read, ReadReady}` (RX);
    - `embedded_io::{ErrorType, Write, WriteReady}` (TX);
    - `embedded_hal_nb::serial::{ErrorType, Read<u8>}` (RX), `embedded_hal_nb::serial::Write<u8>` (TX).
  - `embedded_hal_nb::serial::Error for Error`: Overrun → `ErrorKind::Overrun`, Framing → `FrameFormat`, Parity → `Parity`, Break → `Other`.

- [ ] **Step 1: Write the failing tests**
  - `new_blocking_enables_only_te_re`: after `new_blocking` on SCIF3 with stolen pins (use the pin pair that `gen-pins.py`'s `rza1l_functions.rs` lists for SCIF3), `peek16(SCSCR(3)) == TE | RE`.
  - `nb_read_would_block_on_an_empty_fifo`: SCFDR RX count (low 5 bits) = 0 → `Err(nb::Error::WouldBlock)`.
  - `nb_read_returns_a_byte_and_clears_rdf_dr`:
    - `poke16(SCFDR, 1)`, `poke8(SCFRDR, 0x5A)`, `poke16(SCFSR, RDF | DR)`.
    - Expect `Ok(0x5A)`, with RDF and DR written clear.
  - `nb_read_reports_a_framing_error_from_scfsr`:
    - SCFSR has `FER | ER`.
    - Expect `Err(nb::Error::Other(Error::Framing))`, with the flags written clear.
  - `nb_write_would_block_on_a_full_fifo`: SCFDR TX count (bits 12:8) = 16 → `WouldBlock`; at 15 → `Ok`, with FTDR written.
  - `nb_error_kinds`: `embedded_hal_nb::serial::Error::kind(&Error::Framing) == ErrorKind::FrameFormat`.

- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && cargo test -p rza1-hal --features rza1l uart:: 2>&1 | tail -20`
  Expected: `new_blocking` not found.

- [ ] **Step 3: Implement**
  - The blocking halves poll the hardware directly: errors come from SCFSR/SCLSR, data from SCFDR/SCFRDR.
  - Clear RDF/DR after reading, using the same read-then-write-0 pattern as `clear_rx_errors`.
  - `blocking_flush` spins on TEND.
  - `ReadReady` is RX count > 0. `WriteReady` is `tx_free() > 0`.

- [ ] **Step 4: Run the tests and verify**
  Run: the Task 1 Step 4 commands.
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git commit -am "rza1-hal: blocking Uart with embedded-io and embedded-hal-nb serial"`

### Task 4: Interrupt-driven async receive into a lent buffer; the DMA ring becomes `new_with_rx_dma`

**Files:**
- Modify:
  - `rza1/rza1-hal/src/interrupt.rs` (add `RXI0 = 223, RXI1 = 227, RXI2 = 231, RXI3 = 235, RXI4 = 239`, plus the ID test)
  - `rza1/rza1-hal/src/uart.rs`
  - `crates/deluge-bsp/src/uart.rs` (call `new_with_rx_dma`)
  - the module docs' usage example in `uart.rs`

**Interfaces:**
- Consumes: Tasks 1–2 (`RX_ERROR`, `take_error`, `UART_STATE[ch].rx_waker`).
- Produces:
  - `Instance` gains `type RxInterrupt: Interrupt;`.
  - `pub struct RxInterruptHandler<T: Instance>`: drains the RX FIFO into the lent ring and wakes the reader.
  - `Uart::new<T: Instance>(scif, tx, rx, irq: impl Binding<T::ErrorInterrupt, ErrorInterruptHandler<T>> + Binding<T::RxInterrupt, RxInterruptHandler<T>> + Binding<T::TxInterrupt, TxInterruptHandler<T>> + Copy + 'd, rx_buf: &'d mut [u8], tx_dma: Option<Dma<'d>>, config: Config) -> Uart<'d, Async>`.
    - Any SCIF.
    - Panics if `rx_buf` is empty.
    - FIFO receive trigger (SCFCR RTRG) = 1 byte.
  - `Uart::new_with_rx_dma<T: RxDmaInstance>(scif, tx, rx, rx_dma: Dma<'d>, tx_dma: Option<Dma<'d>>, irq, config) -> Uart<'d, Async>` is today's constructor, renamed.
    - Its doc says the ring is polled every 100 µs because the SCIF's receive request is routed to the DMAC and the GIC can't see it.
    - Its irq bound still has ERI and TXI only.
  - `UartRx<'d, Async>` holds `enum RxPath<'d> { Dma { .. }, Irq }`. `try_read`, `read_byte` and `embedded_io_async::Read` work for both. `steal` only builds the `Dma` path.
  - `impl Drop for UartRx<'_, Async>` (Irq path only): clear SCSCR.RIE, disable RXI at the GIC, and null the channel's ring in `critical_section::with`.

- [ ] **Step 1: Write the failing tests**
  - `rxi_ids_match_the_manual`: `[RXI0..RXI4]::ID == [223, 227, 231, 235, 239]`.
  - `irq_rx_delivers_bytes_without_polling`:
    - Build the Irq-path `UartRx` over a lent `[0u8; 8]` on SCIF2.
    - Shadow SCFDR RX count = 1 and FRDR = 0x42, then call `RxInterruptHandler::<SCIF2>::on_interrupt()`.
    - `try_read() == Ok(Some(0x42))`, then `Ok(None)`.
    - The waker registered by a pending `read_byte` poll was woken. Use a counting waker, as the sdhi tests do.
  - `irq_rx_full_ring_reports_overrun_and_keeps_old_bytes`:
    - Ring of 2. Run the handler 3 times, each with one byte (0x01, 0x02, 0x03).
    - `try_read` checks `take_error` first, so assert `Err(Overrun)`, then `Ok(Some(1))`, `Ok(Some(2))`, `Ok(None)`.
    - A ring of N holds N bytes: keep a count, not a one-slot gap.
  - `irq_rx_drop_stops_the_isr_writing`:
    - Drop the `UartRx`, then run the handler with a shadowed byte.
    - The lent buffer is unchanged.
    - SCSCR.RIE is clear, and GIC ICENABLER for 231 was written.
  - `new_with_rx_dma_keeps_the_ring_path`: today's DMA-ring tests, renamed onto `new_with_rx_dma`.

- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && cargo test -p rza1-hal --features rza1l 2>&1 | tail -20`
  Expected: `RXI0` and `RxInterruptHandler` not found.

- [ ] **Step 3: Implement**
  - Per channel, keep `static IRQ_RING: [Mutex<Cell<Option<RingPtr>>>; NUM_CHANNELS]` (critical-section blocking mutex). `RingPtr { ptr: *mut u8, len: usize, head: usize, count: usize }`.
  - The reader pops from `head - count` modulo `len`, inside the critical section.
  - The handler drains `SCFDR & 0x1F` bytes from FRDR:
    - while `count < len`, store each byte;
    - after that, read and drop them and latch Overrun;
    - then clear RDF|DR and wake `rx_waker`.
  - `read_byte` for the Irq path is a `poll_fn`: register the waker, then `try_read`; `Pending` on `Ok(None)`. No timer.
  - BSP: `init_midi`/`init_pic` call `new_with_rx_dma`, with no behaviour change.

- [ ] **Step 4: Run the tests and verify**
  Run: the Task 1 Step 4 commands.
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git commit -am "rza1-hal: interrupt-driven async uart receive into a lent buffer; DMA ring is new_with_rx_dma"`

### Task 5: RSPI `Config`, the blocking mode and embedded-hal `SpiBus`

**Files:**
- Modify:
  - `rza1/rza1-hal/src/rspi.rs` (`init`, the owned driver, tests)
  - `crates/deluge-bsp/src/bus.rs` (constructor and type)

**Interfaces:**
- Consumes: `mode::{Blocking, Async}` (Task 1).
- Produces:
  - `#[non_exhaustive] pub struct rspi::Config { pub frequency: u32, pub mode: embedded_hal::spi::Mode }`, with `Default` = 1 MHz, `MODE_0`.
  - `#[non_exhaustive] pub enum rspi::Error { Overrun, ModeFault }`, implementing `embedded_hal::spi::Error` (Overrun → `ErrorKind::Overrun`, ModeFault → `ModeFault`), with derives as in Global Constraints.
  - `Rspi<'d, M: Mode>`, with a `width: Width` field, where `enum Width { Bits8, Bits32 }`.
  - `Rspi::new_blocking(rspi, sck, mosi, miso, config) -> Rspi<'d, Blocking>` and `Rspi::new_blocking_txonly(rspi, sck, mosi, config) -> Rspi<'d, Blocking>`. These replace `new`/`new_txonly`, whose names Task 6 reuses for async.
  - `configure_8bit`, `configure_32bit`, `send8`, `send32_blocking` and `wait_end` stay on `Rspi<'d, M>` for the BSP's DMA path, and keep `self.width` in step.
  - `embedded_hal::spi::{ErrorType, SpiBus<u8>, SpiBus<u32>} for Rspi<'d, Blocking>`. Each word size switches the frame width first if it differs.
  - `read`, `transfer` and `transfer_in_place` reset the RX buffer (SPBFCR RXRST, then clear it) and clear SPSR.OVRF before the first frame. `flush` waits for TEND.
  - On a transmit-only `Rspi` (built by `new_blocking_txonly`), `read` and `transfer` are allowed and return whatever the unmuxed MISO pin gives. Document it; there's no runtime check.
  - SPTIE stays set at init, as today (it's the DMAC's transmit request for the BSP's OLED DMA). Document that on `new_blocking*`.

- [ ] **Step 1: Write the failing tests**
  - `config_mode_sets_cpol_cpha`: `new_blocking_txonly` with `MODE_3` → `peek16(SPCMD0(0)) & 0b11 == 0b11`; with `MODE_1` → `== 0b01`. SPCMD0 bit 1 is CPOL and bit 0 is CPHA (manual line ~117874).
  - `config_frequency_sets_spbr`: `frequency = 4_000_000` gives SPBR = 8, the value the existing `spbr_for` test pins.
  - `transfer_resets_the_rx_buffer_first`:
    - Shadow SPSR with SPRF|TEND|SPTEF set and SPDR (8-bit) = 0xA5.
    - `SpiBus::<u8>::transfer(&mut rspi, &mut [0; 1], &[0x3C])`.
    - The write log shows the SPBFCR RXRST write before the SPDR write of 0x3C. The read buffer is `[0xA5]`.
  - `word_size_switches_the_frame_width`:
    - After `SpiBus::<u8>::write`, SPDCR == `SPDCR_8BIT`.
    - After `SpiBus::<u32>::write`, SPDCR == `SPDCR_32BIT`.
  - `overrun_is_reported`: SPSR.OVRF (bit 0) set during `read` → `Err(Error::Overrun)`, and OVRF is written clear.
  - Migrate `new_programs_master_mode_config` to `new_blocking_txonly(.., Config { frequency: 10_000_000, .. })`.

- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && cargo test -p rza1-hal --features rza1l rspi:: 2>&1 | tail -20`
  Expected: `Config` and `new_blocking_txonly` not found.

- [ ] **Step 3: Implement**
  - `init(ch, &Config)` ORs CPOL/CPHA into SPCMD0. `configure_8bit`/`configure_32bit` keep those bits; store the mode bits in the driver and pass them in.
  - Frames poll: SPTEF to write SPDR (8-bit access in Bits8, 32-bit in Bits32), then SPRF to read SPDR.
  - BSP `bus.rs`:
    - `Rspi::new_blocking_txonly(spi, pins.sck, pins.mosi, cfg)`, with `cfg.frequency = RATE_HZ` and `MODE_0`;
    - the type becomes `Rspi<'static, Blocking>`;
    - `steal` is unchanged apart from the type.

- [ ] **Step 4: Run the tests and verify**
  Run: `cd rza1 && cargo test -p rza1-hal --features rza1l rspi:: 2>&1 | tail -5`, then verify.sh.
  Expected: PASS. The BSP `bus` test `the_bus_comes_up_in_32_bit_mode_with_the_oled_dma_aimed_at_spdr` still passes.

- [ ] **Step 5: Commit**
  `git commit -am "rza1-hal: rspi Config, Blocking mode and embedded-hal SpiBus"`

### Task 6: Async RSPI on SPRI/SPTI/SPEI

**Files:**
- Modify:
  - `rza1/rza1-hal/src/interrupt.rs`: add SPEI0..SPTI2 (270–278); `#[cfg(feature = "rza1h")]` SPEI3..SPTI4 (279–284); an ID test.
  - `rza1/rza1-hal/src/rspi.rs`

**Interfaces:**
- Consumes: Task 5 (`Config`, `Error`, `Width`, the frame helpers).
- Produces:
  - `Instance` gains `type ErrorInterrupt: Interrupt; type RxInterrupt: Interrupt; type TxInterrupt: Interrupt;`.
  - `pub struct InterruptHandler<T: Instance>`, implementing `Handler<T::ErrorInterrupt> + Handler<T::RxInterrupt> + Handler<T::TxInterrupt>`.
    - Each clears its own SPCR enable (SPEIE bit 4, SPRIE bit 7, SPTIE bit 5) and wakes a per-channel `AtomicWaker`.
  - `Rspi::new(rspi, sck, mosi, miso, irq: impl Binding<..3..> + Copy + 'd, config) -> Rspi<'d, Async>` and `Rspi::new_txonly(rspi, sck, mosi, irq, config) -> Rspi<'d, Async>`.
    - SPTIE is clear at init in async mode: no DMAC consumer.
    - All three interrupts are enabled at the GIC.
  - `embedded_hal_async::spi::SpiBus<u8>` and `SpiBus<u32>` for `Rspi<'d, Async>`, with the same reset-first semantics as Task 5.
  - An `OnDrop` guard on every async op: on drop, clear SPRIE/SPTIE/SPEIE, spin for TEND and reset the RX buffer.

- [ ] **Step 1: Write the failing tests**
  - `rspi_interrupt_ids_match_the_manual`: SPEI0/SPRI0/SPTI0 == 270/271/272; SPTI2 == 278.
  - `async_transfer_waits_on_spri`:
    - Poll `SpiBus::<u8>::transfer` once with SPSR.SPRF clear: `Pending`, with SPRIE set.
    - Set SPRF and SPDR = 0x11, then run `InterruptHandler::<RSPI0>` for the RX interrupt. SPRIE is cleared and the waker woken.
    - Poll again: `Ready(Ok(()))` with the buffer `[0x11]`.
    - Use the counting-waker / manual-poll helpers the sdhi tests use.
  - `dropped_async_transfer_leaves_the_bus_idle`:
    - Poll a 4-byte transfer once (`Pending`), with TEND shadowed set, then drop it.
    - SPCR has SPRIE, SPTIE and SPEIE clear, and an SPBFCR RXRST write is logged after the drop.
  - `new_async_leaves_sptie_clear`: after `Rspi::new_txonly`, SPCR & SPTIE == 0.

- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && cargo test -p rza1-hal --features rza1l rspi:: 2>&1 | tail -20`
  Expected: `SPEI0` and `InterruptHandler` not found.

- [ ] **Step 3: Implement**
  - Per frame:
    1. Wait SPTEF, via SPTIE + waker, if clear.
    2. Write SPDR.
    3. Wait SPRF, via SPRIE + waker.
    4. Read SPDR.
    5. Check OVRF/MODF, via SPEIE + waker, or by reading SPSR after each frame.
  - Futures register the waker before enabling the interrupt bit, then re-check the flag. Use `embassy_hal_internal::drop::OnDrop` for the guard, as `sdhi.rs` does.
  - The BSP is unchanged (it stays blocking).

- [ ] **Step 4: Run the tests and verify**
  Run: the Task 5 Step 4 commands, with `cd rza1 && cargo test -p rza1-hal --features rza1h rspi::` added.
  Expected: PASS, including under `rza1h`.

- [ ] **Step 5: Commit**
  `git commit -am "rza1-hal: async Rspi on SPRI/SPTI/SPEI with embedded-hal-async SpiBus"`

### Task 7: `ExtiInput` with embedded-hal `Wait` on IRQ0–7 pins

**Files:**
- Create: `rza1/rza1-hal/src/exti.rs`
- Modify:
  - `rza1/tools/gen-pins.py` (`SIGNALS`: add `IRQ([0-7])` → `("crate::exti::IrqPin", f"EXTI{m[1]}")`)
  - the regenerated `rza1/rza1-hal/src/gpio/rza1l_functions.rs`
  - `rza1/rza1-hal/src/lib.rs` (`peripherals!` gets `EXTI0..EXTI7`; `pub mod exti;`)
  - `rza1/rza1-hal/src/gic.rs`: `set_irq_both_edges`, `set_irq_falling_edge` and `clear_irq_pending` become `pub(crate)`, after grepping that nothing outside the HAL calls them; otherwise leave them public and ledger it.

**Interfaces:**
- Consumes: `gpio::Flex`/`Input`, `pin_trait!`, `Pull` (Phase 2).
- Produces:
  - `pub trait exti::Channel: PeripheralType + sealed { const NUMBER: u8; type Interrupt: Interrupt; }` for `peripherals::EXTI0..7`, bound to `typelevel::IRQ0..7`.
  - `pin_trait!(IrqPin, Channel)`.
  - `pub struct exti::InterruptHandler<C: Channel>`: clears IRQRR bit `NUMBER` and wakes `WAKERS[NUMBER]`.
  - `ExtiInput::new<C: Channel>(pin: Peri<'d, impl IrqPin<C>>, ch: Peri<'d, C>, irq: impl Binding<C::Interrupt, InterruptHandler<C>> + 'd) -> ExtiInput<'d>`.
    - The pin goes on its IRQ alternate.
    - The pin level is read through the pin's PPR, the input register on the IRQ function. Check the PAC: if PPR isn't readable in alternate mode, the level comes from ICR1-sensed edges only, and `is_high` isn't offered. Ledger whichever applies.
  - Methods:
    - `async fn wait_for_rising_edge(&mut self)` (ICR1 = 10);
    - `wait_for_falling_edge` (01);
    - `wait_for_any_edge` (11);
    - `wait_for_low` (level-low, 00, which completes on the next interrupt);
    - `wait_for_high` (if the level reads high, return; otherwise wait for a rising edge).
  - `embedded_hal::digital::{ErrorType<Error = Infallible>, InputPin}` (if the level is readable), and `embedded_hal_async::digital::Wait`.
  - Dropping an `ExtiInput` disables its IRQ at the GIC.

- [ ] **Step 1: Write the failing tests** in `exti.rs`:
  - `irq_pin_functions_are_generated`: one `IrqPin<EXTIn>` impl, picked from the regenerated table, compiles in a `fn assert_irq<P: IrqPin<EXTI3>>() {}` call.
  - `wait_for_rising_edge_programs_icr1_and_wakes_on_irq`:
    - Poll once: `Pending`, with ICR1 bits for channel 3 = `0b10` and the GIC enable written for ID 35.
    - Set IRQRR bit 3, then run `InterruptHandler::<EXTI3>`: the IRQRR bit is written clear and the waker woken.
    - Poll: `Ready`.
  - `wait_for_any_edge_sets_both_edges`: ICR1 bits = `0b11`.

- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && python3 tools/gen-pins.py --check; cargo test -p rza1-hal --features rza1l exti:: 2>&1 | tail -20`. Use whatever flag the script documents for check vs write.
  Expected: the module `exti` is not found.

- [ ] **Step 3: Implement**
  - Regenerate the pin table with the script.
  - The ISR leaves the GIC interrupt enabled. Waits register the waker, program ICR1, clear a stale IRQRR bit, then enable.
  - IRQRR clears by writing 0 after reading 1 (as `clear_irq_pending`).

- [ ] **Step 4: Run the tests and verify**
  Run: the Step 2 test command, then verify.sh.
  Expected: PASS, with `rza1/tools/test.sh` green, which covers the generated-table check.

- [ ] **Step 5: Commit**
  `git commit -am "rza1-hal: ExtiInput with embedded-hal Wait on IRQ0-7 pins"`

### Task 8: Docs, CHANGELOG and the bench boot check

**Files:**
- Modify:
  - `rza1/rza1-hal/src/uart.rs` and `rza1/rza1-hal/src/rspi.rs` module docs (usage examples for both modes);
  - `CHANGELOG.md` (a Phase 4 entry under the rza1-hal pre-release section the phase 1–3 entries use).

- [ ] **Step 1: Update the docs**
  - The module docs show `new_blocking` and `new` for each driver, plus the `bind_interrupts!` lines for the new handlers.
  - The CHANGELOG lists:
    - the renamed constructors (`Uart::new` → `new_with_rx_dma`; `Rspi::new[_txonly]` → `new_blocking[_txonly]`);
    - `Config` types; the error types and the Noise→Break deviation;
    - `ExtiInput`.

- [ ] **Step 2: Doc build**
  Run: `cd rza1 && cargo doc -p rza1-hal --features rza1l --no-deps 2>&1 | grep -c warning`
  Expected: no more warnings than on `main` before this phase (record both counts in the ledger).

- [ ] **Step 3: Full verify**
  Run: `bash /tmp/claude-1000/-home-kate-GitHub-deluge-sdk/c1820c3c-843c-48c7-935a-6aa619b4acfc/scratchpad/verify.sh`
  Expected: every line PASS.

- [ ] **Step 4: Bench boot check** (the BSP's UART/RSPI constructors changed)
  - Boot demo-firmware on pi-top with `deluge_jlink.py --select IP=pi-top`, `--until` the line the phase 2 bench check waited on, and `--fail-on '\bfault|panic'`.
  - Confirm in the log: the PIC link comes up at fast baud, the OLED initialises (RSPI blocking path), and CV init completes.
  - If `JLinkRemoteServerCLExe` isn't running on pi-top, start it with `-Port 19020`.
  Expected: those log lines, and no fault or panic.

- [ ] **Step 5: Commit**
  `git commit -am "rza1-hal phase 4: docs and CHANGELOG"`
