# `additive_osc`

A polyphonic **additive** synth for the Deluge whose per-block DSP core is
**C++/[Argon](https://github.com/stellar-aria/argon)** (ARM NEON SIMD), reached
from the Rust app over **FFI**.

It is the [`test_osc`](../test_osc) pad-keyboard synth — the 16-wide grid as an
isomorphic keyboard (one pad right = +1 semitone, one pad up = +5 = a perfect
fourth), with an OLED oscilloscope of its own output — but the sound is not
synthesised in Rust. Every audio block, the active voices are handed across a C
ABI to `additive_render` in [`csrc/additive.cpp`](csrc/additive.cpp), which sums
each voice from up to 64 sine partials using Argon, **four samples at a time** in
SIMD.

## What it demonstrates

- **FFI** — a Rust ↔ C++ boundary. `build.rs` compiles the C++ with the `cc`
  crate; `AdditiveVoice` is shared field-for-field (`#[repr(C)]`), and the C++
  advances each voice's phase in place across blocks.
- **Real-world SIMD DSP** — Argon has no transcendentals, so the example carries
  a **vectorised parabolic sine** plus vectorised phase accumulation and harmonic
  summation. On the device this is NEON; on the x86-64 simulator host Argon
  transparently falls back to SIMDe, so the same source runs under
  `cargo deluge sim`.

## Playing it

Press pads to play (polyphonic, up to 8 voices). Timbre is live:

| Control | Effect |
|---------|--------|
| **Gold encoder 0** | harmonic count, 1–32 (brightness / bandwidth) |
| **Gold encoder 1** | spectral roll-off order, 0–3 (0 = buzzy/equal, 1 ≈ sawtooth, 2–3 = darker) |

The OLED title shows the current `H<harmonics> R<rolloff>`, and the scope shows
the resulting waveform getting richer as you add partials.

## Building — requires Argon

Unlike the other examples, this one needs the **Argon** C++ headers at build
time. They are located via the `ARGON_SRC` environment variable, defaulting to a
sibling checkout at `../../../argon` (i.e. next to the `deluge-sdk` repo):

```sh
git clone https://github.com/stellar-aria/argon ../../../../argon   # if not already a sibling
# or point ARGON_SRC at an existing checkout:
export ARGON_SRC=/path/to/argon

cargo deluge sim          # build for the host + run in the simulator
cargo deluge build        # build the firmware ELF (needs arm-none-eabi-g++)
```

Argon requires a C++23 compiler (GCC ≥ 14.2 / Clang ≥ 20.1). The firmware build
additionally needs `arm-none-eabi-g++`; the host/simulator build needs SIMDe on
the include path (`/usr/include/simde` by default, overridable via `SIMDE_SRC`).

> **CI note:** the SDK's example compile-proofs build every example on the
> firmware target. This one additionally needs `arm-none-eabi-g++` **and** an
> Argon checkout (`ARGON_SRC`) on the runner — wire those into CI (or gate this
> example) before relying on it there.
