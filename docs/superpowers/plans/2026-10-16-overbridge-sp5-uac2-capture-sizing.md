# Overbridge sub-project 5: UAC2 device capture sizing for 8 channels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `AudioClass<8>` compiles and advertises a capture endpoint that fits high speed, so a
Deluge can present 8 channels to an Outbox 8. Existing 2-channel builds keep their descriptors
byte for byte.

**Architecture:**
- **Packet size.** One associated const, `AudioClass::<CH>::CAPTURE_MAX_PACKET`, decides the
  capture ISO IN max packet:
  - the existing 49-frame figure (`49 × CH × 3`) while it fits in 1024 bytes;
  - otherwise the high-speed per-microframe figure, `(ceil(48000 / 8000) + 1) × CH × 3`, which
    is `7 × CH × 3`.
- **Test driver.** A do-nothing embassy-usb driver (`test_usb`, test-only) lets descriptor tests
  run under the QEMU test bucket. Sub-project 4 reuses it.

**Tech Stack:** Rust, embassy-usb 0.6, `usb-classes`; tests run in `tools/test.sh`'s QEMU bucket.

**Spec:** `docs/superpowers/specs/2026-10-15-overbridge-design.md` §7.1, §8.3 and §8.4 step 5.

## Global Constraints

- **Formula.** The high-speed size is `(ceil(rate / 8000) + 1) × CAPTURE_CH × subslot`. At
  48 kHz and 24-bit that is 7 × 24 = 168 bytes for 8 channels; 44.1 kHz gives the same 7 frames.
- **Limit.** The assert bounds the high-speed size at ≤ 1024.
- **No regression.** 2-channel builds keep 294 bytes.
  - Ruling (spec deviation): the spec keeps the 49-frame figure only for full speed. embassy-usb
    builds one descriptor set, and the RUSB1 device always runs high speed.
  - The controller firmware's mic writer caps packets at its 288-byte buffer, not at the
    endpoint's max packet size. Shrinking the 2-channel endpoint would let it overrun.
  - So the 49-frame size stays wherever it fits, and the high-speed size applies only above
    1024 bytes.

## Review Focus

1. **Byte-identical 2-channel descriptors.** Both capture alts stay at 294 bytes. Pinned by
   `two_channel_capture_keeps_the_49_frame_packet`.
2. **Writers must cap at the max packet size.** A writer that ignores the endpoint's max packet
   size would overrun at 8 channels. Doc comment on `CAPTURE_MAX_PACKET`; the controller
   firmware stays 2-channel.
3. **The largest channel count that compiles.** `7 × CH × 3 ≤ 1024` gives CH ≤ 48. Pinned by
   the const assert. A test can't assert a compile error, so this is review-only.

---

### Task 1: Test driver and characterization of today's capture descriptor

**Files:**
- Create: `usb/usb-classes/src/test_usb.rs`
- Modify: `usb/usb-classes/src/lib.rs` (gate `audio` on the feature alone; add `test_usb` under
  `cfg(test)`)
- Modify: `usb/usb-classes/src/audio.rs` (test module)
- Modify: `tools/test.sh:49` (add `device-audio` to the usb-classes test features)

**Interfaces:**
- Produces (test-only, `crate::test_usb`):
  - `pub struct FakeDriver`;
  - `pub fn config_descriptor(f: impl FnOnce(&mut embassy_usb::Builder<'_, FakeDriver>)) -> Vec<u8>`;
  - `pub struct EpDesc { pub iface: u8, pub alt: u8, pub addr: u8, pub attributes: u8, pub max_packet: u16, pub interval: u8 }`;
  - `pub fn endpoints(desc: &[u8]) -> Vec<EpDesc>`.

- [ ] **Step 1: Write the test driver**

