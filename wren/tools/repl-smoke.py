#!/usr/bin/env python3
"""Smoke-test wren-firmware's USB REPL from the bench host (the pi-top).

Usage: repl-smoke.py /dev/ttyACM0

Checks the version banner, one evaluated expression, and an OLED draw, then asks
the operator to confirm the OLED by eye. Exits 0 on pass, 1 on fail.

The tty is opened non-blocking with echo and line discipline turned off via
TCSANOW: the default ECHO bounces the device's output back to it as Wren source,
and `tty.setraw` (TCSAFLUSH) can hang on a CDC-ACM port on the bench.
"""

import os
import select
import sys
import termios
import time


def read_lines(fd, timeout):
    """Collect complete lines received within `timeout` seconds."""
    buf = b""
    end = time.monotonic() + timeout
    while (left := end - time.monotonic()) > 0:
        ready, _, _ = select.select([fd], [], [], left)
        if not ready:
            break
        try:
            buf += os.read(fd, 4096)
        except BlockingIOError:
            continue
    lines = [l.strip() for l in buf.decode(errors="replace").replace("\r", "\n").split("\n")]
    lines = [l for l in lines if l]
    for l in lines:
        print(f"  < {l}")
    return lines


def send(fd, line):
    print(f"  > {line}")
    os.write(fd, line.encode() + b"\n")


def fail(what, lines):
    print(f"FAIL — expected {what}; received: {lines!r}")
    sys.exit(1)


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    fd = os.open(sys.argv[1], os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    attrs = termios.tcgetattr(fd)
    attrs[1] &= ~termios.OPOST
    attrs[3] &= ~(termios.ECHO | termios.ECHONL | termios.ICANON | termios.ISIG | termios.IEXTEN)
    attrs[0] &= ~(termios.ICRNL | termios.IXON)
    termios.tcsetattr(fd, termios.TCSANOW, attrs)
    # Drain whatever is queued (boot output, an earlier session's errors) until the
    # port has been quiet for half a second, then end any partial line the device
    # is holding so the first command starts clean.
    os.write(fd, b"\n")
    deadline = time.monotonic() + 10
    while read_lines(fd, 0.5) and time.monotonic() < deadline:
        pass

    send(fd, "^^v")
    lines = read_lines(fd, 3)
    if not any(l.startswith("wren:") for l in lines):
        fail("a version banner starting 'wren:'", lines)

    send(fd, "System.print(6 * 7)")
    lines = read_lines(fd, 3)
    if "42" not in lines:
        fail("a line '42'", lines)

    send(fd, "Oled.clear()")
    send(fd, 'Oled.text(0, 0, "SPLIT OK")')
    send(fd, "Oled.show()")
    lines = read_lines(fd, 2)
    if any("error" in l.lower() for l in lines):
        fail("no errors from the OLED calls", lines)

    print('PASS — confirm the OLED shows "SPLIT OK"')


if __name__ == "__main__":
    main()
