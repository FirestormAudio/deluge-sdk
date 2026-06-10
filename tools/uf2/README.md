# Deluge SSB UF2 firmware images

The second-stage bootloader can flash a firmware image into the on-board SPI
flash over USB. At the boot menu, select **UPDATE FW** (the menu also auto-enters
update mode when there is nothing to boot); the Deluge appears as a USB drive.
Drag a `.uf2` onto it to program flash, or copy `CURRENT.UF2` off to back up the
currently installed image. Press **BACK** to leave update mode and return to the
boot menu.

The boot menu also offers **DATA TRANSFER**, which exposes the inserted SD card
as a USB mass-storage device (raw block passthrough); **BACK** exits it too.

## Layout the bootloader expects

| Property              | Value                                  |
|-----------------------|----------------------------------------|
| App slot address      | `0x18100000`                            |
| Slot length           | `0x00300000` (3 MB)                      |
| UF2 family ID         | `0x6E275A1C` (custom — Deluge SSB)       |
| Payload per block     | 256 bytes                               |

### Flash map (QSPI, base `0x18000000`)

| Offset                | Region                                   |
|-----------------------|------------------------------------------|
| `0x00000`–`0x7F000`   | First-stage bootloader (FSB) — reserved  |
| `0x7F000`–`0x80000`   | Deluge device-settings sector — reserved |
| `0x80000`–`0x100000`  | Second-stage bootloader (SSB) — reserved |
| `0x100000`–`0x400000` | **App slot** (UF2 / boot-from-flash)     |

The app slot sits **above** the FSB, the settings sector, and the SSB, so a
firmware update can never touch any of them. The driver enforces this in hardware
terms too: every erase/program is refused unless it lies entirely within
`0x100000..0x400000` (see `writable()` in `rza1l-hal/src/spibsc.rs`).

The flash chip is a **Spansion S25FL512** with uniform **256 KB** sectors (no
smaller erase). The slot base (`0x100000`) and length (3 MB) are 256 KB-aligned,
so an erase never spills below the slot. A full 3 MB reflash erases twelve 256 KB
sectors; each sector erase takes up to ~2.6 s, so a full update can take tens of
seconds (progress is shown on the OLED).

The slot address is where the SPIBSC memory-maps the SPI flash chip for reads, so
it is the `targetAddr` the UF2 blocks carry (the bootloader subtracts the
`0x18000000` window base to get the flash offset to erase/program). It is **not**
an execute-in-place address — the image is copied into SRAM and run there (below).

The image is a **raw, RAM-linked `.bin`** (loads/runs at `0x20020000`), the same
artifact `cargo build-fw-bin` produces. The bootloader reads the embedded FSB
metadata at `bin + 0x20` (`code_start` / `code_end` / `code_execute` +
`.BootLoad_ValidProgramTest.` signature), copies `code_start..code_end` from
flash into SRAM via the trampoline, and jumps to `code_execute`.

## Building a `.uf2`

Use the `elf2uf2` host tool (`tools/elf2uf2`), which does ELF → flat `.bin` →
UF2 in one step from the *same* RAM-linked ELF the SD-card boot path consumes —
no `objcopy` or `uf2conv.py` needed:

```sh
cargo build-fw-rel   # -> target/armv7a-none-eabihf/release/demo-firmware (ELF)
cargo elf2uf2 target/armv7a-none-eabihf/release/demo-firmware -o demo-firmware.uf2
```

`elf2uf2` validates the embedded FSB metadata (`code_start` / `code_end` /
`code_execute` + `.BootLoad_ValidProgramTest.` signature) before emitting, so a
non-bootable image is caught on the host rather than as a silent non-boot on the
device. The defaults (`--base 0x18100000`, `--family 0x6E275A1C`, 256-byte
payloads) mirror `spibsc::FLASH_SLOT_ADDR` and `uf2::UF2_FAMILY_DELUGE`. Pass
`--bin` to also emit the intermediate flat binary, `--help` for all options.

The emitted flat image is byte-identical to `arm-none-eabi-objcopy -O binary`,
and the UF2 blocks match what Microsoft's `uf2conv.py` would produce — so the
old two-step pipeline still works if you prefer it:

```sh
cargo build-fw-bin   # -> target/.../demo-firmware.bin  (objcopy -O binary)
uf2conv.py target/armv7a-none-eabihf/release/demo-firmware.bin \
    --base 0x18100000 --family 0x6E275A1C --convert --output demo-firmware.uf2
```

The `--family` **must** match `UF2_FAMILY_DELUGE` in
`app-loader/src/uf2.rs`; blocks with any other family ID are
ignored so a foreign image can never be programmed.
