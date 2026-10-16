# Overbridge sub-project 2: USB host receive DMA Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The RZ/A1L USB host moves an interrupt-on-bulk IN pipe's data with the DMAC, not
byte-by-byte CPU FIFO reads. That is the Overbridge receive stream: 368 bytes × 6857 blocks/s
for a Digitakt, 1012 for a Digitakt II.

**Architecture (spec deviations, ruled below):**
- **One receive pipe, on D1FIFO.** A host driver given a DMA channel
  (`Rusb1HostDriver::enable_rx_dma`) binds the first interrupt-on-bulk IN pipe it opens to
  D1FIFO. Host mode never used D1FIFO, so the existing CPU path on D0FIFO is untouched.
- **4-byte cycle-steal DMA** (D1FBCFG.DFACC = 00, MBW 32-bit). Every block length is a multiple
  of 4, so no partial-unit behaviour arises.
- **Per transfer, under the existing pipe trait.** `xfer_in` arms the pipe (PID=BUF) as
  today. The BRDY handler reads nothing:
  1. it records DTLN as the length;
  2. it sets PID=NAK (the I1 rule);
  3. it completes the transfer.

  Back in the reading task, `xfer_in` then:
  1. DMAs exactly DTLN bytes from D1FIFO into a 1 KB uncached bounce buffer;
  2. awaits the DMAC's end;
  3. copies the bytes to the caller.

  DCLRM=1 clears the FIFO buffer once the DMA has read it. Classes (Overbridge) need no change.

**Spec:** `docs/superpowers/specs/2026-10-15-overbridge-design.md` §4.3–4.5.

## Rulings (spec deviations)

1. **Receive only; transmit stays on the CPU.**
   - **Why:** receive is ~80% of the bytes (368 vs 88 per Digitakt block; 1012 vs 256 for a
     Digitakt II). DMA transmit would need the CPU traffic moved from D0FIFO to CFIFO, a riskier
     refactor of working code.
   - **Cost if wrong:** transmit's CPU cost remains (about 0.6M byte writes/s for a Digitakt).
2. **D1FIFO for receive, not D0FIFO.**
   - **Why:** it leaves every existing CPU transfer exactly as it is.
3. **Cycle-steal 4-byte access, not 32-byte continuous.**
   - **Why:** it removes the partial-unit question the spec wanted settled on hardware first.
   - **Cost if wrong:** 4× the DMA requests (~92k/s for a Digitakt), which the spec already
     judged acceptable.
