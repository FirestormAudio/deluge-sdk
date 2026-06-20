# fixedpoint

A type-safe, `no_std` fixed-point arithmetic library for Rust.

## Features

- **Type-safe**: Different fixed-point formats are distinct types, preventing accidental mixing
- **Generic over precision**: Use const generics to specify fractional bits (e.g., `FixedPoint<31>` for Q31 format)
- **Saturating arithmetic**: All operations saturate on overflow/underflow instead of wrapping
- **Optional rounding**: Control rounding behavior at the type level
- **no_std compatible**: Works in embedded environments
- **Comprehensive testing**: unit + property-based tests, run on host and under QEMU against the real ARM DSP instructions
- **Well-documented**: Extensive docs and examples

## Quick Start

```rust
use fixedpoint::{Q31, Q16};

// Create fixed-point numbers from floats
let a = Q31::from_float(0.5);
let b = Q31::from_float(0.25);

// Arithmetic operations
let sum = a + b;           // 0.75
let diff = a - b;          // 0.25
let product = a * b;       // 0.125
let quotient = a / b;      // 2.0 (saturates to MAX for Q31)

// Convert back to float
println!("Result: {}", sum.to_float());

// Convert between formats
let q16: Q16 = a.convert();
```

## Supported Formats

Common type aliases are provided:

- `Q31` / `Q31Rounded`: 31 fractional bits, range [-1.0, 1.0)
- `Q24` / `Q24Rounded`: 24 fractional bits, range [-128.0, 128.0)
- `Q16` / `Q16Rounded`: 16 fractional bits, range [-32768.0, 32768.0)

Or create custom formats:

```rust
use fixedpoint::FixedPoint;

type Q20 = FixedPoint<20>;  // 20 fractional bits
type Q28Rounded = FixedPoint<28, true>;  // 28 fractional bits with rounding
```

## Operations

All standard arithmetic operations are supported:

```rust
use fixedpoint::Q31;

let a = Q31::from_float(0.5);
let b = Q31::from_float(0.25);

// Arithmetic
let _ = a + b;   // Addition
let _ = a - b;   // Subtraction
let _ = a * b;   // Multiplication
let _ = a / b;   // Division
let _ = -a;      // Negation

// Comparisons
assert!(a > b);
assert!(a != b);
assert_eq!(a, a);

// Special operations
let _ = a.abs();              // Absolute value
let _ = a.mul_add(b, b);      // Fused multiply-add: a + b * b
let _ = a.mul_int(5);         // Multiply by integer
let _ = a.div_int(2);         // Divide by integer
```

## Conversions

```rust
use fixedpoint::{Q31, Q16};

// From float/double
let fp = Q31::from_float(0.75);
let fp = Q31::from_double(0.123456789);

// From integer
let fp = Q16::from_int(42);

// From raw bits
let fp = Q31::from_raw(0x40000000);

// To float/double
let f: f32 = fp.to_float();
let d: f64 = fp.to_double();

// To integer (truncates or rounds)
let i: i32 = fp.to_int();

// Between fixed-point formats
let q31 = Q31::from_float(0.5);
let q16: Q16 = q31.convert();
```

## Rounding

By default, operations truncate. Use the `Rounded` type parameter for rounding:

```rust
use fixedpoint::{Q16, Q16Rounded};

let truncated = Q16::from_float(42.7);
assert_eq!(truncated.to_int(), 42);

let rounded = Q16Rounded::from_float(42.7);
assert_eq!(rounded.to_int(), 43);
```

## Safety and Correctness

This library is tested on two fronts:

- **Unit tests** covering edge cases, overflow, underflow, and special values.
- **Property-based tests** (proptest) verifying mathematical properties —
  commutativity, identities, saturation, round-trips, and division (including
  the divide-by-zero edge) against independent references.

Because the multiply/divide hot paths lower to ARM DSP instructions, the suite
runs in two buckets (see `tools/test.sh` at the workspace root): on the host
(portable path) **and** under QEMU on `armv7-unknown-linux-gnueabihf`, where
e.g. `from_float` is the genuine `VCVT` instruction — so the hardware path is
cross-checked against the portable one, not assumed equivalent.

Run the tests (host):

```bash
cargo test --target x86_64-unknown-linux-gnu
```

## Performance

- All operations are `#[inline(always)]` for zero-cost abstraction
- Uses 64-bit intermediate values for multiplication/division
- Saturating arithmetic compiles to efficient machine code
- `no_std` compatible with no allocations

## Use Cases

Perfect for:
- **Audio DSP**: Filters, oscillators, envelopes (Q31 format matches audio sample range)
- **Embedded systems**: Deterministic arithmetic without floating-point hardware
- **Graphics**: Fixed-point is often faster than float on some architectures
- **Finance**: Exact decimal representation requirements

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.

## Contributing

Contributions are welcome! Please:
- Add tests for new functionality
- Run `cargo test` and `cargo clippy`
- Update documentation as needed
