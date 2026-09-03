# Real Windows VM installation report

Date: 2026-09-03  
Test target: `omarchyinstall-test-clone` (`Jonathan@192.168.122.108`)  
ISO: Omarchy 4.0.2, 6,227,752,960 bytes  
SHA-256: `2ef8e624aa1bec7e277e28056b8535a6c9373ba48d7ede3f1a01cb6d2373cfb8`

## Result

Omarchy was ultimately installed successfully, booted through Limine/UKI, and
logged into as `jonathan`. The installed system reported hostname `omarchy`, a
Btrfs root on `/dev/sda2`, and zero failed systemd units.

The unmodified application flow did **not** reach the Omarchy installer. Two
independent release blockers were confirmed:

1. The copied ISO GRUB loader could not find or boot the ISO on this realistic
   Windows/SATA/UEFI layout.
2. After bypassing that boot failure, Archinstall 4.4 silently discarded the
   `/dev/disk/by-id/...` target emitted by the Windows app. It therefore mounted
   no target and ran `pacstrap /mnt` into the live overlay.

The successful installation used optical ISO boot plus one temporary live
configuration change:

```text
/dev/disk/by-id/ata-QEMU_HARDDISK_QM00001 -> /dev/sda
```

No application source or ISO was permanently modified for the successful run.

## Test environment

- Cloned Windows 11 VM; destructive mutation was explicitly authorized.
- 64 GiB QEMU SATA disk with an existing Windows GPT/UEFI installation.
- Existing Windows ESP and firmware NVRAM entries, rather than a fresh lab ESP.
- Secure Boot disabled.
- VM memory initially 12 GiB, then raised to 16 GiB for diagnosis.
- ISO source: `C:\Users\Jonathan\Downloads\omarchy-4.0.2.iso`.
- Intended Linux identity:
  - username: `jonathan`
  - hostname: `omarchy`
  - timezone: `Europe/Oslo`
  - keyboard: `no-latin1`
  - encryption: disabled

Important staged identifiers:

```text
Disk GUID:             6f16fbc4-8146-4242-9879-f4df2eb1a967
Windows partition:     a667a4a8-4408-4db0-9a68-a8b83050752f
Existing ESP:          b7c42ce2-2c4d-4f50-a5b4-5811a9970f4c
OMARCHYINST PARTUUID:  7fb9ffbb-1a9b-46dd-a3eb-71a7f6337f99
cidata PARTUUID:       2ca1e6ea-53a5-45b4-ad20-1da7aee0ca45
Linux stable disk ID:  /dev/disk/by-id/ata-QEMU_HARDDISK_QM00001
```

## What worked in the Windows backend

The application backend operations were reproduced directly with PowerShell.
These parts worked on the real Windows installation:

- Disabled hibernation/Fast Startup.
- Shrunk C: from 67,671,949,312 to 59,014,905,856 bytes.
- Created an 8 GiB NTFS `OMARCHYINST` partition.
- Created a 64 MiB FAT32 `cidata` partition.
- Copied the 6.2 GB ISO to `OMARCHYINST\omarchy.iso`.
- Wrote the unattended configuration files to `cidata`.
- Staged the EFI loader, GRUB configuration, and unique ISO bait file on the
  existing ESP.
- Created and selected the one-shot firmware entry:

  ```text
  {96d45a7e-a589-11f1-90e9-fe3257a9944a}
  \EFI\OmarchyInstall\BOOTX64.EFI
  ```

- Persisted the journal at
  `%LOCALAPPDATA%\OmarchyInstall\state.json`.
- BootNext worked and entered the staged loader.

The partitioning, copying, ESP write, journal, firmware entry, and BootNext
steps are therefore not the cause of the observed boot failure.

## Blocker 1: the staged GRUB path does not boot

### ISO discovery fails

The generated command:

```grub
search --no-floppy --set=img_part --file /omarchy.iso
```

failed with:

```text
error: commands/search.c:grub_search_fs_file:371:no such device: /omarchy.iso
```

`img_part` remained unset, so the following loopback command was interpreted as
a network path and failed:

```text
error: net/net.c:grub_net_open_real:1419:no server is specified.
```

Label search was also tested and failed:

```grub
search -l OMARCHYINST -s img_part
```

```text
error: commands/search.c:grub_search_label:371:no such device: OMARCHYINST.
```

The files and filesystem were readable. Direct GRUB access worked:

```grub
ls (hd0,gpt4)/omarchy.iso
set img_part=hd0,gpt4
loopback loop ($img_part)/omarchy.iso
ls (loop)/
```

