# stage0

The first stage of the from-scratch Deluge boot chain: the code the RZ/A1L boot ROM runs from
flash offset `0x000000`. It executes in place from the SPI flash window (`0x1800_0000`), linked
with rza1-rt's `link_xip.x` and this crate's `memory_stage0.x` (64 KB of flash, 64 KB of RAM at
`0x2002_0000`).

> [!Warning]
> Stage-0 replaces the Deluge's stock first-stage bootloader. Installing it is for development
> units with a J-Link attached and a full flash backup; it is not part of a normal app workflow.

At boot it first switches the flash controller from the boot ROM's reads (`03h` at a slow clock,
the read cache off) to the stock bootloader's: `0Bh` fast read, SPBR = 1, BRDV = 0 (66.67 Mbps),
the read cache on, as the loader and every stock unit run. The switch runs from RAM (`.data`),
since nothing may execute from the flash window while its timing changes. Then it turns on the L1
I-cache (only that: MMU and D-cache stay off), reads and clears the watchdog's overflow flag
(`WRCSR.WOVF`, which the watchdog's own internal reset leaves set: the last boot ended in a
watchdog reset), then checks boot slots A (`0x010000`) and B (`0x080000`) through the XIP window,
timing the checks with the PMU cycle counter. It parses both 256-byte header pages, and
`deluge_image::slot::select_lazy` picks one, CRC-checking only the bodies the choice depends on:
the booted slot's, plus the pending slot's when it rolls back from one. The CRC table is copied
into RAM with `.data`, since a lookup through the uncached XIP window would cost a flash read. The
choice is `deluge_image::slot::select`'s:

- the valid slot with the higher sequence (A on a tie);
- a confirmed one boots whatever the watchdog flag says;
- a pending one the watchdog killed falls back to the other valid slot, or, when it is the only
  valid slot, boots again.

It writes the boot-info word (`0x5330_ssrr`: slot, reason) to retention RAM at `0x2000_0000` and
branches to the slot's entry. The slot's own reset handler disables and invalidates the I-cache
before it relies on it. With neither slot valid it blinks the SYNC LED (P6_7) `code` short blinks
per round, forever, where `code` is slot A's `SlotError::code()`: 1 bad magic (erased slot),
2 version, 3 header CRC, 4 variant, 5 body length, 6 entry, 7 truncated, 8 body CRC.
`STAGE0_HEARTBEAT` (starts at `0x5A5A_0000`) counts the toggles.

`STAGE0_STATUS` (10 words, for `mem32 <addr> 10` over J-Link) holds the tag `0x53544730`, the state
(1 validating, 2 entering a slot, 3 fault), the watchdog flag, both slots' error codes (`0` =
valid, `0xFF` = header valid, body not needed), both slots' sequences, the boot-info word written,
the cycles the checks took, and the raw `CPG.FRQCR` (low half).

Stage-0 never writes flash and never touches clocks, SDRAM, the MMU, the D-cache or any peripheral
but the flash controller's read mode, the watchdog's reset flag and that one pin. It pulls in no HAL:
the register addresses come from `rza1-pac`.

## Boot timing

Measured on the J-Trace rig (2026-11-15, W25Q32JV, loader slot ~265 KB). Stage-0 runs at the boot
ROM's clocks (`FRQCR` `0x0335`: the CPU at 133 MHz); cycles are `STAGE0_STATUS.crc_cycles`, and
"to the loader" is the PMU cycle count at the loader's entry, both from stage-0's `main`.

| Stage-0 | Slots | Slot check | To the loader |
| --- | --- | --- | --- |
| both bodies, bitwise CRC, ROM reads | A | 144.0 M cycles, 1.08 s | |
| both bodies, bitwise CRC, ROM reads | A, B | 288.9 M cycles, 2.17 s | |
| booted body only, table CRC, ROM reads | A | 151.5 M cycles, 1.14 s | 211.2 M cycles, 1.58 s |
| booted body only, table CRC, ROM reads | A, B | 137.8 M cycles, 1.03 s | |
| booted body only, table CRC, fast reads | A | 23.0 M cycles, 0.17 s | 32.5 M cycles, 0.24 s |
| fast reads, newer B's body bad (A boots) | A, B | 43.9 M cycles, 0.33 s | |

The flash reads, not the CRC, were the cost: the table CRC alone saved ~6% per byte. The ROM
leaves `03h` reads at `SPBCR` = 3 with the read cache off; the stock fast-read setup is ~6.6×
faster, for stage-0's check and the shim's copy alike. The loader's display is up ~110 ms after
its entry, so the droplet appears ~0.35 s after reset (was ~1.7 s, ~2.6 s with two valid slots).

## Building

```sh
cargo build-stage0          # target/armv7a-none-eabihf/release/stage0 (ELF)
cargo build-stage0-bin      # target/armv7a-none-eabihf/release/stage0.bin (raw, for offset 0)
tools/check-xip-image.py --flash 0x18000000:0x10000 --ram 0x20020000:0x10000 \
  --require-symbol STAGE0_HEARTBEAT --require-symbol STAGE0_STATUS target/armv7a-none-eabihf/release/stage0
```

The layout check must pass before an image is installed. Installing it is
`bootloader-flasher`'s `image`/`wipe` modes over J-Link (see that crate's documentation). Slot
images are wrapped and composed with stage-0 by `mkslot`
(`cargo run --target x86_64-unknown-linux-gnu -p deluge-image --example mkslot -- --help`).
