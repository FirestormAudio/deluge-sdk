# arm-dsp

Safe Rust wrappers for the 32-bit Arm DSP instructions (SMMUL, QADD, SSAT and friends), for fast
fixed-point arithmetic on Cortex-A, Cortex-R and Cortex-M cores, with portable fallbacks that
give identical results on every other target. It works on stable Rust; the optional `nightly`
feature uses the unstable `core::arch::arm` intrinsics instead of inline assembly.

## Supported Instructions

- **QADD** - Saturating add
- **QSUB** - Saturating subtract  
- **QDADD** - Saturating double and add
- **QDSUB** - Saturating double and subtract
- **SMMUL** - Signed most significant word multiply (returns high 32 bits)
- **SMMULR** - SMMUL with rounding
- **SMMLA** - Signed most significant word multiply-accumulate
- **SMMLAR** - SMMLA with rounding
- **SMMLSR** - Signed most significant word multiply-subtract with rounding
- **SSAT** - Signed saturate to N bits
- **USAT** - Unsigned saturate to N bits
- **SSAT (LSL)** - Signed saturate with left shift
- **USAT (LSL)** - Unsigned saturate with left shift
- **VCVT** - Float to and from fixed point (`vcvt_f32_to_fixed`, `vcvt_fixed_to_f32`)

## Platform Support

| Target | Native instructions |
|---|---|
| ARMv7-A/R, e.g. Cortex-A9 (`armv7a-none-eabihf`, `armv7-unknown-linux-gnueabihf`) | all, including VCVT |
| Cortex-M4/M7 (`thumbv7em-none-eabi[hf]`) | all except VCVT (the M4F's FPU has it, but Rust reports no VFP feature there, so the conversion uses the fallback) |
| ARMv8-M Mainline, e.g. Cortex-M33 (`thumbv8m.main-none-eabi[hf]`) | with `-C target-cpu=cortex-m33` or `+dsp` |
| ARMv6, Cortex-M0/M3, AArch64, x86 and the rest | none: portable fallback |

The fast paths are chosen at compile time from the `dsp` (and, for VCVT, ARMv7 VFP) target
features, so code using the crate builds and tests unchanged on the host.

## Usage

All functions are available with both ARM instruction names and Rust-style aliases:

```rust
use arm_dsp::*;

// ARM-style names
let sum = qadd(i32::MAX, 100); // Returns i32::MAX (saturated)

// Rust-style aliases
let sum = saturating_add(i32::MAX, 100); // Same result

// Q31 fixed-point multiplication (returns high 32 bits)
let a = 0x40000000; // 0.5 in Q31
let b = 0x40000000; // 0.5 in Q31  
let result = smmul(a, b); // Returns 0x20000000 (0.25 in Q31)
// Or use the Rust-style alias:
let result = mul_high(a, b);

// With rounding
let rounded = smmulr(a, b);
// Or:
let rounded = mul_high_round(a, b);

// Saturate to 16-bit range
let saturated = ssat::<16>(100000); // Returns 32767
// Or:
let saturated = saturate_signed::<16>(100000);
```

## API Reference

### ARM Instruction Names → Rust Aliases

- `qadd` → `saturating_add`
- `qsub` → `saturating_sub`
- `qdadd` → `saturating_double_add`
- `qdsub` → `saturating_double_sub`
- `smmul` → `mul_high`
- `smmulr` → `mul_high_round`
- `smmla` → `mul_accumulate_high`
- `smmlar` → `mul_accumulate_high_round`
- `smmlsr` → `mul_subtract_high_round`
- `ssat` → `saturate_signed`
- `usat` → `saturate_unsigned`
- `ssat_lsl` → `saturate_signed_shl`
- `usat_lsl` → `saturate_unsigned_shl`

## Target configuration

Most targets above enable `dsp` by default. Where one doesn't, name the CPU:

```toml
[target.thumbv8m.main-none-eabihf]
rustflags = ["-C", "target-cpu=cortex-m33"]
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
