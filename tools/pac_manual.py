"""Parse the RZ/A1L manual's Table 46.1 (List of Registers) from its Markdown conversion."""
import re


def a1l_registers(md_path, include_a1lu_only=False):
    """{address: (module, abbreviation, access sizes)} for every register the RZ/A1L has.

    Table 46.1's legend: note *1 = RZ/A1L only, *2 = RZ/A1LU only. A module heading's note applies
    to its continuation rows.
    """
    regs = {}
    module, module_note = None, ''
    for line in open(md_path, encoding='utf-8'):
        if line.startswith('## 46.2'):
            break
        if not line.startswith('|') or line.startswith('|---') or '**Module**' in line:
            continue
        cells = [c.strip() for c in line.strip()[1:-1].split('|')]
        if len(cells) < 6:
            continue
        m, rname, abbr, _bits, addr, acc = cells[:6]
        if m:
            module_note = ' '.join(re.findall(r'\*\d+', m))
            module = re.sub(r'\*+\d*|<br>', ' ', m).strip()
        mm = re.match(r"H['’‘]([0-9A-Fa-f]{8})", addr)
        if not mm:
            continue
        note = module_note + ' ' + ' '.join(re.findall(r'\*\d+', rname + abbr))
        if '*2' in note and not include_a1lu_only:
            continue
        sizes = tuple(sorted({int(x) for x in re.findall(r'\d+', acc)}))
        regs[int(mm.group(1), 16)] = (module, re.sub(r'\*\d+', '', abbr).strip(), sizes)
    return regs
