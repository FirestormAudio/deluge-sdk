/* The stock Deluge first-stage bootloader (DelugeBootloader) executes from SRAM at 0x20020000:
 * its code and initialised data run to about 0x20032F30, and its boot_demo() copy loop sits at
 * 0x20022C40. It copies this image to `_start` word by word, so an image that starts below the
 * end of that code overwrites the loop that is copying it, and the copy dies part-way (v0.1.0:
 * 11 KB in, then a data abort). The zero-init data below the image (rza1-rt's
 * `zero-init-below-image`) lifts `_start` above it; this fails the link if it ever does not.
 * 0x20040000 leaves ~52 KB of margin over the bootloader's code and data. */
ASSERT(_start >= 0x20040000,
       "app-loader: _start is below 0x20040000, inside the stock bootloader's SRAM, which would overwrite its copy loop: keep deluge-bsp's zero-init-below-image feature on")
