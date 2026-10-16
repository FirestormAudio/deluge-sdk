# Overbridge transport — host class, device class, and the USB controller work under them

The SDK gains Elektron's Overbridge protocol in both directions:

- **Host:** a Deluge receives every track of a Digitakt (12 channels from the
  device, 2 to it) or a Digitakt II (42 from, 8 to) as separate 48 kHz streams
  over USB.
- **Device:** a generic embassy-usb Overbridge device class serves the Rust
  Digitakt firmware and, for testing, a second Deluge.

Under both sits RZ/A1L USB controller work:

- Overbridge's high-bandwidth interrupt endpoints are carried on bulk pipes.
- USB FIFO traffic moves to DMA so the ~3–7 MB/s stream does not cost a CPU
  core's worth of FIFO reads.

A separate goal is more analog outputs for the Deluge through an Outbox 8. That
uses the existing UAC2 device class at 8 channels, not Overbridge (§7).

> **Status:** implemented on `feat/overbridge` (sub-projects 1–5, 7, 8; 6 belongs to the
> digitakt repo). Hardware checks are in `docs/hardware-tasks.md`. Deviations from this design,
> decided during implementation:
> - §4.3–4.5: receive DMA only, on D1FIFO, 4-byte cycle steal, per transfer under the existing
>   pipe trait (no `StreamIn`/`StreamOut` ring); transmit stays on the CPU. See the sub-project 2
>   plan's Rulings.
> - §4.1: the host stops an interrupt-on-bulk IN pipe at NAK after every block (a full-size
>   block gets no short packet for SHTNAK to act on).
> - §5.3: `StreamStats` also counts `counter_resyncs` (a counter jump past ~150 ms of gap).
> - §5.6: the shared `HostAudioArena` isn't built (it would save ≤ 32 KB). The UAC rings and
>   the Overbridge rings stay separate statics.
> - §6: the device sender waits for the app's frames (it never drops a block); the device face
>   switch is a pattern in `hw-tests/overbridge-device-firmware`, not an SDK API.
> **Evidence:** the protocol comes from Overwitch (GPL-3; read for the format
> only, no code copied; last checked at commit 40018bb, `src/engine.c`,
> `src/engine.h`, `res/devices.json`). The digitakt repo's Outbox 8 firmware analysis,
> `re/notes/outbox8.md` (commits b994da5 and abf93cc), independently matches it.
> The Digitakt's own descriptors come from its OS 1.54 image (`re/notes/io.md`,
> config descriptor at 0x40252286 in `section_3.bin`), decoded in §2.1 and
> Appendix A.
> **Builds on:** the UAC host stack (`2026-07-16-usb-uac-host-design.md` and
> successors), `rza1l-hal::usb::host`, and `rza1l-hal::dmac`.

---

## 1. Goals and non-goals

**Goals**

- **Transport only.** Deliver each stem of a connected Elektron Overbridge device
  to the application as a 48 kHz `i32`/`f32` stream with fill level and
  continuity statistics, and accept a 48 kHz stream back to the device.
- **Sustain the data rate** on the Deluge's USB0 with no dropped blocks over a
  10-minute run:
  - Digitakt: 368-byte IN and 88-byte OUT blocks, each ~6857/s.
  - Digitakt II: ~1012-byte IN and 256-byte OUT blocks.
- **An Overbridge device class** (embassy-usb, chip-agnostic) that Overwitch and
  the Deluge host class both accept. The board supplies the identity: VID/PID,
  name, and channel counts.
- **Deluge → Outbox 8:** a tested 8-output configuration of the existing UAC2
  device class.

**Non-goals**

- **What apps do with stems:** mixing, routing, recording, display. Apps decide
  later.
- **Sample-rate conversion to the codec's 44.1 kHz.** The ring exposes drift
  statistics, and the SCUX and the existing PI resampler stay available to apps.
- **Overbridge's parameter and control channel.** It isn't documented in
  Overwitch and isn't needed for audio.
- **A default build that presents Elektron's VID from non-Elektron hardware**
  (§7.2).
- **Overbridge through a hub.** The supervisor binds only USB-MIDI on hub ports
  (`hub_task` calls `bind_midi` alone), so an Overbridge or UAC device on a hub
  gets its MIDI and no audio. Extending the hub path is separate work.

## 2. The protocol (as implemented here)

**Setup** (all from Overwitch, and confirmed in the Outbox firmware):

1. Configuration 1.
2. `SET_INTERFACE` interface 1 alt 3: audio from the device, endpoint `0x83`.
3. `SET_INTERFACE` interface 2 alt 3: audio to the device, endpoint `0x03`.

Overwitch does more, and the Outbox does none of it:
- **Halts.** After selecting the alternates, it clears halts on `0x83` and `0x03`.
- **Name.** It issues two vendor device-to-host requests, `bRequest` 1 and 2, with
  wValue 0, wIndex 0 and 32 bytes each. Request 1 returns the name the device
  shows for the host.
  - It then sleeps 100 ms. Its source says that is "required to not send the
    next packet immediately, which can make devices crash".
- **Setting the name.** A vendor host-to-device `bRequest` 1 with a 32-byte
  payload sets that name.
- **Kernel drivers.** It detaches the kernel drivers from interfaces 4 and 5 and
  reattaches them afterwards. Those are the USB-MIDI function's AudioControl and
  MIDIStreaming interfaces (§2.1), not an Overbridge channel.
- **Transfer size.** It queues interrupt transfers of 6–24 blocks (default 24,
  8832 bytes IN for a Digitakt). Each block is one full `wMaxPacketSize` packet,
  so the wire traffic is the same as one block per transfer.
- **Size check.** It refuses a device whose alt-3 `wMaxPacketSize` isn't exactly
  `32 + 7 × frame_bytes` for its `devices.json` entry.

