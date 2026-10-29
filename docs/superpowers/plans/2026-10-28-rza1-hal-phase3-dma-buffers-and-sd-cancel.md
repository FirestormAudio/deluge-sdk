# rza1-hal Phase 3: Checked DMA Buffers and Cancel-Safe SD Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** DMA buffers become a checked uncached type; the SD driver's block reads and writes take buffers instead of raw pointers, and a read or write dropped mid-flight stops its DMA and the SD transfer before it returns.

**Architecture:**
- A new `dma_buf` module provides `Uncached<'a>` and `UncachedMut<'a>`. Each wraps a byte slice and is built only if every byte lies in a normal non-cacheable section of the HAL's own MMU table (the `+0x4000_0000` mirrors). The check reads `memmap`'s section table, so no board fact enters the HAL.
- `Sdhi`'s DMA reads and writes take those types, and its PIO reads and writes take plain slices.
- Each transfer holds an `embassy_hal_internal::drop::OnDrop` guard. If the future is dropped before the transfer ends, the guard synchronously:
  - halts the DMA channel;
  - stops the SD transfer (SD_STOP.STP);
  - soft-resets the controller back to the identification clock (the existing `init`);
  - records the abort.
- `deluge-bsp`'s SD layer sees the abort on its next operation and requires `sd::init` again.

**Tech Stack:** Rust nightly, embassy-hal-internal 0.5 (`drop::OnDrop`), QEMU ARM host tests, pi-top bench (`hw-tests/sd-bench`).

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md`: decision 9 (checked uncached buffers, no per-transfer cache maintenance), §4 "SD DMA" (cancel safety, `OnDrop`), and §5 phase 3 (bench: SD read and write throughput no worse than the High-Speed baseline, plus a cancel-mid-transfer probe).

## Global Constraints

- **Spec decision 9, verbatim:** "The HAL knows each chip's uncached mirror ranges, so a buffer type (name is the builder's call) checks at construction that its memory is DMA-coherent, or builds itself from the mirror. SD, audio and iso DMA accept only that type. There is no per-transfer cache maintenance, so SD throughput is unaffected."
- **Spec §4, verbatim:** "An `embassy_hal_internal::drop::OnDrop` guard aborts the DMA and the SD command and waits for them to stop."
- **`Dma` gets no `Drop` (deviation from spec §4).** §4 says "`Dma` gets a `Drop` that stops its channel". But the SSI, SCUX and UART rings take `Dma` by value and keep running after their handle is gone (`ssi::init`, the `scux_*_path`s, `UartRx::steal`'s stolen `Dma`), so a channel-level `Drop` would stop the codec and the PIC link. The stop-on-drop lives on the transfer, as embassy-stm32's `Transfer` does. Record this as a ruling and report it to the user.
- **No behaviour change on the Deluge** apart from what cancellation adds. `tools/link-layout.sh check` must match, `tools/test.sh` must pass, and every firmware, hw-test and example must build with `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p <pkg>`.
- **Lockstep:** each task updates `deluge-bsp` and any other caller it breaks. DelugeFirmware is not migrated (phase 9).
- **Layering:** nothing under `rza1/` names `deluge`.
- **32-bit target:** `usize` is 32 bits on the device, so the range check must not overflow on `addr + len`.
- **Out of scope:** audio and USB-iso buffers (phases 5–6), and the unmerged `feat/sd-direct-dma` branch, which does per-transfer cache maintenance (decision 9 rules it out). Report the branch to the user as superseded.
- Never push or publish.

## Review Focus

1. **A buffer that straddles the end of a mirror window, or whose `addr + len` wraps past 4 GB on the 32-bit target**, must be refused rather than accepted on its first byte. Pinned by Task 1's `a_range_crossing_out_of_a_mirror_is_refused` and `a_wrapping_range_is_refused`.
2. **Cancelling before the DMA starts** (still in the command phase), **during the data phase**, and **after the data phase but before the future returns** must each leave the controller stopped and reset, and must never abort a transfer that already completed. Pinned by Task 3's three cancellation tests and its `a_completed_read_never_aborts` test.
3. **The operation after a cancel must not hang or read stale data.** The BSP has to refuse to use the card until `sd::init` runs again. Pinned by Task 4's `an_aborted_transfer_requires_reinit` and the Task 5 bench probe.
4. **A buffer whose length isn't a whole number of 512-byte blocks, or is empty**, gets an error, not a partial transfer or a hang. Pinned by Task 2's `a_buffer_that_is_not_whole_blocks_is_refused`.
5. **Throughput must not regress.** The checks run once per buffer construction, not per transfer. Pinned by Task 5's bench comparison against the baseline Task 1 records.

---

## File Structure

| Path | Change |
|---|---|
| `rza1/rza1-hal/src/dma_buf.rs` (new) | `Uncached`, `UncachedMut`, `NotUncached` |
| `rza1/rza1-hal/src/memmap.rs` | `pub fn is_uncached_range(start: usize, len: usize) -> bool`, over the existing section table |
| `rza1/rza1-hal/src/dmac.rs` | `pub(crate) fn halt_channel(ch: u8)` for abort paths (no `Drop` on `Dma`) |
| `rza1/rza1-hal/src/sdhi.rs` | slice/buffer-taking block reads and writes; the `OnDrop` abort; `take_aborted`; `SdhiError::BufferLength`, `#[non_exhaustive]` |
| `crates/deluge-bsp/src/sd.rs` | the bounce buffer as an `UncachedMut`; PIO paths on slices; the abort check |
| `hw-tests/sd-bench/src/tasks/bench.rs`, `Cargo.toml` | the cancel probe (`unstable-pac` to read the DMAC status) |

