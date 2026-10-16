#!/usr/bin/env bash
#
# The checked-in rza1-pac must match its SVD, transforms and the RZ/A1L manual (tools/gen-pac.sh
# --check), and the manual parser must read every row of Table 46.1 (tools/test_pac_manual.py).
#
# gen-pac.sh needs the pinned chiptool, which it installs under target/ on first use. Locally the
# check is skipped until then, to keep tools/test.sh fast on a fresh checkout; under CI (`CI` set)
# it always runs, installing chiptool if needed, so a stale PAC can't pass there.
#
# Usage: tools/check-pac.sh
set -euo pipefail

cd "$(dirname "$0")/.."

python3 -m unittest discover -s tools -p 'test_pac*.py'
if [ -x target/chiptool-bcf538a2e7b8584ae874ee9ab72efb1576fc6152/bin/chiptool ] || [ -n "${CI:-}" ]; then
  tools/gen-pac.sh --check
else
  echo "  (skipped rza1-pac drift and manual check: run tools/gen-pac.sh once to install chiptool)"
fi
