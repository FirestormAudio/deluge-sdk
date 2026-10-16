# Overbridge sub-project 1: host pipes (interrupt-on-bulk, high-speed IITV) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The RZ/A1L USB host can open a high-speed interrupt endpoint larger than 64 bytes,
such as a Digitakt's 368-byte Overbridge IN endpoint, by carrying it on a bulk pipe. It also
polls high-speed interrupt endpoints at the period they ask for.

**Architecture:**
- **Layout function.** A pure `pipe_layout(ep_type, wMaxPacketSize)` decides three things for
  each endpoint: the pipe range it may use, the MXPS to program, and the PIPECFG type it is
  carried as. `HcdAlloc::alloc_pipe` searches the range it's given, and `edpt_open` programs
  what the layout says.
- **IITV function.** A pure `interrupt_iitv(bInterval, high_speed)` replaces the inline
  formula. `edpt_open` reads the device's speed back from DEVADDn.
- **Scope.** Everything stays inside `rza1l-hal::usb::host`. No public signature changes.

**Tech Stack:**
- Rust (no_std), the `rza1l-hal` crate in the `rza1/` workspace.
- Host tests run under QEMU (armv7-unknown-linux-gnueabihf) against the poked-MMIO seam
  (`crate::mmio::test`).

**Spec:** `docs/superpowers/specs/2026-10-15-overbridge-design.md`, §4.1 and §4.2 (and §8.2's
"Pipe configuration" tests).

## Global Constraints

- Interrupt endpoints of 64 bytes or less keep pipes 6–9, as today.
- Interrupt endpoints over 64 bytes are carried as bulk pipes:
  - MaxPacket ≤ 512: the bulk range, searched 5 down to 1, at MXPS 512.
  - MaxPacket over 512: pipes 1–2 only, at MXPS = MaxPacket, up to 1024.
- Such a pipe is configured `PIPECFG_TYPE_BULK | SHTNAK | DBLB` with IITV 0.
- A high-bandwidth endpoint (wMaxPacketSize bits 12:11 non-zero) is rejected at open.
- Allocation failure is an error at pipe open, never a silent fallback to a smaller MXPS.
- High-speed IITV is `bInterval − 1`, capped at 7. Full and low speed keep
  `floor(log2(bInterval))`. A device's speed comes from DEVADDn.USBSPD.
- The `embassy-usb-host` crate stays at 0.1.0, unmodified.
- Work in `rza1/`; the chip crates must not gain a dependency on any `deluge*` or `flare*`
  crate (`rza1/tools/test.sh` enforces this).

## Review Focus

1. **A hub-attached full-speed device's interrupt endpoint** (DEVADDn.USBSPD = FS, behind a
   high-speed hub) must keep the log2 formula. The TRM counts 1 ms frames for it. Pinned in
   Task 2 by `a_full_speed_device_s_interrupt_pipe_counts_frames`.
2. **bInterval 0.** The embassy hub class passes `interval_ms: 0`. It must give IITV 0, not
   underflow. Pinned in Task 2 by `interrupt_iitv_treats_zero_as_one`.
3. **Pipe budget with a Digitakt plus a USB audio interface.** The Digitakt's four bulk-range
   pipes plus one isochronous pipe must all open, and a second isochronous pipe must fail with
   `OutOfPipes`. It must not steal a pipe or open at a wrong size. Pinned in Task 1 by
   `a_digitakt_leaves_one_isochronous_pipe`.
4. **Packet-buffer exhaustion** when opening a large interrupt-on-bulk pipe. The open must
   return an error and free the pipe it took. Pinned in Task 1 by
   `a_full_packet_buffer_fails_the_open_and_frees_the_pipe`. That behaviour exists today, so
   this test is a characterization and passes before the change; the plan says so at the step.
5. **The isochronous path stays unchanged.** Iso pipes stay on 1–2, keep no NRDY interrupt,
   and still get their ring reset. The existing tests `isochronous_*` and
   `nrdy_on_an_isochronous_out_pipe_does_not_fail_its_transfer` cover this. Run them in every
   suite step.