---

### Task 1: The checked uncached buffer types, and the bench baseline

**Files:**
- Create: `rza1/rza1-hal/src/dma_buf.rs`
- Modify: `rza1/rza1-hal/src/memmap.rs`, `rza1/rza1-hal/src/lib.rs` (`pub mod dma_buf;`)
- Test: `rza1/rza1-hal/src/memmap.rs` tests, `rza1/rza1-hal/src/dma_buf.rs` tests

**Interfaces:**
- Produces:

```rust
// memmap.rs: pure, host-testable
/// Whether every byte of `[start, start + len)` lies in a normal non-cacheable section of the
/// HAL's MMU table (the uncached mirrors). False for `len == 0` and for a range that wraps.
pub fn is_uncached_range(start: usize, len: usize) -> bool;

// dma_buf.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct NotUncached;                       // the buffer isn't DMA-coherent memory

pub struct Uncached<'a>(&'a [u8]);            // a DMA source
impl<'a> Uncached<'a> {
    pub fn new(buf: &'a [u8]) -> Result<Self, NotUncached>;
    pub fn as_slice(&self) -> &[u8];
    pub fn len(&self) -> usize;
    pub(crate) fn bus_addr(&self) -> u32;     // the DMAC address of byte 0
}
pub struct UncachedMut<'a>(&'a mut [u8]);     // a DMA destination
impl<'a> UncachedMut<'a> {
    pub fn new(buf: &'a mut [u8]) -> Result<Self, NotUncached>;
    pub fn as_slice(&self) -> &[u8];
    pub fn as_mut_slice(&mut self) -> &mut [u8];
    pub fn len(&self) -> usize;
    pub fn reborrow(&mut self) -> UncachedMut<'_>;   // so one static buffer serves many transfers
    pub(crate) fn bus_addr(&self) -> u32;
}
```

- `new` checks with `memmap::is_uncached_range` on the device. On the host, where tests run and memory has no mirror, it accepts any non-empty slice, as the existing `uncached()` helper treats host memory. Say so in the doc.
- The module doc explains how a board obtains such memory: a `static` buffer accessed only through its `+UNCACHED_MIRROR_OFFSET` alias, with its cached alias cleaned and invalidated once at boot, as `deluge-bsp`'s SD bounce buffer is.

- [ ] **Step 1: Record the bench baseline (before any code change)**

On the pi-top bench, build `hw-tests/sd-bench` from this branch's base (`--features rtt --release`) and run it with `deluge_jlink.py … --until 'sd-bench: done'`. Start `JLinkRemoteServerCLExe -Port 19020` on pi-top if `deluge_jlink.py` says "GDB server never connected" and nothing else holds the probe. Copy its read and write MB/s lines into the ledger as `Baseline:`.

- [ ] **Step 2: Write the failing tests**

