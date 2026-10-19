# Precheck lab

The lab exercises the same read-only precheck path as the GUI without opening a
window. It records every response, checks result stability, reports latency by
component, detects leaked helpers, and can compare a machine against an accepted
baseline.

Build `OmarchyInstaller.exe`, then run from PowerShell:

```powershell
.\scripts\windows-precheck-lab.ps1 `
  -Exe .\src-tauri\target\release\OmarchyInstaller.exe `
  -Iterations 50 `
  -ConcurrentRuns 8 `
  -Concurrency 4 `
  -OutputDirectory .\precheck-lab
```

Add `-IoLoad` to test while the system drive is busy. The command exits nonzero
for a failed run, unstable identity, unstable blockers, an incomplete component,
or a leaked process. `-AllowIncomplete` is intended only for investigating a
known failure; it does not hide instability or process failures.

## Baselines

Create a baseline only after stable runs:

```powershell
.\scripts\windows-precheck-lab.ps1 `
  -Exe .\src-tauri\target\release\OmarchyInstaller.exe `
  -Baseline .\precheck-baseline.json `
  -UpdateBaseline
```

Omit `-UpdateBaseline` on later runs. The lab then fails if authoritative machine
identity or the blocking result changes. It refuses to create a baseline from
failed, incomplete, or unstable data.

## Artifacts

- `summary.json`: counts, stable hashes, baseline differences, and p50/p95/p99/max
  timings for the complete probe and each component.
- `runs.json`: full probe and diagnostics for every run, including errors and
  blocking reasons.

For a single raw response, run:

```powershell
.\OmarchyInstaller.exe --precheck-self-test
```

The storage-specific cross-validation harness remains useful when changing disk
enumeration: `windows-storage-soak.ps1` independently compares native fields to
`Get-Disk`, `Get-Partition`, `Get-Volume`, and WMI.

## Provider failure matrix

The provider fault lab verifies that timeouts, malformed or unexpectedly empty
responses, helper failures, and a panicking BitLocker worker all fail closed,
return promptly, and leave no child process behind:

```powershell
.\scripts\windows-precheck-fault-lab.ps1 `
  -Exe .\src-tauri\target\release\OmarchyInstaller.exe `
  -Iterations 10 `
  -Concurrency 4
```

These faults are accepted only by the private `--precheck-self-test` command.
They do not alter normal GUI probing and never mutate storage. The lab writes a
full `runs.json` plus a compact `summary.json` to `precheck-fault-lab`.

The deterministic policy matrix requires no VM or Windows host:

```powershell
bun run check:prechecks
```

Its cases live in `precheck-scenarios.json`. Every named fault must produce the
exact expected blocker set. Additional generated tests exercise all combinations
of independent target failures and up to sixteen unsupported secondary disks.

The same faults can drive the local development UI without Windows:

```powershell
$env:OMARCHY_STUB_BLOCKS = "ambiguous-esp,incomplete-bitlocker"
bun run tauri:dev
```

This uses the exact fault vocabulary from `precheck-scenarios.json`, so policy
tests and visual error-state testing cannot silently drift apart.

All lab operations are read-only. Shrink-capacity analysis remains deferred, and
the harness never resizes, formats, mounts, or changes a partition.