---

## File Structure

- **Modify `rza1/rza1l-hal/src/usb/host.rs`:**
  - `PipeLayout` and `pipe_layout`, beside `HcdAlloc`;
  - `interrupt_iitv`;
  - `HcdAlloc::alloc_pipe(first, last)`;
  - `edpt_open` wiring;
  - the module doc's pipe table;
  - tests in the existing `mod tests`.
- **Modify `rza1/rza1l-hal/src/usb/regs.rs`:** the PIPEPERI comment block (lines 222–231), so
  it states both formulas.
- **Modify `CHANGELOG.md`:** two lines under `[Unreleased]`.

Both tasks touch `host.rs`. Task 2 assumes Task 1's `edpt_open` shape.

---

### Task 1: Interrupt endpoints over 64 bytes ride bulk pipes

**Files:**
- Modify: `rza1/rza1l-hal/src/usb/host.rs`. Touches `HcdAlloc::alloc_pipe` (~line 253),
  `edpt_open` (~lines 963–1080), the module doc (~lines 28–42), and the test
  `host_pipe_buffers_are_disjoint_and_clear_of_the_dcp` (~line 2683).
- Modify: `CHANGELOG.md`.

**Interfaces:**
- Produces (module-private, in `host.rs`):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PipeLayout {
    /// Lowest and highest pipe the endpoint may use; searched highest first.
    first: usize,
    last: usize,
    /// PIPEMAXP.MXPS to programme.
    mxps: u16,
    /// The transfer type the pipe is configured as (PIPECFG.TYPE).
    carry: EndpointType,
}
fn pipe_layout(ep_type: EndpointType, w_max_packet: u16) -> Result<PipeLayout, HostError>;
impl HcdAlloc { fn alloc_pipe(&mut self, first: usize, last: usize) -> Option<usize>; }
```

- [ ] **Step 1: Write the failing layout tests**

Add to `mod tests` in `rza1/rza1l-hal/src/usb/host.rs`:

```rust
    /// The Digitakt's Overbridge IN endpoint (interrupt, 368 bytes) is too big for an
    /// interrupt pipe (6-9 stop at 64), so it rides a bulk pipe. Pipes 3-5 only take MXPS
    /// 8/16/32/64/512, so it is programmed at 512 and every block arrives as a short packet.
    #[test]
    fn an_interrupt_endpoint_over_64_bytes_is_carried_on_a_bulk_pipe_at_mxps_512() {
        assert_eq!(
            pipe_layout(EndpointType::Interrupt, 368),
            Ok(PipeLayout { first: 1, last: 5, mxps: 512, carry: EndpointType::Bulk })
        );
        assert_eq!(
            pipe_layout(EndpointType::Interrupt, 88),
            Ok(PipeLayout { first: 1, last: 5, mxps: 512, carry: EndpointType::Bulk })
        );
    }

    /// Over 512 bytes (a Digitakt II's 1012-byte IN endpoint) only pipes 1-2 take the size.
    #[test]
    fn an_interrupt_endpoint_over_512_bytes_needs_pipe_1_or_2() {
        assert_eq!(
            pipe_layout(EndpointType::Interrupt, 1012),
            Ok(PipeLayout { first: 1, last: 2, mxps: 1012, carry: EndpointType::Bulk })
        );
        assert_eq!(
            pipe_layout(EndpointType::Interrupt, 1024),
            Ok(PipeLayout { first: 1, last: 2, mxps: 1024, carry: EndpointType::Bulk })
        );
    }

    /// MIDI and hub status endpoints keep the interrupt pipes.
    #[test]
    fn an_interrupt_endpoint_of_64_bytes_or_less_keeps_an_interrupt_pipe() {
        for mps in [1, 8, 64] {
            assert_eq!(
                pipe_layout(EndpointType::Interrupt, mps),
                Ok(PipeLayout { first: 6, last: 9, mxps: mps, carry: EndpointType::Interrupt })
            );
        }
    }

    /// A high-bandwidth interrupt endpoint (wMaxPacketSize bits 12:11 = extra transactions per
    /// microframe) can't ride a bulk pipe, and one over 1024 bytes isn't valid USB.
    #[test]
    fn a_high_bandwidth_or_oversized_interrupt_endpoint_is_rejected() {
        assert!(pipe_layout(EndpointType::Interrupt, (1 << 11) | 368).is_err());
        assert!(pipe_layout(EndpointType::Interrupt, (2 << 11) | 1024).is_err());
        assert!(pipe_layout(EndpointType::Interrupt, 1025).is_err());
    }

    /// Bulk and isochronous endpoints are laid out as before.
    #[test]
    fn bulk_and_isochronous_layouts_are_unchanged() {
        assert_eq!(
            pipe_layout(EndpointType::Bulk, 512),
            Ok(PipeLayout { first: 1, last: 5, mxps: 512, carry: EndpointType::Bulk })
        );
        assert_eq!(
            pipe_layout(EndpointType::Bulk, 64),
            Ok(PipeLayout { first: 1, last: 5, mxps: 64, carry: EndpointType::Bulk })
        );
        assert_eq!(
            pipe_layout(EndpointType::Isochronous, 56),
            Ok(PipeLayout { first: 1, last: 2, mxps: 56, carry: EndpointType::Isochronous })
        );
        assert_eq!(pipe_layout(EndpointType::Control, 64), Err(HostError::OutOfPipes));
    }