```rust
// memmap.rs
#[test] fn the_sram_and_sdram_mirrors_are_uncached() {
    assert!(is_uncached_range(0x6000_0000, 512));          // SRAM mirror
    assert!(is_uncached_range(0x4C00_0000, 64 * 512));     // SDRAM (CS3) mirror
}
#[test] fn cached_memory_is_not_uncached() {
    assert!(!is_uncached_range(0x2000_0000, 512));         // SRAM, cached
    assert!(!is_uncached_range(0x0C00_0000, 512));         // SDRAM, cached
}
#[test] fn a_range_crossing_out_of_a_mirror_is_refused() {
    // The SRAM mirror is 10 sections (0x6000_0000..0x60A0_0000).
    assert!(is_uncached_range(0x609F_FE00, 512));
    assert!(!is_uncached_range(0x609F_FF00, 512));
}
#[test] fn a_wrapping_range_is_refused() { assert!(!is_uncached_range(0xFFFF_FF00, 0x200)); }
#[test] fn an_empty_range_is_refused() { assert!(!is_uncached_range(0x6000_0000, 0)); }

// dma_buf.rs (host)
#[test] fn an_empty_buffer_is_refused() { assert_eq!(UncachedMut::new(&mut []).err(), Some(NotUncached)); }
#[test] fn reborrow_keeps_the_same_memory() // as_slice().as_ptr() equal before and after reborrow
```

- [ ] **Step 3: Run to see them fail**

Run: `cd rza1 && cargo test --target armv7-unknown-linux-gnueabihf -p rza1-hal --features rza1l --lib memmap dma_buf`
Expected: FAIL to compile, because `is_uncached_range` is not defined.

- [ ] **Step 4: Implement**

`is_uncached_range` computes `start.checked_add(len)`, then checks the attribute of each 1 MB section from `start >> 20` to `(end - 1) >> 20` with the table's existing attribute lookup (`attr_at`, or whatever the table helper is named), requiring normal non-cacheable for every one.

- [ ] **Step 5: Verify**

Run: the test command, then `rza1/tools/test.sh`.
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git commit -am "rza1-hal: dma_buf::{Uncached, UncachedMut}, checked against the MMU table's uncached sections"
```

---

### Task 2: SD block reads and writes take buffers

**Files:**
- Modify: `rza1/rza1-hal/src/sdhi.rs` (the public `read_blocks_dma`/`write_blocks_dma`/`read_blocks_sw`/`write_blocks_sw` at ~1696–1745, the inner fns at ~753, ~797, ~940, ~1021, and `SdhiError` at ~599); `crates/deluge-bsp/src/sd.rs` (~276–290, ~615–780, ~915–980).
- Test: `rza1/rza1-hal/src/sdhi.rs` tests.

**Interfaces:**
- Consumes Task 1's `Uncached`, `UncachedMut` and `bus_addr`.
- Produces:

```rust
impl Sdhi<'_> {
    /// Issue `cmd_val` and read `buf.len() / 512` blocks into `buf` by DMA.
    pub async fn read_blocks_dma(&self, cmd_val: u16, buf: UncachedMut<'_>) -> Result<(), SdhiError>;
    /// Issue `cmd_val` and write `buf.len() / 512` blocks from `buf` by DMA.
    pub async fn write_blocks_dma(&self, cmd_val: u16, buf: Uncached<'_>) -> Result<(), SdhiError>;
    /// Read `buf.len() / 512` blocks by PIO (any memory: the CPU copies).
    pub async fn read_blocks_sw(&self, buf: &mut [u8]) -> Result<(), SdhiError>;
    /// Write `buf.len() / 512` blocks by PIO.
    pub async fn write_blocks_sw(&self, buf: &[u8]) -> Result<(), SdhiError>;
}
#[non_exhaustive]
pub enum SdhiError { /* existing variants */, /// The buffer is empty or not a whole number of 512-byte blocks.
    BufferLength }
```

- All four lose `unsafe`.
- `deluge-bsp` builds an `UncachedMut` over its `SD_DMA_BUF` static's mirror alias once, through an `unsafe` slice at the one place that knows the static is only touched through the mirror. It reborrows that buffer per transfer (`buf.reborrow()`, sliced to the chunk, by building the `UncachedMut` over the sub-slice). The PIO `BlockDevice` path passes its block slices.

- [ ] **Step 1: Write the failing test**

```rust
#[test] fn a_buffer_that_is_not_whole_blocks_is_refused() {
    let sdhi = sdhi1();
    let mut odd = [0u8; 513];
    let r = embassy_futures::block_on(sdhi.read_blocks_sw(&mut odd));
    assert_eq!(r, Err(SdhiError::BufferLength));
    let r = embassy_futures::block_on(sdhi.read_blocks_dma(CMD17, UncachedMut::new(&mut odd).unwrap()));
    assert_eq!(r, Err(SdhiError::BufferLength));
    // and no SD_CMD write happened (the command never went out)
}
```

`CMD17` is the existing command constant, or `17` if there is none.

- [ ] **Step 2: Run to see it fail**

Run: `cd rza1 && cargo test --target armv7-unknown-linux-gnueabihf -p rza1-hal --features rza1l --lib sdhi`
Expected: FAIL to compile (the functions take pointers).

- [ ] **Step 3: Implement**

1. Change the signatures; the length check runs before any register write.
2. The inner fns keep their raw-pointer bodies, with `buf.bus_addr()` / `buf.as_mut_ptr()`.
3. Migrate `deluge-bsp/src/sd.rs`.

- [ ] **Step 4: Verify**

Run: the test command, `tools/test.sh`, the firmware builds, and `tools/link-layout.sh check`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: Sdhi block reads and writes take buffers (DMA: Uncached/UncachedMut; PIO: slices)"
```

