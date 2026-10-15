#!/usr/bin/env bash
#
# Phase 5 exit check (spec: "no const OFF_*/*_BASE register tables left in rza1l-hal"), per module
# as each one moves onto rza1-pac. A migrated module may not declare an OFF_*/*_BASE constant or
# name a raw peripheral address (0xE8……, 0xFC……, 0x3F……) outside comments and its test module.
# Only address-typed (`usize`) constants count: `*_BASE` interrupt-ID bases (`u16`) are not tables.
#
# Usage: tools/check-register-tables.sh
set -euo pipefail

cd "$(dirname "$0")/.."

# Modules under crates/rza1l-hal/src that take every register from rza1_pac.
MIGRATED=(cpg.rs stb.rs bsc.rs ostm.rs adc.rs cache.rs mtu2.rs uart.rs rspi.rs gpio.rs gic.rs dmac.rs dmac/raw.rs ssi.rs scux.rs scux/regs.rs)

status=0
for m in "${MIGRATED[@]}"; do
  f="crates/rza1l-hal/src/$m"
  # Everything before the first host-test module; comment lines skipped.
  hits="$(awk '/^#\[cfg\(all\(test/{exit} {print FILENAME":"NR": "$0}' "$f" \
    | grep -vE '^[^:]+:[0-9]+: *(//|\*)' \
    | grep -E 'const +(OFF_[A-Z0-9_]+|[A-Z0-9_]*_BASE) *: *usize|0[xX]([Ee]8|[Ff][Cc]|3[Ff])[0-9A-Fa-f]{2}_?[0-9A-Fa-f]{4}' || true)"
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits"
    status=1
  fi
done
if [ "$status" -ne 0 ]; then
  echo "error: a migrated rza1l-hal module still hand-writes register addresses; take them from rza1_pac" >&2
fi
exit "$status"