```

- [ ] **Step 2: Write the failing `edpt_open` tests**

Add to the same `mod tests`:

```rust
    /// Open one endpoint on device 1 (high speed) of a fresh port-0 allocator, and return the
    /// pipe with the PIPECFG/PIPEMAXP/PIPEPERI values `edpt_open` wrote.
    fn open_on_fresh_port(ep: u8, ty: EndpointType, mps: u16, binterval: u8) -> (usize, u16, u16, u16) {
        use crate::mmio::test;
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_HS);
        h.edpt_open(1, ep, ty, mps, binterval).unwrap();
        let pipe = h.ep_to_pipe(1, ep).unwrap();
        let u = usb(0);
        (pipe, u.pipecfg().read(), u.pipemaxp().read(), u.pipeperi().read())
    }

    fn reserved_blocks(pipe: usize) -> u8 {
        critical_section::with(|cs| unsafe { (*HCD_ALLOC[0].borrow(cs).get()).buf_reserved[pipe] })
    }

    #[test]
    fn a_368_byte_interrupt_in_endpoint_opens_as_a_double_buffered_bulk_pipe() {
        use crate::usb::regs::{PIPECFG_DBLB, PIPECFG_DIR, PIPECFG_SHTNAK, PIPECFG_TYPE_MASK};
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (pipe, cfg, maxp, peri) = open_on_fresh_port(0x83, EndpointType::Interrupt, 368, 1);
        assert!((3..=5).contains(&pipe), "pipe {pipe}");
        assert_eq!(cfg & PIPECFG_TYPE_MASK, PIPECFG_TYPE_BULK);
        assert_ne!(cfg & PIPECFG_DBLB, 0);
        assert_ne!(cfg & PIPECFG_SHTNAK, 0);
        assert_eq!(cfg & PIPECFG_DIR, 0, "IN: host receives");
        assert_eq!(maxp & PIPEMAXP_MXPS_MASK, 512);
        assert_eq!(peri & PIPEPERI_IITV_MASK, 0, "IITV must be 0 on pipes 3-5");
        assert_eq!(reserved_blocks(pipe), 16, "2 banks x 512 bytes");
    }

    #[test]
    fn an_88_byte_interrupt_out_endpoint_opens_as_a_bulk_pipe() {
        use crate::usb::regs::{PIPECFG_DIR, PIPECFG_TYPE_MASK};
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (pipe, cfg, maxp, _) = open_on_fresh_port(0x03, EndpointType::Interrupt, 88, 1);
        assert!((3..=5).contains(&pipe), "pipe {pipe}");
        assert_eq!(cfg & PIPECFG_TYPE_MASK, PIPECFG_TYPE_BULK);
        assert_ne!(cfg & PIPECFG_DIR, 0, "OUT: host sends");
        assert_eq!(maxp & PIPEMAXP_MXPS_MASK, 512);
    }

    #[test]
    fn a_1012_byte_interrupt_endpoint_opens_on_pipe_1_or_2_at_its_own_size() {
        use crate::usb::regs::PIPECFG_TYPE_MASK;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (pipe, cfg, maxp, _) = open_on_fresh_port(0x83, EndpointType::Interrupt, 1012, 1);
        assert!((1..=2).contains(&pipe), "pipe {pipe}");
        assert_eq!(cfg & PIPECFG_TYPE_MASK, PIPECFG_TYPE_BULK);
        assert_eq!(maxp & PIPEMAXP_MXPS_MASK, 1012);
        assert_eq!(reserved_blocks(pipe), 32, "2 banks x 16 blocks");
    }

    #[test]
    fn a_64_byte_interrupt_endpoint_keeps_pipes_6_to_9() {
        use crate::usb::regs::PIPECFG_TYPE_MASK;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (pipe, cfg, maxp, _) = open_on_fresh_port(0x82, EndpointType::Interrupt, 64, 1);
        assert!((6..=9).contains(&pipe), "pipe {pipe}");
        assert_eq!(cfg & PIPECFG_TYPE_MASK, PIPECFG_TYPE_INTR);
        assert_eq!(maxp & PIPEMAXP_MXPS_MASK, 64);
    }

    /// A Digitakt opens its two Overbridge endpoints and its two USB-MIDI bulk endpoints: four
    /// of the five bulk-capable pipes. One isochronous stream (a USB audio interface) still
    /// fits; a second doesn't, and fails cleanly.
    #[test]
    fn a_digitakt_leaves_one_isochronous_pipe() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        crate::mmio::test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_HS);
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1).unwrap();
        h.edpt_open(1, 0x03, EndpointType::Interrupt, 88, 1).unwrap();
        h.edpt_open(1, 0x81, EndpointType::Bulk, 512, 0).unwrap();
        h.edpt_open(1, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        let mut pipes: Vec<usize> =
            [0x83, 0x03, 0x81, 0x01].iter().map(|&ep| h.ep_to_pipe(1, ep).unwrap()).collect();
        pipes.sort();
        pipes.dedup();
        assert_eq!(pipes.len(), 4, "distinct pipes");
        assert!(pipes.iter().all(|p| (1..=5).contains(p)));

        h.edpt_open(2, 0x84, EndpointType::Isochronous, 196, 1).unwrap();
        assert_eq!(
            h.edpt_open(2, 0x05, EndpointType::Isochronous, 196, 1),
            Err(HostError::OutOfPipes)
        );
    }

    /// Characterization: this behaviour exists before the change, and the test pins it.
    /// When the packet buffer can't hold a pipe, the open fails and the pipe goes back.
    #[test]
    fn a_full_packet_buffer_fails_the_open_and_frees_the_pipe() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe {
            let a = &mut *HCD_ALLOC[0].borrow(cs).get();
            *a = HcdAlloc::new();
            // Leave fewer than the 16 blocks a 512-byte double-buffered pipe needs.
            while a.buf.alloc(1).is_some() {}
        });
        assert_eq!(
            h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1),
            Err(HostError::OutOfPipes)
        );
        let used = critical_section::with(|cs| unsafe { (*HCD_ALLOC[0].borrow(cs).get()).pipes_used });
        assert_eq!(used, 0x0001, "only the DCP stays allocated");
    }
