# Hardware tasks

Checks that only real hardware can answer. Each one lists its setup, what passes, and what to
record. Record results under the task, with the date, the commit tested and the device's OS
version. Spec references are to
[`superpowers/specs/2026-10-15-overbridge-design.md`](superpowers/specs/2026-10-15-overbridge-design.md).

## USB host pipes: MIDI and hubs after the polling change (Overbridge sub-project 1)

High-speed interrupt pipes now poll at 2^(bInterval−1) microframes instead of
2^floor(log2(bInterval)). Hub status endpoints and interrupt-endpoint MIDI devices poll less
often than before.

- **Setup:** any host-mode firmware (`uac-host-firmware`, or `overbridge-host-firmware`). A
  USB-MIDI keyboard on the root port; then the same keyboard plus a second MIDI device behind a
  high-speed hub.
- **Pass:** both enumerate, notes arrive with no audible lag, and unplugging and replugging
  through the hub is noticed.
- **Record:** the hub model, and any device that stopped working.

## Host audio mode switching (Overbridge sub-project 7)

- **Setup:** a host build with `usb-host-uac,usb-host-overbridge` and a way to call
  `set_host_audio_mode` (a button handler or a debugger write). An Elektron box in Overbridge
  mode; then a class-compliant audio interface.
- **Pass:** with the Elektron box in Auto, Overbridge binds. Switching to Uac only re-binds
  within about a second, with no audio (its alt 3 isn't UAC); back to Auto re-binds Overbridge.
  Off drops audio but keeps MIDI. With the audio interface, Uac and Auto bind it and Overbridge
  only and Off don't. Switching while a hub is attached changes nothing.
- **Record:** the re-bind time, and anything that needed a replug.

## 1. Digitakt descriptors (spec §8.4 step 1)

- **Setup:** `cargo build-overbridge`, flash, Digitakt in Overbridge USB mode on the back
  port, `log` at debug.
- **Pass:** the logged configuration descriptor matches spec Appendix A byte for byte.
- **Record:** the Digitakt's OS version. If the bytes differ, put the new dump in the spec and
  update `DIGITAKT_CONFIG` in `usb/usb-classes/src/host/overbridge.rs`.

## 2. Digitakt stems (spec §8.4 step 2)

- **Setup:** as in 1. Play a pattern that uses every track. The harness logs `StreamStats` and
  each stem's peak every second.
- **Pass:** 10 minutes with `counter_gaps == 0`, `counter_resyncs == 0` and
  `framing_errors == 0`. Each track's peak
  moves with its own audio, with no crosstalk between stems.
- **Record:** `rate_ratio_ppm`, the `overruns`/`underruns` totals, and the CPU load, for the
  default build (receive DMA) and for `--features cpu-rx` (byte-wise CPU reads). If the DMA
  build logs "receive DMA did not finish" or shows `framing_errors`, record how often; the
  CPU build is the fallback.

## 3. Overbridge OUT path (spec §8.4 step 3)

- **Setup:** as in 2. The harness sends a 1 kHz tone at −12 dBFS on both channels to the
  Digitakt. On the Digitakt, route its Overbridge input to its main output.
- **Pass:** the tone is audible and clean (no clicks) for a minute.
- **Record:** anything the Digitakt shows about the host connection. Also whether the stream
  survives the Digitakt NAKing an OUT block: the RZ/A1L host sends bulk OUT, and after a NAK
  it sends a PING, which an interrupt endpoint doesn't define. If the Digitakt stalls or drops
  the OUT stream after its first NAK, that is the cause.

## 4. Digitakt II (spec §8.4 step 4, when one is available)

- **Setup:** as in 2–3, with a Digitakt II (42 channels from it, 8 to it).
- **Pass:** as in 2–3. This also settles whether a 1012-byte bulk-typed pipe on pipes 1–2
  works (spec §10).
- **Record:** whether the IN pipe opened at MXPS 1012, and any `framing_errors`.

## 5. Outbox 8 (spec §8.4 step 5)

- **Setup:** a device-mode Deluge build using `AudioClass<8>`, into an Elektron Outbox 8.
- **Pass:** 8 distinct channels on the Outbox's 8 outputs.
- **Record:** the sample rate and sync mode the Outbox accepts (spec §7.1), and its channel limit
  if it stops short of 8.

## 6. Overbridge device class (spec §8.4 step 6)

- **Setup:** two Deluges. One runs `overbridge-host-firmware`; the other runs
  `overbridge-device-firmware` built with `--features elektron-id` (`cargo build-overbridge-device
  --features elektron-id`), which presents `0x1935:0x0BF0`. The default build's pid.codes test
  identity binds to neither the host class nor Overwitch. Then connect the device Deluge to
  Overwitch on Linux, with a local `devices.json` entry for PID `0x0BF0` (12 tracks out, 2 in,
  version 2).
- **Pass:** the host Deluge logs stems 0–11 at the device's DC levels ((k + 1) << 24, about
  −42 dBFS for stem 0 rising 6 dB per doubling), with no counter gaps or resyncs. The device's `blocks_in`
  climbs by about 6857 a second (the host harness's tone). Overwitch streams the same stems.
- **Record:** the Overwitch version, and anything it logs about the device. The device side
  receives the interrupt OUT endpoint on a single-buffered bulk pipe, which may answer NYET, a
  handshake an interrupt OUT doesn't define. Record any xHCI/EHCI transfer errors Linux logs
  (`dmesg`) for endpoint 0x03.

## Device face switching (Overbridge sub-project 8)

- **Setup:** `overbridge-device-firmware` built with `--features face-switch` (and
  `elektron-id` to have the Deluge host class or Overwitch bind the Overbridge face), connected
  to a Linux host.
- **Pass:** every 30 s, `dmesg` shows a disconnect followed by a new device. It alternates
  between the Overbridge face (`1209:0001`, or `1935:0BF0`) and the UAC2 face (`1209:0002`,
  which ALSA lists as a sound card). Neither face fails to enumerate after a switch, over at
  least 10 switches.
- **Record:** the time from disconnect to the new face enumerating, and any enumeration errors
  in `dmesg`.

## Four MIDI devices on a hub (shared send pipe)

- **Setup:** any host build. A high-speed hub with four USB-MIDI devices, mixing full-speed
  (MaxPacket 64) and high-speed (512) if you have both. A sequence on the Deluge sends to all
  four while they send notes back.
- **Pass:** all four enumerate. Every device receives every note sent to it, with no stuck or
  repeated notes (a wrong data toggle shows up as silently dropped or duplicated packets). Notes
  from all four arrive. Unplugging one leaves the others working.
- **NAKing device:** make one device stop accepting data while the sequence runs (for example a
  device whose input buffer fills, or one held in a state where it NAKs its OUT endpoint). Each
  send to it gives up after 100 ms; the other three keep receiving their notes and keep sending
  theirs back, with at most a 100 ms hiccup per send to the stuck one. When the device accepts
  data again, it receives notes again.
- **Record:** the devices used, any device that drops notes, and how the NAKing case was made.
