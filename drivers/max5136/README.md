# max5136

A platform-agnostic [`embedded-hal`](https://crates.io/crates/embedded-hal) 1.0 driver for the
Maxim MAX5136, a quad 16-bit SPI DAC. The Deluge uses one for its two CV outputs. The crate is
`no_std`, has no dependency other than `embedded-hal`, and has no Cargo features.

Every command is a 24-bit frame sent MSB first: a command byte, then 16 data bits. The crate
offers two layers:

- `Max5136<SPI>`, a driver over any `embedded_hal::spi::SpiDevice<u8>` (which owns the chip
  select): `new`, `init` (the power-up linearity calibration), `set` and `release`.
- `const` frame helpers for boards that move frames over their own transport: `write_through`,
  `LINEARITY_ON` / `LINEARITY_OFF` and `LINEARITY_SETTLE_MS`. The `*_u32` forms pad a frame
  into a 32-bit word (`frame << 8`) for SPI controllers run in 32-bit mode; the part ignores the
  trailing byte.

At power-up the part needs a linearity calibration: send `LINEARITY_ON`, wait at least
`LINEARITY_SETTLE_MS`, then send `LINEARITY_OFF`. `Max5136::init` does exactly that.

## Usage

```rust,ignore
use max5136::Max5136;

// `spi` is any `SpiDevice<u8>`, `delay` any `DelayNs`.
let mut dac = Max5136::new(spi);
dac.init(&mut delay)?;
dac.set(0, 0x8000)?; // channel 0 to mid-scale (0 = zero scale, 65535 = full scale)
let spi = dac.release();
```

Without the driver, for a bus that sends 32-bit words:

```rust,ignore
bus.send32(max5136::LINEARITY_ON_U32);
delay_ms(max5136::LINEARITY_SETTLE_MS);
bus.send32(max5136::LINEARITY_OFF_U32);
bus.send32(max5136::write_through_u32(1, 0xFFFF)); // channel 1 to full scale
```

`set` and `write_through` panic for a channel of `NUM_CHANNELS` (4) or more. The Deluge's CV
driver, [`deluge_bsp::cv_gate`](../../crates/deluge-bsp/src/cv_gate.rs), takes the second form
over its shared RSPI0 bus.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE) at your
option.
