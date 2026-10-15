"""Parse the RZ/A1L manual's Table 46.1 (List of Registers) from its Markdown conversion."""
import re

HEX = r"H['’‘]\s*([0-9A-Fa-f]{8})"
STRIDE = r"H['’‘]\s*([0-9A-Fa-f]{1,8})"


def _index_range(abbr):
    """The index values an indexed row's abbreviation names: `(m = 0, 1)`, `(x = 0 to 7)`,
    `(y = 0)`."""
    m = re.search(r'\((\w)\s*=\s*([^)]*)\)', abbr.replace('<br>', ' '))
    if not m:
        return None, None
    var, spec = m.group(1), m.group(2)
    r = re.fullmatch(r'\s*(\d+)\s*to\s*(\d+)\s*', spec)
    if r:
        return var, list(range(int(r.group(1)), int(r.group(2)) + 1))
    return var, [int(x) for x in re.findall(r'\d+', spec)]


def _addresses(addr, abbr, line):
    """Every register address one row's address cell names."""
    cell = addr.replace('<br>', ' ')
    # `H'base + i * H'stride`, one element per index the abbreviation names.
    m = re.fullmatch(r'\s*%s\s*\+\s*(\w)\s*\*\s*%s\s*' % (HEX, STRIDE), cell)
    if m:
        var, values = _index_range(abbr)
        if var != m.group(2) or not values:
            raise ValueError('indexed address with no index range in the abbreviation: ' + line)
        base, stride = int(m.group(1), 16), int(m.group(3), 16)
        return [base + i * stride for i in values]
    # `H'start to H'end`: a table (RZ/A1LU's JPEG unit); the register starts at `start`.
    m = re.fullmatch(r'\s*%s\s*to\s*%s\s*' % (HEX, HEX), cell)
    if m:
        return [int(m.group(1), 16)]
    # One address, or several named planes (`Plane A: H'… Plane B: H'… Mirror: H'…`).
    plain = re.fullmatch(r'\s*%s\s*' % HEX, cell)
    if plain:
        return [int(plain.group(1), 16)]
    planes = re.findall(r'(?:Plane [A-Z]|Mirror)\s*:\s*' + HEX, cell)
    if planes and re.fullmatch(r'(?:\s*(?:Plane [A-Z]|Mirror)\s*:\s*%s)+\s*' % HEX, cell):
        return [int(a, 16) for a in planes]
    raise ValueError('unreadable address cell: ' + line)


def a1l_registers(md_path, include_a1lu_only=False):
    """{address: (module, abbreviation, access sizes)} for every register the RZ/A1L has.

    Table 46.1's legend: note *1 = RZ/A1L only, *2 = RZ/A1LU only. A module heading's note applies
    to its continuation rows. An address cell this can't read raises `ValueError` rather than
    dropping the row: a dropped row would read as a register the RZ/A1L lacks.
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
        if not re.search(r"H['’‘]", addr):
            continue
        addrs = _addresses(addr, abbr, line.strip())
        note = module_note + ' ' + ' '.join(re.findall(r'\*\d+', rname + abbr))
        if '*2' in note and not include_a1lu_only:
            continue
        sizes = tuple(sorted({int(x) for x in re.findall(r'\d+', acc)}))
        name = re.sub(r'\*\d+|<br>', '', abbr).strip()
        for a in addrs:
            regs[a] = (module, name, sizes)
    return regs
