# Prior-art notes: WubiUEFI and Instlux

| Field | Value |
| --- | --- |
| **Date reviewed** | 2026-08-31 |
| **Status** | Research notes — not a design decision |
| **WubiUEFI revision** | `1a9452cc9dd025f961c8cdaed6f26fd6b0599575` |
| **Instlux revision** | `2ad91b853d54d480da86148c176e4c7fd748cbf2` |

> [!CAUTION]
> This document records how two other projects approached related problems. It does **not** establish that their choices were correct, safe, current, or appropriate for Omarchy Install. Prior use is not validation. Every idea below must be derived and tested against Omarchy Install's own goals, threat model, supported hardware, recovery requirements, and official-ISO constraints before adoption. “Possible lesson” means “worth investigating,” not “copy this.”

## Why these projects are relevant

Both projects start in Windows and arrange for a Linux installer to run after reboot without requiring the user to create conventional removable installation media. That makes them useful prior art for the Windows-to-Linux handoff, boot discovery, download flow, recovery, and division of responsibility across the reboot boundary.

They are not equivalent to Omarchy Install:

- WubiUEFI is descended from Wubi and includes a loop-file installed-OS model plus substantial Linux-installer customization. Omarchy Install uses Windows only as temporary scaffolding and ultimately performs a native full-disk installation.
- Instlux's real-machine path is predominantly a small network bootstrap. Omarchy Install currently intends to boot the official offline ISO and then erase the staging disk.
- Both contain old or weak security and recovery decisions. Their existence is evidence that an approach has been attempted, not that it should be repeated.

## WubiUEFI: observed approach

