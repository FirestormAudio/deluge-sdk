# ARMv7 DSP Intrinsics

Safe Rust wrappers for ARMv7-A DSP instructions, providing efficient fixed-point arithmetic operations.

## Overview

This crate provides access to ARM DSP instructions like SMMUL, SMMULR, QADD, and QSUB through safe Rust APIs. These are particularly useful for high-performance fixed-point arithmetic in embedded systems.

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

## Platform Support

- **ARM targets with DSP**: Uses native ARM instructions via inline assembly
- **Other platforms**: Falls back to portable Rust implementations

## Usage

All functions are available with both ARM instruction names and Rust-style aliases:

```rust
use armv7_dsp_intrinsics::*;

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

## Target Configuration

For ARM Cortex-A7

```toml
[target.armv7-unknown-linux-gnueabihf]
rustflags = ["-C", "target-cpu=cortex-a7", "-C", "target-feature=+dsp"]
```
