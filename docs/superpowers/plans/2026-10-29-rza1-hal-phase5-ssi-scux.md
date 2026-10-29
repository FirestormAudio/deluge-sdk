# rza1-hal Phase 5: SSI and SCUX Owned Drivers with Lent Rings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** SSI (I²S) and SCUX become owned drivers over any capable instance. They run circular DMA over caller-lent uncached rings and offer an embassy-style copy API plus an interrupt-reachable positional window. The HAL drops all its audio statics, and the Deluge's SCUX paths, the USB-TX one included, move to `deluge-bsp`.

**Architecture:**
- **Rings.** `dmac::ring` adds a circular link-descriptor DMA over a lent ring, split into blocks. Each block's END interrupt counts laps, which gives an absolute DMA position. `ring_buffer::{WritableRing, ReadableRing}` build the copy API (`write`/`read`, with underrun/overrun) and the window (`position`, `ring_mut`/`ring`) on top of it.
- **SSI and SCUX drivers.** `ssi::Ssi` (SSIF0/1/3) hands out `SsiTx`/`SsiRx` rings, or an `ssi::ScuxOutput` when SCUX drives its TX. `scux::Scux::new` splits SCUX into four typed channels. A channel becomes a `scux::Path` (FFD ring → SRC → DVU → FFU ring, or → SSI) whose SSI routes are checked at compile time against the manual's SSIRSEL table.
- **Migration.** New API first, consumers migrated next, legacy deleted last, so every task leaves the tree green.

**Tech Stack:** Rust `no_std`, embassy-hal-internal, embassy-sync `AtomicWaker`, the `rza1_pac::mmio::test` shadow seam, `rza1/tools/gen-pins.py`.

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md`. Relevant parts:
- decisions 2, 7, 8 and 9;
- "My calls": SSI, SCUX;
- §5 item 5.

## Global Constraints

- **Decision 2, verbatim:** "Audio ring buffers are lent by the caller as `&'d mut [u32]`, the embassy-stm32 `RingBufferedSai` / `ReadableRingBuffer` shape. The board crate places the buffer, for example in uncached SRAM. The HAL holds no audio statics. Dropping the driver stops DMA before the borrow ends."
  - Here the lent type is the checked uncached type: `dma_buf::UncachedMut<'d, u32>` (decision 9).
  - A per-DMA-channel link-descriptor table in `dmac` is DMA-controller state, not an audio buffer, so it may be a static.
- **Decision 7:** "The copy API is embassy-style `read`/`write`, with overrun and underrun errors. The positional window gives the hardware DMA position and a writable (or readable) window of the ring ahead of it… It must keep today's latency, and it must be reachable from interrupt context."
- **Decision 8, the phase 6 coupling.** `WritableRing<'d>` and `ReadableRing<'d>` are `Send`. Every window method is synchronous, takes `&self`/`&mut self`, never awaits, and takes no lock beyond a critical section this short. Phase 6's `IsoStream` consumer will own one, through `&mut`, in USB-ISR context. Sample conversion stays with the caller in this phase.
- **SSI:** "becomes an owned driver over any SSI channel: `Peri<'d, SSIx>`, its DMA channel(s) and pins, plus a `#[non_exhaustive] Config` with `Default`."
  - Instances: SSIF0, SSIF1, SSIF3.
  - SSIF2 is half-duplex (one SSIDATA2 pin, one SSIRTI2 request) and stays out until a user needs it. Say so in the module doc.
- **SCUX:** "becomes composable pieces (SRC, DVU, routing), each owned. The Deluge's USB-TX path is assembled from them in `deluge-bsp`." The ~25 public raw `unsafe fn` in `scux` become `pub(crate)` or methods.
- **Layering:** nothing under `rza1/` names "deluge" (`tools/check-layering.sh`). Board facts stay in `deluge-bsp`, which covers SSICR 0x002B_C020, the DMA channel numbers, codec power, AUDIO_XOUT on P7_11, the 44.1 kHz engine and the write-ahead constants.
- **Lockstep:** BSP, firmwares, hw-tests, examples and `deluge-sdk` migrate here. DelugeFirmware is not touched until phase 9.
  - Its C ABI uses `tx_current_ptr`/`tx_buf_start`/`rx_current_ptr`/`rx_buf_start`/`TX_FRAMES`/`RX_FRAMES` and must stay expressible.
  - The mapping is `position()`, `ring_mut().as_ptr()`, `len() / 2`. Record it in the CHANGELOG.
- **Manual facts this plan pins** (from the `pdftotext -layout` extract of `~/Downloads/r01uh0437ej0700_rz_a1l.pdf`):
  - **SSI DMARS** = MID<<2|RID, with MID 0x38+n, RID 01 TX and 10 RX:
    - SSIF0 0xE1/0xE2;
    - SSIF1 0xE5/0xE6;
    - SSIF3 0xED/0xEE.
  - **SCUX DMARS:** FFDn 0x101+4n, FFUn 0x102+4n.
  - **SSIRSEL SOSEL** (SSIFm output ← SCUX channel):
    - SSIF0 ← ch0 (SOSEL0=00) or ch1 (01);
    - SSIF1 ← ch2 (SOSEL1=00);
    - SSIF3 ← ch1 (SOSEL3=00) or ch0 (01).
  - **SSICTRL direct-TX bit:** SSIF0 14, SSIF1 13, SSIF3 30.
  - **FDTSEL/FUTSEL SCKSEL:** WS of SSIFm = 0b1000+m; USB_X1 = 0b0011.
  - **SSI reset:** CPG SWRSTCR1 bit (6 − n).
