# Vendored: block-device-adapters

`BufStream` (byte-level `embedded-io-async` read/write/seek over a block device) and `StreamSlice`
(a window into a stream), from
[MabezDev/embedded-fatfs](https://github.com/MabezDev/embedded-fatfs) at
`518528cc111fcf65c48abbdeb80735a38eada112`. It is MIT/Apache-2.0; both license files are upstream's.
Its `block-device-driver` dependency points at the vendored [sibling](../block-device-driver/).
Its empty `[workspace]` table keeps it out of this repository's workspace, and its `rustfmt.toml`
keeps `cargo fmt --all` from reformatting it.

## Upstream pull requests applied

- **#68:** zero-length reads and writes are a no-op instead of a zero-block device transfer, and
  `StreamSlice` computes the remaining length in `u64` before narrowing. On a 32-bit target a
  remaining length of 4 GiB or more used to truncate.
- **#62:** `BufStream::seek` and `StreamSlice::seek` use checked arithmetic. `StreamSlice` returns
  `StreamSliceError::InvalidSeek` on overflow instead of wrapping.