```

Update the existing `host_pipe_buffers_are_disjoint_and_clear_of_the_dcp` test's two
`a.alloc_pipe(EndpointType::Bulk)` calls to `a.alloc_pipe(1, 5)`.

- [ ] **Step 3: Run the tests to verify they fail**

Run, from `rza1/`:
```bash
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib usb::host 2>&1 | tail -20
```
Expected: compile errors. `PipeLayout`/`pipe_layout` aren't found, and `alloc_pipe` takes one
argument, not two.

- [ ] **Step 4: Implement `PipeLayout`, `pipe_layout` and the range-based `alloc_pipe`**

In `host.rs`, replace `HcdAlloc::alloc_pipe` (~line 253) with:

```rust
    /// Allocate a free pipe in `first..=last`, highest first, as TinyUSB's `find_pipe` does.
    fn alloc_pipe(&mut self, first: usize, last: usize) -> Option<usize> {
        for p in (first..=last).rev() {
            if self.pipes_used & (1 << p) == 0 {
                self.pipes_used |= 1 << p;
                return Some(p);
            }
        }
        None
    }
```

Above `struct HcdAlloc`, add:

```rust
/// How an endpoint is carried: the pipes it may use, the MXPS to programme and the transfer
/// type the pipe is configured as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PipeLayout {
    /// Lowest and highest pipe the endpoint may use; searched highest first.
    first: usize,
    last: usize,
    /// PIPEMAXP.MXPS to programme.
    mxps: u16,
    /// The transfer type the pipe is configured as (PIPECFG.TYPE).
    carry: EndpointType,
}

