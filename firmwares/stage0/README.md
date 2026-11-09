# stage0

The first stage of the from-scratch Deluge boot chain: the code the RZ/A1L boot ROM runs from
flash offset `0x000000`. It executes in place from the SPI flash window (`0x1800_0000`), linked
with rza1-rt's `link_xip.x` and this crate's `memory_stage0.x` (64 KB of flash, 64 KB of RAM at
`0x2002_0000`).

> [!Warning]
> Stage-0 replaces the Deluge's stock first-stage bootloader. Installing it is for development
> units with a J-Link attached and a full flash backup; it is not part of a normal app workflow.

At boot it turns on the L1 I-cache (only that: MMU and D-cache stay off), reads and clears the
watchdog's overflow flag (`WRCSR.WOVF`, which the watchdog's own internal reset leaves set: the
last boot ended in a watchdog reset), then validates boot slots A (`0x010000`) and B (`0x080000`)
through the XIP window (`deluge_image::slot`: a 256-byte header page, then the body CRC), timing
the checks with the PMU cycle counter. `deluge_image::slot::select` picks one:

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

`STAGE0_STATUS` (10 words, for `mem32 <addr> 10` over J-Link) holds the tag `0x53544730`, the
state (1 validating, 2 entering a slot, 3 fault), the watchdog flag, both slots' error codes
(`0` = valid), both slots' sequences, the boot-info word written, the cycles both checks took,
and the raw `CPG.FRQCR` (low half).

Stage-0 never writes flash and never touches clocks, SDRAM, the MMU, the D-cache, the flash
controller or any peripheral but the watchdog's reset flag and that one pin. It pulls in no HAL:
the register addresses come from `rza1-pac`.

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