The host class does what the Outbox does and treats the name as optional. If it
does read the name, it waits 100 ms afterwards before opening the streams.

### 2.1 The Digitakt's descriptors (OS 1.54)

Decoded from the firmware image (Appendix A has the bytes). Hardware test 1
(§8.4) confirms them on a running unit.

- **Device:** VID 0x1935, PID 0x0B2C, bcdUSB 2.00, class EF/02/01 (IAD
  composite), EP0 max packet 64, bcdDevice 0x0001, strings 1/2/3. High speed
  only.
- **Configuration 1:** 6 interfaces, self-powered (bmAttributes 0xC0),
  bMaxPower 0, wTotalLength 263 (patched at runtime; the image holds 0).
- **IAD 0–3, vendor class 0xFF:**

  | Interface | Alt | Endpoint | Type | MaxPacket | bInterval |
  |---|---|---|---|---|---|
  | 0 | 0 | none | | | |
  | 1 | 0 | none | | | |
  | 1 | 1 / 2 / 3 | 0x83 IN | interrupt | 32 / 88 / 368 | 1 |
  | 2 | 0 | none | | | |
  | 2 | 1 / 2 / 3 | 0x03 OUT | interrupt | 32 / 88 / 88 | 1 |
  | 3 | 0 | 0x02 OUT, 0x82 IN | bulk 512, interrupt 64 | | 1 (0x82) |

- **IAD 4–5:** USB-MIDI 1.0 (AudioControl, then MIDIStreaming with bulk 0x01 OUT
  and 0x81 IN, 512 bytes).

By the block formula, alts 1, 2 and 3 carry 0, 2 and 12 channels from the
device, and 0, 2 and 2 to it. What alts 1–2 are for isn't known; hosts select
alt 3. Interface 3 is likely the parameter channel (a non-goal) and isn't
opened.

**Endpoints.** High-speed **interrupt** endpoints with `bInterval` 1, meaning one
transaction per 125 µs microframe. One *block* is one USB packet, and its length
is that endpoint's `wMaxPacketSize` in alt 3.

**Block layout** (byte offsets):

| Offset | Size | Content |
|---|---|---|
| 0 | 2 | header, big-endian: `0x07FF` host→device, `0x0700` device→host |
| 2 | 2 | frame counter, big-endian, +7 per block, wraps at 16 bits |
| 4 | 28 | reserved (zero on transmit, ignored on receive) |
| 32 | … | 7 frames, interleaved by channel |

**Sample formats**

- **v2:** every sample is a big-endian `i32` at full scale.
- **v2.1** (Digitakt II and Digitone II, 42 channels from the device; Overwitch's
  `engine.c` sample loops):
  - Within each frame, the first 14 channels from the device are 4-byte samples
    and the remaining 28 are 3-byte samples. All 8 channels to the device are
    4-byte.
  - A **3-byte** sample is a packed big-endian 24-bit value. It decodes to
    `i32::from_be_bytes([b0, b1, b2, 0])`.
  - A **4-byte** sample in v2.1 is *not* full scale. It is a 24-bit value
    sign-extended into a big-endian `i32`, so it decodes as
    `i32::from_be_bytes(b) << 8` and encodes as `(x >> 8).to_be_bytes()`. That
    applies both ways, to the device's 14 wide channels and to all 8 channels
    sent to it.
  - Every v2.1 sample is therefore 24-bit audio. `proto` normalises all of them
    to full-scale `i32`.
- Block size is `32 + 7 × frame_bytes`:

  | Device | From device | To device |
  |---|---|---|
  | Digitakt | 32 + 7 × 48 = 368 B | 32 + 7 × 8 = 88 B |
  | Digitakt II | 32 + 7 × (56 + 84) = 1012 B | 32 + 7 × 32 = 256 B |

**Rate.** 48 000 / 7 ≈ 6857 blocks/s each way. That is slightly fewer than the
8000 microframes per second, so the device NAKs about one IN token in seven, and
the host may send OUT blocks at any time.

**Clocking.** The device is the clock. The host returns exactly as many frames as
it receives, so the OUT stream is paced by IN arrivals. The OUT stream must run
whenever the IN stream runs, even when it carries silence. Both the Outbox and
Overwitch always run it, and we treat the device as needing it.

**Device table** (`usb-classes`, from Overwitch's `devices.json`, cross-checked
against the Outbox table):

| PID | Device | From / to device | Format |
|---|---|---|---|
| 0x0B2C | Digitakt | 12 / 2 | v2 |
| 0x0B36 | Digitone | 12 / 2 | v2 |
| 0x0B35 | Digitone Keys | 12 / 2 | v2 |
| 0x0B4A | Syntakt | 20 / 8 | v2 |
| 0x0B47 | Analog Four MKII | 8 / 6 | v2 |
| 0x0B48 | Analog Rytm MKII | 12 / 12 | v2 |
| 0x0B50, 0x0B52, 0x0B53 | Analog Heat, MKII, +FX | 4 / 4 | v2 |
| 0x0B2B | Digitakt II | 42 / 8 | v2.1 |
| 0x0B34 | Digitone II | 42 / 8 | v2.1 |

Notes:
- The VID is 0x1935.
- An Elektron device that isn't listed is still driven when its alt-3 endpoint
  sizes give a whole number of channels. The table only supplies names and the
  v2.1 split. Channel counts follow from the block size, and descriptors win
  over the table when they disagree.
- The four PIDs the Outbox lists but Overwitch doesn't (0x0B2A, 0x0B4B–0x0B4D)
  use other framings and are out of scope.

## 3. Approach

Three ways to get the bandwidth were considered:

1. **(Chosen) Bulk pipes carrying interrupt endpoints, with DMA on D0FIFO and
   D1FIFO.**
   - **Why bulk pipes work:** the RZ/A1L's interrupt pipes (6–9) are limited to
     64 bytes, and it can't do high-bandwidth interrupt transfers. On the wire,
     an interrupt IN/OUT transaction and a bulk one use the same tokens,
     handshakes and data toggles. The device can't tell them apart, and the
     host's only obligation is to poll often enough.
   - **Polling:** a bulk pipe is polled every microframe the bus has room for,
     which is at least as often as `bInterval` 1.
   - **Data path:** DMA moves the data between the FIFO and RAM.
2. **Isochronous pipes 1–2.** They are wrong on the wire: the endpoint is
   interrupt, so the device expects handshakes and data toggles.
3. **CPU FIFO access without DMA.** It works for a Digitakt at 32-bit access:
   ~0.8M FIFO reads per second plus an interrupt per block. It doesn't leave
   room for a Digitakt II or for the rest of the Deluge. It stays as the
   fallback path (§4.5).

## 4. Controller layer (`rza1l-hal::usb`)

### 4.1 Interrupt endpoints on bulk pipes

The host's `alloc_pipe` changes:

- **Interrupt endpoints of 64 bytes or less** keep pipes 6–9, as today. That
  covers MIDI and hubs.
- **Interrupt endpoints over 64 bytes** are allocated as bulk pipes:
  - MaxPacket ≤ 512: the bulk range, searched 5 down to 1 as bulk is today (so
    pipes 3–5 first), at MXPS 512;
  - MaxPacket over 512: pipes 1–2 only, at MXPS = the endpoint's MaxPacket, up
    to 1024.
- Configuration is `PIPECFG_TYPE_BULK | SHTNAK | DBLB` with IITV 0.
- A high-bandwidth endpoint (wMaxPacketSize bits 12:11 non-zero) is rejected at
  open. No Overbridge device uses one, and a bulk pipe can't carry it.
- Sub-project 1 keeps no marker for these pipes, since nothing it does differs
  from a bulk pipe. Sub-project 2 adds one if the stream path needs it.

**Why MXPS 512 (TRM §28.3.34, PIPEMAXP).**
- Pipes 3–5 accept only 8, 16, 32, 64 or 512, so a 368-byte endpoint must be
  programmed at 512. Its packets are then always short, which is the framing
  §4.1 "Short packets" relies on.
- Pipes 1–2 accept 1–1024 at any type. A bulk pipe above 512 is outside the USB
  specification, though, so the Digitakt II's 1012-byte IN path is unverified
  until hardware test 4 (§8.4) runs. Sub-project 1 implements the rule and tests
  the register writes only.

**Pipe budget.** Host mode has five bulk-capable pipes (1–5), and pipes 1–2 are
also the only isochronous pipes.
- **A Digitakt** opens its two Overbridge pipes and its two USB-MIDI pipes:
  four of the five. One isochronous UAC stream can still have pipe 1 or 2.
- **A Digitakt II** needs pipe 1 or 2 for its 1012-byte IN stream. A
  class-compliant audio interface that streams both ways uses both, so the two
  can't run at once. The second one to open gets `OutOfPipes`, and the
  supervisor reports it rather than retrying.

Packet-buffer budget (8 KB, 64-byte blocks):

| Device | IN | OUT | Total |
|---|---|---|---|
| Digitakt | 2 × 512 = 16 blocks | 2 × 512 = 16 blocks | 2 KB |
| Digitakt II | 2 × 1024 = 32 blocks, pipe 1–2 | 2 × 512 = 16 blocks, pipe 3–5 | 3 KB |

Both fit alongside MIDI and hub pipes. Allocation failure is an error at pipe
open, never a silent fallback to a smaller MXPS.

**Short packets.**
- *IN:* a 368-byte block on an MXPS-512 pipe ends as a short packet. That
  completes the host transfer as one block, which is the framing we want. A
  device sending exactly MXPS-sized blocks would need a zero-length packet.
  That doesn't happen with the sizes above, and a test asserts it.
- *OUT:* writing 88 bytes and setting BVAL sends one 88-byte packet.
- *Batching.* Overwitch's multi-block transfers (§2) rely on each block being a
  full `wMaxPacketSize` packet. On a pipe at MXPS 512 every block is short, so
  each host transfer here is one block. The device sees the same tokens either
  way.

### 4.2 IITV at high speed

For interrupt pipes 6–9 the host computes `IITV = floor(log2(bInterval))`. That
is right at full speed. At high speed `bInterval` is an exponent (period
2^(bInterval−1) microframes), so the code should use `IITV = bInterval − 1`,
capped at 7.

- **TRM.** §28.4.8 (1)(c): the host issues a token once every 2^IITV
  (micro)frames, and counts 1 ms frames only "for the pipes used for
  communications with the full-speed or low-speed peripheral devices connected
  to a high-speed HUB". A pipe to a high-speed device therefore counts
  microframes.
- **Speed.** `edpt_open` reads the device's speed back from DEVADDn.USBSPD,
  which `device_open` programs before any endpoint opens. High speed gets
  `bInterval − 1`; full and low speed keep `floor(log2(bInterval))`. The public
  signature doesn't change.
- **Effect today.** `floor(log2(b)) ≤ b − 1`, so a high-speed interrupt endpoint
  is polled *more* often than it asks. For example, a hub status endpoint at
  bInterval 12 is polled every 1 ms instead of the 16 ms cap. That costs bus
  time and NAKs, not correctness.

This is fixed independently of Overbridge.

### 4.3 FIFO routing

| Port | Use today (host) | Use after this work |
|---|---|---|
| CFIFO | pipe 0 | pipe 0, plus every CPU-driven pipe (MIDI, hub, control-like) |
| D0FIFO | all non-control pipes, CPU byte access | one **DMA receive** pipe (Overbridge or UAC IN) |
| D1FIFO | unused | one **DMA transmit** pipe (Overbridge or UAC OUT) |