/// The pipe layout for an endpoint of `ep_type` with descriptor `w_max_packet`.
///
/// Interrupt pipes (6-9) stop at 64 bytes, so a larger interrupt endpoint (Overbridge's
/// 368-byte blocks) rides a bulk pipe. On the wire an interrupt and a bulk transaction use the
/// same tokens, handshakes and toggles, and a bulk pipe is polled every microframe the bus has
/// room for, at least as often as `bInterval` 1. Pipes 3-5 take only MXPS 8/16/32/64/512
/// (TRM §28.3.34), so up to 512 bytes the pipe is programmed at 512 and every packet is short;
/// above that only pipes 1-2 take the size. A high-bandwidth endpoint (bits 12:11) can't ride
/// a bulk pipe and is rejected.
fn pipe_layout(ep_type: EndpointType, w_max_packet: u16) -> Result<PipeLayout, HostError> {
    let mps = w_max_packet & PIPEMAXP_MXPS_MASK;
    let layout = |first, last, mxps, carry| Ok(PipeLayout { first, last, mxps, carry });
    match ep_type {
        EndpointType::Control => Err(HostError::OutOfPipes), // control uses pipe 0 directly
        EndpointType::Isochronous => layout(1, 2, mps, EndpointType::Isochronous),
        EndpointType::Bulk => layout(1, 5, mps, EndpointType::Bulk),
        EndpointType::Interrupt if mps <= 64 => layout(6, 9, mps, EndpointType::Interrupt),
        EndpointType::Interrupt if w_max_packet >> 11 != 0 || mps > 1024 => Err(
            HostError::Other("interrupt endpoint is high-bandwidth or over 1024 bytes"),
        ),
        EndpointType::Interrupt if mps <= 512 => layout(1, 5, 512, EndpointType::Bulk),
        EndpointType::Interrupt => layout(1, 2, mps, EndpointType::Bulk),
    }
}
```

Note on the masking: `PIPEMAXP_MXPS_MASK` is 0x7FF, so `mps` can be up to 2047 and `mps > 1024`
catches the oversized case. The rejection check comes after the `mps <= 64` arm, so a
high-bandwidth endpoint of 64 bytes or less keeps its old behaviour (an interrupt pipe with the
mult bits masked off). That's out of scope here.

- [ ] **Step 5: Wire `edpt_open` to the layout**

In `edpt_open`, replace the allocation line and the pipe-buffer block:

```rust
            let layout = pipe_layout(ep_type, mps)?;
            let pipe = alloc
                .alloc_pipe(layout.first, layout.last)
                .ok_or(HostError::OutOfPipes)?;

            // Reserve packet-buffer blocks for programmable pipes.  Pipes 6-8
            // have hardware-fixed buffers (blocks 4/5/6, BUFNMB writes ignored),
            // so they are skipped; all other pipes (bulk/iso 1-5, interrupt 9)
            // MUST have a disjoint PIPEBUF region or their FIFOs overlap at
            // block 0 (the DCP area) and corrupt each other.  DBLB is set for
            // bulk and iso (see PIPECFG below), so those reserve two banks.
            let pipebuf: Option<(u8, u8)> = if (6..=8).contains(&pipe) {
                None
            } else {
                let double_buf =
                    matches!(layout.carry, EndpointType::Bulk | EndpointType::Isochronous);
                match alloc.alloc_pipe_buf(pipe, layout.mxps, double_buf) {
                    Some(v) => Some(v),
                    None => {
                        alloc.free_pipe(pipe);
                        return Err(HostError::OutOfPipes);
                    }
                }
            };
