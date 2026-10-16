#!/usr/bin/env python3
"""Strip chiptool's Cortex-M device glue from a rustfmt'd lib.rs (tools/gen-pac.sh).

chiptool always emits an `Interrupt` enum implementing `cortex_m::interrupt::InterruptNumber`, an
`rt` vector table and a `cortex_m_rt` re-export. The RZ/A1L is a Cortex-A9 whose interrupts go
through the GIC (`rza1l_hal::interrupt`), so they go, as do the crate-level attributes, which the
PAC's own lib.rs sets around `include!`. Each removal asserts its target exists, so a chiptool
change that moves them fails here instead of shipping half-stripped.

Usage: pac-postprocess.py <chiptool lib.rs> <output chip .rs>
"""
import re
import sys

src, dst = sys.argv[1], sys.argv[2]
s = open(src).read()
s, n = re.subn(r'^#!\[[^\n]*\]\n', '', s, flags=re.M)
assert n >= 1, 'no crate attributes to strip'


def cut_item(s, start):
    i = s.index(start)
    depth = 0
    for k in range(s.index('{', i), len(s)):
        if s[k] == '{':
            depth += 1
        elif s[k] == '}':
            depth -= 1
            if depth == 0:
                return s[:i] + s[k + 2:]
    raise SystemExit('unbalanced braces after: ' + start)


s = cut_item(s, '#[derive(Copy, Clone, Debug, PartialEq, Eq)]\npub enum Interrupt {')
s = cut_item(s, 'unsafe impl cortex_m::interrupt::InterruptNumber for Interrupt {')
s = cut_item(s, '#[cfg(feature = "rt")]\nmod _vectors {')
for line in ('#[cfg(feature = "rt")]\npub use cortex_m_rt::interrupt;\n',
             '#[cfg(feature = "rt")]\npub use Interrupt as interrupt;\n'):
    assert line in s, 'missing: ' + line
    s = s.replace(line, '')
assert 'cortex_m' not in s, 'cortex_m glue left behind'
open(dst, 'w').write(s)
