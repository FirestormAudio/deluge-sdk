//! The Deluge's boot header: what its first-stage bootloader (`spibsc_init2.c`) checks before it
//! copies the image from SPI flash to SRAM and jumps to it. rza1-rt's link.x places the
//! `.boot_header` section straight after the vector table, so the header lands at `_start + 0x20`:
//!
//! | Offset | Word | |
//! |---|---|---|
//! | +0x20 | `_start` | code_start: where the copy goes (at least 0x2002_0000; below that is retention RAM the bootloader will not write) |
//! | +0x24 | `__metadata_code_end` | code_end: the copy runs `_start..code_end` |
//! | +0x28 | `_start` | code_execute: the jump target |
//! | +0x2C | `".BootLoad_ValidProgramTest."` | the signature the bootloader validates |
//!
//! `deluge-image` and the app-loader read the same offsets (`FSB_*`); `tools/check-boot-header.py`
//! checks every image.

use core::arch::global_asm;

global_asm!(
    r#"
    .pushsection .boot_header, "a"
    .global __rza1_boot_header
__rza1_boot_header:
    .word _start
    .word __metadata_code_end
    .word _start
    .asciz ".BootLoad_ValidProgramTest."
    .align 4
    .popsection
"#
);