The last command listed `arch/`, `boot/`, `EFI/`, and `shellx64.efi`.

### Kernel loading fails independently

Hard-coding the known partition got past ISO discovery, but GRUB failed at the
`linux` command:

```text
error: loader/efi/linux.c:grub_cmd_linux:542:out of memory.
```

This occurred:

- with 12 GiB of VM memory;
- with 16 GiB of VM memory;
- when loading the kernel through the NTFS-to-ISO loopback;
- after extracting the kernel and initramfs as ordinary files on NTFS;
- in a fresh GRUB session with the minimal command
  `linux /vmlinuz-linux-t2`, without any kernel arguments.

Relevant file sizes were only:

```text
vmlinuz-linux-t2          17,097,216 bytes
initramfs-linux-t2.img   253,458,903 bytes
```

This is not fixed by raising the application memory threshold. It is a loader,
firmware, topology, or kernel-loader compatibility problem. Do not treat the
observed error as proof that the ISO needs to remain loaded in RAM.

The loader did not provide `linuxefi` or `linuxefi.mod`, so that fallback was
unavailable.

### Chainloading the ISO loader is not a fallback

The ISO contains mixed-case `EFI/BOOT/BOOTx64.EFI`. The all-uppercase path was
not found through ISO9660. Loading the exact mixed-case path succeeded, but
`boot` hung. A second EFI loader cannot inherit GRUB's virtual loopback device,
so chainloading from `(loop)` is not a usable design.

### Direct optical boot proves the ISO is healthy

The same verified ISO booted normally when attached as virtual optical media.
The kernel, initramfs, network, custom `cidata` loader, and Omarchy installer all
ran. The failure is specifically in the application's copied-loader/ISO-file
handoff, not in the ISO image itself.

## Blocker 2: Archinstall silently ignores the by-id target

The app generated:

```json
"device": "/dev/disk/by-id/ata-QEMU_HARDDISK_QM00001"
```

That symlink existed and correctly pointed to `/dev/sda`, but Archinstall 4.4
did not accept it as a device model key.

Direct inspection of the parsed configuration proved the behavior:

```text
by-id configuration:
  config type = DiskLayoutType.Default
  device modifications = 0

same JSON with /dev/sda:
  device modifications = 1
  partitions = 2
```

The first run consequently logged:

```text
Could not determine the filesystem: None
No modifications required
Mounting ordered layout
```

`/mnt` was not a mount point, yet the installer continued with:

```text
pacstrap ... /mnt base sudo linux-firmware mkinitcpio linux
```

It wrote approximately 252 MiB into `/mnt/usr` in the live overlay, exhausted
the default 256 MiB ArchISO cowspace, and failed with package extraction errors.
Cloud-init then also failed with `OSError(28, 'No space left on device')`.

The physical disk was fortunately still unchanged after this first attempt.

This is the issue addressed by the pending Omarchy ISO pull request:
<https://github.com/omacom/omarchy-iso/pull/142>.

## Why Lab 7A passed

Lab 7A and the Omarchy ISO integration fixture use canonical virtual disk
paths, normally `/dev/vda`. The real Windows app deliberately emitted a stable
by-id symlink. The JSON was otherwise structurally equivalent.

Lab 7A also differs from this VM in important boot topology:

- fresh QEMU/OVMF state versus an existing Windows NVRAM and ESP;
- VirtIO block device versus SATA;
- purpose-built raw disk layout versus a shrunk Windows GPT disk;
- removable-media fallback path versus the Windows-created BootNext entry.

Lab 7A is valuable but is not a substitute for an existing-Windows SATA/NVMe
test. Both test classes are required.

## Required installer changes

### P0: require an ISO with by-id canonicalization

The safest fix belongs at the ISO/Archinstall adapter boundary:

1. Accept the stable `/dev/disk/by-id/...` identifier from the Windows app.
2. Resolve it with `readlink -f` immediately before constructing
   `ArchConfigHandler`.
3. Replace the JSON device key with the canonical kernel path used by
   Archinstall's current device model.
4. Verify the resolved block device still matches the expected stable identity
   before any destructive operation.

The Windows app should not guess `/dev/sda`, `/dev/vda`, or `/dev/nvme0n1`.
Kernel naming varies with firmware, controller, and driver order. Until PR 142
is included in a released ISO, the app should either:

- reject affected ISO versions with a clear compatibility error; or
- apply an equally safe boot-time canonicalization layer.

