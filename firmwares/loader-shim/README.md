# loader-shim

What [stage-0](../stage0) enters in a loader's boot slot on the Deluge (RZ/A1L). A loader slot
holds this shim, executing in place right after the slot's 256-byte header page, and at
`slot + 0x1000` the app-loader itself: its RAM image under a `deluge_image::app` header. The shim
checks the payload's header, copies it to its load address (the loader's SRAM, `0x2002_0000`),
starts the watchdog if its slot is still pending (the loader kicks it until it confirms itself;
a loader that hangs first is reset and stage-0 rolls back), and branches to the loader.

The shim executes in place from whichever slot holds it, so it is built position-independent
(`-C relocation-model=ropi`): code and constants are reached PC-relative, it finds its slot from
its own address, and its RAM is fixed. One slot image fits either slot. `tools/loader-image.sh`
proves it by linking the shim for slot A and for slot B (`--features slot-b`) and requiring the
same bytes. rza1-rt's reset handler sets VBAR and calls the fault reporter PC-relative for the
same reason.

On a payload it refuses it blinks the SYNC LED (P6_7) `code` short blinks per round, forever:
1 bad magic, 2 version, 3 header CRC, 4 variant, 5 body length, 6 entry, 7 truncated, 8 body
CRC. `SHIM_STATUS` (`mem32 <addr> 5` over J-Link) holds the tag `b"SHIM"`, the state (1 checking,
2 entering, 3 fault), the error code, the payload's length and its entry.

## Building

`tools/loader-image.sh` builds stage-0, the app-loader and both shims, and writes
`target/loader-images/`: `LOADER.SLT` (the loader's slot image; copy it to the card's `/BOOT/`
for UPDATE LOADER) and `stage0+loader.bin` (stage-0 and the slot image as slot A, padded to 1 MB,
for a J-Link install). `tools/check-loader-image.py` checks them.

The shim's RAM is the upper half of the data-retention RAM (`0x2001_0000`), clear of the boot-info
word stage-0 leaves at `0x2000_0000` for the loader and of the loader's own SRAM.