```rust
//! A do-nothing embassy-usb driver for descriptor tests. It hands out endpoints with the
//! requested type and size, and nothing it returns is ever polled.

extern crate std;
use std::vec::Vec;

use embassy_usb::driver::{
    Bus, ControlPipe, Direction, Driver, Endpoint, EndpointAddress, EndpointAllocError,
    EndpointError, EndpointIn, EndpointInfo, EndpointOut, EndpointType, Event, Unsupported,
};
use embassy_usb::{Builder, Config};

pub struct FakeDriver {
    next: [u8; 2],
}

impl FakeDriver {
    pub fn new() -> Self {
        Self { next: [1, 1] }
    }

    fn alloc(&mut self, dir: Direction, ep_type: EndpointType, addr: Option<EndpointAddress>, max_packet_size: u16, interval_ms: u8) -> FakeEp {
        let addr = addr.unwrap_or_else(|| {
            let n = &mut self.next[(dir == Direction::In) as usize];
            *n += 1;
            EndpointAddress::from_parts((*n - 1) as usize, dir)
        });
        FakeEp(EndpointInfo { addr, ep_type, max_packet_size, interval_ms })
    }
}

pub struct FakeEp(EndpointInfo);

impl Endpoint for FakeEp {
    fn info(&self) -> &EndpointInfo {
        &self.0
    }
    async fn wait_enabled(&mut self) {
        unreachable!("descriptor tests never run the device")
    }
}

impl EndpointIn for FakeEp {
    async fn write(&mut self, _: &[u8]) -> Result<(), EndpointError> {
        unreachable!("descriptor tests never run the device")
    }
}

impl EndpointOut for FakeEp {
    async fn read(&mut self, _: &mut [u8]) -> Result<usize, EndpointError> {
        unreachable!("descriptor tests never run the device")
    }
}

pub struct FakeBus;

impl Bus for FakeBus {
    async fn enable(&mut self) {}
    async fn disable(&mut self) {}
    async fn poll(&mut self) -> Event {
        unreachable!("descriptor tests never run the device")
    }
    fn endpoint_set_enabled(&mut self, _: EndpointAddress, _: bool) {}
    fn endpoint_set_stalled(&mut self, _: EndpointAddress, _: bool) {}
    fn endpoint_is_stalled(&mut self, _: EndpointAddress) -> bool {
        false
    }
    async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
        Err(Unsupported)
    }
}

pub struct FakeControl;

impl ControlPipe for FakeControl {
    fn max_packet_size(&self) -> usize {
        64
    }
    async fn setup(&mut self) -> [u8; 8] {
        unreachable!("descriptor tests never run the device")
    }
    async fn data_out(&mut self, _: &mut [u8], _: bool, _: bool) -> Result<usize, EndpointError> {
        unreachable!("descriptor tests never run the device")
    }
    async fn data_in(&mut self, _: &[u8], _: bool, _: bool) -> Result<(), EndpointError> {
        unreachable!("descriptor tests never run the device")
    }
    async fn accept(&mut self) {}
    async fn reject(&mut self) {}
    async fn accept_set_address(&mut self, _: u8) {}
}

impl<'a> Driver<'a> for FakeDriver {
    type EndpointOut = FakeEp;
    type EndpointIn = FakeEp;
    type ControlPipe = FakeControl;
    type Bus = FakeBus;

    fn alloc_endpoint_out(&mut self, ep_type: EndpointType, ep_addr: Option<EndpointAddress>, max_packet_size: u16, interval_ms: u8) -> Result<FakeEp, EndpointAllocError> {
        Ok(self.alloc(Direction::Out, ep_type, ep_addr, max_packet_size, interval_ms))
    }
    fn alloc_endpoint_in(&mut self, ep_type: EndpointType, ep_addr: Option<EndpointAddress>, max_packet_size: u16, interval_ms: u8) -> Result<FakeEp, EndpointAllocError> {
        Ok(self.alloc(Direction::In, ep_type, ep_addr, max_packet_size, interval_ms))
    }
    fn start(self, _: u16) -> (FakeBus, FakeControl) {
        (FakeBus, FakeControl)
    }
}

/// Build an IAD-composite device with `f` adding its classes, and return the configuration
/// descriptor (wTotalLength bytes).
pub fn config_descriptor(f: impl FnOnce(&mut Builder<'_, FakeDriver>)) -> Vec<u8> {
    let mut cfg = [0u8; 2048];
    let mut bos = [0u8; 256];
    let mut msos = [0u8; 256];
    let mut ctl = [0u8; 256];
    let mut config = Config::new(0x1209, 0x0001);
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;
    {
        let mut builder = Builder::new(FakeDriver::new(), config, &mut cfg, &mut bos, &mut msos, &mut ctl);
        f(&mut builder);
        drop(builder.build());
    }
    let total = u16::from_le_bytes([cfg[2], cfg[3]]) as usize;
    cfg[..total].to_vec()
}

/// One endpoint descriptor, with the interface and alternate setting it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpDesc {
    pub iface: u8,
    pub alt: u8,
    pub addr: u8,
    pub attributes: u8,
    pub max_packet: u16,
    pub interval: u8,
}

/// Every endpoint descriptor in a configuration descriptor, in order.
pub fn endpoints(desc: &[u8]) -> Vec<EpDesc> {
    let (mut iface, mut alt) = (0, 0);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < desc.len() && desc[i] >= 2 {
        let d = &desc[i..i + desc[i] as usize];
        match d[1] {
            4 => (iface, alt) = (d[2], d[3]),
            5 => out.push(EpDesc {
                iface,
                alt,
                addr: d[2],
                attributes: d[3],
                max_packet: u16::from_le_bytes([d[4], d[5]]),
                interval: d[6],
            }),
            _ => {}
        }
        i += d.len();
    }
    out
}
```