- **Port ownership.** A port is owned by one pipe for the life of a stream, so
  CURPIPE switches (and the D1FIFO reselect quirk in `fifo.rs`) never happen
  mid-transfer.
- **Port arbitration.** Pipes that don't use DMA share CFIFO under the existing
  per-transfer lock, so their access stays serialised.
- **Device mode** keeps its current routing (pipes 1–2 on D0, 3+ on D1) until
  §6 needs DMA. Both modes then go through the same `FifoPort` assignment
  function.

### 4.4 USB FIFO DMA

**Request sources.** Each USB channel has two DMA request lines. For USB0 these
are `USB0_DMA0` (D0FIFO, DMARS 0b010_0000) and `USB0_DMA1` (D1FIFO,
0b010_0001). The BSP reserves two DMAC channels for them and hands them to the
USB host the way SSI's are handed over today (typed DMA roles).

**Which channels.** `deluge_bsp::Board` hands out all 16 channels by role, but
nothing takes three of them: SCUX FFU0 (`DMA_CH1`), FFU3 (`DMA_CH9`) and FFD3
(`DMA_CH15`). That holds for every firmware in this repo and for
DelugeFirmware's Rust BSP. Sub-project 2 renames two of those roles:
- `DMA_CH1` becomes `Usb0D0FifoDma` (receive). Under fixed priority a lower
  channel number wins, which suits the receive side.
- `DMA_CH15` becomes `Usb0D1FifoDma` (transmit).

The C++ firmware doesn't conflict. Its USB stack builds with `USB_CFG_DMA`
disabled, and its own channels are 2–4, 6–7 and 10–14.

**Access width.**
- Set D*n*FIFOSEL.MBW to 32-bit.
- Use 32-byte continuous transfers: D*n*FBCFG.DFACC = 32-byte continuous,
  following the setup in TRM §28.3.6. Each DMA request then moves 32 bytes
  as 8 longwords to a 32-byte-aligned RAM buffer.
- Blocks are 368, 88, 1012 and 256 bytes, all multiples of 4 but not of 32. The
  tail is handled per direction below.

**Receive (IN, D0FIFO)**
1. Before arming the pipe, select it on D0FIFO and set DREQE, BFRE and DCLRM.
2. The DMAC is armed for MXPS bytes into the next ring slot.
3. With BFRE set, the controller raises BRDY only after the last byte of a short
   packet has been read out, and DTLN reports the received length.
4. The BRDY handler reads DTLN, stops the DMAC channel, and moves the ring to the
   next slot. The slot's length is DTLN; a slot whose DTLN isn't a whole number
   of frames is counted as a framing error (§5.5).
5. Slots are MXPS rounded up to 32 bytes. Short packets need care with 32-byte
   continuous mode: TRM §28.3.6 governs how the controller drops DREQ partway
   through a 32-byte unit. Sub-project 2 starts with a poked-MMIO and hardware
   check of that behaviour.
   - If a partial unit stalls the DMAC, the receive side uses cycle-steal 4-byte
     access instead. That costs 4× the DMA requests (~92k/s for a Digitakt),
     which is still fine.

**Transmit (OUT, D1FIFO)**
1. DREQE and TENDE are set.
2. The DMAC writes the block from RAM.
3. The DMAC's transfer-end signal (TENDE) ends the packet. The controller sets
   BVAL itself, so an 88-byte packet goes out as one short packet with no CPU
   write of BVAL.
4. The 32-byte-unit length problem is avoided by making the tail transfer a
   separate DMAC descriptor in cycle-steal 4-byte mode:
   - 88 = 64 + 24: two 32-byte units, then six longwords.
   - 256 = 8 × 32, a single descriptor.
   The DMAC's link-descriptor mode chains the two pieces.

**Throughput**

| Device | DMA requests per second | Interrupts per second |
|---|---|---|
| Digitakt | (12 + 3) × 6857 ≈ 103k | ~13.7k (one BRDY per block each way) |
| Digitakt II | (32 + 8) × 6857 ≈ 274k | ~13.7k |

The interrupt work is a register read and a ring index update. It isn't
per-byte work.

**Cache.** Ring buffers live in the uncached region the SSI rings use, so DMA
needs no cache maintenance. If they ever move to cached RAM: invalidate a slot
before handing it to the application; clean it before arming transmit.

**Errors.**
- A DMAC error, or a pipe that goes to STALL or NRDY-error, stops both
  directions and reports `TransportError` (§5.5).
- Recovery re-runs the setup steps (§5.2).
- A detach tears down through the existing DTCH path, which must also stop the
  DMAC channels. That path is shared with UAC, and §8 tests it.

### 4.5 Interface to class code

The DMA path sits under a new **streaming pipe** API in `rza1l-hal::usb::host`,
next to the existing per-transfer `embassy-usb-host` channel traits:

```rust
pub struct StreamIn  { /* pipe, DMAC channel, slot ring */ }
pub struct StreamOut { /* pipe, DMAC channel, slot ring */ }

impl StreamIn {
    /// Next received block (len = DTLN), or `Err` on transport error.
    pub async fn next(&mut self) -> Result<Block<'_>, StreamError>;
}
impl StreamOut {
    /// A slot to fill; sent when the guard is committed.
    pub async fn slot(&mut self) -> Result<SlotMut<'_>, StreamError>;
}
```

- **Opening a stream.** Streams open from an endpoint descriptor and a
  `StreamConfig { slots, dma: DmaRole }`. With no DMA role they fall back to
  CPU-driven 32-bit FIFO access on CFIFO. That keeps a working path for tests
  and for boards without spare DMAC channels.
- **Two layers of buffering.** The ring holds `slots` blocks (default 8, about
  1.2 ms) between the interrupt handler and the class. The class's stem ring
  (§5.3) holds the audio.
