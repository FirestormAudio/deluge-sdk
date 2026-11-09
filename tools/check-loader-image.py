#!/usr/bin/env python3
"""Check the Deluge loader's boot-slot image (tools/loader-image.sh's output), independently of
mkslot: its header and body CRC (deluge_image::slot), and the payload (the loader under a
deluge_image::app header): its header, body CRC and variant; and that the install image holds
stage-0 and the slot image as slot A, padded to 1 MB.

Usage: tools/check-loader-image.py <dir>
"""

import struct
import sys
import zlib

SLOTS = (0x10000, 0x80000)
SLOT_LEN = 0x70000
PAYLOAD = 0x1000
VARIANT_OLED = 0
BOOT_REGION = 0x100000


def check_slot(img):
    """Problems with a slot image, as a list."""
    bad = []
    if len(img) < PAYLOAD + 0x40:
        return [f"only {len(img)} bytes"]
    magic, _version, variant, _seq, body_len, body_crc, entry = struct.unpack_from("<IHHIIII", img)
    header_crc, confirm = struct.unpack_from("<II", img, 0xF8)
    if magic != 0x544C5344:
        bad.append("slot magic")
    if variant != VARIANT_OLED:
        bad.append(f"slot variant {variant:#x}")
    if zlib.crc32(img[:0xF8]) != header_crc:
        bad.append("slot header CRC")
    if entry != 0x100:
        bad.append(f"entry {entry:#x}")
    if confirm != 0xFFFFFFFF:
        bad.append("not pending")
    if not 0 < body_len <= SLOT_LEN - 0x100 or len(img) != 0x100 + body_len:
        bad.append(f"body length {body_len:#x}")
    elif zlib.crc32(img[0x100:0x100 + body_len]) != body_crc:
        bad.append("slot body CRC")
    p = img[PAYLOAD:]
    a_magic, a_version, a_variant, load, a_entry, a_len, a_crc = struct.unpack_from("<IHHIIII", p)
    (a_hcrc,) = struct.unpack_from("<I", p, 0x3C)
    if a_magic != 0x50504144:
        bad.append("payload magic")
    if zlib.crc32(p[:0x3C]) != a_hcrc:
        bad.append("payload header CRC")
    if a_version != 1 or a_variant != VARIANT_OLED:
        bad.append(f"payload version {a_version} variant {a_variant:#x}")
    # The loader's _start: above the stock bootloader's SRAM (firmwares/app-loader/stock_fsb.x),
    # inside the shim's LOAD_RANGE.
    if not 0x20040000 <= load < 0x20300000 or load + a_len > 0x20300000 \
            or not load <= a_entry < load + a_len:
        bad.append(f"payload load {load:#x} entry {a_entry:#x}")
    if len(p) < 0x40 + a_len or zlib.crc32(p[0x40:0x40 + a_len]) != a_crc:
        bad.append("payload body CRC")
    return bad


def main():
    out = sys.argv[1]
    failed = False
    a = open(f"{out}/LOADER.SLT", "rb").read()
    bad = check_slot(a)
    failed |= bool(bad)
    print(f"    {'LOADER.SLT':20} {'ok' if not bad else 'BAD: ' + ', '.join(bad)}")
    full = open(f"{out}/stage0+loader.bin", "rb").read()
    stage0 = open(f"{out}/stage0.bin", "rb").read()
    bad = []
    if len(full) != BOOT_REGION:
        bad.append(f"length {len(full):#x}")
    if full[:len(stage0)] != stage0:
        bad.append("stage-0 region")
    if full[SLOTS[0]:SLOTS[0] + len(a)] != a:
        bad.append("slot A isn't LOADER.SLT")
    if full[SLOTS[1]:BOOT_REGION].strip(b"\xff"):
        bad.append("slot B isn't erased")
    failed |= bool(bad)
    print(f"    {'stage0+loader.bin':20} {'ok' if not bad else 'BAD: ' + ', '.join(bad)}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
