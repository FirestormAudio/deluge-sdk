# SD Card Detect, Interrupt-Recorded Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The SDHI's card-detect insert and remove edges are recorded once, by its own status interrupt, and never lost. This deletes the sticky-flag hand-off, the edge-swallowing in `sd::init`, and DelugeFirmware's level backstop.

**Architecture:**
- **The mask is the bug.** The SDHI already has a debounced card-detect function on P7_0 (SD_CD): `SD_OPTION`'s card-detect time, `INFO1_INS_CD`/`INFO1_REM_CD` latches, and the `SDHI1_STATUS` interrupt.
  - `INFO1_MASK_INIT` (`0x031D`) leaves card detect unmasked.
  - But every transfer then writes `0xFFFF`, masking it, so edges sit in the latch until a poll, and an unrelated interrupt clears them.
- **The fix:** every `SD_INFO1_MASK` write keeps `INFO1_DET_CD` unmasked, and the interrupt handler records each edge into one per-port event word that the consumer swaps out.
  - `sd::init` clears only insertion events (the card it just identified), so a removal during an init survives.
- **Consumers:** DelugeFirmware's `deluge_block_poll_card_event` stays the single consumer. The C++ app polls and remounts; nothing else may take events.

**Tech Stack:** Rust; `rza1-hal` (QEMU host tests through the MMIO seam, `scratchpad/ht.sh`), `deluge-bsp`, DelugeFirmware (`./dbt`), the pi-top bench.

**Spec:** none; this follows the user's request of 2026-10-30. The design is the "What would simplify things" proposal in the conversation. The waking of `card_service` from the interrupt was dropped: the app's poll must stay the only event consumer, or a swap is processed out of order.

## Global Constraints

- P7_0 stays muxed to SD_CD (`rza1l_functions.rs:61`): it has no IRQn alternative.
- Never let a card-detect edge be lost: an edge latched in hardware or recorded by the interrupt reaches exactly one `take_card_detect_events` call.
- Every transfer's own interrupt masks are unchanged apart from leaving `INFO1_DET_CD` clear: response (`INFO1_RESP`) and transfer end (`INFO1_DATA_TRNS`).
- `SD_INFO2_MASK` and `SDIO_INFO1_MASK` are untouched.
- The SD bench may write only to the spare 2 GB SDSC card (the one inserted on pi-top), never the SDHC card.
- **SDK:** `verify.sh` all PASS and `rza1/tools/test.sh` passes. **DelugeFirmware:** `./dbt build` and `cargo test --locked`. Pushes are asked separately.

## Review Focus

1. **A card that bounces while seated.**
   - Failure: with the interrupt live, a noisy SD_CD could storm.
   - Expected: `SD_OPTION`'s card-detect time debounces it; the interrupt count stays small.
   - Test: Task 3's bench step counts status interrupts over 30 s idle with the card seated. Expect at most 1.
2. **A card-detect edge during a transfer.**
   - Failure: the interrupt now fires mid-transfer and clears `INFO1` wholesale.
   - Expected: the transfer's RESP/DATA_TRNS bits still reach its waiter, and the edge reaches the event word.
   - Test: `an_edge_during_a_transfer_reaches_both` (Task 1).
3. **A card pulled while `sd::init` runs.**
   - Expected: the removal is reported afterwards, with no level backstop needed.
   - Test: `clearing_insertions_keeps_a_removal` (Task 1), and the bench step "pull during init" (Task 3, hands-on).
4. **Edges before the GIC is up or with interrupts masked** (boot, a fault path).
   - Expected: the hardware latch still counts; `take_card_detect_events` drains it too.
   - Test: `a_latched_edge_with_no_interrupt_is_still_taken` (Task 1).
5. **A swap between two polls.**
   - Expected: `(true, true)`, as today.
   - Test: `a_swap_reports_both` (Task 1), and the hands-on swap in Task 3.

---

### Task 1: rza1-hal — card detect unmasked; edges recorded once

