#!/usr/bin/env bash
#
# format-sdcard.sh — (re)initialise an SD card for use with the Deluge / SSB.
#
# Creates a fresh DOS (MBR) partition table with a single FAT partition spanning
# the card, formats it, and (by default) creates the empty /APPS directory the
# second-stage bootloader scans for ELF application images.  Works on cards that
# have *no* filesystem or partition table at all ("cards without volumes"), as
# well as on cards you simply want to wipe and start over.
#
# It does NOT repair a card that is dead at the hardware level (one that won't
# respond to the SD protocol — e.g. a card bricked by an interrupted write).
# If your PC also can't see the card, this script can't help it; use a new card.
#
# Filesystem choice (override with --fat):
#   auto (default)  FAT16 for cards <= 2 GiB, else FAT32
#   16 | 32         force FAT16 / FAT32   (needs dosfstools / mkfs.vfat)
#   exfat           exFAT                 (needs exfatprogs / mkfs.exfat)
#
# Usage:
#   ./format-sdcard.sh                         # list candidate removable disks
#   ./format-sdcard.sh /dev/sdX                # format whole card (prompts first)
#   ./format-sdcard.sh -L MYCARD /dev/sdX      # custom volume label
#   ./format-sdcard.sh --fat 32 /dev/mmcblk0   # force FAT32
#   ./format-sdcard.sh --no-apps /dev/sdX      # don't create /APPS
#   ./format-sdcard.sh -y /dev/sdX             # skip the confirmation prompt
#
# Needs root (partitioning + mkfs).  Run with sudo.
#
# DANGER: this ERASES the entire target disk.  Pass a *whole-disk* device
# (/dev/sdb, /dev/mmcblk0), never a partition (/dev/sdb1).  Double-check the
# device — there is no undo.

set -euo pipefail

# ── defaults ──────────────────────────────────────────────────────────────────
LABEL="DELUGE"
FAT="auto"
MAKE_APPS=1
ASSUME_YES=0
DEV=""

err()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; }
info() { printf '\033[36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }

# Print the leading comment block (everything from line 2 up to the first
# non-comment line), stripping the leading "# ".
usage() { awk 'NR==1{next} /^#/{sub(/^# ?/,""); print; next} {exit}' "$0"; exit "${1:-0}"; }

# ── argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    -L|--label) LABEL="${2:?--label needs a value}"; shift 2 ;;
    --fat)      FAT="${2:?--fat needs a value}"; shift 2 ;;
    --no-apps)  MAKE_APPS=0; shift ;;
    -y|--yes)   ASSUME_YES=1; shift ;;
    -h|--help)  usage 0 ;;
    -*)         err "unknown option: $1"; usage 1 ;;
    *)          [[ -z "$DEV" ]] || { err "unexpected extra argument: $1"; usage 1; }
                DEV="$1"; shift ;;
  esac
done

# ── helper: list plausible removable disks and exit ───────────────────────────
list_candidates() {
  info "Candidate removable whole disks (pass one explicitly):"
  # RM=1 (removable) and TYPE=disk; show model/size so you can identify the card.
  lsblk -dno NAME,SIZE,RM,TYPE,MODEL | awk '$3==1 && $4=="disk" {
    printf "  /dev/%s  %s  %s\n", $1, $2, substr($0, index($0,$5))
  }'
  echo
  echo "Then run:  sudo $0 /dev/<name>"
}

if [[ -z "$DEV" ]]; then
  list_candidates
  exit 0
fi

# ── preflight checks ──────────────────────────────────────────────────────────
[[ $EUID -eq 0 ]] || { err "must run as root (use sudo)"; exit 1; }

for t in lsblk wipefs sfdisk findmnt udevadm; do
  command -v "$t" >/dev/null 2>&1 || { err "missing required tool: $t"; exit 1; }
done

[[ -b "$DEV" ]] || { err "$DEV is not a block device"; exit 1; }

# Must be a whole disk, not a partition.
DEV_TYPE="$(lsblk -dno TYPE "$DEV" 2>/dev/null || true)"
if [[ "$DEV_TYPE" != "disk" ]]; then
  err "$DEV is type '$DEV_TYPE', not a whole disk."
  err "Pass the whole-disk device (e.g. /dev/sdb), not a partition (e.g. /dev/sdb1)."
  exit 1
fi

# Refuse to touch the disk that backs the running system.
ROOT_SRC="$(findmnt -no SOURCE / 2>/dev/null || true)"
ROOT_DISK="$(lsblk -no PKNAME "$ROOT_SRC" 2>/dev/null | head -n1 || true)"
if [[ -n "$ROOT_DISK" && "/dev/$ROOT_DISK" == "$DEV" ]]; then
  err "$DEV is the disk backing '/'. Refusing to format the system disk."
  exit 1