Silently falling back to a guessed Linux device name is not acceptable for a
disk-wiping installer.

### P0: add destructive-path invariants in the ISO

Before cleanup or partitioning:

- Require exactly one parsed `device_modifications` entry.
- Require it to contain the expected partitions.
- Require the canonical device to be a whole block disk.
- Require its stable identity to match the Windows-probed target.
- Abort if Archinstall drops any requested modification.

Before `pacstrap`:

- Require `/mnt` to be a mount point.
- Require the mount source to be a child of the selected target disk.
- Require the ESP and Btrfs subvolumes to be mounted as configured.
- Abort if `/mnt` resolves to the live overlay.

A minimal mandatory guard is:

```sh
mountpoint -q /mnt || {
  echo "fatal: install target /mnt is not mounted" >&2
  exit 1
}
```

The guard should also validate the mount source, because an unrelated mounted
filesystem would still pass `mountpoint`.

### P0: replace or repair the copied-GRUB boot design

Do not ship the current assumption that the ISO's root
`EFI/BOOT/BOOTx64.EFI`, copied to the Windows ESP and given a custom GRUB
configuration, will behave like the ISO's native boot path.

The replacement must be tested on:

- existing Windows UEFI installations;
- SATA and NVMe devices;
- reused ESPs and populated NVRAM;
- Secure Boot off, plus explicit behavior when it is on;
- 12 GiB and 16 GiB machines;
- both physical hardware and libvirt/QEMU.

Candidate approaches that still need validation include:

1. Ship a dedicated, tested GRUB EFI image rather than copying the ISO's
   removable-media loader.
2. Use systemd-boot with an XBOOTLDR-style FAT partition large enough for the
   extracted 17 MB kernel and 253 MB initramfs, while retaining the 6.2 GB ISO
   on NTFS for `img_loop`.
3. Stage another EFI-native loader that can load the kernel and initramfs from
   a supported filesystem without GRUB's failing Linux loader path.

The current 64 MiB `cidata` partition is too small to double as such a boot
partition. A systemd-boot/XBOOTLDR design would need a larger FAT partition,
probably at least 512 MiB with explicit future-size headroom.

Raising the RAM requirement alone is not a solution: the minimal kernel load
failed at 16 GiB before the initramfs was loaded.

### Implemented VM workaround: EFI-stub kernel launch

The current experimental Windows path now avoids GRUB's failing `linux` and
`initrd` commands. It uses this sequence instead:

1. Firmware starts the ISO's GRUB binary copied to the Windows ESP.
2. GRUB uses `chainloader` to launch the ISO's Linux kernel as an EFI
   application.
3. The kernel EFI stub loads the initramfs from the FAT `cidata` partition.
4. The initramfs mounts `omarchy.iso` from the NTFS `OMARCHYINST` partition.
5. `copytoram=y` copies the live squashfs into RAM and releases the staging
   disk before the destructive installation begins.

To support this, `cidata` was enlarged from 64 MiB to 512 MiB and now also
holds the extracted kernel and approximately 253 MB initramfs. The generated
GRUB entry passes `initrd=`, `img_dev=PARTUUID=...`, `img_loop=/omarchy.iso`,
and the existing `copytoram` arguments to the kernel EFI stub.

This is technically sound and directly avoids the low-memory allocation made
by GRUB's Linux loader. It is also consistent with the motivation behind
omarchy-iso PR 135, which moves UEFI boot to systemd-boot so that the kernel EFI
stub loads the large initramfs.

The implementation is deliberately scoped to the measured QEMU/SATA fixture.
It should work on that VM with the current supported ISO, but this statement is
not yet an end-to-end test result. It is only proven once a fresh Windows clone
boots through this path, reaches the live environment, completes `copytoram`,
loads `cidata`, installs, and boots the installed system without optical-media
or GRUB-console intervention.

Before generalizing this path to real hardware:

- Replace the fixture-specific `hd0` assumption with discovery by the
  `cidata` GPT PARTUUID. Firmware disk ordering is not stable on multi-disk
  machines.
- Discover and validate the kernel and initramfs paths from the supported ISO
  instead of permanently assuming the `linux-t2` filenames.
- Hash the copies written to FAT and compare them with the files from the
  verified mounted ISO. Comparing only their lengths does not prove their
  contents survived staging intact.
- Calculate the FAT partition requirement from the actual boot files plus
  explicit headroom rather than assuming 512 MiB will remain sufficient.
