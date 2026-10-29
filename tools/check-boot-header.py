#!/usr/bin/env python3
"""Check a Deluge image's vector table and boot header.

The first-stage bootloader reads the image's header at fixed offsets from `_start`: the
vector table's 8 words, then code_start (`_start`), code_end (`__metadata_code_end`),
code_execute (`_start`) and the ".BootLoad_ValidProgramTest." signature at +0x2C. deluge-bsp
emits the header (src/boot_header.rs) into the `.boot_header` section that rza1-rt's link.x
places straight after the vector table; this checks it landed there and that the vector table
is 8 branches, with no literal pool that could push the header off +0x20.

With --rtt it also checks that the prefetch-abort RTT dump goes on to `_fault_report` (the
fault hook) rather than parking the core itself.

Usage: tools/check-boot-header.py [--rtt] ELF...
"""

import struct
import sys

SIGNATURE = b".BootLoad_ValidProgramTest.\0"


class Elf:
    def __init__(self, path):
        with open(path, "rb") as f:
            self.data = f.read()
        d = self.data
        if d[:4] != b"\x7fELF" or d[4] != 1 or d[5] != 1:
            raise ValueError(f"{path}: not a little-endian ELF32 file")
        (self.phoff, self.shoff) = struct.unpack_from("<II", d, 0x1C)
        (self.phentsize, self.phnum, self.shentsize, self.shnum) = struct.unpack_from("<HHHH", d, 0x2A)
        self.sections = [struct.unpack_from("<IIIIIIIIII", d, self.shoff + i * self.shentsize)
                         for i in range(self.shnum)]
        self.symbols = {}
        self.addrs = set()
        for sh in self.sections:
            if sh[1] != 2:  # SHT_SYMTAB
                continue
            strtab = self.sections[sh[6]]
            for off in range(sh[4], sh[4] + sh[5], sh[9]):
                name_off, value, _size, info, _other, shndx = struct.unpack_from("<IIIBBH", d, off)
                end = d.index(b"\0", strtab[4] + name_off)
                name = d[strtab[4] + name_off:end].decode()
                if shndx != 0 and name:
                    self.symbols.setdefault(name, value)
                    # Every label and mapping symbol ($a, $d) in a code section bounds a block.
                    if info & 0xF in (0, 2) and shndx < len(self.sections) and self.sections[shndx][2] & 0x4:
                        self.addrs.add(value & ~1)

    def offset_of(self, addr):
        """File offset of a loaded virtual address."""
        for i in range(self.phnum):
            p_type, p_offset, p_vaddr, _paddr, p_filesz = struct.unpack_from(
                "<IIIII", self.data, self.phoff + i * self.phentsize)
            if p_type == 1 and p_vaddr <= addr < p_vaddr + p_filesz:
                return p_offset + addr - p_vaddr
        raise ValueError(f"0x{addr:08x} is not in a loaded segment")

    def word(self, addr):
        return struct.unpack_from("<I", self.data, self.offset_of(addr))[0]


def is_branch(insn):
    # ARM `b`, condition AL: cond 0b1110, bits 27-24 0b1010.
    return insn >> 24 == 0xEA


def branch_target(addr, insn):
    imm = insn & 0xFFFFFF
    if imm & 0x800000:
        imm -= 1 << 24
    return (addr + 8 + (imm << 2)) & 0xFFFFFFFF


def check(path, rtt):
    elf = Elf(path)
    errors = []
    for name in ("_start", "__metadata_code_end"):
        if name not in elf.symbols:
            return [f"no `{name}` symbol"]
    start = elf.symbols["_start"]
    for i in range(8):
        insn = elf.word(start + 4 * i)
        if not is_branch(insn):
            errors.append(f"vector {i} (+0x{4 * i:02x}) is 0x{insn:08x}, not a `b`")
    expect = [("code_start", start), ("code_end", elf.symbols["__metadata_code_end"]),
              ("code_execute", start)]
    for i, (what, value) in enumerate(expect):
        got = elf.word(start + 0x20 + 4 * i)
        if got != value:
            errors.append(f"{what} (+0x{0x20 + 4 * i:02x}) is 0x{got:08x}, expected 0x{value:08x}")
    off = elf.offset_of(start + 0x2C)
    sig = elf.data[off:off + len(SIGNATURE)]
    if sig != SIGNATURE:
        errors.append(f"signature (+0x2c) is {sig!r}, expected {SIGNATURE!r}")
    if rtt:
        for name in ("_pabt_rtt", "_fault_report"):
            if name not in elf.symbols:
                return errors + [f"no `{name}` symbol"]
        begin = elf.symbols["_pabt_rtt"]
        end = min(a for a in elf.addrs if a > begin)
        last = None
        for addr in range(begin, end, 4):
            if is_branch(elf.word(addr)):
                last = addr
        if last is None:
            errors.append("`_pabt_rtt` ends in no `b`")
        elif branch_target(last, elf.word(last)) != elf.symbols["_fault_report"]:
            target = branch_target(last, elf.word(last))
            errors.append(f"`_pabt_rtt` ends in `b 0x{target:08x}`, not `b _fault_report`")
    return errors


def main(argv):
    rtt = "--rtt" in argv
    paths = [a for a in argv if a != "--rtt"]
    if not paths:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    failed = False
    for path in paths:
        for e in check(path, rtt):
            print(f"{path}: {e}", file=sys.stderr)
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
