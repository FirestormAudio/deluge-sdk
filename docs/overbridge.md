# Overbridge 2 wire format

The Elektron Overbridge 2 protocol as implemented in `usb-classes` (`overbridge::proto` and the
host and device classes). Sources: Overwitch (read for the format only) and an analysis of the
Outbox 8 firmware. Checks that need real hardware are in [`hardware-tasks.md`](hardware-tasks.md).

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
  MIDIStreaming interfaces (see "The Digitakt's descriptors"), not an Overbridge channel.
- **Transfer size.** It queues interrupt transfers of 6–24 blocks (default 24,
  8832 bytes IN for a Digitakt). Each block is one full `wMaxPacketSize` packet,
  so the wire traffic is the same as one block per transfer.
- **Size check.** It refuses a device whose alt-3 `wMaxPacketSize` isn't exactly
  `32 + 7 × frame_bytes` for its `devices.json` entry.

The host class does what the Outbox does and treats the name as optional. If it
does read the name, it waits 100 ms afterwards before opening the streams.

## The Digitakt's descriptors (OS 1.54)

Decoded from the firmware image (the bytes are at the end of this page). Hardware task 1
in `hardware-tasks.md` confirms them on a running unit.

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

## Digitakt (OS 1.54) Overbridge configuration descriptor

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