**Files:**
- Modify: `rza1/rza1-hal/src/sdhi.rs`. The mask writes at lines 330, 705, 715, 780, 802, 824, 849, 897, 922, 981, 987, 1000, 1023, 1068, 1074, 1087 and 1108; `interrupt_handler` (420-450); `SdhiState` (197-215); `take_card_detect_events` (1195-1228); the tests module.

**Interfaces:**
- Produces:
  - `const INFO1_MASK_IDLE: u16 = !INFO1_DET_CD;` (`0xFFE7`): the mask between transfers.
  - `fn info1_mask(unmasked: u16) -> u16 { !(unmasked | INFO1_DET_CD) }`. Every transfer mask write becomes `write_value(info1_mask(INFO1_RESP))` or `info1_mask(INFO1_DATA_TRNS)`, and every `0xFFFF` restore becomes `INFO1_MASK_IDLE`. `INFO1_MASK_INIT` stays (it already leaves card detect clear).
  - In `SdhiState`, `cd_removed` and `cd_inserted` become one field, `cd_events: AtomicU8`, with the bits `CD_REMOVED = 1` and `CD_INSERTED = 2`. `interrupt_handler` ORs the edges it read into it (once).
  - `take_card_detect_events` swaps `cd_events` to 0, and also drains the hardware latch (`INFO1_DET_CD` only, write-0-to-clear as today) for the interrupts-masked case. Returns `(removed, inserted)`, unchanged.
  - New: `pub fn Sdhi::clear_insertion_events(&self)` clears `CD_INSERTED` (and an `INFO1_INS_CD` still latched), leaving a removal. Doc: for an init that has just identified the card those insertions describe.
- [ ] **Step 1: Write the failing tests** (in the `sdhi` tests module; use `sdhi1()`, `poll_once`, `interrupt_saw`, `serial()`; `SD_INFO1_MASK` is `crate::pac::SDHI1.sd_info1_mask().addr()`):
  - `card_detect_stays_unmasked_through_a_transfer`: run `a_completed_read_never_aborts`'s sequence, then assert that every logged write to `SD_INFO1_MASK` has `value & INFO1_DET_CD == 0`, and the last one equals `0xFFE7`.
  - `a_card_detect_edge_is_recorded_once_by_the_interrupt`: `interrupt_saw(INFO1_INS_CD)`, then `take_card_detect_events() == (false, true)`, then `== (false, false)`.
  - `an_edge_during_a_transfer_reaches_both`: start the read, then `interrupt_saw(INFO1_RESP | INFO1_REM_CD)`. The read proceeds to its data phase (pending, not failed), and after it completes, `take == (true, false)`.
  - `a_latched_edge_with_no_interrupt_is_still_taken`: `test::poke16(SD_INFO1, INFO1_REM_CD)` with no handler run, then `take == (true, false)`, and the latch bit was written 0.
  - `clearing_insertions_keeps_a_removal`: `interrupt_saw(INFO1_INS_CD | INFO1_REM_CD)`, then `clear_insertion_events()`, then `take == (true, false)`.
  - `a_swap_reports_both`: `interrupt_saw(INFO1_REM_CD)`, then `interrupt_saw(INFO1_INS_CD)`, then `take == (true, true)`.
- [ ] **Step 2: Run them to verify they fail**
  Run: `scratchpad/ht.sh rza1l sdhi`
  Expected:
  - `card_detect_stays_unmasked_through_a_transfer` FAILS on a `0xFFFF` write;
  - `clearing_insertions_keeps_a_removal` fails to compile (no method);
  - the others may pass on today's sticky flags. Ledger which.
- [ ] **Step 3: Implement** as in Interfaces. Remove the sticky-flag docs, and the "card-detect interrupts sit masked almost all the time" paragraph of `take_card_detect_events`'s doc.
- [ ] **Step 4: Verify**
  Run: `scratchpad/ht.sh rza1l sdhi`, then `rza1/tools/test.sh`.
  Expected: all pass. The existing abort/reset tests are unchanged, apart from mask values they may assert; if one does, update it to `0xFFE7` and ledger it.