- **SSICR fields** (decoded from the bench-proven 0x002B_C020):
  - CKS bit 30 (0 = AUDIO_X1);
  - DWL bits 21:19 (101 = 24-bit);
  - SWL bits 18:16 (011 = 32-bit);
  - SCKD bit 15 and SWSD bit 14 (1 = master);
  - DEL bit 8 (0 = I²S one-clock delay);
  - CKDV bits 7:4 (0010 = ÷4).
- **Pins:** the BSP's bench-proven muxing puts SSITxD0 on plain `set_as_alternate`, not `set_as_alternate_software_io`. The SSI driver uses the plain `gpio::mux`. Don't "fix" this to Table 41.7 without a bench run.
- **Done when:**
  - `verify.sh` (`/tmp/claude-1000/-home-kate-GitHub-deluge-sdk/c1820c3c-843c-48c7-935a-6aa619b4acfc/scratchpad/verify.sh`) prints PASS for every line;
  - the Task 10 bench check passes.
  - HAL unit tests: `ht.sh rza1l [filter]` (`…/scratchpad/ht.sh`).
- **32-bit target:** absolute positions are `u64`, and ring index maths is modulo the length. Nothing in it can overflow `usize`.

## Review Focus

1. **A driver dropped while its DMA runs.** The DMA must be halted (EN and TACT clear) and the SSI's TEN/REN cleared before `drop` returns, because the lent ring's borrow ends right after. Test `dropping_a_ring_halts_its_channel_before_returning` (Task 2) and `dropping_ssi_tx_clears_ten_then_halts` (Task 5).
2. **A position read just after the DMA crossed a block boundary, before that block's END interrupt has counted.** The absolute position must never go backwards. Test `absolute_position_is_monotonic_across_an_uncounted_block_end` (Task 2).
3. **A copy-API writer or reader stalled for more than a ring.** Expected: one `Err(Underrun)`/`Err(Overrun)`, then data resumes at the lead/anchor, not garbage from a stale cursor. Tests `a_writer_lapped_by_the_dma_reports_underrun_once` and `a_reader_lapped_by_the_dma_reports_overrun_once` (Task 3).
4. **CRSA reading one past the ring's end at the descriptor reload** (the survey saw this on silicon). `position()` must stay below `len()`. Test `position_wraps_a_crsa_one_past_the_end` (Task 2).
5. **An ISR reaching the BSP's ring before `audio::init`, or while a task holds it.** `with_tx`/`with_rx` must return `None` and never panic or deadlock. Test `with_tx_before_init_is_none` (Task 8).

---

### Task 1: Word-typed uncached buffers

**Files:**
- Modify: `rza1/rza1-hal/src/dma_buf.rs`

**Interfaces:**
- Produces:
  - `pub trait Word: sealed::Sealed + Copy + 'static {}` for `u8, u16, u32, i32`.
  - `pub struct UncachedMut<'a, W: Word = u8>(&'a mut [W])` and `pub struct Uncached<'a, W: Word = u8>(&'a [W])`. Existing byte users (sdhi) are unchanged through the default parameter.
  - `UncachedMut::<W>::new(buf: &'a mut [W]) -> Result<Self, NotUncached>` checks the byte range `buf.as_ptr()..+len*size_of::<W>()`.
  - `pub(crate) fn bus_addr(&self) -> u32`, `pub fn len(&self) -> usize` (in words), `as_slice`/`as_mut_slice` typed `[W]`.

- [ ] **Step 1: Write the failing tests**
  - `a_word_buffer_checks_its_byte_range`:
    - On the host `memmap` passes any non-empty slice, so assert `UncachedMut::<u32>::new(&mut [0u32; 4]).unwrap().len() == 4`.
    - `UncachedMut::<u32>::new(&mut []).is_err()` (empty, as the byte rule).
    - Through `memmap::is_uncached_range`, the byte length checked is 16 for 4 words: call the range helper the impl uses with a `[u32; 4]` and assert it was passed `len == 16`. Add a `#[cfg(test)]` hook that records the last checked length, or test the helper directly.
  - `the_byte_default_still_names_sdhi_buffers`: `let _: UncachedMut<'_> = UncachedMut::new(&mut [0u8; 512][..]).unwrap();`.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l dma_buf::`
  Expected: compile errors: `UncachedMut` takes no type parameter.
- [ ] **Step 3: Implement.** Make the two types generic, with the `Word` sealed trait. Keep every existing method's behaviour for `u8`.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l dma_buf::`, then `verify.sh`.
  Expected: PASS, with sdhi and all builds green.
- [ ] **Step 5: Commit** `rza1-hal: dma_buf buffers are generic over the DMA word`

### Task 2: `dmac::ring` — circular block DMA with an absolute position

**Files:**
- Create: `rza1/rza1-hal/src/dmac/ring.rs`; declare `pub(crate) mod ring;` in `dmac.rs`.
- Modify: `rza1/rza1-hal/src/dmac.rs` (`BlockInterruptHandler` also bumps a per-channel block counter).

