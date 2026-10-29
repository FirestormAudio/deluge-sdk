# rza1-hal Phase 6: USB Iso Streams Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Owned USB isochronous streams replace the RUSB1 device driver's global ISR hooks. Each stream is bound, per port and pipe, to a consumer the board implements, and it runs in the BRDY interrupt. The host driver's iso IN rings become caller-lent.

**Architecture:**
- **Streams.** A new `usb::iso` module defines two consumer traits, `IsoOutConsumer` and `IsoInProducer`. `Rusb1EndpointOut::into_iso_stream(consumer)` and `Rusb1EndpointIn::into_iso_stream(producer)` bind a `&'static mut dyn` consumer into a per-(port, pipe) table. The BRDY ISR calls it directly: FIFO to consumer for OUT, producer to FIFO for IN. `pipe_iso_in_activate` primes both planes for any bound IN stream. Dropping a stream unbinds it in a critical section.
- **Migration.** The fn-pointer hooks survive for one task as adapters over the table, then go.
- **Host.** The 2 × 8 × 1024 B host iso IN rings become a caller-lent `IsoInBuffers`.

**Tech Stack:** Rust `no_std`, embassy-usb-driver 0.2, embassy-hal-internal `OnDrop`, the `rza1_pac::mmio::test` shadow seam.

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md`. Relevant parts:
- decisions 3 and 8;
- "My calls": `IsoStream`'s consumers, USB host iso, SD DMA's `OnDrop` line;
- §5 item 6.

## Global Constraints

- **Decision 3, verbatim:** "The RUSB1 driver hands out an RZ/A1-specific `IsoStream<'d>` … that owns the pipe and a caller-lent ring. The USB ISR fills or drains that ring directly, so the 'prime both BRDY planes' timing stays in the HAL. It is an extension next to the standard `embassy_usb_driver::Driver` impl, not a replacement. The hooks are deleted."
- **Decision 8:**
  - "On BRDY, the ISR moves the packet straight from the FIFO into the audio ring … with no task hop. Sample conversion lives where the ISR can reach it: a consumer trait the board crate implements, or a HAL-provided converter."
  - This plan takes the consumer trait. The consumer reaches phase 5's ring through `deluge_bsp::audio::{with_tx, with_rx}`.
- **"`IsoStream`'s consumers are `deluge-bsp` and the firmwares, not `usb-classes`."** `usb-classes` stays free of any HAL dependency (layering rule 3, `tools/check-layering.sh`).
- **"USB host iso is in scope.** `usb/host.rs`'s HAL-owned static iso IN rings (2 pipes × 8 × 1024 B) become caller-lent, using the same buffer type and stream shape as device iso."
  - The host rings stay CPU-filled from the FIFO (no DMA), so they need no uncached memory.
  - "Same shape": lent with `&'static mut`, as the device consumers are.
  - The class reads them through the unchanged `UsbPipe::request_in`, because `usb-classes` can't name the HAL.
- **The consumer is lent as `&'static mut dyn …`.** Phase 4 did the same for the UART's RXI buffer: an ISR-reached borrow must not be forgettable. Decision 3's `IsoStream<'d>` lifetime is the pipe borrow; the stream types carry no other borrow.
- **Spec "my call":** the host's local `OnDrop` (`usb/host.rs:197`) is replaced by `embassy_hal_internal::drop::OnDrop`.
- **Behaviour to keep exactly:**
  - OUT, no read pending: drain `DTLN` bytes, BCLR, then the consumer. A pipe with nothing bound leaves the packet for `read()`.
  - OUT after a completed `read()` on a bound pipe: the consumer gets the read's bytes too.
  - IN: the producer fills up to MPS, then BVAL if short (a zero-length packet included). Never BCLR.
  - IN enable (`endpoint_set_enabled(true)`) on a bound pipe: `pipe_iso_in_activate` primes both planes (TRM §28.4 BRDY condition 4).
