#!/usr/bin/env bash
# VISION PR 7a: boot the official Omarchy ISO as a file on NTFS, with official
# GRUB on a sibling ESP, copytoram=y, img_dev=PARTUUID=.
#
# Usage:
#   ./scripts/lab-7a-copytoram.sh              # build fixture, check item 1
#   ./scripts/lab-7a-copytoram.sh --download   # fetch GitHub-latest ISO first (~6 GiB)
#   ./scripts/lab-7a-copytoram.sh --boot       # then QEMU (serial); you watch 2–6
#   ./scripts/lab-7a-copytoram.sh --autoinstall # encrypted install into the QEMU image
#   OMARCHY_LAB_ENCRYPT=0 ... --autoinstall     # exercise the unencrypted path
#   OMARCHY_ISO=/path/to.iso ./scripts/lab-7a-copytoram.sh
set -euo pipefail

BOOT=0
DOWNLOAD=0
AUTOINSTALL=0
ISO="${OMARCHY_ISO:-}"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/omarchy-install/iso"
WORK="${OMARCHY_LAB_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/omarchy-install/lab-7a}"
OVMF_CODE="${OVMF_CODE:-}"
for c in /usr/share/OVMF/OVMF_CODE.fd /usr/share/edk2/x64/OVMF_CODE.4m.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd; do
  [[ -z $OVMF_CODE && -f $c ]] && OVMF_CODE=$c
done
OVMF_VARS_SRC="${OVMF_VARS_SRC:-}"
for c in /usr/share/OVMF/OVMF_VARS.fd /usr/share/edk2/x64/OVMF_VARS.4m.fd; do
  [[ -z $OVMF_VARS_SRC && -f $c ]] && OVMF_VARS_SRC=$c
done

usage() {
  awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "$0"
}

while (( $# )); do
  case "$1" in
    --boot) BOOT=1; shift ;;
    --autoinstall) AUTOINSTALL=1; BOOT=1; shift ;;
    --download) DOWNLOAD=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *)
      ISO=$1
      shift
      ;;
  esac
done

need() {
  command -v "$1" >/dev/null || {
    echo "missing tool: $1" >&2
    exit 2
  }
}

