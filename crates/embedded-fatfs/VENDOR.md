# Fork: deluge-embedded-fatfs

An async, `no_std` FAT12/16/32 filesystem, vendored from
[MabezDev/embedded-fatfs](https://github.com/MabezDev/embedded-fatfs) at
`518528cc111fcf65c48abbdeb80735a38eada112` (core crate only: `Cargo.toml`, `src/`, `LICENSE`,
`README.md`). It is MIT-licensed; `LICENSE` is upstream's, unchanged.

Upstream has no usable crates.io release, so this fork is published as `deluge-embedded-fatfs`
(the library is still `embedded_fatfs`). Its empty `[workspace]` table keeps it out of this repository's workspace, and its `rustfmt.toml`
keeps `cargo fmt --all` from reformatting it.

## Upstream pull requests applied

- **#64:** FAT12/16 no longer write FAT32-only status flags into the BPB;
  `total_sectors_16`/`total_sectors_32` are validated as mutually exclusive; `NullTimeProvider`
  returns a valid 1980-01-01 timestamp.
- **#55:** `FileSystem::new` seeks the storage to offset 0 instead of only asserting it there,
  which release builds compiled out.
- **#59:** a resumed `FileContext` is checked against the on-disk directory entry before it is
  trusted. `to_file_with_context` and `try_to_file_with_context` become `async`. Supersedes #46.

## Local changes

- **`Dir::rename` with a multi-component destination.** The destination path was resolved from the
  wrong directory and then discarded, so `"DIR/NAME"` renamed to `/NAME`. Fixed in `src/dir.rs`.
- **`..` in first-level FAT32 directories** now carries cluster 0, as the FAT specification
  requires (a port of `rafalh/rust-fatfs` `c4bb769`).
- **Forward seeks** across clusters continue from the current cluster instead of walking the chain
  from its start.
- **Cluster-map accessors** (`FileSystem::follow_chain`, `cluster_first_sector`,
  `sectors_per_cluster`, `bytes_per_sector`; `FileContext::first_cluster`, `size`,
  `entry_position`) so a caller can map a file's extents and stream them straight from the card.
- **A directory name index** (`src/dir_index.rs`, `alloc` only): lookups check a bounded cache of
  name hashes first and always verify a hit against the entry on disk, so the cache can go stale
  without giving a wrong answer.

- **Features (0.2.0).** The defaults are `lfn` and `unicode`, so a default build is `no_std`;
  `chrono` is opt-in and implies `std`; `log` and `defmt` may both be on, with `defmt` used;
  the unused `elain` dependency is gone.

## Not applied

- **#66** (updating a moved directory's own `..` entry): not yet exercised by our tests.
- **#32** (a corrupt LFN entry aborts the whole listing): rejected, because listing what can be
  read is preferable after a power loss.
- **`format_volume` fixes:** the SDK never formats a card.