- **Scratch buffers grow from 512 to 1024 B,** the largest high-speed iso packet. `AudioClass<6>`'s 882 B capture packet must not be clamped.
- **Alt settings sharing an endpoint address** (user decision 2026-10-30: fix in this phase, Task 2).
  - Within one SET_INTERFACE, embassy-usb 0.6 calls `endpoint_set_enabled` once per alt's endpoint, in descriptor order.
  - UAC2's alt 1 and alt 2 share their endpoint addresses, so alt 2's disable lands after alt 1's enable and wins. Selecting alt 1 (16-bit) leaves the stream disabled.
  - The driver now ignores a disable for a pipe it has enabled since the port's latest SETUP packet.
  - DelugeFirmware's `set_alternate_setting` re-enable becomes unnecessary. It is removed in phase 9.
- **Lockstep:** BSP, firmwares and hw-tests migrate here. DelugeFirmware waits for phase 9: it uses `register_iso_*` and `pipe_reset`/`pipe_enable`/`pipe_brdy_enable`/`pipe_iso_in_activate`, and its `set_alternate_setting` workaround needs public re-enable calls. This phase keeps those three pipe functions public, `#[doc(hidden)]`, until phase 9 drops the workaround (Task 2 makes it unnecessary).
- **Done when:**
  - `verify.sh` (`…/scratchpad/verify.sh`) prints PASS for every line;
  - HAL tests pass: `ht.sh rza1l`;
  - the Task 5 bench check passes.

## Review Focus

1. **A stream dropped while its pipe's BRDY is pending or in flight.** The ISR must never call a consumer whose `drop` has returned. Expected: unbinding in a critical section, after masking the pipe's BRDYENB. Test `a_dropped_out_stream_is_never_called_again` (Task 1).
2. **An OUT packet longer than the 1024 B scratch** (a misbehaving host). Expected: the packet is drained and dropped, `bad_packets` is bumped, and nothing writes past the scratch. Test `an_oversized_out_packet_is_dropped_not_overflowed` (Task 1).
3. **A producer returning more than it was given, or than MPS.** Expected: clamped to `min(returned, mps)`. Test `a_producer_overlong_return_is_clamped` (Task 1).
4. **Opening a host iso IN pipe without lent buffers.** Expected: the open fails with an error and doesn't panic. Test `an_iso_in_pipe_without_buffers_fails_to_open` (Task 4).
5. **A bus reset, or an alt re-select, while an IN stream is bound.** Expected: re-enabling primes both planes again, as the hook did. Test `re_enabling_a_bound_in_pipe_primes_both_planes` (Task 1).

---

### Task 1: `usb::iso` streams in the device driver

**Files:**
- Create: `rza1/rza1-hal/src/usb/iso.rs` (`pub mod iso;` in `usb/mod.rs`)
- Modify:
  - `rza1/rza1-hal/src/usb/pipe.rs` (the hook statics become adapters over the iso table);
  - `rza1/rza1-hal/src/usb/driver.rs` (`dcd_int_handler` BRDY dispatch, `endpoint_set_enabled`, the endpoint methods).

**Interfaces:**
- Produces:
  - `pub trait IsoOutConsumer: Send { fn on_packet(&mut self, packet: &[u8]); }`. It runs in the USB interrupt with interrupts masked: integer work, short.
  - `pub trait IsoInProducer: Send { fn fill_packet(&mut self, buf: &mut [u8]) -> usize; }`. `buf.len()` is the endpoint's MPS (≤ 1024). It returns the bytes to send; 0 sends a zero-length packet.
  - `pub struct IsoOutStream { port: u8, pipe: u8 }` and `pub struct IsoInStream { port: u8, pipe: u8, mps: u16 }`. Each has `pub fn pipe(&self) -> u8` and `pub fn bad_packets(&self) -> u32` (OUT only).
  - `impl Rusb1EndpointOut { pub fn into_iso_stream(self, consumer: &'static mut dyn IsoOutConsumer) -> IsoOutStream }` and `impl Rusb1EndpointIn { pub fn into_iso_stream(self, producer: &'static mut dyn IsoInProducer) -> IsoInStream }`. Both panic unless `info.ep_type == EndpointType::Isochronous`, and they bind under a critical section.
  - Drop masks the pipe's BRDYENB, then unbinds, both inside one critical section.
  - Crate-internal: `static BINDINGS: [[critical_section::Mutex<Cell<Binding>>; PIPE_COUNT]; 2]` with `enum Binding { None, Out(*mut dyn IsoOutConsumer), In(*mut dyn IsoInProducer, u16) }`. `Send` is asserted, with a SAFETY note: the pointer is only dereferenced in the port's ISR or under a critical section.
  - Crate-internal: `pub(crate) fn in_bound(port: u8, pipe: usize) -> bool` and `pub(crate) unsafe fn out_brdy(u: Usb, port: u8, n: usize) -> bool`. The latter returns whether the packet was consumed: drain into `SCRATCH: [u8; 1024]`, BCLR, consumer, as `pipe_xfer_out_brdy`'s hook branch does now.
  - Crate-internal: `pub(crate) unsafe fn in_brdy(u: Usb, port: u8, n: usize)` (as `pipe_xfer_in_brdy`, with mps up to 1024).
  - `static BAD_PACKETS: [[AtomicU32; PIPE_COUNT]; 2]`.