**Interfaces:**
- Consumes: `dma_buf::UncachedMut<'d, u32>`, `BlockDma<'d>`, `Dma::{init_with_link_descriptor, channel_start, current_src, current_dst, halt}`.
- Produces (crate-internal):
  - `pub(crate) const MAX_RING_BLOCKS: usize = 16;`
  - `static RING_DESC: [[LinkDesc; MAX_RING_BLOCKS]; 16]`: per channel, 32-byte aligned, written through `crate::uncached`. Each descriptor is header `0b1101`, src, dst, byte count, CHCFG, 0, 0, NXLA → the next one, with the last looping to the first.
  - `static BLOCKS_DONE: [AtomicU32; 16]`, incremented by `BlockInterruptHandler` before it sets `DMAC_BLOCK_READY`.
  - `pub(crate) enum Dir { MemToPeriph { dst: u32 }, PeriphToMem { src: u32 } }`.
  - `pub(crate) struct RingDma<'d> { dma: BlockDma<'d>, buf: UncachedMut<'d, u32>, blocks: usize, frame_words: usize, dir: Dir }`, with:
    - `pub(crate) fn start(dma: BlockDma<'d>, buf: UncachedMut<'d, u32>, blocks: usize, frame_words: usize, chcfg: u32, dir: Dir, dmars: u32) -> Self`. It panics unless `1 <= blocks <= MAX_RING_BLOCKS` and `buf.len() % (blocks * frame_words) == 0`. CHCFG has DEM clear (an END interrupt per block) and the channel's index in bits 2:0. It resets `BLOCKS_DONE` to 0, then `init_with_link_descriptor` + `channel_start`.
    - `pub(crate) fn len(&self) -> usize` (words).
    - `pub(crate) fn position(&self) -> usize`: the word index the DMA moves next, `((CRSA or CRDA) − bus_addr) / 4 % len`, rounded down to a frame.
    - `pub(crate) fn absolute(&self) -> u64`: words moved since `start`, monotonic; algorithm below.
    - `pub(crate) async fn wait_block(&self)` (`BlockDma::wait_block`).
    - `pub(crate) fn channel(&self) -> u8`.
    - `pub(crate) fn buf(&self) -> &[u32]` / `buf_mut(&mut self) -> &mut [u32]`.
  - `impl Drop for RingDma<'_>`: `self.dma.halt()` (stop and wait for EN and TACT clear) before the buffer borrow ends.

`absolute()` algorithm, needed because the count lags the DMA by the END interrupt's latency:
```rust
loop {
    let c1 = BLOCKS_DONE[ch].load(Acquire);
    let p = self.position();
    let c2 = BLOCKS_DONE[ch].load(Acquire);
    if c1 != c2 { continue; }
    let block_words = self.len() / self.blocks;
    let in_block = (p / block_words) as u32;
    // The DMA is in block (c1 % blocks), or already in the next one whose END hasn't counted.
    let counted = if in_block == c1 % self.blocks as u32 { c1 } else { c1 + 1 };
    return counted as u64 * block_words as u64 + (p % block_words) as u64;
}
```
Document that the absolute position is right while the END interrupt's latency is under one block (2.9 ms at 128 frames, 44.1 kHz).

- [ ] **Step 1: Write the failing tests.** Use `ring.rs`'s own test module; bind `DMAINT5 => BlockInterruptHandler<DMA_CH5>` as `dmac`'s tests do, and take a serial lock.
  - `a_ring_links_its_blocks_in_a_loop`:
    - Start 4 blocks over 64 words (frame 2).
    - For MemToPeriph, descriptor *i* has src = `bus_addr + i*64` (bytes), dst = the peripheral register, byte count 64, NXLA_i = desc_{(i+1)%4}, and CHCFG DEM (bit 24) clear.
    - NXLA of the channel → desc_0.
  - `position_wraps_a_crsa_one_past_the_end`: poke CRSA = `bus_addr + 256` (one past a 64-word ring) → `position() == 0`.
  - `position_rounds_down_to_a_frame`: CRSA = `bus_addr + 4*5` → 4.
  - `absolute_position_is_monotonic_across_an_uncounted_block_end`:
    - Blocks done = 0, CRSA in block 1 at word 18 → `absolute() == 18`.
    - Run the handler once (count 1) at the same CRSA → still 18.
  - `absolute_counts_whole_laps`: with count 5 (via the handler, five times) and CRSA at block 1, word 20, in a 4-block 64-word ring → `absolute() == 5*16 + 4`. Block (5%4)=1 matches, so this is counted.
  - `dropping_a_ring_halts_its_channel_before_returning`: after `drop`, the log contains the CHCTRL CLREN write, then a CHSTAT read showing EN|TACT clear, before the test reads it. Shadow CHSTAT = 0.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l dmac::ring`
  Expected: the module is not found.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh`.
  Expected: PASS.
- [ ] **Step 5: Commit** `rza1-hal: dmac ring: circular block DMA over a lent ring with an absolute position`

### Task 3: `ring_buffer` — copy API and positional window

**Files:**
- Create: `rza1/rza1-hal/src/ring_buffer.rs` (`pub mod ring_buffer;`)