- **Why not `embassy-usb-host`'s channel traits.** They are one-transfer-per-await
  and can't express continuously armed DMA. Their crate stays at 0.1.0, unmodified.

## 5. Overbridge host class (`usb-classes`, feature `host-overbridge`)

### 5.1 Modules

- `overbridge::proto`: `no_std`, no USB dependency.
  - The device table, and `Format { v2, v2_1 { wide: 14 } }`.
  - Block encode and decode between wire blocks and `[i32; 7 × ch]` frames.
  - Counter tracking.
  - Shared by the host and device classes.
- `host::overbridge`:
  - The driver: matching, setup, the stream task, the stem ring.
  - Bound by the BSP's USB host supervisor next to `host::uac` and `host::midi`,
    under the runtime mode in §5.6. Overbridge is tried first for VID 0x1935,
    since some Elektron devices also
    expose class-compliant UAC in other USB modes.

### 5.2 Setup sequence

1. **Match.** The VID is 0x1935, and configuration 1 has interface 1 alt 3 with
   an interrupt IN endpoint and interface 2 alt 3 with an interrupt OUT endpoint.
2. **Size.** Derive the channel counts from each endpoint's MaxPacket and
   `Format`. Reject the device when the arithmetic doesn't give a whole number
   of channels.
3. **Select alternates.** `SET_INTERFACE`(1, 3), then `SET_INTERFACE`(2, 3).
4. **Name.** Optionally issue vendor request 1 for the device name (32 bytes), then wait
   100 ms (§2).
   A failure here isn't fatal.
5. **Open streams.** Open `StreamIn` and `StreamOut` (§4.5), and prime the OUT
   stream with 2 silent blocks so the device sees host traffic immediately.
6. **Report.** Publish `OverbridgeInfo { pid, name, ch_from, ch_to, format }` to
   the application.

### 5.3 Stem ring (the app-facing API)

```rust
pub struct OverbridgeHandle { /* shared state */ }

impl OverbridgeHandle {
    pub fn info(&self) -> Option<OverbridgeInfo>;
    /// Copy up to `out.len() / ch_from` frames of interleaved stems.
    pub fn read(&self, out: &mut [i32]) -> usize;
    /// Queue frames to the device (interleaved, ch_to wide); returns frames taken.
    pub fn write(&self, inp: &[i32]) -> usize;
    pub fn stats(&self) -> StreamStats;
}

pub struct StreamStats {
    pub fill_in: u32, pub fill_out: u32,        // frames
    pub blocks_in: u32, pub blocks_out: u32,
    pub counter_gaps: u32,                      // missing blocks by the +7 counter
    pub overruns: u32, pub underruns: u32,
    pub rate_ratio_ppm: i32,                    // device frames vs local time
}
```

**Read side**
- Frames are stored decoded: interleaved `i32`, full scale = `i32` range. An
  `f32` convenience view is a thin wrapper.
- The ring size is a const generic, default 1024 frames (21 ms).
- The stream task decodes each received block straight into the ring, so the
  interrupt handler never touches samples.
- When the reader falls behind, the oldest frames are dropped and `overruns`
  counts them.

**Write side**
- For every IN block the stream task sends one OUT block, encoding 7 frames
  from the write ring.
- It pads with silence and counts `underruns` when the app hasn't written enough.
- Pacing OUT by IN keeps the device's frame count balanced without a separate
  clock.

**Rate ratio**
- Measured against the BSP's audio clock: frames received per 44.1 kHz period
  counter, filtered.
- Apps resample with it (or with the fill level) if they want stems in the
  Deluge's audio graph. The class never resamples.

### 5.4 Interaction with UAC host

- `host::uac::MAX_CHANNELS = 8` is unchanged. Overbridge has its own ring sized
  by `ch_from`, so a 42-channel device costs 42 × 4 × 1024 = 168 KB of ring only
  when it's connected and only for the board that enabled the feature.
- The ring memory comes from the host-audio arena (§5.6), which the board sizes
  as `HostAudioArena<BYTES>`. The arena is shared with the UAC ring, since only
  one audio device is bound at a time. A device whose ring wouldn't fit reports
  `TooManyChannels` instead of truncating, so stem indices never silently shift.

### 5.6 Host audio composition: features, binding and runtime mode

The goal is for a single firmware image to switch between UAC and Overbridge at
runtime.

**Build-time features**

| Crate | Feature | Contents |
|---|---|---|
| `usb-classes` | `host-midi` | USB-MIDI host driver (exists) |
| `usb-classes` | `host-uac` | UAC host driver (exists) |
| `usb-classes` | `host-overbridge` | §5 |
| `deluge-bsp` | `usb-host` | supervisor, hubs, MIDI (today it also pulls in UAC) |
| `deluge-bsp` | `usb-host-uac` | `usb-host` + `host-uac` |
| `deluge-bsp` | `usb-host-overbridge` | `usb-host` + `host-overbridge` |

- Splitting `usb-host` changes behaviour for existing users, so `usb-host` keeps
  enabling UAC for one release, with a CHANGELOG note.
- Features decide what can be bound. The runtime mode decides what is bound.

**Pipe and resource budget.** Host mode has five bulk-capable pipes (1–5), and
pipes 1–2 are also the only isochronous ones. Every case below fits, because the
supervisor binds one root-port device at a time:

| Root-port device | Pipes |
|---|---|
| Audio interface with MIDI | MIDI 5, 4; UAC 2, 1 |
| Digitakt, Overbridge mode | MIDI 5, 4; Overbridge 3, 2 |
| Digitakt II, Overbridge mode | MIDI 5, 4; Overbridge IN 2 (over 512 B), OUT 3 |

The two DMAC roles (§4.4) and the arena are likewise held by whichever audio
driver is bound.