- Rewiring:
  - `pipe_xfer_out_brdy`'s no-read branch calls `iso::out_brdy`.
  - Its post-read call passes the read buffer to the bound consumer.
  - `dcd_int_handler` routes bound IN pipes to `iso::in_brdy`.
  - `endpoint_set_enabled` calls `pipe_iso_in_activate` when `iso::in_bound(port, p)`.
  - `pipe_iso_in_activate` uses `iso::in_brdy`.
- Adapter: `register_iso_out_hook` / `register_iso_in_hook` keep their signatures until Task 2. Each binds a `static mut` fn-pointer adapter struct that implements the trait, for port 0.

- [ ] **Step 1: Write the failing tests** in `iso.rs`. Mirror the host iso tests' shadow setup for FIFO registers (`host.rs` `iso_in_pipe` / `packet` helpers): configure pipe 1 as iso, poke D0FIFOCTR DTLN/FRDY, and have D0FIFO reads return a value.
  - `an_out_stream_hands_each_packet_to_its_consumer`: a consumer records `(len, first byte)`. After `iso::out_brdy` with DTLN = 288, it saw `(288, …)`, and D0FIFOCTR BCLR was written.
  - `an_oversized_out_packet_is_dropped_not_overflowed`: DTLN = 1100. The consumer is not called, `bad_packets() == 1`, BCLR is written, and no more than 1024 bytes are read.
  - `an_in_stream_fills_from_its_producer_and_commits_short_packets`: mps 294; the producer returns 288. 288 bytes are written to D0FIFO and BVAL is set.
  - `a_full_size_capture_packet_is_not_clamped`: mps 882; the producer sees `buf.len() == 882`.
  - `a_producer_overlong_return_is_clamped`: it returns 5000 with mps 294; 294 bytes are written and no BVAL (full packet).
  - `a_dropped_out_stream_is_never_called_again`: drop, then `out_brdy`, and the consumer isn't called. BRDYENB bit 1 was written clear before the binding went.
  - `re_enabling_a_bound_in_pipe_primes_both_planes`: `Bus::endpoint_set_enabled(addr, true)` on a bound IN pipe calls the producer twice (two fills) before PID = BUF.
  - `binding_a_bulk_endpoint_panics`: `#[should_panic]`.
  - `streams_are_per_port`: binding port 0 pipe 1 leaves `in_bound(1, 1) == false`.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l usb::iso`
  Expected: the module is not found.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh`.
  Expected: PASS, with the firmwares still building on the adapters.
- [ ] **Step 5: Commit** `rza1-hal: usb iso streams bound to an interrupt-context consumer`

### Task 2: Shared-address alt endpoints stay enabled

**Files:**
- Modify: `rza1/rza1-hal/src/usb/driver.rs` (the SETUP path in `dcd_int_handler`, `Bus::endpoint_set_enabled`).

**Interfaces:**
- Consumes: nothing from Task 1. It touches the same `endpoint_set_enabled`, so take Task 1's version.
- Produces (crate-internal):
  - `static SETUP_GEN: [AtomicU32; 2]`, bumped by the ISR on every SETUP packet (where `SETUP_PKT` is stored).
  - `static ENABLED_GEN: [[AtomicU32; PIPE_COUNT]; 2]`, holding the generation in which each pipe was last enabled through `endpoint_set_enabled(true)`. 0 means never.
  - `endpoint_set_enabled(addr, false)` does nothing for a pipe whose `ENABLED_GEN` equals the port's current `SETUP_GEN`, because an alt selected by this request uses it.
  - Every other disable is unchanged: bus reset, unconfigure, an alt whose endpoints differ.
  - Bus reset clears `ENABLED_GEN` for the port.
