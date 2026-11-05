# Vendored: block-device-driver

The `BlockDevice` trait crate from
[MabezDev/embedded-fatfs](https://github.com/MabezDev/embedded-fatfs) at
`518528cc111fcf65c48abbdeb80735a38eada112`, unchanged. It is MIT/Apache-2.0; both license files are
upstream's.

It is vendored alongside [`block-device-adapters`](../block-device-adapters/VENDOR.md), which
path-depends on it. Its empty `[workspace]` table keeps it out of this repository's workspace, and
its `rustfmt.toml` keeps `cargo fmt --all` from reformatting it.