---

### Task 3: A dropped SD transfer aborts

**Files:**
- Modify: `rza1/rza1-hal/src/sdhi.rs`, `rza1/rza1-hal/src/dmac.rs`
- Test: `rza1/rza1-hal/src/sdhi.rs` tests

**Interfaces:**
- Consumes Task 2's API.
- Produces:
  - `pub(crate) fn dmac::halt_channel(ch: u8)`: `raw::halt`, then wait (bounded spin of at most 10 000 reads) until CHSTAT.EN reads 0.
  - In `Sdhi`: an `aborted: AtomicBool` field, and `pub fn take_aborted(&self) -> bool` ("whether a transfer was dropped mid-flight since the last call; the card must be re-identified").
  - Abort sequence, synchronous, in this order:
    1. `halt_channel` on the transfer's DMA channel (if any);
    2. `SD_STOP = 0x0001` (STP), then `SD_STOP = 0x0000`;
    3. `init(port, sd_option)` (soft reset, masks, identification clock);
    4. `aborted = true`.
  - Every transfer entry point holds `OnDrop::new(abort)` from before its first register write, and `defuse()`s it after its last. That covers `read_blocks_dma`, `write_blocks_dma`, `read_blocks_sw`, `write_blocks_sw`, `read_status_block_sw`, `send_cmd` and `stop_transfer`.

- [ ] **Step 1: Write the failing tests** (host; poll a future once with `core::task::Waker::noop()`, then drop it)

```rust
fn poll_once<F: Future>(f: F) { let mut f = core::pin::pin!(f); let _ = f.as_mut().poll(&mut Context::from_waker(Waker::noop())); }

#[test] fn a_read_dropped_in_its_command_phase_aborts() {
    let sdhi = sdhi1(); test::reset();
    let mut buf = [0u8; 512];
    poll_once(sdhi.read_blocks_dma(CMD17, UncachedMut::new(&mut buf).unwrap())); // pends on the response
    assert!(sdhi.take_aborted());
    let w = test::writes();
    assert!(w.contains(&(SD_STOP_1, 0x0001)) && w.contains(&(SOFT_RST_1, SOFT_RST_DEASSERT as u32)));
}
#[test] fn a_read_dropped_in_its_data_phase_halts_the_dma() {
    // poke SD_INFO1 so the response phase completes, poll once (now pending on DATA_TRNS), drop:
    // the DMA channel's CHCTRL gets CLREN and SD_STOP gets STP.
}
#[test] fn a_write_dropped_mid_flight_aborts() // as above for write_blocks_dma
#[test] fn a_completed_read_never_aborts() {
    // poke the response and DATA_TRNS flags and the DMA TC so the read completes in one poll:
    // take_aborted() is false and no SD_STOP or SOFT_RST write follows the transfer.
}
```

`SD_STOP_1`/`SOFT_RST_1` are SDHI1's register addresses, from the existing `the_pac_names_the_same_sdhi_registers` test.

- [ ] **Step 2: Run to see them fail**

Run: the Task 2 test command.
Expected: FAIL, because `take_aborted` is not defined.

- [ ] **Step 3: Implement**

The guard closure captures the port, `sd_option`, `&self.aborted` and the DMA channel number. It must not await.

- [ ] **Step 4: Verify**