**Interfaces:**
- Consumes: `dmac::ring::RingDma` (Task 2).
- Produces (public):
  - `pub struct RingBuf<'d> { pub dma: BlockDma<'d>, pub buf: UncachedMut<'d, u32>, pub blocks: usize }`: what a driver takes per direction.
  - `#[non_exhaustive] pub enum Error { Overrun, Underrun }`, with derives per the error-enum rule, `defmt` behind the feature, `Display`, and `core::error::Error`.
  - `pub struct WritableRing<'d> { ring: RingDma<'d>, cursor: Option<u64>, lead: usize }` (memory → peripheral):
    - `pub fn len(&self) -> usize`, `pub fn frame_words(&self) -> usize`;
    - `pub fn position(&self) -> usize`;
    - `pub fn ring_mut(&mut self) -> &mut [u32]`: the window;
    - `pub fn set_lead(&mut self, words: usize)`: default `len / 2`, rounded to a frame, panics if ≥ `len`;
    - `pub async fn write(&mut self, data: &[u32]) -> Result<usize, Error>`;
    - `pub fn write_immediate(&mut self, data: &[u32]) -> Result<usize, Error>`: never waits; 0 when full.
  - `pub struct ReadableRing<'d>` (peripheral → memory):
    - `len`, `frame_words`, `position`;
    - `pub fn ring(&self) -> &[u32]`;
    - `pub async fn read(&mut self, out: &mut [u32]) -> Result<usize, Error>`;
    - `pub fn read_immediate(&mut self, out: &mut [u32]) -> Result<usize, Error>`;
    - `pub fn block_signal(&self) -> BlockSignal`.
  - `#[derive(Clone, Copy)] pub struct BlockSignal { ch: u8 }` with `pub async fn wait(&self)`. Waiting on a block needs no borrow of the ring, so a task can await while an ISR owns the ring.
  - Both rings are `Send`. `pub(crate) fn new(ring: RingDma<'d>) -> Self` for drivers.
- Copy semantics, with the absolute position `hw = ring.absolute()`:
  - **Writer:**
    - With no cursor, anchor `cursor = hw + lead`.
    - If `cursor < hw`, return `Err(Underrun)` once and re-anchor (the next call writes).
    - Free = `len − (cursor − hw)`. Copy `min(free, data.len())` rounded down to whole frames into `cursor % len`, with wrap.
    - `write` waits on the block signal while free is 0, and returns n > 0.
  - **Reader:**
    - With no cursor, anchor `cursor = hw`.
    - If `hw − cursor > len`, return `Err(Overrun)` once and re-anchor to `hw`.
    - Available = `hw − cursor`.
  - Mixing the window and the copy API is allowed but uncoordinated; document that the copy cursor ignores window writes.

- [ ] **Step 1: Write the failing tests.** Drive the DMA position by poking CRSA/CRDA and running the block handler, as Task 2 does.
  - `the_first_write_lands_lead_words_ahead_of_the_dma`:
    - 4 blocks, 64 words, lead 16, DMA at absolute 4.
    - `write_immediate(&[1,2,3,4])` returns `Ok(4)`, and `ring_mut()[20..24] == [1,2,3,4]`.
  - `a_full_ring_takes_nothing_until_the_dma_moves`:
    - Write until 0 (64 − 16 lead … the free space is `len − (cursor − hw)`).
    - Move the DMA by one block: free is 16.
  - `a_writer_lapped_by_the_dma_reports_underrun_once`:
    - After a write, advance the DMA past the cursor (run the handler 5 times, a whole lap plus one).
    - Expect `Err(Underrun)`, then `Ok(n)` written `lead` ahead of the new position.
  - `a_reader_gets_what_the_dma_wrote_and_overrun_after_a_lap`:
    - RX ring: anchor; advance CRDA by 6 words; `read_immediate` into a `[0; 8]` → `Ok(6)` with those words.
    - Then run the handler 5 times (more than a ring ahead) → `Err(Overrun)`, then `Ok(0)`.
  - `a_reader_lapped_by_the_dma_reports_overrun_once`: as above, with a second read giving `Ok(_)`, not an error.
  - `async_write_waits_for_a_block`: `poll` `write` on a full ring → `Pending`; run the handler → `Ready(Ok(n))`. Use the counting waker as in the uart tests.
  - `rings_are_send`: `fn assert_send<T: Send>() {} assert_send::<WritableRing<'static>>(); assert_send::<ReadableRing<'static>>();`
  - `writes_are_whole_frames`: `write_immediate(&[1,2,3])` with frame 2 → `Ok(2)`.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l ring_buffer`
  Expected: the module is not found.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh`.
  Expected: PASS.
- [ ] **Step 5: Commit** `rza1-hal: ring_buffer: lent DMA rings with a copy API and a positional window`

### Task 4: SSI pins, `ssi::Config` and `ssi::Instance`

**Files:**
- Modify:
  - `rza1/tools/gen-pins.py`: SIGNALS gains `^SSISCK([013])$` → `crate::ssi::SckPin`; `^SSIWS([013])$` → `WsPin`; `^SSITxD([013])$` → `TxdPin`; `^SSIRxD([013])$` → `RxdPin`, each with instance `SSIF{n}`.
  - Regenerate `rza1/rza1-hal/src/gpio/rza1l_functions.rs`.
  - `rza1/rza1-hal/src/ssi.rs`: add new items beside the legacy ones.

**Interfaces:**
- Produces:
  - `pin_trait!` `ssi::{SckPin, WsPin, TxdPin, RxdPin}`.
  - `pub trait Instance: PeripheralType + sealed + 'static { const INDEX: u8; const DMARS_TX: u32; const DMARS_RX: u32; }` for SSIF0 (0, 0xE1, 0xE2), SSIF1 (1, 0xE5, 0xE6) and SSIF3 (3, 0xED, 0xEE). Add `fn regs(n) -> crate::pac::ssif::Ssif` mapping 0/1/3, plus 2 for completeness.
  - `#[non_exhaustive] pub enum ClockDiv { Div1, Div2, Div4, Div8, Div16, Div32, Div64, Div128, Div6, Div12, Div24, Div48, Div96 }`: CKDV encodings 0000…1100 in that order.
  - `pub enum Role { Master(ClockDiv), Slave }`.
  - `pub enum DataBits { B8, B16, B18, B20, B22, B24, B32 }` (DWL 000…110).
  - `pub enum SystemWordBits { B8, B16, B24, B32, B48, B64, B128, B256 }` (SWL 000…111).
  - `pub enum ClockSource { AudioX1, AudioClk }` (CKS 0/1).
  - `#[non_exhaustive] pub struct Config { pub role: Role, pub data_bits: DataBits, pub system_word_bits: SystemWordBits, pub clock_source: ClockSource }`, with `Default` = `Master(Div4)`, `B24`, `B32`, `AudioX1`.
  - `pub(crate) fn ssicr(config: &Config) -> u32` (I²S: DEL = 0, both polarities 0, CHNL 0; TEN/REN 0).