- Remove the old GRUB search-bait staging if testing confirms that the
  chainloader path has no dependency on it.
- Version or fingerprint the ISO bootloader. If PR 135 lands,
  `EFI/BOOT/BOOTX64.EFI` becomes systemd-boot and will not consume the generated
  GRUB configuration. At that point the app needs a systemd-boot/XBOOTLDR path
  or an explicit branch for older GRUB-based ISOs.

This change does not eliminate the `copytoram` requirement. It only moves
kernel/initramfs loading away from GRUB. The large live filesystem still has to
be copied to RAM so the installer can safely wipe the disk containing
`OMARCHYINST`.

### P1: strengthen the real-machine test matrix

Add a test that starts from an installed Windows image and runs the same backend
operations as the application:

1. Probe the Windows disk and firmware.
2. Shrink NTFS.
3. Create `OMARCHYINST` and `cidata`.
4. Copy the ISO.
5. Stage the real EFI loader and configuration.
6. Create BootNext through `bcdedit`.
7. Reboot without attaching the ISO as optical media.
8. Assert that Linux reaches the installer.
9. Assert that `cidata` is loaded.
10. Assert that the selected disk is parsed and mounted before package writes.
11. Complete installation and boot the installed UKI.

At minimum, cover these virtual controllers:

- VirtIO block (`/dev/vda`);
- SATA (`/dev/sda`);
- NVMe (`/dev/nvme0n1`).

The test should fail immediately if GRUB shows `no such device`, `no server is
specified`, or `out of memory`.

### P1: make the Linux-side handoff observable

Persist a small phase/result file somewhere the Windows app can inspect after a
failed reboot, or make it retrievable from `cidata`. Include:

- stable target requested;
- canonical target resolved;
- parsed modification count;
- partition/format result;
- `/mnt` mount source;
- current phase;
- final failure summary.

The real failure otherwise appears to users as a frozen GRUB screen or a live
installer failure with no explanation in Windows.

### P2: reduce misleading cloud-init noise

The FAT volume is labeled `cidata` for the custom `omarchy-cidata-load` helper,
but it is not a standard NoCloud seed. Cloud-init logged:

```text
device /dev/sda5 with label=cidata not a valid seed
```

The custom loader still worked and copied `user_configuration.json` and
`user_credentials.json`, so this warning did not cause the installation
failure. Consider either adding harmless valid NoCloud metadata or using a
non-conflicting custom label, after checking that doing so does not alter ISO
startup behavior.

## Diagnostic artifacts that are not app bugs

- Optical boot used to bypass GRUB did not include the app's `copytoram=y` and
  `copytoram_size=8G` arguments. Its live cowspace was therefore only 256 MiB.
  That made the unmounted-`/mnt` bug fail quickly, but does not establish that
  the normal app path would also have a 256 MiB overlay.
- Cloud-init's `cidata not a valid seed` warning did not prevent the custom
  Omarchy loader from finding the unattended configuration.
- The successful retry printed pacman-key/GPG warnings but completed, generated
  the UKI, booted, and reported zero failed systemd units. These warnings should
  be reviewed separately but were not blockers in this run.

## Successful bypass sequence

The downstream installer was validated as follows:

1. Attach the verified ISO as virtual optical media.
2. Boot the stock ISO path.
3. Confirm `omarchy-cidata-load` copied the app-generated unattended files.
4. Replace only the target in a temporary live copy of the JSON:

   ```text
   /dev/disk/by-id/ata-QEMU_HARDDISK_QM00001 -> /dev/sda
   ```

5. Confirm Archinstall parsed one device and two partitions.
6. Run the real `/usr/local/bin/omarchy-iso-install` orchestrator.
7. Confirm `/mnt` was mounted from `/dev/sda2[/@]` before installation.
8. Complete package installation, user creation, Limine installation, UKI
   generation, user finalization, and factory snapshot creation.
9. Eject the ISO and boot from disk.
10. Log in as `jonathan` and confirm the Omarchy desktop starts.
11. Run `systemctl --failed`; result: `0 loaded units listed`.

## Release gate recommendation

Do not release the destructive Windows installer against Omarchy 4.0.2 with
the current copied-GRUB and by-id behavior.

Release should require all of the following:

- the by-id canonicalization fix is present in the supported ISO;
- a hard pre-pacstrap mount/source invariant is present;
- the real Windows/SATA/UEFI boot path reaches Linux without manual GRUB input;
- the same path completes installation and boots the installed system;
- failure recovery preserves enough state to explain which invariant failed.