Run: the test command, then `rza1/tools/test.sh`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: a dropped SD transfer halts its DMA, stops and resets the controller, and is reported"
```

---

### Task 4: The BSP requires re-identification after an abort

**Files:**
- Modify: `crates/deluge-bsp/src/sd.rs`
- Test: `crates/deluge-bsp/src/sd.rs`. The device module is `cfg(target_os = "none")`, so the abort check goes in a host-testable helper.

**Interfaces:**
- Consumes `Sdhi::take_aborted` (Task 3).
- Produces `fn card_usable(aborted: bool, ready: &AtomicBool) -> Result<(), SdError>`. It clears `ready` and returns `Err(SdError::NotInitialized)` if `aborted`, otherwise returns according to `ready`. Every card operation (`read_sector(s)`, `write_sector(s)`, the PIO `BlockDevice` path) calls it first, with `controller().take_aborted()` and `CARD_READY`. `sd::init` clears the flag (`take_aborted()`) before re-identifying.

- [ ] **Step 1: Write the failing test**

```rust
#[test] fn an_aborted_transfer_requires_reinit() {
    let ready = AtomicBool::new(true);
    assert_eq!(card_usable(true, &ready), Err(SdError::NotInitialized));
    assert!(!ready.load(Ordering::Relaxed));
    assert_eq!(card_usable(false, &ready), Err(SdError::NotInitialized), "still until init");
    ready.store(true, Ordering::Relaxed);
    assert_eq!(card_usable(false, &ready), Ok(()));
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host-uac,usb-host-overbridge --lib an_aborted`
Expected: FAIL to compile.

- [ ] **Step 3: Implement and wire it in**

- [ ] **Step 4: Verify**

Run: the test, `tools/test.sh`, the firmware builds, and `tools/link-layout.sh check`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "deluge-bsp: an aborted SD transfer makes the card need sd::init again"
```

---

### Task 5: Bench — throughput and the cancel probe; CHANGELOG

**Files:**
- Modify: `hw-tests/sd-bench/src/tasks/bench.rs`, `hw-tests/sd-bench/Cargo.toml` (`rza1-hal` gains `unstable-pac`), `CHANGELOG.md`

**Interfaces:**
- Consumes the BSP's `sd::read_sectors`, `sd::init` and `card_usable` behaviour.
- Produces a cancel phase in sd-bench. It logs `cancel: PASS` or `cancel: FAIL <why>`.

- [ ] **Step 1: Add the cancel phase**

After the throughput phase:
1. Hash sector range `[base, base + 64)` with a normal read.
2. Fill a 64-sector buffer with `0xA5`.
3. Run `embassy_futures::select::select(sd::read_sectors(base, 64, buf), Timer::after_micros(400))`. If the read wins, log it and retry with 128 sectors and 200 µs, up to three times. If it never loses, log `cancel: FAIL read always won`.
4. Read the SD receive channel's status (`deluge_bsp::board::SdRxDma`'s number, through `rza1_hal::pac::DMAC.ch(n).chstat()`). Its EN bit must be 0.
5. The next `sd::read_sectors` must return `Err(SdError::NotInitialized)`.
6. `sd::init()` must succeed, and a fresh read of the range must hash the same as step 1.

- [ ] **Step 2: Run on the bench**

Run: `deluge_jlink.py <sd-bench elf> --select IP=pi-top --until 'sd-bench: done' --fail-on 'panic|FAIL'`.
Expected:
- `cancel: PASS`.
- Read and write MB/s within 3% of Task 1's baseline, or better.
- Record both in the ledger.

- [ ] **Step 3: Full checks**

Run: `tools/test.sh`, `tools/check-layering.sh`, `tools/link-layout.sh check`, every package build, and `cd rza1 && cargo doc --no-deps -p rza1-hal --features rza1l`.
Expected: pass, with no more than 19 doc warnings.

- [ ] **Step 4: CHANGELOG**

Under `[Unreleased] / Changed`, cover:
- `dma_buf::{Uncached, UncachedMut}`;
- the new `Sdhi` signatures;
- `SdhiError::BufferLength` and `#[non_exhaustive]`;
- cancel safety and `take_aborted`;
- the BSP's re-init after an abort;
- the deliberate absence of a `Drop` on `Dma`, and why.

- [ ] **Step 5: Commit**

```bash
git commit -am "sd-bench: cancel probe; CHANGELOG: rza1-hal phase 3"
```
