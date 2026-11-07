# deluge-flash-protect

Status-register block protection for JEDEC SPI-NOR flash, with no hardware dependency: which
status-register bytes protect a part's bottom 1 MB, how to write them so that no permanent
(OTP) state can be reached, and the checks around a write (re-identify the part, read twice,
verify, never retry a failed write). The caller supplies the bus (`SrBus`).

Used by the Deluge SDK's boot chain and its flash tools. Supported parts: AT25SF321B, W25Q32JV,
S25FL132K, AT25QF641B.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE) at your
option.
