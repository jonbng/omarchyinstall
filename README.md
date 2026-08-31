# Omarchy Install

Windows-to-Omarchy without a USB stick. You run this app on Windows; it stages a real installer partition, reboots into the official Omarchy ISO, and that installer **replaces Windows**. Not Wubi — see [docs/VISION.md](docs/VISION.md).

Tauri 2 desktop app. **Production is Windows only** (NSIS / MSI). `tauri dev` works on macOS and Linux so the UI and IPC can be built there; Win32 code is compiled only on Windows.

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

On macOS/Linux, `host_info` reports `nativeWindows: false`. Commands that call `platform::require_windows()` error with “this operation is only available on Windows”.

## Production build

Run on a Windows machine (WebView2 + MSVC build tools):

```sh
bun run tauri:build
```

Installers land in `src-tauri/target/release/bundle/{nsis,msi}/`.

## Rust layout

| Path | Role |
| --- | --- |
| `src-tauri/src/lib.rs` | App builder, plugins, command registration |
| `src-tauri/src/commands.rs` | Frontend IPC |
| `src-tauri/src/platform/windows.rs` | Win32 (`windows` crate) |
| `src-tauri/src/platform/stub.rs` | macOS/Linux stand-in |
| `src-tauri/src/error.rs` | Serializable errors for `invoke` |

Add new OS work under `platform/`, then expose it from `commands.rs`. Keep Windows-only crates in `[target.'cfg(windows)'.dependencies]`.
