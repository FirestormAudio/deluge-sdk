#!/usr/bin/env bash
#
# libdeluge-sys's build script must link the libdeluge under an explicit DELUGE_SDK_ROOT even
# when pkg-config also knows one (`cargo deluge linux` sets the root to the bundle's sysroot).
# Builds the crate with a decoy deluge.pc and checks which directory the script emits.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/root/lib" "$tmp/root/include" "$tmp/decoy/lib" "$tmp/pc"
cat > "$tmp/pc/deluge.pc" <<PC
libdir=$tmp/decoy/lib
includedir=$tmp/decoy/include
Name: deluge
Description: decoy
Version: 0.1.0
Libs: -L\${libdir} -ldeluge
Cflags: -I\${includedir}
PC

# The search paths come from cargo's own report of the build script, whatever its build layout.
paths="$(PKG_CONFIG_PATH="$tmp/pc" PKG_CONFIG_ALLOW_CROSS=1 DELUGE_SDK_ROOT="$tmp/root" \
  cargo check -q --message-format=json --manifest-path linux/libdeluge-sys/Cargo.toml \
    --target x86_64-unknown-linux-gnu --target-dir "$tmp/target" |
  python3 -c 'import json,sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "build-script-executed" and "libdeluge-sys" in m["package_id"]:
        print("\n".join(m["linked_paths"]))')"
if [ "$paths" != "native=$tmp/root/lib" ]; then
  printf 'search paths:\n%s\n' "$paths"
  echo "error: libdeluge-sys did not prefer DELUGE_SDK_ROOT over pkg-config" >&2
  exit 1
fi
echo "==> libdeluge-sys links the DELUGE_SDK_ROOT it is given"
