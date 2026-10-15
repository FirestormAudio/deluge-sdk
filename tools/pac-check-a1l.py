#!/usr/bin/env python3
"""Check the RZ/A1L PAC against the RZ/A1L manual's register list (Table 46.1), both ways:

- every register the PAC defines is an RZ/A1L register, at a width the manual allows;
- every RZ/A1L register is in the PAC, except the modules in NOT_YET (with the reason).

Input is chiptool's IR after all transforms (`chiptool generate --debug-ir-output`).

Usage: pac-check-a1l.py <ir.yaml> <ch46-list-of-registers.md>
"""
import os
import sys

import yaml

sys.path.insert(0, os.path.dirname(__file__))
from pac_manual import a1l_registers  # noqa: E402

# Manual modules the PAC doesn't describe yet, and why.
NOT_YET = {
    'Debugger interface': 'CoreSight debug components; no driver uses them',
}

# Port-indexed GPIO arrays start at port 0, which has none of these registers (port 0 is
# input-only): their element 0 is a documented hole, not a register.
GPIO_PORT0_HOLES = {'GPIO.%s[0]' % r for r in ('p', 'psr', 'pm', 'pnot', 'pmsr', 'pbdc', 'pipc')}

ir_path, md_path = sys.argv[1:3]
ir = yaml.safe_load(open(ir_path))
manual = a1l_registers(md_path)


def offsets(item):
    arr = item.get('array')
    if not arr:
        return [0]
    if 'offsets' in arr:
        return arr['offsets']
    return [i * arr['stride'] for i in range(arr['len'])]


def walk(block, base, out, path):
    for item in ir['block/' + block]['items']:
        for k, o in enumerate(offsets(item)):
            addr = base + item['byte_offset'] + o
            name = '%s.%s%s' % (path, item['name'], '[%d]' % k if item.get('array') else '')
            if 'block' in item:
                walk(item['block'], addr, out, name)
            else:
                out.append((addr, item.get('bit_size', 32), name))


pac = []
for key, dev in ir.items():
    if key.startswith('device/'):
        for p in dev['peripherals']:
            walk(p['block'], p['base_address'], pac, p['name'])

errors = []
for addr, bits, name in pac:
    if name in GPIO_PORT0_HOLES:
        continue
    m = manual.get(addr)
    if m is None and addr & 3:
        # A byte or halfword view of a register the manual lists (RSPI's SPDR, USB's FIFO ports):
        # fine if that register allows the view's width.
        whole = manual.get(addr & ~3)
        if whole is not None and bits in whole[2] and bits < 32:
            continue
    if m is None:
        errors.append('PAC register %s at 0x%08X is not an RZ/A1L register' % (name, addr))
    elif bits not in m[2]:
        errors.append('PAC register %s at 0x%08X is %d bits; the manual allows %s (%s)'
                      % (name, addr, bits, m[2], m[1]))
in_pac = {a for a, _, _ in pac}
for addr, (module, abbr, _) in sorted(manual.items()):
    if addr not in in_pac and module not in NOT_YET:
        errors.append('RZ/A1L register %s (%s) at 0x%08X is missing from the PAC' % (abbr, module, addr))

for e in errors:
    print(e)
skipped = sum(1 for m in manual.values() if m[0] in NOT_YET)
print('==> %d PAC registers, %d RZ/A1L registers (%d in modules not yet covered), %d problems'
      % (len(pac), len(manual), skipped, len(errors)))
sys.exit(1 if errors else 0)
