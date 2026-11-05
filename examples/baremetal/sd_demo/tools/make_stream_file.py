#!/usr/bin/env python3
"""Write sd_demo's stream file: 8 MiB of the test pattern, to copy onto the card's root.

Byte i of the file is byte i % 4 of word i // 4, little-endian, where word k is
k * 0x9E3779B9 (mod 2**32). sd_demo checks every byte it reads against it.

Usage: make_stream_file.py [DIR]   (default: the current directory)
"""

import os
import struct
import sys

NAME = "sd_demo_stream.bin"
WORDS = (8 << 20) // 4


def main() -> None:
    out = os.path.join(sys.argv[1] if len(sys.argv) > 1 else ".", NAME)
    with open(out, "wb") as f:
        for start in range(0, WORDS, 1 << 16):
            words = range(start, min(start + (1 << 16), WORDS))
            values = ((k * 0x9E3779B9) & 0xFFFFFFFF for k in words)
            f.write(struct.pack(f"<{len(words)}I", *values))
    print(out)


if __name__ == "__main__":
    main()
