# Overbridge sub-project 4: device class and device-mode interrupt-on-bulk Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A board can present itself as an Overbridge 2 device that Overwitch and the Deluge
host class both accept. That serves the Rust Digitakt firmware, and a Deluge as the device side
of a loopback test. Under it, the RZ/A1L device driver carries interrupt endpoints over 64 bytes
on bulk pipes.

**Architecture:**
- **`rza1l-hal::usb::driver`**:
  - an interrupt endpoint over 64 bytes is allocated as a bulk pipe (MXPS 512 on pipes 3–5 or
    9–15, or its own size on pipes 1–2 above 512);
  - its descriptor still says interrupt and its declared size;
  - a shared endpoint that outgrows its pipe across alternate settings (Overbridge's 32 → 88 →
    368 bytes on one address) moves to a pipe that carries it.
- **`usb_classes::overbridge::device`** (feature `device-overbridge`, embassy-usb):
  - **Descriptors:** the Digitakt's layout (spec §2.1, §6). Interfaces 0–3 are vendor class;
    interfaces 1 and 2 have alts 0–3 on endpoints 0x83/0x03; interface 3 has bulk 0x02 and
    interrupt 0x82.
  - **Control:** vendor requests 1/2 (name) and OUT 1 (host name).
  - **Streaming:** an `OverbridgeSender`/`OverbridgeReceiver` pair over an `OverbridgeHandle`
    whose "rx" is from the host and "tx" is to the host. The sender waits for 7 frames the app
    has written (the device's audio clock paces it), encodes and writes a block.
- **`OverbridgeHandle::wait_tx_frames`**: an async wait, woken by `write`.
- **Harness:** `hw-tests/overbridge-device-firmware`, with a development identity by default
  and Elektron's VID only behind a non-default `elektron-vid` feature (spec §7.2).

**Tech Stack:** Rust no_std; embassy-usb 0.6; `rza1l-hal` (QEMU poked-MMIO tests);
`usb-classes` (`test_usb::FakeDriver`).

**Spec:** `docs/superpowers/specs/2026-10-15-overbridge-design.md` §2.1, §6, §7.2, §8.1 (device
class), §8.4 step 6.

## Global Constraints

- **Endpoint addresses:** audio IN `0x83` and OUT `0x03`, because Overwitch addresses them
  directly. Interface 3 uses `0x02` OUT bulk 512 and `0x82` IN interrupt 64.
- **Alt sizes:** alt 1 = 32 bytes; alt 2 = the 2-channel block (88 bytes) in both directions;
  alt 3 = `block_len` for the configured channels; `bInterval` 1.
- **Names:** vendor IN `bRequest` 1 and 2 return the name padded to 32 bytes; vendor OUT
  `bRequest` 1 stores the host's 32-byte name.
- **The class has no identity built in.** VID/PID come from the board. Elektron's VID only
  appears in a non-default feature.
- **Device-mode pipes:** an interrupt endpoint ≤ 64 keeps pipes 6–8. Over 64 it goes on a bulk
  pipe: MXPS 512 on 3–5/9–15 up to 512 bytes; pipes 1–2 at its own size above 512.
  Double-buffered for IN; no continuous mode.

## Review Focus

1. **Shared-endpoint growth across alts.** A 32-byte interrupt alt 1 must not leave the 368-byte
   alt 3 on a 64-byte interrupt pipe, and the moved-from pipe and buffer must be freed. Pinned
   by `a_shared_interrupt_endpoint_that_outgrows_its_pipe_moves`.
2. **The class must stream through the largest alt's handle.** Earlier alts' handles point at
   the abandoned pipe. Covered by the class keeping only the alt-3 endpoints (review the code).
3. **A host that never reads.** `send_block` waits on the endpoint, the app's `write` returns
   fewer frames, and nothing panics. Covered by the handle's tests (write returns taken frames).
4. **Vendor requests for other recipients** (interface or endpoint) or other request numbers
   must fall through (`None`), not be claimed. Pinned by `other_vendor_requests_are_not_claimed`.
5. **A short or malformed OUT block from the host** is counted as a framing error, never pushed.
   Pinned by `a_bad_block_from_the_host_is_counted_not_pushed`.

---

### Task 1: Device-mode interrupt-on-bulk in `rza1l-hal`

**Files:**
- Modify: `rza1/rza1l-hal/src/usb/driver.rs` (`PortAlloc::alloc_pipe`, a release helper,
  `device_layout`, `alloc_endpoint_in`, `alloc_endpoint_out`, tests)

**Interfaces:**
- Produces (module-private): `fn device_layout(ep_type: EndpointType, max_packet_size: u16) -> (XferType, u16, bool)`,
  returning (carry type, MXPS, needs pipes 1–2); and
  `PortAlloc::{alloc_pipe(xfer_type, wide: bool), release(pipe)}`.

- [ ] **Step 1: Write the failing tests**

Append to `driver.rs`'s `mod tests`:

```rust
    /// The allocation tests share port 1's allocator; run them one at a time.
    static ALLOC_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fresh_port1() -> Rusb1Driver<'static> {
        test::reset();
        unsafe { Rusb1Driver::new(1) }
    }

    fn pipe_cfg(pipe: u8) -> PipeConfig {
        critical_section::with(|cs| unsafe { (*PORT_ALLOC[1].borrow(cs).get()).pipe_cfg[pipe as usize].unwrap() })
    }

    fn pipes_used() -> u16 {
        critical_section::with(|cs| unsafe { (*PORT_ALLOC[1].borrow(cs).get()).pipes_used })
    }

    #[test]
    fn device_layout_carries_large_interrupt_endpoints_as_bulk() {
        assert_eq!(device_layout(EndpointType::Interrupt, 64), (XferType::Interrupt, 64, false));
        assert_eq!(device_layout(EndpointType::Interrupt, 368), (XferType::Bulk, 512, false));
        assert_eq!(device_layout(EndpointType::Interrupt, 1012), (XferType::Bulk, 1012, true));
        assert_eq!(device_layout(EndpointType::Bulk, 512), (XferType::Bulk, 512, false));
        assert_eq!(device_layout(EndpointType::Isochronous, 294), (XferType::Isochronous, 294, false));
    }

    #[test]
    fn a_368_byte_interrupt_in_endpoint_rides_a_double_buffered_bulk_pipe() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let ep = d.alloc_endpoint_in(EndpointType::Interrupt, Some(EndpointAddress::from(0x83)), 368, 1).unwrap();
        assert!(matches!(ep.pipe, 3..=5 | 9..=15), "pipe {}", ep.pipe);
        assert_eq!((ep.info.ep_type, ep.info.max_packet_size), (EndpointType::Interrupt, 368), "the descriptor keeps what the class asked");
        let c = pipe_cfg(ep.pipe);
        assert_eq!((c.xfer_type, c.mps, c.double_buf, c.continuous), (XferType::Bulk, 512, true, false));
        assert_eq!(ep.buf_bytes, 368, "one block per write");
    }

    #[test]
    fn an_88_byte_interrupt_out_endpoint_rides_a_bulk_pipe() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let ep = d.alloc_endpoint_out(EndpointType::Interrupt, Some(EndpointAddress::from(0x03)), 88, 1).unwrap();
        assert!(matches!(ep.pipe, 3..=5 | 9..=15), "pipe {}", ep.pipe);
        let c = pipe_cfg(ep.pipe);
        assert_eq!((c.xfer_type, c.mps), (XferType::Bulk, 512));
    }

    #[test]
    fn a_1012_byte_interrupt_endpoint_takes_pipe_1_or_2() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let ep = d.alloc_endpoint_in(EndpointType::Interrupt, None, 1012, 1).unwrap();
        assert!(matches!(ep.pipe, 1..=2), "pipe {}", ep.pipe);
        assert_eq!(pipe_cfg(ep.pipe).mps, 1012);
    }

    #[test]
    fn a_shared_interrupt_endpoint_that_outgrows_its_pipe_moves() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let addr = Some(EndpointAddress::from(0x83));
        let alt1 = d.alloc_endpoint_in(EndpointType::Interrupt, addr, 32, 1).unwrap();
        assert!(matches!(alt1.pipe, 6..=8), "a 32-byte interrupt endpoint starts on an interrupt pipe");
        let alt2 = d.alloc_endpoint_in(EndpointType::Interrupt, addr, 88, 1).unwrap();
        assert!(matches!(alt2.pipe, 3..=5 | 9..=15), "88 bytes needs a bulk pipe: pipe {}", alt2.pipe);
        assert_eq!(pipes_used() & (1 << alt1.pipe), 0, "the interrupt pipe is released");
        let alt3 = d.alloc_endpoint_in(EndpointType::Interrupt, addr, 368, 1).unwrap();
        assert_eq!(alt3.pipe, alt2.pipe, "368 bytes fits the same MXPS-512 pipe");
        assert_eq!(alt3.buf_bytes, 368);
        let map = critical_section::with(|cs| unsafe { (*PORT_ALLOC[1].borrow(cs).get()).ep_to_pipe[1][3] });
        assert_eq!(map, alt3.pipe);
    }

    #[test]
    fn small_interrupt_and_bulk_endpoints_are_unchanged() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let i = d.alloc_endpoint_in(EndpointType::Interrupt, None, 64, 1).unwrap();
        assert!(matches!(i.pipe, 6..=8));
        assert_eq!(pipe_cfg(i.pipe).xfer_type, XferType::Interrupt);
        let b = d.alloc_endpoint_out(EndpointType::Bulk, None, 512, 0).unwrap();
        assert!(matches!(b.pipe, 3..=5 | 9..=15));
        assert_eq!((pipe_cfg(b.pipe).xfer_type, pipe_cfg(b.pipe).mps), (XferType::Bulk, 512));
    }
```

These need `PipeConfig: Copy` and `XferType: Debug + PartialEq`. Check their derives in
`pipe.rs`, and add `Debug` if it's missing.

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rza1 && CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib usb::driver 2>&1 | grep -E "^error|test result|FAILED" | head
```
Expected: a compile error, because `device_layout` isn't found.

- [ ] **Step 3: Implement**

Next to `endpoint_type_to_xfer`, add:

```rust
/// How a device-mode endpoint is carried: the pipe type, the MXPS to programme, and whether
/// it needs pipes 1-2.
///
/// Interrupt pipes (6-8) stop at 64 bytes, so a larger interrupt endpoint (Overbridge's
/// 368-byte audio blocks) rides a bulk pipe; on the wire the host's interrupt transactions
/// look the same. Pipes 3-5 and 9-15 take only MXPS 8/16/32/64/512 (TRM §28.3.34), so up to
/// 512 bytes it is programmed at 512 (every packet short); above that only pipes 1-2 take the
/// size.
fn device_layout(ep_type: EndpointType, max_packet_size: u16) -> (XferType, u16, bool) {
    match ep_type {
        EndpointType::Interrupt if max_packet_size > 512 => (XferType::Bulk, max_packet_size, true),
        EndpointType::Interrupt if max_packet_size > 64 => (XferType::Bulk, 512, false),
        t => (endpoint_type_to_xfer(t), max_packet_size, false),
    }
}
```

Change `PortAlloc::alloc_pipe(&mut self, xfer_type: XferType, _is_in: bool)` to
`alloc_pipe(&mut self, xfer_type: XferType, wide: bool)`, and make the range match:

```rust
        let (start, end) = match xfer_type {
            XferType::Control => return Some(0),
            XferType::Isochronous => (1, 3), // pipes 1-2
            XferType::Bulk if wide => (1, 3), // over 512 bytes: only pipes 1-2 take it
            XferType::Bulk => (3, 16),       // pipes 3-5, 9-15 (6-8 skipped below)
            XferType::Interrupt => (6, 9),   // pipes 6-8
        };
```

Add to `impl PortAlloc`:

```rust
    /// Free `pipe`, its packet-buffer reservation and its configuration.
    fn release(&mut self, pipe: usize) {
        if let Some(c) = self.pipe_cfg[pipe].take() {
            let banks = if c.double_buf { 2 } else { 1 };
            self.buf.free(c.buf_start, c.buf_blocks as usize * banks);
        }
        self.pipes_used &= !(1u16 << pipe);
    }
```

In **both** `alloc_endpoint_out` and `alloc_endpoint_in`:

1. Replace `let xfer_type = endpoint_type_to_xfer(ep_type);` with
   `let (xfer_type, mxps, wide) = device_layout(ep_type, max_packet_size);`.

2. In the reuse branch (`if existing_pipe != 0 {`), before the `let cur = ...` grow logic,
   insert:

   ```rust
                    // A shared endpoint whose larger alt setting needs another kind of pipe
                    // (Overbridge's 32-byte alt 1 on an interrupt pipe, then 88 bytes) moves:
                    // release the old pipe and allocate afresh below. Earlier alts' endpoint
                    // handles keep the old pipe number; a class streams through the handle from
                    // its largest alt setting.
                    let fits = alloc.pipe_cfg[existing_pipe]
                        .as_ref()
                        .is_some_and(|c| c.xfer_type == xfer_type && (!wide || existing_pipe <= 2));
                    if !fits {
                        alloc.release(existing_pipe);
                        alloc.ep_to_pipe[DIR][ep_num as usize] = 0;
                    } else {
   ```

   Here `DIR` is the literal `0` in `alloc_endpoint_out` and `1` in `alloc_endpoint_in`. Close
   the new `else {` after the existing `return Ok(...)`, so a moving endpoint falls through to the
   fresh allocation. In the grow logic, compare and store `mxps` instead of `max_packet_size`:
   `&& mxps > cur_mps`, `mps_to_blocks(mxps)`, `c.mps = mxps;`. In `alloc_endpoint_in`'s reuse
   return, `buf_bytes` stays `max_packet_size`.

3. In the fresh allocation:
   - call `.alloc_pipe(xfer_type, wide)`;
   - use `mps_to_blocks(mxps)` for the buffer size;
   - store `mps: mxps` in `PipeConfig`.

   In `alloc_endpoint_in`:
   - `let double_buf = xfer_type == XferType::Isochronous || (xfer_type == XferType::Bulk && ep_type == EndpointType::Interrupt);`
   - the continuous-mode attempt runs only when `ep_type == EndpointType::Bulk` (add that to its
     `if`);
   - the non-continuous `buf_bytes` stays `max_packet_size`.

   `EndpointInfo` keeps `ep_type` and `max_packet_size` as requested.

- [ ] **Step 4: Run the tests to verify they pass**

Run the Step 2 command.
Expected: all `usb::driver` tests pass, including the two existing BEMP tests.

- [ ] **Step 5: Commit**

```bash
git add rza1/rza1l-hal/src/usb
git commit -m "rza1l-hal: device mode carries interrupt endpoints over 64 bytes on bulk pipes; shared endpoints move when they outgrow theirs"
```

---

### Task 2: `OverbridgeHandle::wait_tx_frames`

**Files:**
- Modify: `usb/usb-classes/src/overbridge/stems.rs`

**Interfaces:**
- Produces: `pub async fn wait_tx_frames(&self, frames: usize)`, which resolves once the send
  ring holds at least `frames`. `write` wakes it.

- [ ] **Step 1: Write the failing test**

Add to `stems.rs` tests:

```rust
    #[test]
    fn wait_tx_frames_resolves_once_the_app_has_written_enough() {
        use core::future::Future;
        use core::task::{Context, Poll};
        let h: OverbridgeHandle<1024, 256> = OverbridgeHandle::new();
        h.begin(info(12, 2), 12, 2).unwrap();
        let mut wait = core::pin::pin!(h.wait_tx_frames(7));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        assert!(wait.as_mut().poll(&mut cx).is_pending());
        h.write(&[0; 12]); // 6 frames
        assert!(wait.as_mut().poll(&mut cx).is_pending());
        h.write(&[0; 2]); // the 7th
        assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes \
  --features device-audio,device-midi,host-midi,host-uac,host-overbridge,device-overbridge --lib overbridge 2>&1 | grep -E "^error|test result" | head
```
Expected: a compile error, because `wait_tx_frames` isn't found.

- [ ] **Step 3: Implement**

In `stems.rs`, add `use embassy_sync::waitqueue::AtomicWaker;`. Add a `tx_waker: AtomicWaker`
field to `OverbridgeHandle`, initialised with `AtomicWaker::new()` in `new()`. Wake it at the
end of `write`:

```rust
    pub fn write(&self, inp: &[i32]) -> usize {
        let taken = self.with(|i| if i.info.is_some() { i.tx.push_some(inp) } else { 0 });
        if taken > 0 {
            self.tx_waker.wake();
        }
        taken
    }

    /// Wait until the send ring holds at least `frames` frames. A device's sender uses it to
    /// pace blocks by the app's audio clock.
    pub async fn wait_tx_frames(&self, frames: usize) {
        core::future::poll_fn(|cx| {
            self.tx_waker.register(cx.waker());
            if self.with(|i| i.tx.frames) >= frames {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        })
        .await
    }
```

- [ ] **Step 4: Run, then commit**

Run the Step 2 command. Expected: all overbridge tests pass.

```bash
git add usb/usb-classes/src/overbridge/stems.rs
git commit -m "usb-classes: OverbridgeHandle::wait_tx_frames, woken by write"
```

---

### Task 3: `overbridge::device`: descriptors and control requests

**Files:**
- Create: `usb/usb-classes/src/overbridge/device.rs`
- Modify: `usb/usb-classes/src/overbridge/mod.rs`
- Modify: `usb/usb-classes/Cargo.toml` (only if `embassy-usb` needs adding for the feature)

**Interfaces:**
- Produces (`usb_classes::overbridge::device`):
  - `pub struct OverbridgeConfig { pub format: Format, pub ch_to_host: u8, pub ch_from_host: u8, pub name: &'static str }`;
  - `pub struct State<'a>` with `pub const fn new() -> Self`;
  - `pub struct Shared` with `fn host_name(&self) -> [u8; 32]` and `fn streaming(&self) -> bool`;
  - `pub fn new<'d, D: Driver<'d>>(builder: &mut Builder<'d, D>, state: &'d mut State<'d>, config: OverbridgeConfig) -> (OverbridgeSender<D::EndpointIn>, OverbridgeReceiver<D::EndpointOut>, &'d Shared)`;
  - `OverbridgeSender<E>` / `OverbridgeReceiver<E>` (Task 4 adds their methods).

- [ ] **Step 1: Write the failing tests**

Create `device.rs` with only:

```rust
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::test_usb::{EpDesc, config_descriptor, endpoints};
    use embassy_usb::control::{Recipient, Request, RequestType};
    use embassy_usb::driver::Direction;
    use std::vec::Vec;

    fn digitakt() -> OverbridgeConfig {
        OverbridgeConfig { format: Format::V2, ch_to_host: 12, ch_from_host: 2, name: "Digitakt" }
    }

    fn ep(iface: u8, alt: u8, addr: u8, attributes: u8, max_packet: u16, interval: u8) -> EpDesc {
        EpDesc { iface, alt, addr, attributes, max_packet, interval }
    }

    /// Spec §2.1 / Appendix A, interfaces 0-3.
    #[test]
    fn the_descriptors_match_the_digitakts() {
        let desc = config_descriptor(|b| {
            let state = std::boxed::Box::leak(std::boxed::Box::new(State::new()));
            let _ = new(b, state, digitakt());
        });
        assert_eq!(
            endpoints(&desc),
            [
                ep(1, 1, 0x83, 3, 32, 1),
                ep(1, 2, 0x83, 3, 88, 1),
                ep(1, 3, 0x83, 3, 368, 1),
                ep(2, 1, 0x03, 3, 32, 1),
                ep(2, 2, 0x03, 3, 88, 1),
                ep(2, 3, 0x03, 3, 88, 1),
                ep(3, 0, 0x02, 2, 512, 0),
                ep(3, 0, 0x82, 3, 64, 1),
            ]
        );
        // Interfaces 0-3 are vendor class 0xFF, grouped by one IAD.
        let ifaces: Vec<(u8, u8, u8)> = desc
            .windows(9)
            .filter(|d| d[0] == 9 && d[1] == 4)
            .map(|d| (d[2], d[3], d[5]))
            .collect();
        assert_eq!(ifaces.iter().filter(|i| i.2 == 0xFF).count(), 1 + 4 + 4 + 1);
        assert!(desc.windows(8).any(|d| d[0] == 8 && d[1] == 0x0B && d[2] == 0 && d[3] == 4 && d[4] == 0xFF), "IAD over interfaces 0-3");
    }

    #[test]
    fn a_digitakt_ii_identity_sizes_alt_3_from_its_channels() {
        let desc = config_descriptor(|b| {
            let state = std::boxed::Box::leak(std::boxed::Box::new(State::new()));
            let _ = new(b, state, OverbridgeConfig { format: Format::V2_1, ch_to_host: 42, ch_from_host: 8, name: "Digitakt II" });
        });
        let alt3: Vec<(u8, u16)> = endpoints(&desc).iter().filter(|e| e.alt == 3).map(|e| (e.addr, e.max_packet)).collect();
        assert_eq!(alt3, [(0x83, 1012), (0x03, 256)]);
    }

    fn vendor(direction: Direction, request: u8, recipient: Recipient, length: u16) -> Request {
        Request { direction, request_type: RequestType::Vendor, recipient, request, value: 0, index: 0, length }
    }

    fn control() -> (Control<'static>, &'static Shared) {
        let shared: &'static Shared = std::boxed::Box::leak(std::boxed::Box::new(Shared::new()));
        (Control::new(shared, "Digitakt", InterfaceNumber(1), InterfaceNumber(2)), shared)
    }

    #[test]
    fn vendor_requests_1_and_2_return_the_name_padded_to_32_bytes() {
        let (mut c, _) = control();
        for r in [1, 2] {
            let mut buf = [0xAAu8; 64];
            let Some(InResponse::Accepted(data)) = c.control_in(vendor(Direction::In, r, Recipient::Device, 32), &mut buf) else {
                panic!("request {r} not answered");
            };
            assert_eq!(data.len(), 32);
            assert_eq!(&data[..8], b"Digitakt");
            assert!(data[8..].iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn vendor_out_request_1_stores_the_host_name() {
        let (mut c, shared) = control();
        let mut name = [0u8; 32];
        name[..5].copy_from_slice(b"Deluge");
        assert!(matches!(c.control_out(vendor(Direction::Out, 1, Recipient::Device, 32), &name), Some(OutResponse::Accepted)));
        assert_eq!(shared.host_name(), name);
    }

    #[test]
    fn other_vendor_requests_are_not_claimed() {
        let (mut c, _) = control();
        let mut buf = [0u8; 64];
        assert!(c.control_in(vendor(Direction::In, 3, Recipient::Device, 32), &mut buf).is_none());
        assert!(c.control_in(vendor(Direction::In, 1, Recipient::Interface, 32), &mut buf).is_none());
        assert!(c.control_out(vendor(Direction::Out, 2, Recipient::Device, 32), &[0; 32]).is_none());
    }

    #[test]
    fn streaming_needs_alt_3_on_both_audio_interfaces() {
        let (mut c, shared) = control();
        assert!(!shared.streaming());
        c.set_alternate_setting(InterfaceNumber(1), 3);
        assert!(!shared.streaming());
        c.set_alternate_setting(InterfaceNumber(2), 3);
        assert!(shared.streaming());
        c.set_alternate_setting(InterfaceNumber(1), 0);
        assert!(!shared.streaming());
        c.set_alternate_setting(InterfaceNumber(1), 3);
        c.reset();
        assert!(!shared.streaming(), "a bus reset drops the alternates");
    }
}
```

Note the "Deluge" in the host-name test is 6 bytes; use `name[..6]`.

- [ ] **Step 2: Run them to verify they fail**

Add to `overbridge/mod.rs`:

```rust
#[cfg(all(feature = "device-overbridge", any(target_os = "none", test)))]
pub mod device;
```

Run Task 2's Step 2 command with `--lib overbridge::device`.
Expected: compile errors (`State`, `new`, `Control`... not found).

- [ ] **Step 3: Implement**

Put above the tests in `device.rs`:

```rust
//! The Overbridge 2 **device** class (embassy-usb, chip-agnostic): what a Digitakt presents to
//! Overwitch, and what the Deluge host class binds.
//!
//! The layout copies the Digitakt's (spec §2.1). Interfaces 0-3 are vendor class under one IAD.
//! Interfaces 1 (to the host) and 2 (from it) carry alternate settings 0-3, with alt 3 the one
//! hosts select. Interface 3 is the parameter channel, which isn't implemented; it is present
//! so the interface numbers match. A board composes USB-MIDI after it (interfaces 4-5, as on
//! the Digitakt) and supplies the identity: VID/PID through the embassy-usb `Config`, and the
//! name and channel counts here.

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb::driver::{Direction, Driver, EndpointAddress};
use embassy_usb::types::InterfaceNumber;
use embassy_usb::{Builder, Handler};

use super::proto::{self, CounterTracker, Format, FRAMES_PER_BLOCK, MAX_BLOCK, MAX_CHANNELS};

const VENDOR_CLASS: u8 = 0xFF;
/// Audio endpoints' number: Overwitch addresses 0x83 and 0x03 directly.
const EP_AUDIO: usize = 3;
/// Interface 3's endpoints, as on the Digitakt.
const EP_AUX: usize = 2;
/// Vendor `bRequest` for the name (IN, and OUT to set the host's), and the second IN request.
const REQ_NAME: u8 = 1;
const REQ_NAME_2: u8 = 2;
/// Bytes of a name.
pub const NAME_LEN: usize = 32;

/// What the device presents, besides the VID/PID the board gives embassy-usb.
#[derive(Clone, Copy, Debug)]
pub struct OverbridgeConfig {
    pub format: Format,
    /// Channels this device sends to the host (Digitakt: 12).
    pub ch_to_host: u8,
    /// Channels this device receives from the host (Digitakt: 2).
    pub ch_from_host: u8,
    /// Returned by the name requests, padded to 32 bytes.
    pub name: &'static str,
}

/// State the control handler and the board share.
pub struct Shared {
    host_name: Mutex<CriticalSectionRawMutex, RefCell<[u8; NAME_LEN]>>,
    alt_in: AtomicU8,
    alt_out: AtomicU8,
}

impl Shared {
    pub const fn new() -> Self {
        Self { host_name: Mutex::new(RefCell::new([0; NAME_LEN])), alt_in: AtomicU8::new(0), alt_out: AtomicU8::new(0) }
    }

    /// The name the host last set (vendor OUT request 1), or zeros.
    pub fn host_name(&self) -> [u8; NAME_LEN] {
        self.host_name.lock(|n| *n.borrow())
    }

    /// The host has selected alternate setting 3 on both audio interfaces.
    pub fn streaming(&self) -> bool {
        self.alt_in.load(Ordering::Relaxed) == proto::STREAM_ALT && self.alt_out.load(Ordering::Relaxed) == proto::STREAM_ALT
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// The class's control handler.
pub struct Control<'a> {
    shared: &'a Shared,
    name: [u8; NAME_LEN],
    if_in: InterfaceNumber,
    if_out: InterfaceNumber,
}

impl<'a> Control<'a> {
    fn new(shared: &'a Shared, name: &str, if_in: InterfaceNumber, if_out: InterfaceNumber) -> Self {
        let mut padded = [0u8; NAME_LEN];
        let n = name.len().min(NAME_LEN);
        padded[..n].copy_from_slice(&name.as_bytes()[..n]);
        Self { shared, name: padded, if_in, if_out }
    }

    fn is_ours(req: &Request) -> bool {
        req.request_type == RequestType::Vendor && req.recipient == Recipient::Device
    }
}

impl Handler for Control<'_> {
    fn reset(&mut self) {
        self.shared.alt_in.store(0, Ordering::Relaxed);
        self.shared.alt_out.store(0, Ordering::Relaxed);
    }

    fn set_alternate_setting(&mut self, iface: InterfaceNumber, alternate_setting: u8) {
        if iface == self.if_in {
            self.shared.alt_in.store(alternate_setting, Ordering::Relaxed);
        } else if iface == self.if_out {
            self.shared.alt_out.store(alternate_setting, Ordering::Relaxed);
        }
    }

    fn control_out(&mut self, req: Request, data: &[u8]) -> Option<OutResponse> {
        if !Self::is_ours(&req) || req.request != REQ_NAME {
            return None;
        }
        let mut name = [0u8; NAME_LEN];
        let n = data.len().min(NAME_LEN);
        name[..n].copy_from_slice(&data[..n]);
        self.shared.host_name.lock(|h| *h.borrow_mut() = name);
        Some(OutResponse::Accepted)
    }

    fn control_in<'b>(&'b mut self, req: Request, buf: &'b mut [u8]) -> Option<InResponse<'b>> {
        if !Self::is_ours(&req) || !matches!(req.request, REQ_NAME | REQ_NAME_2) {
            return None;
        }
        let n = NAME_LEN.min(buf.len()).min(req.length as usize);
        buf[..n].copy_from_slice(&self.name[..n]);
        Some(InResponse::Accepted(&buf[..n]))
    }
}

/// Storage for the class; give it a `'static` (or `'d`) home, as with embassy-usb's classes.
pub struct State<'a> {
    control: MaybeUninit<Control<'a>>,
    shared: Shared,
}

impl State<'_> {
    pub const fn new() -> Self {
        Self { control: MaybeUninit::uninit(), shared: Shared::new() }
    }
}

impl Default for State<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Sends blocks to the host (interface 1, endpoint 0x83).
pub struct OverbridgeSender<E> {
    ep: E,
    format: Format,
    ch: usize,
    counter: u16,
    block: [u8; MAX_BLOCK],
    frames: [i32; FRAMES_PER_BLOCK * MAX_CHANNELS],
}

/// Receives blocks from the host (interface 2, endpoint 0x03).
pub struct OverbridgeReceiver<E> {
    ep: E,
    format: Format,
    ch: usize,
    counter: CounterTracker,
    block: [u8; MAX_BLOCK],
    frames: [i32; FRAMES_PER_BLOCK * MAX_CHANNELS],
}

/// Add the Overbridge interfaces (0-3) to `builder`. Returns the sender, the receiver, and the
/// shared state. Stream through these handles: they are the largest alternate setting's, and
/// the driver may have moved the endpoint's pipe when that alt was added.
pub fn new<'d, D: Driver<'d>>(
    builder: &mut Builder<'d, D>,
    state: &'d mut State<'d>,
    config: OverbridgeConfig,
) -> (OverbridgeSender<D::EndpointIn>, OverbridgeReceiver<D::EndpointOut>, &'d Shared) {
    let (to_host, from_host) = (config.ch_to_host as usize, config.ch_from_host as usize);
    assert!(to_host <= MAX_CHANNELS && from_host <= MAX_CHANNELS, "an Overbridge block carries at most 64 channels");
    let in_sizes = [proto::HEADER_LEN, proto::block_len(config.format.from_device_frame_bytes(2)), proto::block_len(config.format.from_device_frame_bytes(to_host))];
    let out_sizes = [proto::HEADER_LEN, proto::block_len(config.format.to_device_frame_bytes(2)), proto::block_len(config.format.to_device_frame_bytes(from_host))];
    let ep_in_addr = EndpointAddress::from_parts(EP_AUDIO, Direction::In);
    let ep_out_addr = EndpointAddress::from_parts(EP_AUDIO, Direction::Out);

    let mut func = builder.function(VENDOR_CLASS, 0, 0);
    // Interface 0: no endpoints.
    func.interface().alt_setting(VENDOR_CLASS, 0, 0, None);
    // Interface 1: audio to the host, alts 0-3.
    let mut i1 = func.interface();
    let if_in = i1.interface_number();
    i1.alt_setting(VENDOR_CLASS, 0, 0, None);
    let mut ep_in = None;
    for size in in_sizes {
        ep_in = Some(i1.alt_setting(VENDOR_CLASS, 0, 0, None).endpoint_interrupt_in(Some(ep_in_addr), size as u16, 1));
    }
    // Interface 2: audio from the host, alts 0-3.
    let mut i2 = func.interface();
    let if_out = i2.interface_number();
    i2.alt_setting(VENDOR_CLASS, 0, 0, None);
    let mut ep_out = None;
    for size in out_sizes {
        ep_out = Some(i2.alt_setting(VENDOR_CLASS, 0, 0, None).endpoint_interrupt_out(Some(ep_out_addr), size as u16, 1));
    }
    // Interface 3: the parameter channel (not implemented), present so the numbering matches.
    let mut i3 = func.interface();
    let mut a3 = i3.alt_setting(VENDOR_CLASS, 0, 0, None);
    let _ = a3.endpoint_bulk_out(Some(EndpointAddress::from_parts(EP_AUX, Direction::Out)), 512);
    let _ = a3.endpoint_interrupt_in(Some(EndpointAddress::from_parts(EP_AUX, Direction::In)), 64, 1);
    drop(func);

    let State { control, shared } = state;
    let shared: &'d Shared = shared;
    let control = control.write(Control::new(shared, config.name, if_in, if_out));
    builder.handler(control);

    let sender = OverbridgeSender { ep: ep_in.unwrap(), format: config.format, ch: to_host, counter: 0, block: [0; MAX_BLOCK], frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS] };
    let receiver = OverbridgeReceiver { ep: ep_out.unwrap(), format: config.format, ch: from_host, counter: CounterTracker::default(), block: [0; MAX_BLOCK], frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS] };
    (sender, receiver, shared)
}
```

If the `alt_setting(...).endpoint_*` chaining borrows awkwardly, bind each alt to a `let mut alt`
first. If `builder.function(..)` returns a builder whose drop order matters, follow the borrow
checker. The alt order (0, then 1–3 in `in_sizes` order) is what the test asserts.

- [ ] **Step 4: Run the tests to verify they pass**

Run the Step 2 command.
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add usb/usb-classes/src/overbridge usb/usb-classes/Cargo.toml
git commit -m "usb-classes: overbridge::device, the Overbridge 2 device class's descriptors and control requests"
```

---

### Task 4: Device streaming: `OverbridgeSender` and `OverbridgeReceiver`

**Files:**
- Modify: `usb/usb-classes/src/overbridge/device.rs`

**Interfaces:**
- Produces:
  - `OverbridgeSender<E: EndpointIn>`:
    - `async fn wait_enabled(&mut self)`;
    - `async fn send_block<const RX: usize, const TX: usize>(&mut self, h: &OverbridgeHandle<RX, TX>) -> Result<(), EndpointError>`.
  - `OverbridgeReceiver<E: EndpointOut>`:
    - `async fn wait_enabled(&mut self)`;
    - `async fn receive_block<const RX: usize, const TX: usize>(&mut self, h: &OverbridgeHandle<RX, TX>) -> Result<(), EndpointError>`.
  - For the device, the handle's "rx" is from the host and "tx" is to the host.

- [ ] **Step 1: Write the failing tests**

Add to `device.rs` tests:

```rust
    use crate::overbridge::stems::{OverbridgeHandle, OverbridgeInfo};
    use core::cell::RefCell as Cell;
    use embassy_futures::block_on;
    use embassy_usb::driver::{Endpoint, EndpointError, EndpointIn, EndpointInfo, EndpointOut, EndpointType};

    /// An endpoint that records writes and replays scripted reads.
    struct Scripted {
        info: EndpointInfo,
        written: Cell<Vec<Vec<u8>>>,
        reads: Cell<Vec<Vec<u8>>>,
    }

    impl Scripted {
        fn new(addr: u8) -> Self {
            Self {
                info: EndpointInfo { addr: addr.into(), ep_type: EndpointType::Interrupt, max_packet_size: 368, interval_ms: 1 },
                written: Cell::new(Vec::new()),
                reads: Cell::new(Vec::new()),
            }
        }
    }

    impl Endpoint for Scripted {
        fn info(&self) -> &EndpointInfo {
            &self.info
        }
        async fn wait_enabled(&mut self) {}
    }

    impl EndpointIn for Scripted {
        async fn write(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
            self.written.borrow_mut().push(buf.to_vec());
            Ok(())
        }
    }

    impl EndpointOut for Scripted {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
            let mut reads = self.reads.borrow_mut();
            if reads.is_empty() {
                return Err(EndpointError::Disabled);
            }
            let r = reads.remove(0);
            buf[..r.len()].copy_from_slice(&r);
            Ok(r.len())
        }
    }

    fn device_handle() -> OverbridgeHandle<1024, 1024> {
        let h = OverbridgeHandle::new();
        // The device receives `ch_from_host` (2) and sends `ch_to_host` (12).
        h.begin(OverbridgeInfo { pid: 0, name: "Digitakt", ch_from: 12, ch_to: 2, format: Format::V2 }, 2, 12).unwrap();
        h
    }

    #[test]
    fn a_block_goes_to_the_host_once_seven_frames_are_written() {
        let h = device_handle();
        let mut s = OverbridgeSender { ep: Scripted::new(0x83), format: Format::V2, ch: 12, counter: 0, block: [0; MAX_BLOCK], frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS] };
        let frames: Vec<i32> = (0..7 * 12).map(|i| (i as i32) << 12).collect();
        assert_eq!(h.write(&frames), 7);
        block_on(s.send_block(&h)).unwrap();
        block_on(s.send_block(&h).or_else_never()).ok();
        let written = s.ep.written.borrow();
        assert_eq!(written.len(), 1, "the second waits for 7 more frames");
        let mut back = [0i32; 7 * 12];
        assert_eq!(proto::decode_from_device(Format::V2, 12, &written[0], &mut back), Ok(0));
        assert_eq!(&back[..], &frames[..]);
        assert_eq!(h.stats().blocks_out, 1);
    }

    #[test]
    fn blocks_from_the_host_reach_the_handle() {
        let h = device_handle();
        let mut r = OverbridgeReceiver { ep: Scripted::new(0x03), format: Format::V2, ch: 2, counter: CounterTracker::default(), block: [0; MAX_BLOCK], frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS] };
        let frames: Vec<i32> = (0..14).map(|i| i << 20).collect();
        let mut block = [0u8; 88];
        proto::encode_to_device(Format::V2, 2, 7, &frames, &mut block);
        r.ep.reads.borrow_mut().push(block.to_vec());
        block_on(r.receive_block(&h)).unwrap();
        let mut out = [0i32; 14];
        assert_eq!(h.read(&mut out), 7);
        assert_eq!(&out[..], &frames[..]);
        assert_eq!(h.stats().blocks_in, 1);
    }

    #[test]
    fn a_bad_block_from_the_host_is_counted_not_pushed() {
        let h = device_handle();
        let mut r = OverbridgeReceiver { ep: Scripted::new(0x03), format: Format::V2, ch: 2, counter: CounterTracker::default(), block: [0; MAX_BLOCK], frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS] };
        r.ep.reads.borrow_mut().push(std::vec![0x07, 0x00, 0, 0]); // short, and a device-to-host header
        block_on(r.receive_block(&h)).unwrap();
        assert_eq!(h.stats().framing_errors, 1);
        assert_eq!(h.read(&mut [0; 2]), 0);
        assert!(matches!(block_on(r.receive_block(&h)), Err(EndpointError::Disabled)));
    }
```

`or_else_never` isn't a real combinator. For the "second send waits" check, poll once by hand
instead:

```rust
        {
            use core::future::Future;
            let mut second = core::pin::pin!(s.send_block(&h));
            let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
            assert!(second.as_mut().poll(&mut cx).is_pending());
        }
```

Use that block in place of the `or_else_never` line.

- [ ] **Step 2: Run them to verify they fail**

Run Task 2's Step 2 command.
Expected: compile errors (`send_block`, `receive_block` not found).

- [ ] **Step 3: Implement**

Add to `device.rs` above the tests:

```rust
use embassy_usb::driver::{EndpointError, EndpointIn, EndpointOut};

use super::stems::OverbridgeHandle;

impl<E: EndpointIn> OverbridgeSender<E> {
    /// Wait until the host has enabled the endpoint (selected alt 1-3 on interface 1).
    pub async fn wait_enabled(&mut self) {
        self.ep.wait_enabled().await;
    }

    /// Send the next 7 frames the app has written to `h` as one block. Waits until there are 7,
    /// so the app's audio clock paces the stream (the device is the clock).
    pub async fn send_block<const RX: usize, const TX: usize>(&mut self, h: &OverbridgeHandle<RX, TX>) -> Result<(), EndpointError> {
        h.wait_tx_frames(FRAMES_PER_BLOCK).await;
        let n = FRAMES_PER_BLOCK * self.ch;
        h.pull_tx(&mut self.frames[..n]);
        let len = proto::encode_from_device(self.format, self.ch, self.counter, &self.frames[..n], &mut self.block);
        self.ep.write(&self.block[..len]).await?;
        self.counter = self.counter.wrapping_add(FRAMES_PER_BLOCK as u16);
        h.update_stats(|s| s.blocks_out = s.blocks_out.wrapping_add(1));
        Ok(())
    }
}

impl<E: EndpointOut> OverbridgeReceiver<E> {
    /// Wait until the host has enabled the endpoint (selected alt 1-3 on interface 2).
    pub async fn wait_enabled(&mut self) {
        self.ep.wait_enabled().await;
    }

    /// Receive one block from the host into `h`. A malformed block is counted, not pushed.
    pub async fn receive_block<const RX: usize, const TX: usize>(&mut self, h: &OverbridgeHandle<RX, TX>) -> Result<(), EndpointError> {
        let got = self.ep.read(&mut self.block).await?;
        let n = FRAMES_PER_BLOCK * self.ch;
        match proto::decode_to_device(self.format, self.ch, &self.block[..got], &mut self.frames[..n]) {
            Ok(counter) => {
                let gaps = self.counter.observe(counter);
                h.push_rx(&self.frames[..n]);
                h.update_stats(|s| {
                    s.blocks_in = s.blocks_in.wrapping_add(1);
                    s.counter_gaps = s.counter_gaps.saturating_add(gaps);
                });
            }
            Err(_) => h.update_stats(|s| s.framing_errors = s.framing_errors.saturating_add(1)),
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run, then commit**

Run Task 2's Step 2 command. Expected: all pass.

```bash
git add usb/usb-classes/src/overbridge/device.rs
git commit -m "usb-classes: Overbridge device streaming (sender paced by the app's audio, receiver)"
```

---

### Task 5: Device harness, docs and suites

**Files:**
- Create: `hw-tests/overbridge-device-firmware/` (`Cargo.toml`, `build.rs`, `src/main.rs`,
  `src/tasks/{mod,blink,pic,stems}.rs`)
- Modify: root `Cargo.toml` members, `.cargo/config.toml` aliases
- Modify: `docs/hardware-tasks.md` (task 6 names the harness and its features)
- Modify: `CHANGELOG.md`

- [ ] **Step 1: The harness**

Base it on `hw-tests/overbridge-host-firmware` (same bring-up), but in device mode, following
`firmwares/demo-firmware/src/main.rs`'s device branch: `init_device_mode`, an `embassy_usb::Config`
and `Builder` with static descriptor buffers, and a task running `UsbDevice::run`.
- **Identity:** `0x1209:0x0001` (pid.codes test PID, "not for distribution") by default. With the
  non-default feature `elektron-vid`, use `0x1935:0x0BF0`, an Elektron VID with a PID no device
  table lists. Document on the feature that it is for the developer's own bench (spec §7.2).
- **Config:** device class `0xEF/0x02/0x01`, `composite_with_iads = true`, self-powered,
  `max_power = 0`.
- **Classes:** the Overbridge class with `OverbridgeConfig { format: V2, ch_to_host: 12,
  ch_from_host: 2, name: "Deluge" }`, then `embassy_usb::class::midi::MidiClass::new(&mut builder, 1, 1, 512)`
  for interfaces 4–5.
- **Tasks:**
  - a sender task: `wait_enabled`, then `send_block` in a loop, re-waiting on error;
  - a receiver task, the same with `receive_block`;
  - a stems task that writes 48 frames of 12 channels every 1 ms, channel k being a DC level
    `(k + 1) << 24`. This is 48 000 frames/s by `embassy_time` (use an `Instant`-based frame
    budget so it doesn't drift);
  - a report task logging the handle's stats and the host name each second.
- **Handle:** a static `OverbridgeHandle<{2 * 1024}, {12 * 1024}>`. Call `begin` at start, with
  `OverbridgeInfo { pid: 0, name: "Deluge", ch_from: 12, ch_to: 2, format: V2 }` and rx 2 / tx 12.

Build it both ways:

```bash
cargo build -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem --release --no-default-features -p overbridge-device-firmware 2>&1 | grep -E "^error|Finished"
cargo build -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem --release --no-default-features -p overbridge-device-firmware --features elektron-vid 2>&1 | grep -E "^error|Finished"
```
Expected: both finish.

- [ ] **Step 2: Docs, CHANGELOG, commit**

In `docs/hardware-tasks.md` task 6, name the harness (`cargo build-overbridge-device
--features elektron-vid`), and note that its default identity won't bind to the Deluge host class
or Overwitch.

CHANGELOG `### Added`:

```markdown
- Elektron Overbridge 2 device class (`usb-classes` feature `device-overbridge`,
  `overbridge::device`). It has the Digitakt's interface layout and name requests; the board
  supplies the identity and channel counts. The RZ/A1L device driver carries interrupt
  endpoints over 64 bytes on bulk pipes.
```

```bash
git add hw-tests/overbridge-device-firmware Cargo.toml Cargo.lock .cargo/config.toml docs/hardware-tasks.md CHANGELOG.md
git commit -m "hw-tests: overbridge-device-firmware (development identity; Elektron VID behind elektron-vid)"
```

- [ ] **Step 3: Suites**

Run `rza1/tools/test.sh` and `tools/test.sh` (the latter with the known flare exception), and
record the results.