- [ ] **Step 1: Write the failing tests**
  - `the_deluge_codec_config_encodes_0x002b_c020`: `ssicr(&Config::default()) == 0x002B_C020`.
  - `slave_mode_clears_sckd_and_swsd`: `ssicr(&Config{ role: Role::Slave, ..}) & (3 << 14) == 0`.
  - `data_and_word_bits_land_in_dwl_and_swl`: B16/B16 → bits 21:19 = 001 and 18:16 = 001.
  - `ssi_instances_know_their_dma_requests`: the three triples above.
  - `ssi_pin_functions_are_generated`: a generic `fn f<P: TxdPin<SSIF0>>() {}` called with `P6_10`, and with `P6_8` as `SckPin<SSIF0>`. These are the BSP's bench pins.
- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && python3 tools/gen-pins.py && ht.sh rza1l ssi::`
  Expected: `Config`/`ssicr`/`TxdPin` are not found. The generator succeeds.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh` (it includes the `gen-pins --check`).
  Expected: PASS.
- [ ] **Step 5: Commit** `rza1-hal: ssi Config, Instance and pin functions`

### Task 5: The owned SSI driver

**Files:**
- Modify: `rza1/rza1-hal/src/ssi.rs` (new driver section; legacy functions untouched until Task 9).

**Interfaces:**
- Consumes: Tasks 3–4 (`RingBuf`, `WritableRing`, `ReadableRing`, `Config`, `Instance`, pin traits).
- Produces:
  - `pub struct Ssi;`, a namespace for the constructors.
  - `Ssi::new<T: Instance>(ssi: Peri<'d, T>, sck: Peri<'d, impl SckPin<T>>, ws: Peri<'d, impl WsPin<T>>, txd: Peri<'d, impl TxdPin<T>>, rxd: Peri<'d, impl RxdPin<T>>, tx: RingBuf<'d>, rx: RingBuf<'d>, config: Config) -> (SsiTx<'d>, SsiRx<'d>)`: full duplex.
  - `Ssi::new_rx_only<T: Instance>(ssi, sck, ws, rxd, rx: RingBuf<'d>, config) -> SsiRx<'d>`.
  - `Ssi::new_with_scux_tx<T: Instance>(ssi, sck, ws, txd, rxd, rx: RingBuf<'d>, config) -> (ScuxOutput<'d, T>, SsiRx<'d>)`: TX is fed by SCUX (Task 7 enables it).
  - `pub struct SsiTx<'d>` derefs to `WritableRing<'d>`, and `pub struct SsiRx<'d>` derefs to `ReadableRing<'d>`. Both have `DerefMut`.
  - `pub struct ScuxOutput<'d, T: Instance>`, with `pub(crate) fn enable(&self)` (clear SSIFCR.TFRST, set SSICR.TEN) for Task 7.
  - Drop:
    - `SsiTx` clears SSICR.TEN (read-modify-write in a critical section), then its ring halts.
    - `SsiRx` clears REN, then halts.
    - `ScuxOutput` clears TEN.