[WubiUEFI](https://github.com/hakuna-m/wubiuefi) divides its work between a generic backend, a Windows-specific backend, a UI, and Linux-side installation hooks.

The Windows-side flow is expressed as an ordered collection of tasks and subtasks. Broadly, it selects a target, creates directories and uninstall state, obtains and verifies installation media, extracts boot files, generates a preseed configuration, modifies the boot path, and creates virtual disks. Tasks carry weights for aggregate progress, can add subtasks at runtime, and propagate cancellation.

Media acquisition can check local media before downloading. Its download path supports resume and tries multiple mirrors described by a signed metalink. It extracts the kernel and initrd from the ISO and checks them against checksums found inside the image.

For UEFI systems it mounts the EFI System Partition, copies shim/GRUB into a private EFI directory, creates a BCD entry, and arranges for firmware to boot that entry. It records identifying state for later uninstall. It also disables hibernation around the operation.

The reboot handoff is explicit. A generated preseed file supplies configuration to the Linux installer, and Linux-side success and failure hooks finish the workflow. On failure, the hook attempts to save `/var/log` as `installation-logs.zip` on the Windows-visible host volume before rebooting.

### Possible lessons to investigate

- A backend-owned task graph may give long-running operations clearer nested progress, cancellation boundaries, and error reporting.
- Local-media selection and mirror fallback could complement Omarchy Install's existing resumable download.
- Creating recovery metadata before the first mutation is a useful sequencing principle.
- A precise Windows-to-Linux handoff contract, with explicit success and failure behavior, makes the reboot boundary easier to reason about.
- Post-reboot diagnostics that survive a failed Linux installation would substantially improve supportability.
- Searching for a deliberately created marker can be safer than relying on firmware disk numbering.
- Testing more than one BCD-entry construction strategy may reveal OEM compatibility differences.
- A `BootNext`/`bootsequence` write should be read back and validated in lab testing rather than merely assumed to have taken effect.

### Reasons not to copy it directly

- Some integrity paths can fail open when metadata or hashes are unavailable.
- Parts of the code reflect obsolete cryptography and security assumptions.
- Drive letters and broad searches across Windows volumes are weaker identities than exact GPT, partition, and volume identifiers.
- It makes persistent firmware display-order changes in addition to setting the next boot.
- It restores assumed power-state defaults rather than necessarily restoring the exact prior state.
- Cancellation is not the same thing as transactional rollback.
- Parsing command output can be brittle and localization-sensitive.
- Its Linux installation path is heavily patched and preseed-driven, conflicting with Omarchy Install's goal of handing off to the official ISO rather than maintaining another installer.

## Instlux: observed approach

[Instlux](https://github.com/belphegor-belbel/instlux) uses a relatively small NSIS Windows bootstrapper. For its openSUSE real-machine route it downloads a kernel and initrd rather than a full ISO, writes them under `C:\openSUSE`, and configures GRUB4DOS/BCD to start them.

The kernel command line contains an `install=http://...` source. Consequently, the Linux installer downloads the distribution after it has booted. Instlux writes an `openSUSE_hitme.txt` marker and uses GRUB's `find --set-root` to discover the filesystem containing the staged files. It writes uninstall support before modifying boot configuration and records the BCD identifier for cleanup.

It can use local installation media as an alternative. It explicitly refuses its real-machine route on UEFI systems and checks BitLocker conversion status on the system drive. The repository also includes VirtualBox, Hyper-V, and WSL paths, which are not directly relevant to Omarchy Install's native full-disk goal.

### Possible lessons to investigate

- A tiny network bootstrap avoids keeping a roughly 6 GB ISO on the disk that will be erased. It could also avoid copying that ISO into RAM before repartitioning.
- A unique marker is a simple way to reduce ambiguity when boot code must locate staged files.
- Windows can remain narrowly responsible for preparing a boot handoff while Linux owns installation.
- Local-media and network-source modes can coexist as separate product modes.

The network-bootstrap idea is a different architecture, not a straightforward optimization. It gives up the current offline-install property, introduces availability and integrity requirements for installation-time networking, and would require Omarchy to maintain a suitable stage-one environment. It should only be considered as a separately designed future mode.

### Reasons not to copy it directly

- Downloads and repository URLs use HTTP without an adequate modern cryptographic verification chain.
- The real-machine implementation is BIOS/GRUB4DOS-oriented and explicitly excludes UEFI.
- It creates a persistent boot-menu entry rather than Omarchy Install's intended one-shot handoff.
- Drive-letter and system-volume assumptions are not sufficiently strong disk identity.
- It has no equivalent to Omarchy Install's crash-safe transaction journal and identity-checked rollback.
- Its BitLocker check is narrower than the target-disk-wide checks Omarchy Install needs.
- Several URLs and platform assumptions are hard-coded.
- It does not provide Omarchy's unattended configuration and final-OS identity contract.

## Comparison with the current Omarchy Install design

Omarchy Install is already stronger than these projects in several important areas:

- Pinned GPG and SHA-256 verification fails closed.
- The atomic, versioned journal records pending operations before mutation and uses exact disk, GPT, partition, and volume identity for rollback.
- Secrets such as the user's password are excluded from the journal.
- Boot entries have per-operation descriptions and are intended for one-shot use.
- Power state is restored from recorded prior state instead of an assumed default.
- EFI files live in a private namespace, with an explicit rule never to modify `EFI/Microsoft`.
- Rollback validates that it is acting on the same disk and partitions that were originally staged.

There is one concrete discovery issue worth investigating. The generated GRUB configuration currently searches for a generic path:

```grub
search --no-floppy --set=img_part --file /omarchy.iso
```

If another attached filesystem also contains `/omarchy.iso`, that lookup may be ambiguous. WubiUEFI and Instlux demonstrate marker-based discovery, but that does not prove marker search is the best answer. Omarchy Install should independently compare an unpredictable per-operation marker, filesystem UUID, PARTUUID, and any constraints imposed by the official ISO's GRUB build.

The most significant missing capability suggested by WubiUEFI is a Linux-side failure artifact that survives an unsuccessful handoff. This is harder for Omarchy Install because a successful start of the full-disk installer destroys Windows and the staging partitions. Any solution therefore needs a fresh threat, privacy, lifecycle, and failure analysis; copying WubiUEFI's host-volume ZIP is not directly possible and might expose sensitive logs.

## Research priorities, not implementation commitments

1. Design and lab-test a post-reboot diagnostics strategy, including where artifacts could safely survive and whether they may contain credentials.
2. Test the one-shot UEFI handoff across representative OEM firmware, including immediate read-back of the resulting firmware boot sequence.
3. Compare unique-marker and stable-partition-identity approaches for GRUB staging discovery.
4. Evaluate whether a task graph would materially improve progress reporting and recovery semantics without obscuring the transaction journal.
5. Evaluate local-ISO selection and trusted mirror fallback within the existing fail-closed verification model.
6. Treat a small network stage-one as a separate future architecture study if reducing RAM and staging-space requirements becomes more important than offline installation and stock-ISO handoff.

None of these items should be implemented merely because WubiUEFI or Instlux did something similar. Each requires its own design argument, explicit invariants, failure-mode analysis, and hardware testing.
