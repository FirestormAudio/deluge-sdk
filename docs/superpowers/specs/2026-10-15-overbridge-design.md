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

> **Status:** design proposal, awaiting review before planning.
> **Evidence:** the protocol comes from Overwitch (GPL-3; read for the format
> only, no code copied). The digitakt repo's Outbox 8 firmware analysis,
> `re/notes/outbox8.md` (commits b994da5 and abf93cc), independently matches it.
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
- **Hubs with several Overbridge devices.** It should work, since the host stack
  supports hubs, but it isn't tested or tuned for in this spec.

## 2. The protocol (as implemented here)

**Setup** (all from Overwitch, and confirmed in the Outbox firmware):

1. Configuration 1.
2. `SET_INTERFACE` interface 1 alt 3: audio from the device, endpoint `0x83`.
3. `SET_INTERFACE` interface 2 alt 3: audio to the device, endpoint `0x03`.

Overwitch also clears halts and reads the device name through vendor requests 1
and 2. The Outbox does neither. The host class does what the Outbox does and
treats the name as optional.

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

- **v2:** every sample is a big-endian `i32`.
- **v2.1** (Digitakt II and Digitone II, 42 channels from the device): within
  each frame, the first 14 channels are 4-byte samples and the remaining 28 are
  3-byte samples. To the device it stays `i32`.
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
  - pipes 3–5 when MaxPacket ≤ 512, at MXPS 512;
  - pipes 1–2 otherwise, at MXPS = the endpoint's MaxPacket, up to 1024.
- Configuration is `PIPECFG_TYPE_BULK | SHTNAK | DBLB` with IITV 0.
- The pipe remembers that it carries an interrupt endpoint (`PipeKind::IntrOnBulk`)
  so that statistics and teardown stay correct. Nothing else differs.

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

### 4.2 IITV at high speed

For interrupt pipes 6–9 the host computes `IITV = floor(log2(bInterval))`. That
is right at full speed. At high speed `bInterval` is an exponent (period
2^(bInterval−1) microframes), so the code should use `IITV = bInterval − 1`
(TRM §28.3.35; confirm against its table that IITV counts microframes on a
high-speed connection before landing the fix).

This is fixed independently of Overbridge. Today a high-speed MIDI or hub
interrupt endpoint is polled less often than it asks.

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
  - Bound by the BSP's USB host supervisor next to `host::uac` and `host::midi`.
    Overbridge is tried first for VID 0x1935, since some Elektron devices also
    expose class-compliant UAC in other USB modes.

### 5.2 Setup sequence

1. **Match.** The VID is 0x1935, and configuration 1 has interface 1 alt 3 with
   an interrupt IN endpoint and interface 2 alt 3 with an interrupt OUT endpoint.
2. **Size.** Derive the channel counts from each endpoint's MaxPacket and
   `Format`. Reject the device when the arithmetic doesn't give a whole number
   of channels.
3. **Select alternates.** `SET_INTERFACE`(1, 3), then `SET_INTERFACE`(2, 3).
4. **Name.** Optionally issue vendor requests 1/2 for the device name (32 bytes).
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
- The ring memory is a `static` sized by the app: `OverbridgeRing<CH, FRAMES>`.
  A device with more channels than `CH` reports `TooManyChannels` instead of
  truncating, so stem indices never silently shift.

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

**Descriptor layout** (matching what hosts look for, §2):
- One configuration.
- Interface 0: as on the real device, copied from the descriptors captured in
  §8.4 step 1. Until then, a vendor-specific interface with no endpoints. Its
  contents are not needed for audio.
- Interfaces 1 and 2, each with alts 0–3. Only alt 3 has an endpoint: interrupt,
  high speed, `bInterval` 1, MaxPacket = block size. Alts 1–2 exist as zero-bandwidth
  placeholders so that alt 3 has the number hosts select.
- Interface 3: optional USB-MIDI, composed by the board with `device-midi`.
- The board supplies an `OverbridgeConfig { ch_to_host, ch_from_host, format,
  name: &'static str }` along with the VID/PID passed to the embassy-usb builder.

**Behaviour**
- **Name.** Vendor requests 1/2 return `name`, padded to 32 bytes.
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
- Vendor requests 1/2.

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

1. **Capture descriptors.** With the Digitakt in Overbridge mode, dump its
   configuration descriptor through the current host supervisor's enumeration
   log. That checks the alt-3 sizes against §2 and feeds §8.1.
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

Sub-projects 1, 3 and 5 can proceed in parallel. 3 can stream a Digitakt over
the CFIFO fallback before 2 lands, which gives an early end-to-end check. 6 is
listed for completeness: it is planned in the digitakt repo once the ColdFire
USB device driver supports it.

## 10. Open questions

- **IN header check.** Does a real Digitakt ever send a header other than
  `0x0700`? The Outbox firmware's IN header check wasn't traced. The host
  class counts framing errors rather than failing, so this only affects stats.
- **Does the device need Overwitch's clear-halt or name requests?** The Outbox
  sends neither and works, so the host class doesn't depend on them.
- **The Outbox generic UAC path:** the channel limit and accepted sync mode
  (§7.1). Settled by hardware test 5.