**Runtime mode.** A `HostAudioMode` stored in an atomic:
- **Auto** (default): bind Overbridge when §5.2's match succeeds, otherwise UAC.
  Elektron boxes present Overbridge (PID 0x0Bxx) or class-compliant audio
  (0x10xx) according to their own USB-mode setting. Changing that setting makes
  the box re-enumerate, so under Auto the switch needs nothing from the Deluge.
- **Uac only** and **Overbridge only**: the other driver is never tried.
- **Off:** no audio driver. MIDI still binds.

A mode a build can't honour (Overbridge only, without `host-overbridge`)
behaves as Off and logs it once.

**Binding order on the root port**
1. MIDI, as today.
2. Overbridge, if the mode allows it and §5.2 matches.
3. Otherwise UAC, if the mode allows it.

**Changing the mode while a device is attached**
1. The supervisor's post-bind wait (today it waits only for a detach) also waits
   on a mode-change signal.
2. On a change, it signals the bound audio task to stop. The task's exit drops
   its pipes, DMA channels and arena claim, exactly as on a detach. MIDI tasks
   stop as well.
3. It calls `Rusb1HostDriver::bus_reset`, so the device re-enumerates through the
   normal attach path and is bound under the new mode.

A reset, not re-binding in place, gives the device a clean configuration and
alternate settings, and reuses the tested detach path. A switch costs about
one enumeration (~1 s), and MIDI on the same device drops and returns with it.

**Shared arena.** Because only one audio device is bound at a time,
- the UAC ring and the Overbridge stem ring are carved from one board-sized
  static, `HostAudioArena<BYTES>`;
- the bound driver claims it at bind time and releases it when its task exits;
- the board sizes it for the largest case it supports (≈168 KB for a 42-channel
  Digitakt II at 1024 frames). A UAC-only board keeps today's small ring.

**The app-facing handle.** One handle follows whichever driver is bound, so an
app doesn't need rebuilding and can follow a mode change live:

```rust
pub enum HostAudio<'a> {
    None,
    Uac(UacInput<'a>),              // today's capture/playback API, ≤ 8 channels
    Overbridge(&'a OverbridgeHandle), // §5.3
}
pub fn host_audio() -> HostAudio<'static>;
pub fn set_host_audio_mode(mode: HostAudioMode);
pub fn host_audio_mode() -> HostAudioMode;
```

- The two read paths stay distinct. UAC is an N-channel input, and Overbridge is
  named stems.
- A connection counter lets an app notice that the device or driver changed
  between two reads.

**Device side (later).** The Deluge as a USB device could switch between its
UAC2 face and an Overbridge face the same way:
1. drop the `UsbDevice`;
2. disconnect from the bus (D+ pull-up off);
3. rebuild with the other configuration;
4. reconnect.

The host sees an unplug and a new device. This is sub-project 8, after 4.

### 5.5 Errors

| Condition | Response |
|---|---|
| A STALL on setup, or `SET_INTERFACE` fails | `SetupFailed`; retry once after 100 ms, then give up until re-plug |
| A transport error (DMAC error, pipe error, NRDY storm) | stop both streams, re-run §5.2 from step 3; after 3 failures in 10 s, give up until re-plug |
| A wrong header (not `0x0700`), or a DTLN that isn't a whole number of frames | drop the block, count it as a `framing_error`; 64 consecutive → transport error |
| A counter gap | count the missing blocks; no recovery needed |
| Detach | the existing DTCH teardown; the handle reports `info() == None` |

## 6. Overbridge device class (`usb-classes`, feature `device-overbridge`)

**Users**
- The Rust Digitakt firmware (digitakt repo, ColdFire MCF5441x USB OTG). It
  presents the real Digitakt identity and channel counts.
- A Deluge in a two-Deluge loopback test. It presents a test identity (below).

