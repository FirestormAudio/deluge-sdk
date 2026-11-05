# Fork: deluge-block-device-adapters

`BufStream` (byte-level `embedded-io-async` read/write/seek over a block device) and `StreamSlice`
(a window into a stream), from
[MabezDev/embedded-fatfs](https://github.com/MabezDev/embedded-fatfs) at
`518528cc111fcf65c48abbdeb80735a38eada112`, published as `deluge-block-device-adapters` (the
library is still `block_device_adapters`). It is MIT-licensed like upstream; the license files
are upstream's. It depends on `block-device-driver` 0.2 from crates.io, which is upstream's
unchanged. Its empty `[workspace]` table keeps it out of this repository's workspace, and its `rustfmt.toml`
keeps `cargo fmt --all` from reformatting it.

## Upstream pull requests applied

- **#68:** zero-length reads and writes are a no-op instead of a zero-block device transfer, and
  `StreamSlice` computes the remaining length in `u64` before narrowing. On a 32-bit target a
  remaining length of 4 GiB or more used to truncate.
- **#62:** `BufStream::seek` and `StreamSlice::seek` use checked arithmetic. `StreamSlice` returns
  `StreamSliceError::InvalidSeek` on overflow instead of wrapping.
