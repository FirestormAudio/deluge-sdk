# deluge-fft

A portable-SIMD, `no_std`, const-generic FFT for the Deluge SDK audio pipeline.

## Design

- **No heap** — all working memory is on the stack; no global allocator needed.
- **Compile-time twiddle table** — `W_N^k` factors are evaluated once at compile
  time via Taylor-series `const fn`s over `f64`, stored in Flash/ROM on embedded
  targets. The hot loop reads a flat table and does arithmetic — zero runtime
  trig cost.
- **Portable SIMD** — butterfly inner loops operate on `core::simd::Simd<f32, LANES>`.
  On Cortex-A9 with `+neon`, LLVM emits `float32x4_t` instructions for `LANES = 4`.
- **Multiple layouts** — AoS `[Complex; N]` for interop; SoA `FftBuf<N>` for
  maximally-vectorisable sequential loads/stores.
- **Radix-4 / radix-8** — `process_r4_simd` / `process_r8_simd_soa` merge stage
  pairs/triples for fewer passes over the array.
- **Real-input FFT** — `RealFft<N, LANES>` packs N real samples into a length-N/2
  complex FFT plus post-processing, ~2× faster for real audio.

## Usage

```toml
[dependencies]
deluge-fft = "0.1"
```

```rust,ignore
use deluge_fft::{Fft, Complex};

let mut data: [Complex; 1024] = make_signal();
Fft::<1024>::new().process(&mut data);
```

## Toolchain

Uses the nightly features `portable_simd` and `generic_const_exprs`, so a
nightly toolchain is required.

## Features

| Feature | Description |
|---|---|
| `test-utils` | Expose the reference DFT and round-trip helpers used by the test suite. |

## Benchmarks

A Criterion harness (`benches/fft_bench.rs`) compares against `rustfft`. The
QEMU runner in `.cargo/config.toml` emulates the Cortex-A9 (VFPv3 + NEON) so
benchmark codegen is representative of the device:

```bash
cargo bench --target armv7-unknown-linux-gnueabihf
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