**Descriptor layout** (the Digitakt's, §2.1):
- One configuration, IAD composite (device class EF/02/01).
- Interface 0: vendor-specific, no endpoints.
- Interfaces 1 and 2, each with alts 0–3. Alts 1–3 each have one interrupt
  endpoint, high speed, `bInterval` 1.
  - Alt 3's MaxPacket is the block size for the board's channel counts.
  - Alts 1 and 2 copy the Digitakt's sizes: 32 bytes, then the 2-channel block
    (88 bytes). Hosts select only alt 3, but matching the real layout costs
    nothing.
- Interface 3: vendor-specific, with bulk OUT and interrupt IN, as on the
  Digitakt. It accepts nothing yet (the parameter channel is a non-goal), and
  it is only present so that interface numbers match.
- Interfaces 4–5: optional USB-MIDI, composed by the board with `device-midi`.
- The board supplies an `OverbridgeConfig { ch_to_host, ch_from_host, format,
  name: &'static str }` along with the VID/PID passed to the embassy-usb builder.

**Behaviour**
- **Name.** Vendor IN request 1 returns `name`, padded to 32 bytes.
  - IN request 2's meaning is unknown; it returns the same 32 bytes.
  - Vendor OUT request 1 stores a host-supplied name, which the board may show.
- **Clock.** The device is the clock. A task driven by the board's audio clock
  produces one 7-frame block per 7 frames of its audio and writes it to the IN
  endpoint. When the host isn't reading, a full endpoint makes the task drop
  the block and count it.
- **Receiving.** OUT blocks are decoded into a receive ring; missing blocks play
  as silence.
- **Interface.** The class exposes the same `read`/`write`/`stats` shape as the
  host handle, with directions mirrored.

**Controller requirements** (both are in the existing driver's capability set; a
board whose controller can't meet them fails at build time through embassy-usb's
`alloc_endpoint` returning an error):

- **ColdFire:** the MCF5441x controller supports high-speed interrupt endpoints
  up to 1024 bytes natively.
- **RZ/A1L in device mode:** it has the same 64-byte interrupt pipe limit. The
  RUSB1 embassy-usb driver maps an interrupt endpoint over 64 bytes to a bulk
  pipe, using the same `IntrOnBulk` rule as §4.1. The descriptors still say
  interrupt, since the driver reports what the class asked for.

**Testing against Overwitch.** Overwitch recognises devices by VID 0x1935 and
its `devices.json`. A Deluge test build uses VID 0x1935 with an unassigned PID,
and a local `devices.json` entry, only on the developer's machine.
- Overwitch rejects the device unless alt 3's `wMaxPacketSize` equals
  `32 + 7 × frame_bytes` from that entry exactly.
- It expects interface 2 alt 3 and interface 1 alt 3 to exist, it clears halts
  on both endpoints, and it issues vendor requests 1 and 2 (§2). The device class
  answers all of these.
- It detaches kernel drivers on interfaces 4 and 5, so the optional USB-MIDI
  function should sit at those numbers, as on the Digitakt.
- `bRequest` 1 host-to-device (set name) is accepted, and the name is stored.

## 7. Deluge → Outbox 8: more analog outputs

### 7.1 Primary: an 8-channel UAC2 device (no Overbridge)

The Outbox's firmware drives Overbridge only for Elektron PIDs in its table. For
any other device it runs a generic UAC1/UAC2 host driver (`outbox8.md`, unknown
VID path at 0x60046A70). That driver reads the AudioControl header (bcdADC), the
clock source and output terminals, and up to ~8 AudioStreaming interfaces.

So the Deluge, as a USB device with its own IDs, needs to present 8 channels of
device→host audio. That is `AudioClass<CAPTURE_CH = 8>`, the "capture" side.

**Required change.** `AudioClass::new` sizes the capture endpoint for 49 frames
per packet. That is a full-speed figure, but the endpoint is high speed with
`bInterval` 1. At 8 × 3 bytes it gives 1176 bytes, and the const assert rejects
`CAPTURE_CH = 8` today. The fix is to size per microframe:

- `MaxPacket = (ceil(rate / 8000) + 1) × CAPTURE_CH × subslot`.
- At 48 kHz, 24-bit: 7 × 24 = 168 bytes.
- At 44.1 kHz: 6 frames + 1 of stuffing, the same 7 × 24.

The 16-bit alt follows the same formula, and the 49-frame figure is kept only
for a full-speed connection. The assert becomes `≤ 1024` on the high-speed size.

**Unknowns, to settle on hardware.** The Outbox's generic path wasn't traced
far enough to know:
- its channel limit;
- whether it accepts an asynchronous source with implicit feedback (the class's
  current mode in `audio.rs`);
- whether it needs 48 kHz.

The hardware test (§8.4) answers these first. If the generic path caps below 8
channels, or rejects the format, that becomes a known limit, documented in the
spec's follow-up.

### 7.2 Fallback: an Overbridge device with Elektron's IDs (opt-in only)

The Outbox gives Overbridge treatment to Elektron VID/PIDs, so a Deluge running
§6 with, for example, `0x1935:0x0B2C` and 12 to-host / 2 from-host channels
would take that path. That means using Elektron's USB vendor ID on non-Elektron
hardware, so:

- it is a cargo feature, `elektron-id`, off by default, never enabled by any
  SDK example or board default;
- it carries a doc comment saying it is for the developer's own bench;
- it is only pursued if §7.1 can't reach 8 outputs.

The device class itself (§6) has no Elektron identity built in. Identity always
comes from the board.

## 8. Testing

### 8.1 Host unit tests (x86_64 bucket)

**`proto`**
- **Golden blocks.** Encode and decode round trips at the table's channel counts.
  Hand-built golden blocks for each layout:
  - Digitakt from-device (368 B), to-device (88 B);
  - Digitakt II 42-channel v2.1 (1012 B), to-device (256 B).
  The bytes follow Overwitch's documented layout and the Outbox's encoder
  (0x60047E66).
- **v2.1 sample widths.** A 4-byte wide-channel sample `00 12 34 56` decodes to
  `0x1234_5600`. A 3-byte sample `12 34 56` decodes to the same value. A
  to-device value `0x1234_5678` encodes as `00 12 34 56`. Negative values
  sign-extend: `-0x100` encodes as `ff ff ff ff`.
- **Counter.** Wrap at 0xFFFF, and gap counting.
- **Sizing.** Channel derivation from MaxPacket, and rejection of non-integral
  sizes.

**`host::overbridge`**, with the existing `host/mock.rs`
- A mock Digitakt with real descriptors captured from hardware (§8.4 step 1)
  drives the setup sequence. Assert the exact control requests and the
  alt-3 selections.
- A streamed run with injected gaps, framing errors and a detach. Assert the
  stats, the error table in §5.5, and that `read` returns the injected stems in
  order.

**`device-overbridge`**, with embassy-usb's descriptor builder in a test harness
- The descriptor bytes, with alts 0–3 and endpoint sizes.
- Vendor IN requests 1/2 and OUT request 1.

### 8.2 HAL tests (QEMU bucket, poked MMIO)

**Pipe configuration**
- An interrupt endpoint of 368 bytes → pipe 3–5, `TYPE_BULK`, MXPS 512, IITV 0.
- An interrupt endpoint of 1012 bytes → pipe 1–2, MXPS 1012.
- 64 bytes or less → pipe 6–9.
- High-speed IITV is `bInterval − 1`.
- Packet buffer exhaustion is an error.

**DMA**
- The FIFO select writes: MBW 32, DREQE, BFRE, DCLRM, TENDE, DFACC.
- DMARS values 0b010_0000 and 0b010_0001.
- The descriptor chain for 88 bytes: 2 × 32-byte units, then a 24-byte
  cycle-steal tail.
- BRDY handling uses DTLN for the slot length.

**Fallback.** Routing on CFIFO with `dma: None`.

### 8.3 `AudioClass` change

Descriptor tests for `CAPTURE_CH = 8` at high speed (168-byte MaxPacket) and full
speed (49-frame figure). The existing 2-channel tests are unchanged.

### 8.4 Hardware (manual, logged in `docs/hardware-tasks.md` style)

1. **Confirm descriptors.** With the Digitakt in Overbridge mode, dump its
   configuration descriptor through the current host supervisor's enumeration
   log. Compare it byte for byte with Appendix A, which came from OS 1.54's
   image. A unit on a newer OS may differ; if it does, the log replaces the
   appendix as §8.1's fixture.
2. **Digitakt stems.** A harness firmware shows the per-track level of all 12
   stems on the OLED and logs `StreamStats` over RTT every second.
   - Pass: 10 minutes with `counter_gaps == 0`, `framing_errors == 0`, and a CPU
     load figure recorded.
   - Repeat with DMA off (CFIFO fallback) to record the difference.
3. **OUT path.** The harness sends a 1 kHz tone to the Digitakt's Overbridge
   input. Confirm it's audible on the Digitakt.
4. **Digitakt II** (when available). Same as steps 2–3 at 42/8.
5. **Outbox.** A Deluge running `AudioClass<8>` into the Outbox. Confirm 8
   distinct channels on its 8 outputs, and record the sample rate and
   sync-mode findings for §7.1.
6. **Device class.** Two Deluges, one host and one device (test identity),
   loop stems through. Then the Deluge device against Overwitch on Linux with
   the local `devices.json` entry.

## 9. Work breakdown (each gets its own plan)

| # | Sub-project | Depends on | Repo |
|---|---|---|---|
| 1 | Host pipes: interrupt-on-bulk allocation, high-speed IITV fix | — | deluge-sdk |
| 2 | USB FIFO DMA and the streaming-pipe API (§4.3–4.5) | 1 | deluge-sdk |
| 3 | `overbridge::proto` and `host-overbridge` with the stem ring | 1 (2 for full rate) | deluge-sdk |
| 4 | `device-overbridge`, plus RUSB1 device-mode interrupt-on-bulk | proto from 3 | deluge-sdk |
| 5 | `AudioClass` high-speed capture sizing; Outbox hardware test | — | deluge-sdk |
| 6 | Digitakt firmware binds `device-overbridge` | 4 | digitakt |
| 7 | Host audio composition (§5.6): feature split, `HostAudioMode`, re-bind by port reset, shared arena, `HostAudio` handle | 3 | deluge-sdk |
| 8 | Device-side runtime switch between UAC2 and Overbridge faces (§5.6) | 4 | deluge-sdk |

Sub-projects 1, 3 and 5 can proceed in parallel. Sub-project 7's feature split
and mode policy can start before 3, with only UAC behind the mode. 3 can stream a Digitakt over
the CFIFO fallback before 2 lands, which gives an early end-to-end check. 6 is
listed for completeness: it is planned in the digitakt repo once the ColdFire
USB device driver supports it.

## 10. Open questions

- **IN header check.** Does a real Digitakt ever send a header other than
  `0x0700`? Neither source settles it. The Outbox firmware's IN header check
  wasn't traced, and Overwitch never checks the IN header (it only prints it at
  debug level 3). The host class counts framing errors rather than failing, so
  this only affects stats.