- [ ] **Step 1: Write the failing tests** in driver.rs's test module, using the existing device-layout helpers. Allocate the UAC2 pattern: two alts on one OUT address and one IN address, so they map to the same pipes.
  - `selecting_alt_1_keeps_its_shared_endpoint_enabled`: bump `SETUP_GEN` (as a SETUP packet would), then `endpoint_set_enabled(addr, true)` followed by `endpoint_set_enabled(addr, false)`. PIPECTR PID ends BUF and BRDYENB stays set.
  - `a_later_request_can_disable_it`: after that, bump `SETUP_GEN` (a new SET_INTERFACE to alt 0), then `endpoint_set_enabled(addr, false)`. PID ends NAK.
  - `a_bus_reset_forgets_the_enables`: after a bus reset, a disable in the same generation disables.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l usb::driver`
  Expected: `selecting_alt_1_keeps_its_shared_endpoint_enabled` fails (PID ends NAK).
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh`.
  Expected: PASS.
- [ ] **Step 5: Commit** `rza1-hal: an alt setting's shared endpoint stays enabled through SET_INTERFACE`

### Task 3: Firmwares on iso streams; the hooks go

**Files:**
- Modify:
  - `firmwares/demo-firmware/src/tasks/audio.rs`;
  - `firmwares/controller-firmware/src/tasks/audio.rs`;
  - their `main.rs` where the tasks are spawned, if signatures change;
  - `rza1/rza1-hal/src/usb/pipe.rs`: delete `register_iso_out_hook`, `register_iso_in_hook`, `is_iso_in_hook_pipe`, the hook statics and the adapters;
  - `pipe_reset`, `pipe_enable`, `pipe_brdy_enable` and `pipe_iso_in_activate` stay `pub` but `#[doc(hidden)]`, documented "for DelugeFirmware's alt-setting workaround until phase 9".

**Interfaces:**
- Consumes: Task 1.
- Produces, in each firmware:
  - `struct SpeakerSink { write_off: usize, lfsr: u32 }`, implementing `IsoOutConsumer`. Its body is today's `iso_out_to_ssi` arithmetic, moved verbatim: `HOOK_WRITE_OFF`/`HOOK_LFSR` become fields, and the re-anchor stays keyed on `USB_AUDIO_STREAMING`.
  - demo-firmware only: `struct MicSource { read_off: usize }`, implementing `IsoInProducer` (`iso_in_from_ssi` moved verbatim).
  - The `'static` consumers come from `static mut` storage taken once (`unsafe { &mut *addr_of_mut!(SPEAKER) }`, with a SAFETY note: taken once, in the task that binds it). `uac2_task` binds and keeps the `IsoOutStream` alive for its loop.
  - The supervisors are unchanged (they watch `PACKET_SEQ`/`MIC_PACKET_SEQ`).
  - controller-firmware's task-side mic (`ep_in.write`) is unchanged: it doesn't use the hook.
- [ ] **Step 1: Write the failing check.** Add to `tools/check-layering.sh` that `rg -n 'register_iso_(in|out)_hook' rza1 crates firmwares hw-tests examples` finds nothing. Run it: it fails (both firmwares and pipe.rs).
- [ ] **Step 2: Run it to verify it fails**
  Run: `tools/check-layering.sh`
  Expected: FAIL naming the hook users.
- [ ] **Step 3: Migrate the firmwares, then delete the hooks** as in Interfaces.
- [ ] **Step 4: Run the full verify**
  Run: `verify.sh`.
  Expected: every line PASS.
- [ ] **Step 5: Commit** `firmwares: USB audio through iso streams; the iso hooks are gone`

### Task 4: Caller-lent host iso IN rings; embassy `OnDrop`

**Files:**
- Modify:
  - `rza1/rza1-hal/src/usb/host.rs`;
  - `crates/deluge-bsp/src/usb/host.rs` (lends the buffers in `usb_host_supervisor` or its caller);
  - `hw-tests/uac-host-firmware/src/main.rs` and `hw-tests/overbridge-host-firmware/src/main.rs`, if the lending sits at `init_host_mode`'s call.

