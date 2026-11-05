#!/usr/bin/env python3
"""Check the layout of an image that executes in place (XIP) from the SPI flash window.

The RZ/A1L boot ROM branches to the start of flash with nothing but read-only flash and RAM
under it, so an XIP image must enter at the flash base with its vector table there, keep every
loaded byte inside its flash window (anything else would never reach the chip), run its
writable segments (.data, .bss, stacks) inside its RAM window, and fit the flash length.
rza1-rt's link_xip.x produces this layout; this checks an ELF actually has it.

Usage: tools/check-xip-image.py --flash BASE:LEN --ram BASE:LEN [--require-symbol NAME]... ELF
"""

import argparse
import importlib.util
import os
import struct
import sys

# The ELF reader and vector-table helpers are shared with check-boot-header.py.
_spec = importlib.util.spec_from_file_location(
    "check_boot_header", os.path.join(os.path.dirname(os.path.abspath(__file__)), "check-boot-header.py"))
_cbh = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_cbh)


def window(text):
    base, length = text.split(":")
    return int(base, 0), int(length, 0)


def inside(lo, hi, win):
    return win[0] <= lo and hi <= win[0] + win[1]


def check(path, flash, ram, required):
    elf = _cbh.Elf(path)
    errors = []
    entry = struct.unpack_from("<I", elf.data, 0x18)[0]
    if entry != flash[0]:
        errors.append(f"entry is 0x{entry:08x}, expected the flash base 0x{flash[0]:08x}")

    footprint = 0
    for i in range(elf.phnum):
        p_type, _off, vaddr, paddr, filesz, memsz = struct.unpack_from(
            "<IIIIII", elf.data, elf.phoff + i * elf.phentsize)
        if p_type != 1 or memsz == 0:  # PT_LOAD with something in it
            continue
        if filesz and not inside(paddr, paddr + filesz, flash):
            errors.append(f"segment {i} loads at 0x{paddr:08x}..0x{paddr + filesz:08x}, outside flash")
        if filesz:
            footprint = max(footprint, paddr + filesz - flash[0])
        if not inside(vaddr, vaddr + memsz, flash) and not inside(vaddr, vaddr + memsz, ram):
            errors.append(f"segment {i} runs at 0x{vaddr:08x}..0x{vaddr + memsz:08x}, "
                          "in neither the flash nor the RAM window")
    if footprint > flash[1]:
        errors.append(f"image needs 0x{footprint:x} bytes of flash, more than 0x{flash[1]:x}")

    start = elf.symbols.get("_start")
    if start != flash[0]:
        errors.append(f"`_start` is {start if start is None else hex(start)}, expected 0x{flash[0]:08x}")
    else:
        for i, handler in enumerate(_cbh.VECTORS):
            addr = start + 4 * i
            insn = elf.word(addr)
            if not _cbh.is_branch(insn):
                errors.append(f"vector {i} (+0x{4 * i:02x}) is 0x{insn:08x}, not a `b`")
            elif _cbh.branch_target(addr, insn) != elf.symbols.get(handler):
                errors.append(f"vector {i} (+0x{4 * i:02x}) does not branch to `{handler}`")

    for name in required:
        addr = elf.symbols.get(name)
        if addr is None:
            errors.append(f"no `{name}` symbol")
        elif not inside(addr, addr + 1, ram):
            errors.append(f"`{name}` is at 0x{addr:08x}, outside RAM")
    return errors


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--flash", type=window, required=True)
    ap.add_argument("--ram", type=window, required=True)
    ap.add_argument("--require-symbol", action="append", default=[])
    ap.add_argument("elf")
    args = ap.parse_args()
    try:
        errors = check(args.elf, args.flash, args.ram, args.require_symbol)
    except (OSError, ValueError) as e:
        print(f"{args.elf}: {e}", file=sys.stderr)
        return 1
    for e in errors:
        print(f"{args.elf}: {e}", file=sys.stderr)
    if not errors:
        print(f"{args.elf}: XIP layout ok")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
