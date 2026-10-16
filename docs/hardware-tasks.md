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

## 1. Digitakt descriptors (spec §8.4 step 1)

- **Setup:** `cargo build-overbridge`, flash, Digitakt in Overbridge USB mode on the back
  port, `log` at debug.
- **Pass:** the logged configuration descriptor matches spec Appendix A byte for byte.
- **Record:** the Digitakt's OS version. If the bytes differ, put the new dump in the spec and
  update `DIGITAKT_CONFIG` in `usb/usb-classes/src/host/overbridge.rs`.

## 2. Digitakt stems (spec §8.4 step 2)

- **Setup:** as in 1. Play a pattern that uses every track. The harness logs `StreamStats` and
  each stem's peak every second.
- **Pass:** 10 minutes with `counter_gaps == 0` and `framing_errors == 0`. Each track's peak
  moves with its own audio, with no crosstalk between stems.
- **Record:** `rate_ratio_ppm`, the `overruns`/`underruns` totals, and the CPU load. The CPU
  path reads the FIFO a byte at a time, so the load matters here. Repeat once FIFO DMA lands
  (sub-project 2) and record the difference.

## 3. Overbridge OUT path (spec §8.4 step 3)

- **Setup:** as in 2. The harness sends a 1 kHz tone at −12 dBFS on both channels to the
  Digitakt. On the Digitakt, route its Overbridge input to its main output.
- **Pass:** the tone is audible and clean (no clicks) for a minute.
- **Record:** anything the Digitakt shows about the host connection.

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

- **Setup:** two Deluges, one hosting and one presenting the Overbridge device class with a test
  identity. Then the device Deluge against Overwitch on Linux, with a local `devices.json` entry
  for its test PID.
- **Pass:** stems loop through Deluge to Deluge, and Overwitch streams them.
- **Record:** the Overwitch version, and anything it logs about the device.