**Interfaces:**
- Produces:
  - `pub struct IsoInBuffers { rings: [IsoInRing; 2] }` with `pub const fn new() -> Self`: 2 pipes × 8 × 1024 B. `IsoInRing` stays private.
  - `impl Rusb1HostDriver<'d> { pub fn lend_iso_in_buffers(&self, bufs: &'static mut IsoInBuffers) }`, stored per port in `static ISO_IN: [critical_section::Mutex<Cell<Option<NonNull<IsoInBuffers>>>>; 2]`.
  - `iso_in_ring(port, pipe, f)` returns `Option<R>`; `None` means no buffers lent.
  - `open_endpoint` for an iso IN pipe without buffers returns `Err(PipeError::…)`, using the error variant the allocator already maps to `HostError` for "no resource". Pick it from `alloc_pipe`'s existing error mapping and ledger the choice.
  - `iso_in_brdy` with no buffers BCLRs and drops.
  - The local `OnDrop` struct is replaced by `embassy_hal_internal::drop::OnDrop` at its four uses (`defuse()` keeps its meaning).
  - The BSP lends `static mut HOST_ISO_IN: IsoInBuffers` once, in the function that receives the `Rusb1HostDriver`.
- [ ] **Step 1: Write the failing tests** in `host.rs`'s test module:
  - `an_iso_in_pipe_without_buffers_fails_to_open`: a fresh driver, no lend, then opening an iso IN endpoint returns `Err`.
  - `lent_buffers_carry_isochronous_in_packets`: the existing `isochronous_in_packets_are_kept_until_read` body, after `lend_iso_in_buffers(Box::leak(Box::new(IsoInBuffers::new())))`.
  - Migrate the other iso IN tests (`a_spurious_…`, `a_detach_ends_…`, `a_digitakt_leaves_…`) to lend first.
- [ ] **Step 2: Run them to verify they fail**
  Run: `ht.sh rza1l usb::host`
  Expected: `IsoInBuffers`/`lend_iso_in_buffers` are not found.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the tests and verify**
  Run: `ht.sh rza1l`, then `verify.sh`.
  Expected: PASS.
- [ ] **Step 5: Commit** `rza1-hal: host iso IN rings are caller-lent; embassy OnDrop`

### Task 5: Docs, CHANGELOG, bench

**Files:**
- Modify:
  - the `usb/iso.rs` module doc (a usage example; ISR context; the one-consumer-per-pipe and `'static` rationale);
  - the `usb/mod.rs` module doc;
  - `rza1/rza1-hal/README.md` (the `usb` row);
  - `CHANGELOG.md` (an `[Unreleased]` phase 6 entry: the removed hooks, the new streams, `lend_iso_in_buffers`, DelugeFirmware's migration notes, the alt-1 known issue).
- [ ] **Step 1: Docs and CHANGELOG** as listed.
- [ ] **Step 2: Doc build**
  Run: `cd rza1 && cargo doc -p rza1-hal --features rza1l --no-deps 2>&1 | grep -c ^warning`
  Expected: ≤ 11.
- [ ] **Step 3: Full verify**
  Run: `verify.sh`.
  Expected: all PASS.
- [ ] **Step 4: Bench**
  - demo-firmware alt 2 (24-bit): `speaker-test -D plughw:Deluge` exits 0 at real time, and a 3-second `arecord -D hw:Deluge -f S24_3LE -c 2 -r 44100` gives exactly 793,844 bytes with non-zero samples.
  - demo-firmware alt 1 (16-bit), the Task 2 fix: `speaker-test -D hw:Deluge -F S16_LE -c 2 -r 44100` exits 0 at real time, and `arecord -D hw:Deluge -f S16_LE -c 2 -r 44100 -d 3` gives exactly 529,244 bytes (3 × 44100 × 4 + 44) with non-zero samples. Before the fix these stall.
  - controller-firmware boots and `speaker-test` works.
  - Digitakt host streaming (uac-host-firmware) and the Overbridge soak need the Digitakt cabled to the Deluge's USB port. Ask the user then, or list them as open.
  Expected: the device checks pass.
- [ ] **Step 5: Commit** `rza1-hal phase 6: docs, CHANGELOG`