- Bring-up (per instance; the legacy `start` sequence with `T`'s registers):
  1. SWRSTCR1 bit (6 − n) pulse.
  2. SSITDMR = CONT (1 << 8); SSICR = `ssicr(&config)`; SSIFCR = 0xA3.
  3. Start the ring DMAs: CHCFG as legacy `tx_chcfg`/`rx_chcfg` with DEM clear; dst SSIFTDR / src SSIFRDR; DMARS from `T`.
  4. Clear TFRST, set TIE (TX); clear RFRST, set RIE (RX).
  5. Set TEN and/or REN together.
- Frame words = 2 (stereo I²S, CHNL = 0).
- `Ssi::new*` don't touch AUDIO_XOUT or codec power: those are the board's.

- [ ] **Step 1: Write the failing tests.** Use SSIF0, taking `DMA_CH6`/`DMA_CH7` via `steal` with `BlockInterruptHandler` bindings and pins P6_8..P6_11. Lend rings as leaked `[u32; 256]` slices, wrapped with `UncachedMut::new`.
  - `ssi_new_programs_the_codec_config_and_starts_both_rings`:
    - SSICR (`peek32(0xE820_B000)`) = 0x002B_C020 | the legacy `SSICR_TEN` | `SSICR_REN` constants.
    - DMARS3 = 0x00E2_00E1. Both channels have SETEN in the log, after the SSICR write. SSIFCR ends with TFRST/RFRST clear and TIE/RIE set.
  - `rx_only_leaves_ten_clear`.
  - `dropping_ssi_tx_clears_ten_then_halts`: the SSICR write with TEN clear comes before ch6's CLREN in the log; REN stays set.
  - `ssi_tx_is_a_writable_ring`: `tx.len() == 256`, `tx.frame_words() == 2`.
  - `the_scux_tx_variant_starts_rx_only`: TEN clear after `new_with_scux_tx`, and `ScuxOutput::enable` then sets it.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l ssi::`
  Expected: `Ssi` is not found.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh`.
  Expected: PASS.
- [ ] **Step 5: Commit** `rza1-hal: owned SSI driver over lent rings`

### Task 6: SCUX channels and FFD→FFU paths

**Files:**
- Modify:
  - `rza1/rza1-hal/src/scux.rs`: a new section; the raw functions stay, used internally.
  - `rza1/rza1-hal/src/scux/regs.rs` only if a constant is missing.

**Interfaces:**
- Consumes: Task 3 (`RingBuf`, rings), the existing `configure_*`/`start_path`/`stop_path`/`intifs`/`input_timing`, `AudioInfo`, `RampConfig`.
- Produces:
  - Marker types `pub struct Ch0; Ch1; Ch2; Ch3;` and `pub trait ChannelId: sealed { const N: u8; }`.
  - `pub struct Scux<'d>`. `Scux::new(scux: Peri<'d, SCUX>) -> Channels<'d>` runs `reset()` once and returns `pub struct Channels<'d> { pub ch0: Channel<'d, Ch0>, pub ch1: Channel<'d, Ch1>, pub ch2: Channel<'d, Ch2>, pub ch3: Channel<'d, Ch3> }`.
  - `pub enum Src { Bypass, Async { input_hz: u32, output_hz: u32 } }` (`#[non_exhaustive]`).
    - SRC unit = N/2, pair = N%2.
    - Async sets INTIFS = `intifs(input_hz, output_hz)`.
    - On an FFU path, FDTSEL and FUTSEL are written exactly as legacy `src_path::init` writes them for the same rates. The test compares against that path's write log.
  - `pub enum Dvu { Bypass, Volume { volume: u32, ramp: Option<RampConfig>, zero_cross_mute: bool } }` (`#[non_exhaustive]`).
  - `Channel<'d, C>::into_ffu_path(self, format: AudioInfo, ffd: RingBuf<'d>, src: Src, dvu: Dvu, ffu: RingBuf<'d>) -> Path<'d>`: IPC N = FfdToSrcAsync (FfdToSrcSync for Bypass); OPC N = ToFfu.
    - FFD DMA: dst DMATDn, DMARS 0x101+4N, CHCFG `ffd_chcfg`.
    - FFU DMA: src DMATUn, DMARS 0x102+4N, `ffu_chcfg` with DEM cleared (the ring counts blocks).
    - Then `start_path` with this channel's bit in each mask.
  - `pub struct Path<'d>` with `pub fn split(self) -> (Control<'d>, WritableRing<'d>, Option<ReadableRing<'d>>)`.
  - `pub struct Control<'d>` with:
    - `pub fn set_volume(&mut self, audio_ch: u8, vol: u32)`;
    - `pub fn set_volume_all(&mut self, vol: u32)`;
    - `pub fn fade_to(&mut self, vol: u32, ramp: RampConfig)`;
    - `pub fn set_input_rate(&mut self, input_hz: u32)`, which panics on a Bypass path;
    - Drop runs `stop_path` for this channel's bits.
  - Every shared-register write (DMACR, FDTSEL/FUTSEL, SSICTRL, SSIRSEL) is a read-modify-write in a critical section, so two channels' paths coexist.

- [ ] **Step 1: Write the failing tests**
  - `a_channel2_ffu_path_routes_ffd2_src1_0_ffu2`: after `into_ffu_path` on ch2 with `Src::Async { 48_000, 44_100 }`, assert:
    - IPC2 = 3 (FfdToSrcAsync) and OPC2 = 3 (ToFfu);
    - the INTIFS of SRC unit 1 pair 0 = `intifs(48_000, 44_100)`;
    - DMARS for the FFD/FFU channels = 0x109 / 0x10A;
    - both rings started;
    - DMACR has the ch2 bits and only those.

    Read the register addresses from the PAC, as the existing scux tests do.
  - `two_paths_keep_each_others_dmacr_bits`: ch1 then ch2 → both channels' bits are set.
  - `dropping_control_stops_only_its_channel`.
  - `set_volume_writes_its_dvu_channel`: the DVU2 volume register for `audio_ch` 1.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l scux::`
  Expected: `Scux`/`Channel` are not found.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh`.
  Expected: PASS.
- [ ] **Step 5: Commit** `rza1-hal: scux channels and FFD-to-FFU paths over lent rings`

### Task 7: SCUX → SSI paths, compile-time routes

**Files:**
- Modify: `rza1/rza1-hal/src/scux.rs`.

**Interfaces:**
- Consumes: Task 5 (`ssi::ScuxOutput<'d, S>`, `ssi::Instance`), Task 6.
- Produces:
  - `pub trait RoutesTo<S: ssi::Instance>: ChannelId { const SOSEL_SHIFT: u32; const SOSEL: u32; }` for:
    - (Ch0, SSIF0, 0, 0b00);
    - (Ch1, SSIF0, 0, 0b01);
    - (Ch2, SSIF1, 4, 0b00);
    - (Ch1, SSIF3, 12, 0b00);
    - (Ch0, SSIF3, 12, 0b01).
  - `const fn ssictrl_tx_bit(ssi: u8) -> u32`: 0 → 14, 1 → 13, 3 → 30.
  - `Channel<'d, C>::into_ssi_path<S: ssi::Instance>(self, format: AudioInfo, ffd: RingBuf<'d>, src: Src, dvu: Dvu, out: ssi::ScuxOutput<'d, S>) -> Path<'d> where C: RoutesTo<S>`:
    1. IPC N = FfdToSrc*, OPC N = ToSsi.
    2. SSIRSEL SOSEL field = `C::SOSEL`.
    3. FDTSEL = `input_timing` for Async, else DIVEN | WS of SSIF`S::INDEX` (0b1000 + m). FUTSEL = DIVEN | WS of SSIF`S::INDEX`.
    4. SSICTRL |= `ssictrl_tx_bit(S::INDEX)`.
    5. `start_path`. The DVU volume is applied after start (`apply_dvu_after_init`).
    6. Then `out.enable()`.
    - The returned `split()` gives `None` for the FFU ring. `Control` keeps the `ScuxOutput`, so dropping it clears TEN, then stops the path.

- [ ] **Step 1: Write the failing tests**
  - `the_dvu_path_routes_ch0_to_ssif0`: ch0 + `ScuxOutput<SSIF0>` with `Src::Bypass` and `Dvu::Volume`. Assert the same register writes the legacy `dvu_path::init` makes:
    - OPC0 = 1 (ToSsi);
    - SOSEL0 = 00;
    - FDTSEL0 = FUTSEL0 = DIVEN | 8;
    - SSICTRL bit 14;
    - SSICR TEN set last, after the start.

    To get the expected list, record the legacy path's write log in the same test (run legacy `dvu_path::init` under a fresh shadow first) and compare the SCUX register subset.
  - `ch0_to_ssif3_selects_sosel3_01` and `ch1_to_ssif0_selects_sosel0_01` (the SSIRSEL field values).
  - `the_route_table_matches_ssirsel`: `<Ch2 as RoutesTo<SSIF1>>::SOSEL_SHIFT == 4` and the other four entries.
  - Not a test: say in the doc that `Ch3` routes to no SSI here (SSIF2 is excluded) and `Ch2` to SSIF1 only. A wrong pair is a compile error.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l scux::`
  Expected: `into_ssi_path`/`RoutesTo` are not found.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh`.
  Expected: PASS.
- [ ] **Step 5: Commit** `rza1-hal: scux paths into an SSI, routes checked at compile time`

### Task 8: `deluge-bsp` on the new drivers

**Files:**
- Modify:
  - `crates/deluge-bsp/src/audio.rs`;
  - `crates/deluge-bsp/src/audio_block.rs`;
  - `crates/deluge-bsp/src/scux_dvu_path.rs`;
  - `crates/deluge-bsp/src/scux_src_path.rs`;
  - `crates/deluge-bsp/src/scux_usb_tx_path.rs`;
  - `crates/deluge-bsp/src/system.rs` (the `SSICR` const becomes `fn codec_ssi_config() -> ssi::Config`; the legacy `SCUX_FF*_DMA_CH` u8 constants go);
  - Not `board.rs`: `BlockDma::new` gives each channel's interrupt the DMAC priority from `BOARD_CONFIG` (`dmac.rs:71`). The BSP binds `DMAINTn => BlockInterruptHandler` for each ring's channel.

**Interfaces:**
- Consumes: Tasks 3, 5, 6 and 7.
- Produces (BSP public API; keep the existing `init*` signatures):
  - The rings are BSP statics: `#[repr(align(32))]` cached arrays, lent through their uncached mirror (as `sd.rs`'s bounce buffer is).
    - SSI TX 2048 words, 8 blocks;
    - SSI RX 4096 words, 16 blocks (128-frame blocks, `BLOCK_FRAMES`);
    - DVU FFD 4096 words, 16 blocks;
    - SRC/USB-TX FFD 2048 words / FFU 4096 words.
  - `pub fn with_tx<R>(f: impl FnOnce(&mut WritableRing<'static>) -> R) -> Option<R>` and `pub fn with_rx<R>(f: impl FnOnce(&mut ReadableRing<'static>) -> R) -> Option<R>`:
    - each in a `critical_section::Mutex<RefCell<Option<…>>>`;
    - `None` before init, and `None` when re-entered (`try_borrow_mut`), never a panic;
    - with SCUX, `with_tx` is the DVU FFD ring.
  - `pub fn rx_block_signal() -> Option<BlockSignal>`.
  - `scux_dvu_path` keeps `set_volume(audio_ch, vol)`, `set_volume_stereo(vol)`, `fade_to(vol, ramp)` and `update_engine_rate(hz)`, as safe functions over the stored `Control`. Its `tx_buf_*`/`tx_current_ptr`/`DVU_PATH_BUF_LEN` go; consumers use `audio::with_tx`.
  - `scux_src_path` / `scux_usb_tx_path`: `pub fn init(...)`, safe, takes the same `Peri`s plus `fin_hz, fout_hz`, and builds `into_ffu_path` on ch1 / ch2. Each provides `with_tx`/`with_rx`. The HAL's `scux::{src_path, usb_tx_path}` modules are no longer used (they're deleted in Task 9).
  - `audio_block` re-implemented on `with_tx`/`with_rx` windows with the same write-ahead constants and `ring_follow::plan`. `wait_block` awaits `rx_block_signal()`.

- [ ] **Step 1: Write the failing tests** (BSP host tests, where the rings are host-allocatable):
  - `with_tx_before_init_is_none`: `assert!(deluge_bsp::audio::with_tx(|_| ()).is_none());`
  - `with_tx_reentered_is_none`: inside `with_tx` on a test-installed ring, a nested `with_tx` returns `None`. Install through a `#[cfg(test)] fn install_test_rings()`.
  - `codec_ssi_config_is_the_bench_value`: `rza1_hal::ssi::ssicr(&system::codec_ssi_config()) == 0x002B_C020`. Expose `ssicr` as `#[doc(hidden)] pub` for this, or assert the fields.
- [ ] **Step 2: Run them to verify they fail**
  Run: `cargo test -p deluge-bsp --lib audio 2>&1 | tail`
  Expected: `with_tx` is not found.
- [ ] **Step 3: Implement** as in Interfaces, and migrate the consumers of the removed BSP items in the same commit, so the tree stays green.
  - Files:
    - `firmwares/demo-firmware/src/{main.rs, tasks/audio.rs, tasks/analysis.rs, tasks/encoder.rs}`;
    - `firmwares/controller-firmware/src/{main.rs, tasks/audio.rs, tasks/encoder.rs}`;
    - `hw-tests/uac-host-firmware/src/{main.rs, tasks/speaker.rs}`;
    - `crates/deluge-sdk/src/{audio.rs, plat/device.rs}`, where they touch them.
  - The hook bodies keep their arithmetic. Only the ring access changes:
    - `iso_out_to_ssi` becomes `audio::with_tx(|tx| { let len = tx.len(); let dma = tx.position(); let ring = tx.ring_mut(); … })`, with the same `WRITE_AHEAD`, re-anchor and conversion, and `HOOK_WRITE_PTR` becomes `static HOOK_WRITE_OFF: AtomicUsize`;
    - `iso_in_from_ssi` becomes `audio::with_rx`;
    - `analysis.rs` reads through `with_tx`;
    - uac-host `speaker.rs` uses `with_tx` with `ring_follow::plan`;
    - `encoder.rs` keeps calling `scux_dvu_path::set_volume_stereo`.
- [ ] **Step 4: Run the tests and verify**
  Run: `verify.sh`.
  Expected: PASS.
- [ ] **Step 5: Commit** `deluge-bsp: audio and SCUX paths on the owned SSI/SCUX drivers; USB-TX path moves here`

### Task 9: The HAL drops its audio statics

**Files:**
- Delete:
  - `rza1/rza1-hal/src/scux/{dvu_path,src_path,usb_tx_path}.rs`;
  - the legacy `ssi` free functions and statics (`init*`, `wait_rx_block`, `enable_tx`, `*_current_ptr`, `*_buf_*`, `*_dma_ch`, `TX_*`/`RX_*` consts, `TX_BUF`/`RX_BUF`/descriptor statics, `SSI_CH`);
  - `scux::{FFD_DMA, FFU_DMA}` and the eight descriptor statics.
- `scux`'s raw `unsafe fn`s become `pub(crate)`. Remove `configure_mix`/`set_mix_source_gain`: nothing uses them.
- Modify: `tools/check-layering.sh`.

**Interfaces:**
- Consumes: Task 8, which has already moved every consumer off these items.

- [ ] **Step 1: Write the failing check.** Add to `tools/check-layering.sh` a check that `rg -n 'static mut' rza1/rza1-hal/src/{ssi.rs,scux.rs,scux/}` finds nothing. Run it first: it fails on today's statics.
- [ ] **Step 2: Run it to verify it fails**
  Run: `tools/check-layering.sh`
  Expected: FAIL naming `ssi.rs`/`scux.rs` statics.
- [ ] **Step 3: Delete the legacy items.**
- [ ] **Step 4: Run the full verify**
  Run: `verify.sh`.
  Expected: every line PASS, including the new static check.
- [ ] **Step 5: Commit** `rza1-hal: SSI/SCUX legacy statics and path modules removed`

### Task 10: Docs, CHANGELOG, bench

**Files:**
- Modify:
  - the `ssi.rs` / `scux.rs` / `ring_buffer.rs` module docs (usage for each constructor; the window vs copy API; SSIF2 excluded; the route table);
  - `CHANGELOG.md` (an `[Unreleased]` entry: renamed and removed items, the DelugeFirmware mapping in Global Constraints, the new per-block DMA interrupts);
  - `crates/deluge-bsp/README.md` and `docs/advanced-guide.md` where they name the removed SCUX paths.

- [ ] **Step 1: Docs and CHANGELOG** as listed.
- [ ] **Step 2: Doc build**
  Run: `cd rza1 && cargo doc -p rza1-hal --features rza1l --no-deps 2>&1 | grep -c ^warning`
  Expected: ≤ 11 (phase 4's count).
- [ ] **Step 3: Full verify**
  Run: `verify.sh`.
  Expected: all PASS.
- [ ] **Step 4: Bench** (pi-top; start `JLinkRemoteServerCLExe -Port 19020` there if needed).
  - `demo-firmware --features rtt --release`:
    - boots with no fault or panic;
    - on the Pi, `speaker-test` to the Deluge's UAC2 device (the SCUX DVU path);
    - a 3-second `arecord` of exactly 793,844 bytes (the SSI RX → USB IN path).
  - Examples `audio_passthru` and `audio_passthru_irq` (built with `deluge/audio-irq`) boot, and their logs show blocks flowing.
  - Glitch-free playback, the controller-firmware volume encoder, and uac-host-firmware Digitakt playback through SCUX 48→44.1 kHz need someone at the bench (and the Digitakt cabled). List them as open if nobody is there.
  Expected: those results; none of the automated checks fail.
- [ ] **Step 5: Commit** `rza1-hal phase 5: docs, CHANGELOG`