- [ ] **Step 5: Commit** `rza1-hal: SD card detect stays unmasked; the status interrupt records each edge once`

### Task 2: deluge-bsp — init keeps a removal; CHANGELOG

**Files:**
- Modify: `crates/deluge-bsp/src/sd.rs` (`init`'s success arm around line 448, and the module doc on edges); `CHANGELOG.md`.

**Interfaces:**
- Consumes: Task 1's `Sdhi::clear_insertion_events`.
- Produces: on success `sd::init` calls `controller().clear_insertion_events()` instead of `take_card_detect_events()`. Its comment says a removal during the init stays recorded for the next poll. `deluge_bsp::sd::take_card_detect_events` is unchanged.
- [ ] **Step 1: Write the failing test**
  - If `deluge-bsp`'s host build can drive `sd::init` through the seam, add `an_init_keeps_a_removal_seen_during_it`: an edge recorded before init succeeds, then `take_card_detect_events() == (true, false)`.
  - Otherwise ledger that this is covered by Task 1's `clearing_insertions_keeps_a_removal` plus the Task 3 bench, and skip to Step 3.
- [ ] **Step 2: Run it to verify it fails**
  Run: `scratchpad/bt.sh sd`
  Expected: FAIL (the removal is swallowed).
- [ ] **Step 3: Implement** as in Interfaces. Add a CHANGELOG entry under Unreleased: card detect is interrupt-recorded; `Sdhi::clear_insertion_events` is new; `sd::init` keeps a removal.
- [ ] **Step 4: Verify**
  Run: `verify.sh`.
  Expected: all PASS.
- [ ] **Step 5: Commit** `deluge-bsp: sd::init keeps a card removal seen while it ran`

### Task 3: DelugeFirmware — drop the level backstop; bench

**Repo:** DelugeFirmware, branch `sd-card-detect-irq` from local `next`, building through `local-sdk.toml`. The SDK pin is bumped only after the SDK is pushed, which is asked separately.

**Files:**
- Modify: `src/bsp/rust/src/storage/sd.rs` (`deluge_block_poll_card_event`). Delete the `// Level backstop for a lost removal edge` block (the `sd::is_ready() && !sd::is_inserted()` branch) and its comment. Keep the `LAST_INIT_OK` clear on the removal edge.
- [ ] **Step 1: Delete the backstop.** No host test can reach this device-only function: the removal path is covered by Task 1's tests and the bench below. Ledger that.
- [ ] **Step 2: Build and host-test**
  Run: `./dbt build`; `cd src/bsp/rust && cargo test --config local-sdk.toml`; restore `Cargo.lock`.
  Expected: links; all suites pass.
- [ ] **Step 3: Bench, automated** (pi-top, RAM run of the RelWithDebInfo image).
  - Boot to the main loop with the SD mounted.
  - Status-interrupt count:
    - Add a temporary `log::info!` counter in the HAL's status handler, or read `ICDABR`/the handler count over J-Link.
    - Over 30 s idle with the card seated, it is at most 1.
    - Remove the counter before committing.
  - Then run `hw-tests/sd-bench` on the spare card, reads and writes.
  - Expected: throughput within 5% of the Phase 3 figure (5.9 MB/s on the spare card at 16.7 MHz), and no errors.
- [ ] **Step 4: Bench, hands-on (the user).** With the image running:
  - pull the card: RTT shows `sd: card removed`, and the app shows its ejected state;
  - reinsert it: `card inserted; re-init requested`, then remount;
  - swap cards quickly: both events, then a remount;
  - pull during the ~1.3 s re-init: a removal is reported afterwards.
  - List these as open if the user doesn't run them.
- [ ] **Step 5: Commit** `refactor(sd): the SDK records card-detect edges by interrupt; the level backstop goes`
