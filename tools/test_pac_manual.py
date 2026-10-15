"""Tests for tools/pac_manual.py, the RZ/A1L manual's Table 46.1 parser (python3 -m unittest)."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from pac_manual import a1l_registers  # noqa: E402

MANUAL = os.path.join(os.path.dirname(__file__), '..', 'vendor', 'docs', 'rza1',
                      'rza1l-hw-manual', 'ch46-list-of-registers.md')


class TheRealTable(unittest.TestCase):
    regs = a1l_registers(MANUAL)

    def test_an_indexed_row_names_every_element(self):
        # RSCAN0CmCFG (m = 0, 1) at H'E803A000 + m * H'0010: CAN channel 1 is an RZ/A1L register.
        self.assertTrue(0xE803A000 in self.regs, hex(0xE803A000))
        self.assertTrue(0xE803A010 in self.regs, "C1CFG")
        # RSCAN0TMCp (p = 0 to 31) at H'E803A250 + p * H'0001, byte-wide.
        self.assertTrue(0xE803A26F in self.regs, hex(0xE803A26F))
        self.assertEqual(self.regs[0xE803A26F][2], (8,))
        # RSCAN0RMIDq (q = 0 to 31): the last element, and nothing past it.
        self.assertIn(0xE803A600 + 31 * 0x10, self.regs)
        self.assertFalse(0xE803A600 + 32 * 0x10 in self.regs)

    def test_a_multi_plane_row_names_every_plane(self):
        # CEU CAMOR: Plane A H'E8210010, Plane B H'E8211010, Mirror H'E8212010.
        for addr in (0xE8210010, 0xE8211010, 0xE8212010):
            self.assertTrue(addr in self.regs, hex(addr))


class Malformed(unittest.TestCase):
    def table(self, row):
        f = tempfile.NamedTemporaryFile('w', suffix='.md', delete=False)
        f.write('|**Module**|**Register Name**|**Abbreviation**|**Number**<br>**of Bits**|'
                '**Address**|**Access Size**|\n|---|---|---|---|---|---|\n' + row + '\n')
        f.close()
        self.addCleanup(os.unlink, f.name)
        return f.name

    def test_an_address_cell_it_cannot_read_is_an_error(self):
        path = self.table("|Timer|Odd register|ODD|32|somewhere near H'E8000000|32|")
        with self.assertRaises(ValueError):
            a1l_registers(path)

    def test_an_indexed_row_without_its_range_is_an_error(self):
        path = self.table("|Timer|Indexed register|IDXn|32|H'E8000000 + n * H'0004|32|")
        with self.assertRaises(ValueError):
            a1l_registers(path)

    def test_two_rows_at_one_address_are_an_error(self):
        path = self.table("|Timer|First register|ONE|32|H'E8000000|32|\n"
                          "||Second register|TWO|32|H'E8000000|32|")
        with self.assertRaises(ValueError):
            a1l_registers(path)


if __name__ == '__main__':
    unittest.main()