- **Does the device need Overwitch's clear-halt or name requests?** The Outbox
  sends neither and works, so the host class doesn't depend on them.
- **The Outbox generic UAC path:** the channel limit and accepted sync mode
  (§7.1). Settled by hardware test 5.
- **Bulk above 512 bytes on pipes 1–2.** The TRM allows MXPS up to 1024 on
  pipes 1–2 at any type, but a 1012-byte bulk-typed pipe is non-standard. It
  needs hardware test 4 (Digitakt II) before sub-project 3 relies on it. If it
  fails, the fallback is an isochronous-typed pipe 1–2 with software data
  toggles, which is unattractive and to be designed then.
- **What interfaces 1 and 2's alts 1–2 select.** They are 0- and 2-channel
  layouts (§2.1); nothing known uses them.

## Appendix A. Digitakt (OS 1.54) Overbridge configuration descriptor

263 bytes, from `section_3.bin` at 0x40252286, with wTotalLength (bytes 2–3)
set to the value the firmware patches in at runtime:

```
09 02 07 01 06 01 00 c0 00 08 0b 00 04 ff 00 00
00 09 04 00 00 00 ff 00 00 00 09 04 01 00 00 ff
00 00 00 09 04 01 01 01 ff 00 00 00 07 05 83 03
20 00 01 09 04 01 02 01 ff 00 00 00 07 05 83 03
58 00 01 09 04 01 03 01 ff 00 00 00 07 05 83 03
70 01 01 09 04 02 00 00 ff 00 00 00 09 04 02 01
01 ff 00 00 00 07 05 03 03 20 00 01 09 04 02 02
01 ff 00 00 00 07 05 03 03 58 00 01 09 04 02 03
01 ff 00 00 00 07 05 03 03 58 00 01 09 04 03 00
02 ff 00 00 00 07 05 02 02 00 02 00 07 05 82 03
40 00 01 08 0b 04 02 01 01 00 00 09 04 04 00 00
01 01 00 00 09 24 01 00 01 09 00 01 05 09 04 05
00 02 01 03 00 00 07 24 01 00 01 41 00 06 24 02
01 01 00 06 24 02 02 02 00 09 24 03 01 03 01 02
01 00 09 24 03 02 04 01 01 01 00 09 05 01 02 00
02 00 00 00 05 25 01 01 01 09 05 81 02 00 02 00
00 00 05 25 01 01 03
```

Device descriptor (0x4025238D): `12 01 00 02 ef 02 01 40 35 19 2c 0b 01 00 01 02 03 01`.
