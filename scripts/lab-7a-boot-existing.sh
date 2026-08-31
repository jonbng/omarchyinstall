#!/usr/bin/env bash
# Boot the existing lab-7a disk without rebuilding or erasing it.
set -euo pipefail

WORK="${OMARCHY_LAB_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/omarchy-install/lab-7a}"
DISK="$WORK/disk.img"
VARS="$WORK/OVMF_VARS.fd"
RAM_MB="${OMARCHY_LAB_MEMORY_MB:-16384}"
OVMF_CODE="${OVMF_CODE:-}"

for candidate in \
  /usr/share/OVMF/OVMF_CODE.fd \
  /usr/share/edk2/x64/OVMF_CODE.4m.fd \
  /usr/share/edk2-ovmf/x64/OVMF_CODE.fd; do
  [[ -z $OVMF_CODE && -f $candidate ]] && OVMF_CODE=$candidate
done

[[ -f $DISK ]] || { echo "missing installed lab disk: $DISK" >&2; exit 2; }
[[ -f $VARS ]] || { echo "missing lab OVMF variables: $VARS" >&2; exit 2; }
[[ -n $OVMF_CODE && -f $OVMF_CODE ]] || { echo "OVMF firmware not found" >&2; exit 2; }

echo "Booting existing $DISK without rebuilding it"
echo "SSH forward: 127.0.0.1:2222 -> guest:22"

# shellcheck disable=SC2086
exec qemu-system-x86_64 \
  -cpu host -enable-kvm -machine q35,accel=kvm \
  -smp 4 -m "$RAM_MB" \
  -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
  -drive if=pflash,format=raw,file="$VARS" \
  -drive file="$DISK",format=raw,if=none,id=labdisk \
  -device virtio-blk-pci,drive=labdisk,serial=omarchy-lab,bootindex=1 \
  -netdev user,id=labnet,hostfwd=tcp:127.0.0.1:2222-:22 \
  -device virtio-net-pci,netdev=labnet \
  -usb -device usb-tablet \
  -serial stdio \
  ${OMARCHY_LAB_QEMU_EXTRA:-} \
  -boot order=c
