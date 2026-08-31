# Omarchy Install

Windows-to-Omarchy without a USB stick. You run this app on Windows; it stages a real installer partition, reboots into the official Omarchy ISO, and that installer **replaces Windows**. Not Wubi — see [docs/VISION.md](docs/VISION.md).

Tauri 2 desktop app. **Production is Windows only** and ships as one portable `OmarchyInstaller.exe`; there is nothing to install or uninstall. `tauri dev` works on macOS and Linux so the UI and IPC can be built there; Win32 code is compiled only on Windows.

## Clone

```sh
git clone --recurse-submodules <url>
```

If you already cloned without submodules:

```sh
git submodule update --init
```

`--recursive` also initializes `archiso` inside `references/omarchy-iso` (upstream’s own submodule). That is only needed if you are building the ISO.

## References

Upstream checkouts, tracked as submodules:

| Path | Repo | Branch |
| --- | --- | --- |
| `references/omarchy` | [omacom/omarchy](https://github.com/omacom/omarchy) | `quattro` |
| `references/omarchy-iso` | [omacom/omarchy-iso](https://github.com/omacom/omarchy-iso) | `quattro` |
| `references/omarchy-site` | [omacom/omarchy-site](https://github.com/omacom/omarchy-site) | `master` |

## Dev

```sh
bun install
bun run tauri:dev
```

Rust unit tests (no WebView required):

```sh
bun run check:rust
```

On macOS/Linux, `host_info` reports `nativeWindows: false`. The wizard is a full **dry run**: probe data is a canned Windows PC, the 6 GB ISO download is skipped, and mutate IPC succeeds without touching disks, EFI, or rebooting. Generated `grub.cfg` / cidata and `state.json` land in `$XDG_DATA_HOME/OmarchyInstall` (default `~/.local/share/OmarchyInstall`).

Inject probe blockers with `OMARCHY_STUB_BLOCKS=secure-boot,ram bun run tauri:dev`. Set `OMARCHY_STUB_REAL_ISO=1` to actually download and verify the official ISO.

## Production build

Run on a Windows machine with MSVC build tools:

```sh
bun run tauri:build
```

The portable executable lands at `src-tauri/target/release/OmarchyInstaller.exe`. It has the MSVC runtime and WebView2 loader linked statically. If the WebView2 runtime itself is unavailable, the executable keeps running as an authenticated loopback backend and opens the same UI in the default browser. Pass `--browser` to test that path explicitly.

CI does the same on `windows-latest` (`.github/workflows/windows.yml`): push/PR to `main`, or **Actions → Windows → Run workflow**. The artifact contains only `OmarchyInstaller.exe`. It is currently unsigned, so SmartScreen may warn.

Portable means the app itself is not registered with Windows. Runtime data that must survive a relaunch—logs, the ISO cache, and the rollback journal—still lives under `%LOCALAPPDATA%\OmarchyInstall`.

## Rust layout

| Path | Role |
| --- | --- |
| `src-tauri/src/lib.rs` | App builder, plugins, command registration |
| `src-tauri/src/browser.rs` | Authenticated loopback fallback when WebView2 is unavailable |
| `src-tauri/src/commands.rs` | Frontend IPC |
| `src-tauri/src/platform/windows/` | Win32: `elevation`, `probe`, `mutate` |
| `src-tauri/src/platform/stub.rs` | macOS/Linux stand-in |
| `src-tauri/src/error.rs` | Serializable errors for `invoke` |

Add new OS work under `platform/`, then expose it from `commands.rs`. Keep Windows-only crates in `[target.'cfg(windows)'.dependencies]`.