In `lib.rs`, change `#[cfg(all(target_os = "none", feature = "device-audio"))]` above
`pub mod audio;` to `#[cfg(feature = "device-audio")]`. Then add:

```rust
#[cfg(test)]
mod test_usb;
```

In `tools/test.sh:49`, change `--features device-midi,host-midi,host-uac` to
`--features device-audio,device-midi,host-midi,host-uac`.

- [ ] **Step 2: Write the characterization test**

Append to `audio.rs`:

```rust
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::test_usb::{config_descriptor, endpoints};
    use std::vec::Vec;

    /// The capture (ISO IN) endpoint's wMaxPacketSize in each alternate setting.
    fn capture_max_packets<const CH: usize>() -> Vec<u16> {
        let desc = config_descriptor(|b| {
            let _ = AudioClass::<CH>::new(b, 288);
        });
        endpoints(&desc)
            .into_iter()
            .filter(|e| e.addr & 0x80 != 0 && e.attributes & 0x03 == 0x01)
            .map(|e| e.max_packet)
            .collect()
    }

    /// Existing 2-channel builds keep their descriptor: 49 frames × 2 ch × 3 bytes in both alts.
    #[test]
    fn two_channel_capture_keeps_the_49_frame_packet() {
        assert_eq!(capture_max_packets::<2>(), [294, 294]);
    }
}
```

- [ ] **Step 3: Run it**

Run (repo root):
```bash
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes --features device-audio,device-midi,host-midi,host-uac --lib audio 2>&1 | tail -5
```
Expected: PASS. This test characterizes existing behaviour, and the harness is test
infrastructure, so there is no RED here; Task 2 has the RED.

- [ ] **Step 4: Commit**

```bash
git add usb/usb-classes/src/test_usb.rs usb/usb-classes/src/lib.rs usb/usb-classes/src/audio.rs tools/test.sh
git commit -m "usb-classes: descriptor test driver; AudioClass builds and is tested off-target"
```

---

### Task 2: High-speed capture sizing above 1024 bytes

**Files:**
- Modify: `usb/usb-classes/src/audio.rs` (`AudioClass::new`, its doc, the two capture endpoint
  allocations, the tests)
- Modify: `CHANGELOG.md`

**Interfaces:**
- Produces: `pub const CAPTURE_MAX_PACKET: u16` on `impl<const CAPTURE_CH: usize> AudioClass<CAPTURE_CH>`.

- [ ] **Step 1: Write the failing tests**

Add to `audio.rs`'s `mod tests`:

```rust
    /// 8 channels × 49 frames × 3 bytes = 1176 bytes is over the 1024-byte high-speed limit, so
    /// the endpoint is sized per microframe: (ceil(48000/8000) + 1) = 7 frames × 8 × 3 = 168.
    #[test]
    fn eight_channel_capture_is_sized_per_microframe() {
        assert_eq!(capture_max_packets::<8>(), [168, 168]);
    }

    #[test]
    fn the_capture_max_packet_constant_matches_the_descriptor() {
        assert_eq!(AudioClass::<2>::CAPTURE_MAX_PACKET, 294);
        assert_eq!(AudioClass::<6>::CAPTURE_MAX_PACKET, 882);
        assert_eq!(AudioClass::<8>::CAPTURE_MAX_PACKET, 168);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run the Task 1 Step 3 command.
Expected: a compile error. `CAPTURE_MAX_PACKET` isn't found, and `AudioClass::<8>` trips the
const assert ("capture ISO IN max packet exceeds the 1024-byte high-speed limit").

- [ ] **Step 3: Implement**

In `impl<const CAPTURE_CH: usize> AudioClass<CAPTURE_CH>`, add above `new`:

```rust
    /// The capture (ISO IN) endpoint's wMaxPacketSize, in both alternate settings.
    ///
    /// 49 frames × `CAPTURE_CH` × 3 bytes while that fits in 1024 bytes, which covers a 1 ms
    /// packet with a stuffing frame at full speed and over-provisions at high speed. Above that
    /// the endpoint is sized per high-speed microframe: (ceil(48000 / 8000) + 1) = 7 frames,
    /// which also covers 44.1 kHz (6 frames plus 1). The RUSB1 device always runs at high speed
    /// (SYSCFG.HSE), so an 8-channel build depends on it.
    ///
    /// Writers must cap each packet at this size (or at the endpoint's
    /// `info().max_packet_size`): a writer that sends the whole SSI backlog would overrun it.
    pub const CAPTURE_MAX_PACKET: u16 = {
        let full_speed = 49 * CAPTURE_CH * 3;
        let high_speed = (48_000usize.div_ceil(8_000) + 1) * CAPTURE_CH * 3;
        assert!(
            high_speed <= 1024,
            "capture ISO IN max packet exceeds the 1024-byte high-speed limit"
        );
        (if full_speed <= 1024 { full_speed } else { high_speed }) as u16
    };
```

In `new`, replace the `const { assert!(49 * CAPTURE_CH * 3 <= 1024, ...) };` block with:

```rust
        let _ = Self::CAPTURE_MAX_PACKET; // evaluates the size check at compile time
```

Replace both `(49 * CAPTURE_CH * 3) as u16` endpoint sizes (alt 1 and alt 2) with
`Self::CAPTURE_MAX_PACKET`. Change the comment on the alt 1 line to
`// see CAPTURE_MAX_PACKET`. In `new`'s doc, replace the sentence
"The capture ISO IN MPS is derived from `CAPTURE_CH`: `49 * CAPTURE_CH * 3` bytes (49-frame
stuffing headroom, 24-bit samples)." with "The capture ISO IN MPS is
[`Self::CAPTURE_MAX_PACKET`]."

- [ ] **Step 4: Run the tests to verify they pass**

Run the Task 1 Step 3 command.
Expected: 3 passed.

- [ ] **Step 5: CHANGELOG and commit**

Under `## [Unreleased]` → `### Added` in `CHANGELOG.md`:

```markdown
- `usb-classes` `AudioClass`: up to 48 capture channels. Above 1024 bytes the capture endpoint
  is sized per high-speed microframe (7 frames), so `AudioClass<8>` (168-byte packets) builds,
  for example for an Elektron Outbox 8. `AudioClass::CAPTURE_MAX_PACKET` gives the size.
```

```bash
git add usb/usb-classes/src/audio.rs CHANGELOG.md
git commit -m "usb-classes: AudioClass sizes capture per microframe above 1024 bytes (8 channels fit)"
```

- [ ] **Step 6: Full suite**

```bash
tools/test.sh > "$SCRATCH/sp5-root.log" 2>&1; echo $?; tail -5 "$SCRATCH/sp5-root.log"
```
Expected: exit 0. Known exception: while `../flare` is ahead of CI's pin, the suite stops at
`deluge-wren-core` (E0004). In that case, check that the `usb-classes` line passed before it.

## After the plan (hardware)

Spec §8.4 step 5, Outbox: a Deluge running `AudioClass<8>` into the Outbox. Logged in
`docs/hardware-tasks.md` by sub-project 3's plan, which creates that file for all of §8.4.