```

In the same function, make the PIPEMAXP write use `layout.mxps`:

```rust
                u.pipemaxp().write_value(
                    ((dev_addr as u16) << PIPEMAXP_DEVSEL_SHIFT) | (layout.mxps & PIPEMAXP_MXPS_MASK),
                );
```

Make the type match use `layout.carry`:

```rust
                let type_field: u16 = match layout.carry {
                    EndpointType::Bulk => PIPECFG_TYPE_BULK | PIPECFG_SHTNAK | PIPECFG_DBLB,
                    EndpointType::Interrupt => PIPECFG_TYPE_INTR,
                    EndpointType::Isochronous => PIPECFG_TYPE_ISO | PIPECFG_DBLB,
                    EndpointType::Control => unreachable!("control EP handled by DCP"),
                };
```

Make the IITV condition read `if layout.carry == EndpointType::Interrupt {`. Leave the formula
inside it alone; Task 2 replaces it. Leave the isochronous NRDY and ring code keyed on
`ep_type`: an isochronous endpoint's carry is always isochronous.

- [ ] **Step 6: Run the tests to verify they pass**

Run the Step 3 command.
Expected: every `usb::host` test passes, including the existing `isochronous_*` tests and
`host_pipe_buffers_are_disjoint_and_clear_of_the_dcp`. The characterization test
`a_full_packet_buffer_fails_the_open_and_frees_the_pipe` passes as well. It would also have
passed before Step 4 had the file compiled, which is expected and noted in the test's doc.

- [ ] **Step 7: Update the module doc and CHANGELOG**

In `host.rs`'s module doc, replace the pipe table rows `| 1-2  | ISO or Bulk   |`,
`| 3-5  | Bulk          |` and `| 6-9  | Interrupt     |` with:

```rust
//! | 1-2  | ISO, Bulk, or Interrupt over 512 bytes (as bulk) |
//! | 3-5  | Bulk, or Interrupt of 65-512 bytes (as bulk, MXPS 512) |
//! | 6-9  | Interrupt of 64 bytes or less |
```

In `CHANGELOG.md`, add a line under `## [Unreleased]` → `### Added`:

```markdown
- `rza1l-hal` USB host: an interrupt endpoint over 64 bytes (such as Overbridge's 368-byte
  audio blocks) opens on a bulk pipe (MXPS 512 on pipes 3-5, or its own size up to 1024 on
  pipes 1-2) instead of failing for lack of a large interrupt pipe.
```

- [ ] **Step 8: Commit**

```bash
git add rza1/rza1l-hal/src/usb/host.rs CHANGELOG.md
git commit -m "rza1l-hal: usb host carries interrupt endpoints over 64 bytes on bulk pipes"
```

---

### Task 2: High-speed interrupt pipes count microframes

**Files:**
- Modify: `rza1/rza1l-hal/src/usb/host.rs`: `edpt_open`'s IITV block (~line 1041) and
  `mod tests`.
- Modify: `rza1/rza1l-hal/src/usb/regs.rs:222-231`, the PIPEPERI comment.
- Modify: `CHANGELOG.md`.

**Interfaces:**
- Consumes: Task 1's `PipeLayout`, and `layout.carry` in `edpt_open`.
- Produces: `fn interrupt_iitv(binterval: u8, high_speed: bool) -> u16` (module-private).

- [ ] **Step 1: Write the failing `edpt_open` tests**

Add to `mod tests`. They use Task 1's `open_on_fresh_port` helper.

```rust
    #[test]
    fn a_high_speed_device_s_interrupt_pipe_counts_microframes() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (_, _, _, peri) = open_on_fresh_port(0x81, EndpointType::Interrupt, 8, 4);
        assert_eq!(peri & PIPEPERI_IITV_MASK, 3, "2^3 microframes = 1 ms");
    }

    #[test]
    fn a_full_speed_device_s_interrupt_pipe_counts_frames() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        // A full-speed device behind a high-speed hub: DEVADDn says full speed.
        test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_FS | (1 << DEVADD_HUBPORT_SHIFT));
        h.edpt_open(1, 0x81, EndpointType::Interrupt, 8, 4).unwrap();
        assert_eq!(usb(0).pipeperi().read() & PIPEPERI_IITV_MASK, 2, "2^2 frames = 4 ms");
    }
```

- [ ] **Step 2: Run them against today's formula**

Run, from `rza1/`:
```bash
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib usb::host 2>&1 | tail -20
```
Expected:
- `a_high_speed_device_s_interrupt_pipe_counts_microframes` FAILS: left 2 (today's
  `floor(log2(4))`), right 3.
- `a_full_speed_device_s_interrupt_pipe_counts_frames` PASSES, because today's formula is right
  at full speed. It is a characterization test that pins Review Focus item 1 through the change.

- [ ] **Step 3: Write the failing `interrupt_iitv` tests**

Add to `mod tests`:

```rust
    /// High speed: bInterval is an exponent, period 2^(bInterval-1) microframes, and the
    /// controller counts microframes for a high-speed device (TRM §28.4.8 (1)(c)). IITV tops
    /// out at 7 (16 ms).
    #[test]
    fn interrupt_iitv_at_high_speed_is_binterval_minus_one() {
        assert_eq!(interrupt_iitv(1, true), 0);
        assert_eq!(interrupt_iitv(4, true), 3);
        assert_eq!(interrupt_iitv(8, true), 7);
        assert_eq!(interrupt_iitv(12, true), 7);
        assert_eq!(interrupt_iitv(16, true), 7);
    }

    /// Full and low speed (including a full-speed device behind a high-speed hub, which the
    /// controller counts in 1 ms frames): bInterval is in frames, rounded down to a power of 2.
    #[test]
    fn interrupt_iitv_at_full_speed_rounds_binterval_down_to_a_power_of_two() {
        assert_eq!(interrupt_iitv(1, false), 0);
        assert_eq!(interrupt_iitv(2, false), 1);
        assert_eq!(interrupt_iitv(10, false), 3);
        assert_eq!(interrupt_iitv(255, false), 7);
    }

    /// embassy-usb-host's hub class opens its status endpoint with interval 0.
    #[test]
    fn interrupt_iitv_treats_zero_as_one() {
        assert_eq!(interrupt_iitv(0, true), 0);
        assert_eq!(interrupt_iitv(0, false), 0);
    }

```

Run the Step 2 command.
Expected: a compile error, because `interrupt_iitv` isn't found.

- [ ] **Step 4: Implement**

Add next to `pipe_layout` in `host.rs`:

```rust
/// PIPEPERI.IITV for an interrupt pipe. The controller issues the pipe's token once every
/// 2^IITV frames, counted in microframes for a high-speed device and in 1 ms frames for a
/// full- or low-speed one, including one behind a high-speed hub (TRM §28.4.8 (1)(c)). At high
/// speed bInterval is an exponent (period 2^(bInterval-1) microframes); otherwise it is a
/// period in frames, rounded down here to a power of two. 0 (embassy's hub class) counts as 1.
fn interrupt_iitv(binterval: u8, high_speed: bool) -> u16 {
    let b = binterval.max(1);
    let n = if high_speed { b - 1 } else { (u8::BITS - 1 - b.leading_zeros()) as u8 };
    n.min(7) as u16
}
```

In `edpt_open`, replace the IITV block with:

```rust
                // PIPEPERI: IITV for interrupt pipes (6-9); bulk pipes (1-5, including
                // interrupt endpoints carried as bulk) must have IITV=0. The device's speed is
                // the USBSPD that device_open programmed into DEVADDn.
                let iitv: u16 = if layout.carry == EndpointType::Interrupt {
                    let high_speed = u.devadd(dev_addr as usize).read() & DEVADD_USBSPD_HS
                        == DEVADD_USBSPD_HS;
                    interrupt_iitv(binterval, high_speed)
                } else {
                    0
                };
                u.pipeperi().write_value(iitv & PIPEPERI_IITV_MASK);
```

Update `edpt_open`'s doc line about `binterval`:

```rust
    /// `binterval` is the endpoint descriptor's bInterval: an exponent for a high-speed
    /// interrupt endpoint, a period in frames otherwise; ignored for bulk and isochronous pipes.
    /// Pass 0 or 1 for the shortest period.
```

In `regs.rs`, replace the last two comment lines of the PIPEPERI block
(`// Conversion from USB bInterval (FS device, 1-255 frames) to IITV:` and the formula line
after it) with:

```rust
// Conversion from USB bInterval to IITV (see host::interrupt_iitv):
//   high-speed device:  IITV = (bInterval - 1).min(7)         (microframes)
//   full/low speed:     IITV = floor(log2(bInterval)).min(7)  (1 ms frames, also behind a HS hub)
```

Add a line under `CHANGELOG.md` → `## [Unreleased]`, in a `### Fixed` section; create it after
`### Changed` if it's missing:

```markdown
- `rza1l-hal` USB host: a high-speed device's interrupt endpoint is polled at the period its
  bInterval asks for (2^(bInterval-1) microframes). It was polled more often: a hub's status
  endpoint at bInterval 12 every 1 ms instead of 16 ms.
```

- [ ] **Step 5: Run the tests to verify they pass**

Run the Step 2 command.
Expected: all `usb::host` tests pass.

- [ ] **Step 6: Run the full chip-crate suite and the root suite**

```bash
rza1/tools/test.sh > /tmp/claude-1000/sp1-rza1.log 2>&1; echo "rza1: $?"; tail -5 /tmp/claude-1000/sp1-rza1.log
tools/test.sh > /tmp/claude-1000/sp1-root.log 2>&1; echo "root: $?"; tail -5 /tmp/claude-1000/sp1-root.log
```
Expected: both exit 0.
- `rza1/tools/test.sh` covers the PAC checks, the rza1l-hal QEMU tests and the device build.
- `tools/test.sh` covers everything downstream, including the BSP's USB host supervisor,
  `usb-classes` and the firmware builds.

- [ ] **Step 7: Commit**

```bash
git add rza1/rza1l-hal/src/usb/host.rs rza1/rza1l-hal/src/usb/regs.rs CHANGELOG.md
git commit -m "rza1l-hal: usb host polls high-speed interrupt endpoints at 2^(bInterval-1) microframes"
```

---

## After the plan (hardware, by the user)

These aren't plan tasks, because nothing here can run them:
- **MIDI regression.** A USB-MIDI keyboard and a hub still enumerate and play on the Deluge,
  since hub and MIDI interrupt pipes change their IITV.
- **Digitakt open.** With a Digitakt in Overbridge mode, `edpt_open` succeeds for 0x83 and 0x03.
  Streaming needs sub-project 3. This is also when to compare the descriptor dump with the
  spec's Appendix A (§8.4 step 1).