4. **Per-transfer DMA under `UsbPipe`, not a continuously armed `StreamIn` ring.**
   - **Why:** it fits the per-block NAK hold (review I1), needs no class or trait changes, and
     keeps the CPU-path fallback automatic (no DMA channel → today's behaviour).
   - **Cost:** two interrupts per block (BRDY, then DMA end) instead of one.
5. **BFRE stays 0, and the DMA count is DTLN.**
   - **Why:** BFRE=1 needs the transaction counter for full-size packets (no BRDY otherwise) and
     makes buffered-packet handling subtle.
   - **Cost:** the CPU starts the DMA after BRDY, not before.

## Global Constraints

- **DMARS:** USB0 D1FIFO = `0x87`, USB1 D1FIFO = `0x8F` (Renesas `r_usb_dma.c`; MID
  0b100001, RID 11).
- **CHCFG:** SAD (fixed source), SDS=DDS=32-bit, AM bus-cycle (`0x200`), LVL, HIEN, SEL =
  ch & 7, DEM=0 (end interrupt on), REQD=0.
- **D1FIFOSEL:** CURPIPE=pipe, MBW=32-bit, DCLRM=1. Then DREQE=1 once CURPIPE reads back.
  D1FBCFG = 0.
- **No DMA channel, or D1FIFO already bound:** the pipe uses the CPU path, exactly as before.

## Review Focus

1. **A packet length not a multiple of 4** (device glitch). DMA can't move it; the transfer
   must return 0 bytes (a framing error upstream) and clear the FIFO. Pinned by
   `a_misaligned_length_is_dropped_not_dmaed`.
2. **Zero-length packet:** BCLR, return 0, no DMA. Pinned by `a_zero_length_packet_needs_no_dma`.
3. **Closing the DMA pipe** must release D1FIFO, so another pipe can claim it later. Pinned by
   `closing_the_dma_pipe_releases_d1fifo`.
4. **A DMA that never ends** (hardware fault) must not hang the stream task forever. The drain
   uses a timeout and returns `PipeError::Timeout`. Pinned by `a_dma_that_never_ends_times_out`.
5. **CPU pipes** (MIDI bulk, a second interrupt-on-bulk pipe) must be unaffected. Pinned by
   `only_one_pipe_takes_the_dma` plus the existing suite.

---

### Task 1: HAL receive DMA

**Files:**
- Modify: `rza1/rza1l-hal/src/usb/regs.rs` (`FIFOSEL_DCLRM`)
- Modify: `rza1/rza1l-hal/src/usb/host.rs`
- Modify: `rza1/rza1l-hal/src/dmac.rs` (pub(crate) CHCFG constant `CHCFG_AM_BUS_CYCLE`)

**Interfaces:**
- Produces: `impl Rusb1HostDriver { pub fn enable_rx_dma(&self, dma: crate::dmac::Dma<'static>) }`.

- [ ] **Step 1: Write the failing tests** (in `host.rs` `mod tests`)

```rust
    fn dma_host() -> Rusb1HostDriver<'static> {
        use crate::mmio::test;
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        reset_rx_dma(0);
        test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_HS);
        h.enable_rx_dma(unsafe { crate::dmac::Dma::steal::<crate::peripherals::DMA_CH1>() });
        h
    }

    #[test]
    fn enabling_rx_dma_routes_usb0_d1fifo_to_the_channel() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let _h = dma_host();
        let chcfg = crate::pac::DMAC.ch(1).chcfg().read();
        let want = crate::dmac::CHCFG_SAD | crate::dmac::CHCFG_SDS_32BIT | crate::dmac::CHCFG_DDS_32BIT
            | crate::dmac::CHCFG_AM_BUS_CYCLE | crate::dmac::CHCFG_LVL | crate::dmac::CHCFG_HIEN | 1;
        assert_eq!(chcfg, want);
        assert_eq!(crate::pac::DMAC.dmars(0).read() >> 16, 0x87, "channel 1 is DMARS0's odd half");
        assert_eq!(crate::pac::DMAC.ch(1).n0sa().read(), usb(0).d1fifo().addr() as u32);
    }

    #[test]
    fn an_interrupt_on_bulk_in_pipe_takes_d1fifo_dma() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        let pipe = h.ep_to_pipe(1, 0x83).unwrap() as u16;
        let u = usb(0);
        assert_eq!(
            u.d1fifosel().read(),
            pipe | (MBW_32 << FIFOSEL_MBW_SHIFT) | FIFOSEL_DCLRM | FIFOSEL_DREQE
        );
        assert_eq!(u.d1fbcfg().read(), 0, "cycle steal, no TENDE");
    }

    #[test]
    fn only_one_pipe_takes_the_dma() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        h.edpt_open(2, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        h.edpt_open(1, 0x81, EndpointType::Bulk, 512, 0).unwrap();
        let first = h.ep_to_pipe(1, 0x83).unwrap() as u16;
        assert_eq!(usb(0).d1fifosel().read() & FIFOSEL_CURPIPE_MASK, first);
    }

    #[test]
    fn closing_the_dma_pipe_releases_d1fifo() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        h.edpt_close(1, 0x83);
        assert_eq!(usb(0).d1fifosel().read(), 0);
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        assert_ne!(usb(0).d1fifosel().read() & FIFOSEL_DREQE, 0, "the next pipe can claim it");
    }

    /// BRDY on the DMA pipe reads nothing: it records DTLN, holds the pipe at NAK and completes.
    #[test]
    fn brdy_on_the_dma_pipe_records_the_length_without_reading() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        let pipe = h.ep_to_pipe(1, 0x83).unwrap();
        let u = usb(0);
        let mut buf = [0u8; 368];
        pipectr(u, pipe).write_value(PIPECTR_PID_BUF);
        critical_section::with(|cs| unsafe {
            let x = &mut *HCD_XFER[pipe].borrow(cs).get();
            x.buf = buf.as_mut_ptr();
            x.length = 368;
            x.remaining = 368;
        });
        test::poke16(u.d1fifoctr().addr(), FIFOCTR_FRDY | 368);
        test::reset_log();
        unsafe { pipe_brdy_in(u, pipe) };
        assert!(
            !test::log().iter().any(|a| a.addr == u.d1fifo().addr() || a.addr == u.d0fifo().addr()),
            "the handler must not read the FIFO"
        );
        let remaining = critical_section::with(|cs| unsafe { (*HCD_XFER[pipe].borrow(cs).get()).remaining });
        assert_eq!(remaining, 0);
        assert_ne!(HCD_PIPE_DONE.load(Ordering::Acquire) & (1 << pipe), 0);
        assert_eq!(pipectr(u, pipe).read() & PIPECTR_PID_MASK, PIPECTR_PID_NAK);
    }

    /// The drain DMAs DTLN bytes into the bounce buffer, then copies them out.
    #[test]
    fn the_drain_copies_what_the_dma_moved() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        // The DMA "ran": the bounce buffer holds the block, and CHSTAT.TC is set.
        unsafe {
            let b = &mut *core::ptr::addr_of_mut!(RX_BOUNCE[0]);
            for (i, w) in b.0.iter_mut().enumerate() {
                *w = i as u32;
            }
        }
        crate::mmio::test::poke32(crate::pac::DMAC.ch(1).chstat().addr(), crate::dmac::CHSTAT_TC);
        let mut out = [0u8; 368];
        let n = drive(h.dma_drain(&mut out, 368), 10).unwrap().unwrap();
        assert_eq!(n, 368);
        assert_eq!(&out[..8], &[0, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(crate::pac::DMAC.ch(1).n0tb().read(), 368);
    }

    #[test]
    fn a_misaligned_length_is_dropped_not_dmaed() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        let mut out = [0u8; 368];
        assert_eq!(drive(h.dma_drain(&mut out, 7), 10).unwrap(), Ok(0));
        assert_ne!(usb(0).d1fifoctr().read() & FIFOCTR_BCLR, 0);
    }

    #[test]
    fn a_zero_length_packet_needs_no_dma() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        let mut out = [0u8; 368];
        assert_eq!(drive(h.dma_drain(&mut out, 0), 10).unwrap(), Ok(0));
    }

    #[test]
    fn a_dma_that_never_ends_times_out() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        let mut out = [0u8; 368];
        assert_eq!(drive(h.dma_drain(&mut out, 368), 50).unwrap(), Err(PipeError::Timeout));
    }
```

Notes:
- Check the `mmio::test` API for the log reset and entry fields (`reset_log` / `Access { addr, .. }`)
  and adapt the names. If there is no reset, filter the log by position before and after.
- `edpt_close(dev, ep)` is the public close used by `Rusb1Pipe::drop`; use whatever name the
  file has.

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rza1 && CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib usb::host 2>&1 | grep -E "^error" | sort | uniq -c
```
Expected: compile errors.

- [ ] **Step 3: Implement**

- **`regs.rs`:** `pub const FIFOSEL_DCLRM: u16 = 0x2000; // DnFIFOSEL only: auto buffer clear after read`.
- **`dmac.rs`:** `pub(crate) const CHCFG_AM_BUS_CYCLE: u32 = 1 << 9;`, documented as
  "AM = 010? no: AM bits [10:8], bus-cycle mode = 0b010 → 0x200, as Renesas' USB driver uses". Make
  `CHSTAT_TC` and the CHCFG constants `pub(crate)` if they aren't already.
- **`host.rs`:**
  - Statics:
    ```rust
    /// Per port: the DMA channel that drains the receive pipe bound to D1FIFO.
    static RX_DMA: [critical_section::Mutex<core::cell::RefCell<Option<crate::dmac::Dma<'static>>>>; 2] = ...;
    /// Per port: the pipe bound to D1FIFO for DMA receive, or 0.
    static RX_DMA_PIPE: [AtomicU8; 2] = [AtomicU8::new(0), AtomicU8::new(0)];
    /// Per port: 1 KB, 32-byte-aligned bounce buffer the DMAC writes received blocks into. Only
    /// ever touched through its uncached alias (`crate::uncached`).
    #[repr(align(32))]
    struct Bounce([u32; 256]);
    static mut RX_BOUNCE: [Bounce; 2] = [Bounce([0; 256]), Bounce([0; 256])];
    ```
    and a `#[cfg(test)] fn reset_rx_dma(port)` that clears both statics.
  - `enable_rx_dma`: `init_register_mode_rx(chcfg, src = d1fifo addr, dmars = 0x87 / 0x8F)`, then
    store the channel.
  - In `edpt_open`, after the PIPECFG write (still in the pipe-config `unsafe` block): if
    interrupt-on-bulk IN, `RX_DMA[p]` is `Some`, and `RX_DMA_PIPE[p] == 0`, claim:
    `d1fbcfg = 0`; `d1fifosel = pipe | MBW_32<<SHIFT | DCLRM`; wait for CURPIPE; then
    `|= DREQE`; `RX_DMA_PIPE[p] = pipe`.
  - In the close path (the one clearing `HCD_PIPE_ISO_IN`): if `RX_DMA_PIPE[p] == pipe`, write
    `d1fifosel = 0` and set it to 0.
  - **In `pipe_brdy_in`,** before the ISO check: if `n == RX_DMA_PIPE[port]`, call
    `dma_brdy_in(u, n)` and return. It:
    1. reads `d1fifoctr` DTLN;
    2. sets `len = DTLN.min(xfer.remaining)` and `xfer.remaining -= len`;
    3. if DTLN == 0, writes `d1fifoctr = BCLR`;
    4. sets PID=NAK, nulls `xfer.buf`, sets DONE and wakes.

    `pipe_brdy_in` has no port argument, so derive the port from `u` (compare
    `u.d1fifosel().addr()` with `usb(0)`'s) or pass it in from the caller.
  - **In `xfer_in`,** after the ISO check, set `let dma = RX_DMA_PIPE[p] == pipe`. After
    `poll_pipe(pipe).await?` returns `n`, if `dma`, return `self.dma_drain(buf, n).await`.
  - **`dma_drain(&self, buf, n)`:**
    1. If `n == 0`, return `Ok(0)`.
    2. If `n % 4 != 0` or `n > 1024`, write `d1fifoctr = BCLR` and return `Ok(0)`.
    3. Take the channel from `RX_DMA`; with none, return `Err(PipeError::Canceled)`.
    4. Arm it: `start_transfer_rx(uncached(bounce), n)` after `stop`-free re-arm. Write
       CHCTRL CLREN and SWRST through a new `pub(crate) unsafe fn rearm_rx(ch, src, dst, count)`
       in `dmac/raw.rs`, which keeps DMARS: CLREN, SWRST|CLRTC, N0SA=src, N0DA=dst, N0TB=count,
       SETEN.
    5. `select(dma.wait_transfer_complete(), Timer::after_millis(2))`. On timeout, halt the
       channel (CLREN), BCLR the FIFO, put the channel back and return `Err(PipeError::Timeout)`.
    6. Copy `n.min(buf.len())` bytes from the uncached bounce into `buf`, put the channel back,
       and return the count.

- [ ] **Step 4: Run all `usb::` tests; rza1 suite; commit**

```bash
cd rza1 && CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib usb:: 2>&1 | grep "test result"
cd .. && rza1/tools/test.sh > "$SCRATCH/sp2-rza1.log" 2>&1; echo $?
git add rza1 && git commit -m "rza1l-hal: USB host receive DMA for an interrupt-on-bulk IN pipe on D1FIFO"
```

---

### Task 2: Board wiring and harness

**Files:**
- Modify: `crates/deluge-bsp/src/board.rs`: `pub type Usb0RxDma = ScuxFfu0Dma;`. DMA_CH1 is
  unused as SCUX FFU0, and its doc says so: "or USB0 receive DMA; a firmware uses it for one".
- Modify: `hw-tests/overbridge-host-firmware/src/main.rs`: bind `DMAINT1` to
  `rza1l_hal::dmac::InterruptHandler<DMA_CH1>`, build `Dma::new(board.dma.scux_ffu0, Irqs)`, and
  call `host_driver.enable_rx_dma(dma)` before spawning the supervisor. Add a `--no-dma` build
  feature (`cpu-rx`) that skips it, for hardware task 2's comparison.
- Modify: `docs/hardware-tasks.md` (task 2: run with and without `cpu-rx`; record CPU load and
  gaps for each).
- Modify: `CHANGELOG.md`.

- [ ] **Step 1:** Implement the above, then build both harness variants and the other firmwares:

```bash
cargo build -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem --release --no-default-features -p overbridge-host-firmware 2>&1 | grep -E "^error|Finished"
cargo build -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem --release --no-default-features -p overbridge-host-firmware --features cpu-rx 2>&1 | grep -E "^error|Finished"
```

- [ ] **Step 2:** CHANGELOG `### Added`:

```markdown
- `rza1l-hal` USB host: `Rusb1HostDriver::enable_rx_dma` gives the driver a DMAC channel. The
  first interrupt-on-bulk IN pipe opened afterwards (an Overbridge audio stream) is drained by
  DMA through D1FIFO instead of byte-wise CPU reads. Without it, transfers use the CPU as before.
```

Commit:

```bash
git add crates/deluge-bsp/src/board.rs hw-tests/overbridge-host-firmware docs/hardware-tasks.md CHANGELOG.md
git commit -m "overbridge-host-firmware: drain the Overbridge stream with USB receive DMA (cpu-rx to compare)"
```