fi

# Removable hint — warn (don't hard-fail; some USB readers report 0).
DEV_BASE="$(basename "$DEV")"
RM_FLAG="$(cat "/sys/block/$DEV_BASE/removable" 2>/dev/null || echo 0)"
if [[ "$RM_FLAG" != "1" ]]; then
  warn "$DEV is not marked removable. Make ABSOLUTELY sure this is your SD card."
fi

# ── filesystem selection ──────────────────────────────────────────────────────
DEV_BYTES="$(blockdev --getsize64 "$DEV" 2>/dev/null || echo 0)"
TWO_GIB=$((2 * 1024 * 1024 * 1024))

case "$FAT" in
  auto) if (( DEV_BYTES > 0 && DEV_BYTES <= TWO_GIB )); then FAT=16; else FAT=32; fi ;;
  16|32|exfat) ;;
  *) err "invalid --fat value: $FAT (use auto|16|32|exfat)"; exit 1 ;;
esac

case "$FAT" in
  16)    MKFS=(mkfs.vfat -F 16 -n "$LABEL"); PART_TYPE="0e"; FS_DESC="FAT16" ;;
  32)    MKFS=(mkfs.vfat -F 32 -n "$LABEL"); PART_TYPE="0c"; FS_DESC="FAT32" ;;
  exfat) MKFS=(mkfs.exfat -n "$LABEL");      PART_TYPE="07"; FS_DESC="exFAT" ;;
esac
command -v "${MKFS[0]}" >/dev/null 2>&1 || {
  err "missing tool for $FS_DESC: ${MKFS[0]}"
  [[ "$FAT" == "exfat" ]] && err "install exfatprogs" || err "install dosfstools"
  exit 1
}

# ── confirmation ──────────────────────────────────────────────────────────────
echo
info "Target device:"
lsblk -o NAME,SIZE,TYPE,FSTYPE,LABEL,MOUNTPOINT "$DEV" || true
echo
warn "This will ERASE ALL DATA on $DEV and create one $FS_DESC partition labelled '$LABEL'."
(( MAKE_APPS )) && echo "       An empty /APPS directory will be created."

if (( ! ASSUME_YES )); then
  read -r -p "Type the device path again to confirm ($DEV): " CONFIRM
  [[ "$CONFIRM" == "$DEV" ]] || { err "confirmation did not match; aborting."; exit 1; }
fi

# ── compute the partition node name ───────────────────────────────────────────
# /dev/sdb -> /dev/sdb1 ; /dev/mmcblk0 -> /dev/mmcblk0p1 ; /dev/nvme0n1 -> ...p1
if [[ "$DEV" =~ [0-9]$ ]]; then
  PART="${DEV}p1"
else
  PART="${DEV}1"
fi

# ── do the work ───────────────────────────────────────────────────────────────
info "Unmounting any mounted partitions of $DEV ..."
# Unmount every child mountpoint of the device (ignore failures for unmounted).
while read -r mp; do
  [[ -n "$mp" ]] && { umount "$mp" 2>/dev/null || umount -l "$mp" 2>/dev/null || true; }
done < <(lsblk -nro MOUNTPOINT "$DEV" | sed '/^$/d')

info "Wiping existing filesystem/partition signatures ..."
wipefs -a "$DEV" >/dev/null

info "Creating DOS partition table with one $FS_DESC partition ..."
# Single primary partition, default start (1 MiB aligned), spanning the disk.
sfdisk --quiet --wipe always --wipe-partitions always "$DEV" <<EOF
label: dos
- - ${PART_TYPE} *
EOF

info "Settling kernel partition table ..."
partprobe "$DEV" 2>/dev/null || true
udevadm settle || true

# Wait for the partition node to appear (up to ~5 s).
for _ in $(seq 1 50); do
  [[ -b "$PART" ]] && break
  sleep 0.1
done
[[ -b "$PART" ]] || { err "partition node $PART did not appear"; exit 1; }

info "Formatting $PART as $FS_DESC (label: $LABEL) ..."
"${MKFS[@]}" "$PART"

if (( MAKE_APPS )); then
  info "Creating /APPS directory ..."
  MNT="$(mktemp -d)"
  trap 'umount "$MNT" 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT
  mount "$PART" "$MNT"
  mkdir -p "$MNT/APPS"
  sync
  umount "$MNT"
  rmdir "$MNT"
  trap - EXIT
fi

sync
info "Done. $DEV is now a $FS_DESC card labelled '$LABEL'$([[ $MAKE_APPS -eq 1 ]] && echo " with /APPS")."
echo "    Verify with:  lsblk -f $DEV"
