#!/usr/bin/env bash
#
# The memory layout every firmware links with, as linker-defined symbols: stack sizes and
# bounds, the SRAM heap's end, the MMU table, BSS start and the RTT control block. Symbols that
# move with code size (`_start`, `end`, `__sram_heap_start`) are left out, so a code change
# doesn't touch the baseline; a layout change does, and has to be committed deliberately.
#
# Usage: tools/link-layout.sh check         build every firmware, diff against the baseline
#        tools/link-layout.sh update        build every firmware, rewrite the baseline
#        tools/link-layout.sh print <elf>…  print one ELF's layout symbols
set -euo pipefail

cd "$(dirname "$0")/.."

BASELINE=tools/link-layout.baseline
SYMS='(ABT|FIQ|IRQ|SVC|PROGRAM)_STACK_SIZE|(abt|fiq|irq|svc|program)_stack_(start|end)|__sram_heap_end|ttb_mmu1_base|__bss_start__|_SEGGER_RTT|INTERNAL_RAM_END|__rza1_status_led_(p|pm|pmc|pipc|mask)'
PLAIN=(blinky oled_hello oled_menu oled_hmenu input_demo pad_paint midi_cv clock_jacks sd_demo
       audio_passthru audio_passthru_irq test_osc additive_osc button_leds usb_log
       demo-firmware deluge-controller msc-firmware app-loader wren-firmware
       uac-host-firmware sd-bench vbus-probe wp-probe)
RTT=(blinky demo-firmware app-loader wren-firmware uac-host-firmware)

# Build one package and print the path of the executable it produced.
build_elf() {
  local elf
  elf=$(cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem \
          -p "$1" "${@:2}" --message-format=json 2>/dev/null \
        | python3 -c '
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("reason") == "compiler-artifact" and msg.get("executable"):
        print(msg["executable"])' | tail -1)
  if [ -z "$elf" ]; then
    echo "error: building $1 $* failed:" >&2
    cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p "$1" "${@:2}" >&2 || true
    exit 1
  fi
  echo "$elf"
}

print_layout() { # label elf
  arm-none-eabi-nm "$2" | awk -v label="$1" -v re="^($SYMS)\$" '$3 ~ re { print label, $3, $1 }' | sort -k2
}

collect() {
  local p elf
  for p in "${PLAIN[@]}"; do elf=$(build_elf "$p"); print_layout "$p" "$elf"; done
  for p in "${RTT[@]}"; do elf=$(build_elf "$p" --features rtt); print_layout "$p+rtt" "$elf"; done
}

case "${1:-}" in
  print)
    shift
    for elf in "$@"; do print_layout "$(basename "$elf")" "$elf"; done
    ;;
  check)
    got=$(mktemp)
    trap 'rm -f "$got"' EXIT
    collect > "$got"
    if diff -u "$BASELINE" "$got"; then
      echo "==> Link layout matches $BASELINE."
    else
      echo "error: the link layout moved (- baseline, + now); if that is intended, run '$0 update' and commit the baseline" >&2
      exit 1
    fi
    ;;
  update)
    collect > "$BASELINE"
    echo "==> Rewrote $BASELINE; review 'git diff $BASELINE' before committing."
    ;;
  *)
    echo "usage: $0 check | update | print <elf>..." >&2
    exit 2
    ;;
esac
