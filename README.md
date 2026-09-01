# Omarchy Install

Install [Omarchy](https://omarchy.org/) from Windows without creating a bootable USB drive.

Omarchy Install is a portable Windows application that downloads and verifies the official Omarchy ISO, creates temporary installer partitions on the Windows boot disk, and configures a one-time UEFI boot into the official installer. The Omarchy installer then replaces Windows with a normal, native Omarchy installation.

> [!CAUTION]
> This project is experimental and its successful install path **erases Windows and every file on the target disk**. It is not a dual-boot installer. Back up anything you want to keep before using it.

For the complete design, safety model, and current engineering constraints, see [docs/VISION.md](docs/VISION.md).

## What it does

1. Checks that the PC and Windows disk are supported.
2. Collects the account and installation settings needed by Omarchy.
3. Downloads the latest official Omarchy ISO and verifies its SHA-256 checksum and GPG signature.
4. Shrinks the Windows partition and creates temporary installer and configuration partitions.
5. Stages the ISO bootloader without modifying `EFI/Microsoft` and configures a one-time UEFI boot entry.
6. Reboots into the official Omarchy installer, which wipes the target disk and installs Omarchy normally.

Windows is only the bootstrap environment. Omarchy is not installed inside an NTFS file, and Windows is not retained.

## Requirements

The current installer supports PCs with:

- 64-bit Windows 10 or Windows 11, running on a UEFI/GPT boot disk
- Administrator access
- Secure Boot disabled
- At least 12 GiB of installed RAM and about 10 GiB usable RAM
- At least 8 GiB plus 64 MiB of shrinkable space on the Windows partition
- A working internet connection for the roughly 6 GiB Omarchy ISO download
- BitLocker fully turned off and decrypted on the target disk is strongly recommended; suspending protection is not sufficient, and continuing without decryption requires an explicit recovery-risk acknowledgement

Intel RST/VMD/RAID, Dynamic Disks, Storage Spaces, legacy BIOS, and ARM Windows are not supported. The app performs a read-only machine check before allowing disk changes and explains any blockers it finds.

## Install Omarchy

1. Back up all files from the Windows PC and keep any BitLocker recovery keys somewhere off the PC.
2. Disable Secure Boot in the PC's firmware settings. Fully decrypting BitLocker-protected volumes is strongly recommended; you may continue without doing so after acknowledging the recovery and rollback risk.
3. Download `OmarchyInstaller-windows-x64.exe` from the [latest GitHub Release](https://github.com/jonbng/omarchyinstall/releases/latest).
4. Optionally verify the download against the accompanying `.sha256` file.
5. Open the executable normally and approve the Administrator prompt. The executable requests elevation automatically and is portable; there is no setup program to install.
6. Follow the wizard, review the selected disk carefully, and enter `ERASE WINDOWS` only when you are ready for that disk to be erased.
7. Leave the PC connected to power and the network while the ISO is downloaded, verified, and staged.
8. At the final prompt, reboot into the installer. Once the live Omarchy installer starts, the Windows installation cannot be recovered by this app.

Releases are currently unsigned, so Windows SmartScreen may display a warning. If the WebView2 runtime is unavailable, the same locally authenticated interface opens in the default browser.

### Before the final reboot

Choosing **Undo and exit** after staging attempts to remove the temporary partitions, boot entry, and EFI files and expand Windows again. This rollback is only available before rebooting into the live installer. A separate backup remains essential.

Runtime logs, the downloaded ISO, and rollback state are stored in `%LOCALAPPDATA%\OmarchyInstall`.

## Development

The desktop app uses Tauri 2, React 19, TypeScript, Vite, and Rust. Production builds target Windows, while the UI and IPC flow can be developed safely on Linux or macOS.

### Prerequisites

- [Bun](https://bun.sh/)
- The stable [Rust toolchain](https://rustup.rs/)
- [Tauri's platform prerequisites](https://v2.tauri.app/start/prerequisites/)
- On Windows, Visual Studio Build Tools with the MSVC C++ workload and a Windows SDK

Clone and install the dependencies:

```sh
git clone https://github.com/jonbng/omarchyinstall.git
cd omarchyinstall
bun install --frozen-lockfile
```

The submodules in `references/` contain upstream source used for design research and are not needed to build the application. Initialize the relevant reference only when you need it, for example:

```sh
git submodule update --init references/omarchy references/omarchy-iso references/omarchy-site
```

Start the desktop app:

```sh
bun run tauri:dev
```

On Linux and macOS this is a dry run: it uses a simulated compatible Windows PC, skips the ISO download, and never changes disks, EFI variables, or reboot state. Dry-run artifacts are written under `$XDG_DATA_HOME/OmarchyInstall`, or `~/.local/share/OmarchyInstall` by default.

Useful development commands:

```sh
bun run build          # Type-check and build the frontend
bun run check:rust     # Run Rust unit tests
bun run tauri:build    # Build the portable executable on Windows
```

To exercise blocked-machine UI states on a development host:

```sh
OMARCHY_STUB_BLOCKS=secure-boot,ram bun run tauri:dev
```

Set `OMARCHY_STUB_REAL_ISO=1` to download and verify the real official ISO during a development run.

## Windows builds

The [Windows workflow](.github/workflows/windows.yml) runs tests and builds the portable executable on pushes and pull requests. It can also be started from **Actions → Windows → Run workflow**.

The local build output is:

```text
src-tauri/target/release/OmarchyInstaller.exe
```

The MSVC runtime and WebView2 loader are linked statically. The CI artifact contains only the portable executable; the upstream repositories under `references/` are not bundled.

## Creating a GitHub Release

The [Release workflow](.github/workflows/release.yml) creates a release whenever a version tag beginning with `v` is pushed. It builds and tests the Windows app, verifies that the tag matches the project version, and publishes:

- `OmarchyInstaller-windows-x64.exe`
- `OmarchyInstaller-windows-x64.exe.sha256`
- automatically generated release notes

To publish a release:

1. Set the same version in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.
2. Commit and merge the version change to `main`.
3. Create and push the matching tag:

```sh
git tag -a v0.1.0 -m "Omarchy Install v0.1.0"
git push origin v0.1.0
```

The workflow fails instead of publishing if, for example, tag `v0.2.0` points to source files that still declare version `0.1.0`. Use a normal semantic version without the leading `v` in the three source files, and use the leading `v` only for the Git tag.

## Project layout

| Path | Purpose |
| --- | --- |
| `src/wizard/` | React installation wizard and Tauri IPC bridge |
| `src-tauri/src/commands.rs` | Commands exposed to the frontend |
| `src-tauri/src/platform/windows/` | Windows machine probe, elevation, disk staging, and rollback |
| `src-tauri/src/platform/stub.rs` | Safe Linux/macOS development implementation |
| `src-tauri/src/download.rs` | Official ISO resolution, download, and verification |
| `src-tauri/src/journal.rs` | Persistent operation state used for rollback |
| `docs/VISION.md` | Detailed product and implementation design |
| `references/` | Upstream and prior-art source checkouts; not shipped |

When adding operating-system-specific behavior, keep it behind `platform/`, expose it through `commands.rs`, and keep Windows-only crates under `[target.'cfg(windows)'.dependencies]`.