resolve_latest() {
  local json tag ver
  json=$(curl -fsSL -A "OmarchyInstall-lab" \
    -H "Accept: application/vnd.github+json" \
    https://api.github.com/repos/omacom/omarchy/releases/latest)
  tag=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["tag_name"])' <<<"$json")
  ver=${tag#v}
  printf '%s\n' "$ver"
}

download_latest() {
  mkdir -p "$CACHE"
  local ver url dest
  ver=$(resolve_latest)
  url="https://iso.omarchy.org/omarchy-${ver}.iso"
  dest="$CACHE/omarchy-${ver}.iso"
  echo "Latest GitHub tag → $url"
  echo "This is about 6 GiB. Sidecars: ${url}.sha256  ${url}.sig"
  curl -fL --retry 3 -C - -A "OmarchyInstall-lab" -o "$dest" "$url"
  curl -fL -A "OmarchyInstall-lab" -o "$dest.sha256" "${url}.sha256"
  curl -fL -A "OmarchyInstall-lab" -o "$dest.sig" "${url}.sig"
  (cd "$CACHE" && sha256sum -c "$(basename "$dest").sha256")
  ISO=$dest
}

find_iso() {
  [[ -n $ISO && -f $ISO ]] && return 0
  local f
  shopt -s nullglob
  for f in "$CACHE"/omarchy-*.iso; do
    [[ -f $f ]] || continue
    ISO=$f
  done
  shopt -u nullglob
  [[ -n $ISO && -f $ISO ]]
}

need qemu-system-x86_64
need qemu-img
need qemu-nbd
need parted
need mkfs.vfat
need mkfs.ntfs
need 7z
need python3
need curl
if (( AUTOINSTALL )); then
  need openssl
fi

if (( DOWNLOAD )); then
  download_latest
fi

if ! find_iso; then
  echo "No official ISO on disk."
  echo "Latest is not compiled in. Fetch it with:"
  echo "  $0 --download"
  echo "or set OMARCHY_ISO=/path/to/omarchy-<version>.iso"
  echo "QEMU=$(command -v qemu-system-x86_64)"
  echo "KVM=$([[ -e /dev/kvm ]] && echo yes || echo no)"
  exit 2
fi

echo "ISO=$ISO ($(du -h "$ISO" | awk '{print $1}'))"

mkdir -p "$WORK"
DISK="$WORK/disk.img"
ESP_MNT="$WORK/mnt-esp"
NTFS_MNT="$WORK/mnt-ntfs"
CIDATA_MNT="$WORK/mnt-cidata"
EXTRACT="$WORK/extract"
REPORT="$WORK/report.txt"
NBD=""

cleanup() {
  if [[ -n ${CIDATA_MNT:-} ]]; then sudo umount "$CIDATA_MNT" 2>/dev/null || true; fi
  if [[ -n ${NTFS_MNT:-} ]]; then sudo umount "$NTFS_MNT" 2>/dev/null || true; fi
  if [[ -n ${ESP_MNT:-} ]]; then sudo umount "$ESP_MNT" 2>/dev/null || true; fi
  if [[ -n ${NBD:-} ]]; then sudo qemu-nbd --disconnect "$NBD" 2>/dev/null || true; fi
}
trap cleanup EXIT

sudo modprobe nbd max_part=16
DISK_GIB=10
(( AUTOINSTALL )) && DISK_GIB=64
qemu-img create -f raw "$DISK" "${DISK_GIB}G"
NBD=$(sudo qemu-nbd --format=raw --connect=/dev/nbd0 "$DISK" && echo /dev/nbd0)
sudo parted --script "$NBD" mklabel gpt
sudo parted --script "$NBD" mkpart ESP fat32 1MiB 513MiB
sudo parted --script "$NBD" set 1 esp on
if (( AUTOINSTALL )); then
  CIDATA_START_MIB=$(( DISK_GIB * 1024 - 64 ))
  sudo parted --script "$NBD" mkpart OMARCHYINST ntfs 513MiB "${CIDATA_START_MIB}MiB"
  sudo parted --script "$NBD" mkpart cidata fat32 "${CIDATA_START_MIB}MiB" 100%
else
  sudo parted --script "$NBD" mkpart OMARCHYINST ntfs 513MiB 100%
fi
sudo partprobe "$NBD"
sudo udevadm settle || true
sleep 1
ESP="${NBD}p1"
NTFS="${NBD}p2"
CIDATA=""
(( AUTOINSTALL )) && CIDATA="${NBD}p3"
sudo mkfs.vfat -F32 -n ESP "$ESP"
sudo mkfs.ntfs -F -f -L OMARCHYINST "$NTFS"
if (( AUTOINSTALL )); then
  sudo mkfs.vfat -F32 -n cidata "$CIDATA"
fi
PARTUUID=$(sudo blkid -s PARTUUID -o value "$NTFS")
echo "OMARCHYINST PARTUUID=$PARTUUID"

mkdir -p "$ESP_MNT" "$NTFS_MNT" "$CIDATA_MNT" "$EXTRACT"
sudo mount "$ESP" "$ESP_MNT"
sudo mount -t ntfs3 "$NTFS" "$NTFS_MNT" 2>/dev/null || sudo mount -t ntfs-3g "$NTFS" "$NTFS_MNT"
if (( AUTOINSTALL )); then
  sudo mount "$CIDATA" "$CIDATA_MNT"
fi
sudo cp --sparse=always "$ISO" "$NTFS_MNT/omarchy.iso"
sync

rm -rf "$EXTRACT"
mkdir -p "$EXTRACT"
# ISO9660 filenames are case-insensitive, but 7z member matching is not.  In
# particular, Omarchy 4.0.2 stores this as BOOTx64.EFI and 7z exits zero when
# the all-uppercase spelling matches nothing.  Discover the archived spelling
# and verify that extraction actually produced a file.
mapfile -t EFI_MEMBERS < <(
  7z l -ba "$ISO" | awk '{print $NF}' |
    awk 'tolower($0) == "efi/boot/bootx64.efi"'
)
if (( ${#EFI_MEMBERS[@]} != 1 )); then
  echo "FAIL: expected exactly one EFI/BOOT/BOOTX64.EFI (case-insensitive), found ${#EFI_MEMBERS[@]}" >&2
  exit 1
fi
7z e -y -o"$EXTRACT/efi" "$ISO" "${EFI_MEMBERS[0]}" >/dev/null
EFI_FILE=$(find "$EXTRACT/efi" -maxdepth 1 -type f -iname 'bootx64.efi' -print -quit)
if [[ -z $EFI_FILE ]]; then
  echo "FAIL: 7z did not extract ${EFI_MEMBERS[0]}" >&2
  exit 1
fi
# Unique *.uuid bait (Joliet/RR). Do not assume /.disk/
mapfile -t UUIDS < <(7z l -ba "$ISO" | awk '{print $NF}' | grep -E '\.uuid$' || true)
if (( ${#UUIDS[@]} == 0 )); then
  echo "FAIL: no *.uuid in ISO" >&2
  exit 1
fi
echo "uuid files: ${UUIDS[*]}"
BAIT=""
for u in "${UUIDS[@]}"; do
  case "$u" in
    boot/*|*/boot/*) BAIT=$u ;;
  esac
done
[[ -n $BAIT ]] || BAIT=${UUIDS[0]}
BAIT=${BAIT#/}
echo "search bait=$BAIT"
7z e -y -o"$EXTRACT/bait" "$ISO" "$BAIT" >/dev/null
BAIT_FILE=$(find "$EXTRACT/bait" -type f | head -1)

sudo mkdir -p "$ESP_MNT/EFI/OmarchyInstall" "$ESP_MNT/EFI/BOOT" "$ESP_MNT/boot/grub"
sudo cp "$EFI_FILE" "$ESP_MNT/EFI/OmarchyInstall/BOOTX64.EFI"
# The real installer creates a Boot#### entry pointing at OmarchyInstall and
# selects it with BootNext.  This disposable QEMU disk has a fresh OVMF NVRAM
# store and no such entry, so also provide the standard removable-media path.
sudo cp "$EFI_FILE" "$ESP_MNT/EFI/BOOT/BOOTX64.EFI"
sudo mkdir -p "$ESP_MNT/$(dirname "$BAIT")"
sudo cp "$BAIT_FILE" "$ESP_MNT/$BAIT"

ISO_BYTES=$(stat -c%s "$ISO")
ISO_GIB=$(( (ISO_BYTES + 1024*1024*1024 - 1) / (1024*1024*1024) ))
COPYTORAM=$(( ISO_GIB + 2 ))
(( COPYTORAM < 8 )) && COPYTORAM=8

sudo tee "$ESP_MNT/boot/grub/grub.cfg" >/dev/null <<EOF
insmod part_gpt
insmod ntfs
insmod ntfscomp
insmod iso9660
insmod loopback

search --no-floppy --set=img_part --file /omarchy.iso
set iso_path="/omarchy.iso"
export iso_path
loopback loop (\${img_part})\${iso_path}
set root=(loop)

set default=0
set timeout=0

menuentry "Omarchy Installer" --id 'archlinux' {
    set gfxpayload=keep
    linux /arch/boot/x86_64/vmlinuz-linux-t2 \\
        archisobasedir=arch \\
        img_dev=PARTUUID=$PARTUUID \\
        img_loop="\${iso_path}" \\
        copytoram=y \\
        copytoram_size=${COPYTORAM}G \\
        splash xe.enable_panel_replay=0 initramfs_async=0
    initrd /arch/boot/x86_64/initramfs-linux-t2.img
}
EOF

if (( AUTOINSTALL )); then
  # Fixed credentials are intentional: this disk image is disposable and the
  # goal is to exercise the ISO's unattended handoff, not provision a real PC.
  LAB_USERNAME=lab
  LAB_PASSWORD=omarchy
  LAB_HOSTNAME=omarchy-lab
  # The stock ISO's archinstall 4.4 device model keys disks by their canonical
  # kernel path.  Passing the otherwise-valid /dev/disk/by-id symlink leaves
  # /mnt unmounted and pacstrap eventually exhausts the live tmpfs.  Omarchy's
  # own QEMU integration fixture likewise uses /dev/vda.
  LAB_DEVICE=${OMARCHY_LAB_DEVICE:-/dev/vda}
  LAB_ENCRYPT=${OMARCHY_LAB_ENCRYPT:-1}
  case "$LAB_ENCRYPT" in
    1|true|yes)
      LAB_ENCRYPT=true
      DISK_ENCRYPTION_JSON=',
    "disk_encryption": {
      "encryption_type": "luks",
      "lvm_volumes": [],
      "iter_time": 2000,
      "partitions": ["8c2c2b92-1070-455d-b76a-56263bab24aa"],
      "encryption_password": "omarchy"
    }'
      CREDENTIALS_ENCRYPTION_JSON=',
  "encryption_password": "omarchy"'
      ;;
    0|false|no)
      LAB_ENCRYPT=false
      DISK_ENCRYPTION_JSON=''
      CREDENTIALS_ENCRYPTION_JSON=''
      ;;
    *)
      echo "OMARCHY_LAB_ENCRYPT must be 1/true/yes or 0/false/no" >&2
      exit 2
      ;;
  esac
  DISK_BYTES=$(( DISK_GIB * 1024 * 1024 * 1024 ))
  BOOT_START=$(( 1024 * 1024 ))
  BOOT_SIZE=$(( 2 * 1024 * 1024 * 1024 ))
  ROOT_START=$(( BOOT_START + BOOT_SIZE ))
  ROOT_SIZE=$(( DISK_BYTES - ROOT_START - 1024 * 1024 ))
  PASSWORD_HASH=$(printf '%s\n' "$LAB_PASSWORD" | openssl passwd -6 -stdin)

  sudo tee "$CIDATA_MNT/user_configuration.json" >/dev/null <<EOF
{
  "app_config": null,
  "archinstall-language": "English",
  "auth_config": {},
  "audio_config": { "audio": "pipewire" },
  "bootloader_config": { "bootloader": "Limine", "uki": false, "removable": false },
  "custom_commands": [],
  "omarchy_install": {
    "mode": "full_disk",
    "defer_provisioning": false,
    "target_mount": "/mnt",
    "boot": {
      "esp_mount": "/boot",
      "esp_path": "/EFI/limine",
      "efi_binary": "limine_x64.efi",
      "enable_fallback": true
    },
    "storage": { "kernel": "linux" }
  },
  "disk_config": {
    "config_type": "default_layout",
    "device_modifications": [{
      "device": "$LAB_DEVICE",
      "wipe": true,
      "partitions": [
        {
          "btrfs": [],
          "dev_path": null,
          "flags": ["boot", "esp"],
          "fs_type": "fat32",
          "mount_options": [],
          "mountpoint": "/boot",
          "obj_id": "ea21d3f2-82bb-49cc-ab5d-6f81ae94e18d",
          "size": { "sector_size": { "unit": "B", "value": 512 }, "unit": "B", "value": $BOOT_SIZE },
          "start": { "sector_size": { "unit": "B", "value": 512 }, "unit": "B", "value": $BOOT_START },
          "status": "create",
          "type": "primary"
        },
        {
          "btrfs": [
            { "mountpoint": "/", "name": "@" },
            { "mountpoint": "/home", "name": "@home" },
            { "mountpoint": "/var/log", "name": "@log" },
            { "mountpoint": "/var/cache/pacman/pkg", "name": "@pkg" }
          ],
          "dev_path": null,
          "flags": [],
          "fs_type": "btrfs",
          "mount_options": ["compress=zstd"],
          "mountpoint": null,
          "obj_id": "8c2c2b92-1070-455d-b76a-56263bab24aa",
          "size": { "sector_size": { "unit": "B", "value": 512 }, "unit": "B", "value": $ROOT_SIZE },
          "start": { "sector_size": { "unit": "B", "value": 512 }, "unit": "B", "value": $ROOT_START },
          "status": "create",
          "type": "primary"
        }
      ]
    }]$DISK_ENCRYPTION_JSON
  },
  "hostname": "$LAB_HOSTNAME",
  "kernels": ["linux"],
  "network_config": { "type": "iso" },
  "ntp": true,
  "parallel_downloads": 8,
  "script": null,
  "services": [],
  "swap": true,
  "timezone": "UTC",
  "locale_config": {
    "kb_layout": "us",
    "sys_enc": "UTF-8",
    "sys_lang": "en_US.UTF-8"
  }
}
EOF

  sudo tee "$CIDATA_MNT/user_credentials.json" >/dev/null <<EOF
{
  "root_enc_password": "$PASSWORD_HASH",
  "users": [{
    "enc_password": "$PASSWORD_HASH",
    "groups": [],
    "sudo": true,
    "username": "$LAB_USERNAME"
  }]$CREDENTIALS_ENCRYPTION_JSON
}
EOF
  printf '%s\n' "$LAB_ENCRYPT" | sudo tee "$CIDATA_MNT/user_encrypt_installation.txt" >/dev/null
  sudo python3 -m json.tool "$CIDATA_MNT/user_configuration.json" >/dev/null
  sudo python3 -m json.tool "$CIDATA_MNT/user_credentials.json" >/dev/null
  sync
fi

{
  echo "=== 7a offline (checklist 1) ==="
  echo "ISO=$ISO"
  echo "PARTUUID=$PARTUUID"
  echo "bait=$BAIT"
  echo "copytoram_size=${COPYTORAM}G"
  echo -n "NTFS has omarchy.iso: "
  sudo test -f "$NTFS_MNT/omarchy.iso" && echo PASS || echo FAIL
  echo -n "ESP BOOTX64.EFI: "
  sudo test -f "$ESP_MNT/EFI/OmarchyInstall/BOOTX64.EFI" && echo PASS || echo FAIL
  echo -n "ESP QEMU fallback BOOTX64.EFI: "
  sudo test -f "$ESP_MNT/EFI/BOOT/BOOTX64.EFI" && echo PASS || echo FAIL
  echo -n "ESP bait $BAIT: "
  sudo test -f "$ESP_MNT/$BAIT" && echo PASS || echo FAIL
  echo -n "grub.cfg copytoram=y: "
  sudo grep -q 'copytoram=y' "$ESP_MNT/boot/grub/grub.cfg" && echo PASS || echo FAIL
  echo -n "grub.cfg PARTUUID: "
  sudo grep -q "img_dev=PARTUUID=$PARTUUID" "$ESP_MNT/boot/grub/grub.cfg" && echo PASS || echo FAIL
  echo -n "grub.cfg not /.disk/: "
  sudo grep -q '/.disk/' "$ESP_MNT/boot/grub/grub.cfg" && echo FAIL || echo PASS
  echo -n "no EFI/Microsoft write: "
  sudo test ! -e "$ESP_MNT/EFI/Microsoft" && echo PASS || echo FAIL
  if (( AUTOINSTALL )); then
    echo -n "cidata FAT32 label: "
    [[ $(sudo blkid -s LABEL -o value "$CIDATA") == cidata ]] && echo PASS || echo FAIL
    echo -n "cidata required files: "
    sudo test -f "$CIDATA_MNT/user_configuration.json" && \
      sudo test -f "$CIDATA_MNT/user_credentials.json" && echo PASS || echo FAIL
    echo "autoinstall target=$LAB_DEVICE (${DISK_GIB}G, wipe=true)"
    echo "installed login=$LAB_USERNAME password=$LAB_PASSWORD encryption=$LAB_ENCRYPT"
  fi
} | tee "$REPORT"

if (( AUTOINSTALL )); then
  sudo umount "$CIDATA_MNT"
fi
sudo umount "$NTFS_MNT"
sudo umount "$ESP_MNT"
sudo qemu-nbd --disconnect "$NBD"
NBD=""
trap - EXIT

echo
echo "Offline item 1 written to $REPORT"
if (( AUTOINSTALL )); then
  echo "AUTOINSTALL ARMED: the Omarchy installer will wipe the 64G QEMU image."
  echo "It must skip the configurator and install directly from cidata."
  echo "LUKS and installed login password: omarchy"
else
  echo "Items 2–6 need a live boot (GRUB, initramfs ntfs3, copytoram unmount, installer)."
fi
echo "Watch serial for:"
echo "  2. GRUB menu 'Omarchy Installer' (not ISO-volume archisosearchuuid)"
echo "  3. rootfs on tmpfs / copytoram; dmesg | grep ntfs"
echo "  4. after login: findmnt /run/archiso/bootmnt  → should FAIL"
echo "     findmnt | grep OMARCHYINST                 → should be empty"
if (( AUTOINSTALL )); then
  echo "  5. cidata is loaded and the configurator is skipped"
  echo "  6. orchestrator wipes and installs onto the disposable virtual disk"
else
  echo "  5. configurator still lists the virtual disk (expected)"
  echo "  6. skip a full wipe unless this VM is disposable"
fi
echo

if (( BOOT == 0 )); then
  echo "Next: $0 --boot"
  echo "Needs display or use: extra QEMU args via OMARCHY_LAB_QEMU_EXTRA='-nographic'"
  exit 0
fi

if [[ -z $OVMF_CODE || ! -f $OVMF_CODE ]]; then
  echo "OVMF firmware not found. Install ovmf / edk2-ovmf." >&2
  exit 2
fi
VARS="$WORK/OVMF_VARS.fd"
cp "${OVMF_VARS_SRC:-$OVMF_CODE}" "$VARS" 2>/dev/null || {
  echo "Need OVMF_VARS. Set OVMF_VARS_SRC." >&2
  exit 2
}
# If we copied CODE by mistake, try sibling VARS
if [[ ${OVMF_VARS_SRC:-} == "" ]]; then
  for c in /usr/share/OVMF/OVMF_VARS.fd /usr/share/edk2/x64/OVMF_VARS.4m.fd; do
    [[ -f $c ]] && cp "$c" "$VARS" && break
  done
fi

RAM_MB=${OMARCHY_LAB_MEMORY_MB:-16384}
echo "Booting $DISK with $OVMF_CODE (${RAM_MB}M RAM, serial=stdio)"
# shellcheck disable=SC2086
qemu-system-x86_64 \
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
