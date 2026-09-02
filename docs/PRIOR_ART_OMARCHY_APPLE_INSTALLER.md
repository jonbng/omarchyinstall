# Prior-art notes: Omarchy Apple Installer

| Field | Value |
| --- | --- |
| **Date reviewed** | 2026-09-02 |
| **Source** | `references/omarchy-apple-installer` |
| **Status** | Research notes — not a design decision |

> [!CAUTION]
> This document records how another project approached a related installer problem. It does **not** establish that its choices were correct, safe, production-proven, or appropriate for Omarchy Install. Prior use is not validation. Every idea below must be evaluated against Omarchy Install's own goals, threat model, Windows behavior, supported hardware, recovery requirements, and stock-ISO constraint before adoption.

## Executive summary

The Apple installer is more useful as a reference for trust boundaries and mutation discipline than as a reference for installing Linux. Its strongest ideas are:

1. Bind user approval to an exact, immutable execution plan.
2. Keep the UI unprivileged and perform mutations through a small authenticated helper.
3. Prevent concurrent execution across both UI instances and privileged clients.
4. Distinguish recorded intent, verified completion, and ambiguous mutation state.
5. Read critical data back from its final destination after writing it.
6. Derive progress from durable, validated execution records.
7. Test the second boot and the preservation or removal of surrounding state.

Its direct-image Apple installation architecture should not be copied into Omarchy Install. It requires custom OS images, a patched Asahi engine, Apple Recovery authorization, and a platform-specific boot chain. Omarchy Install deliberately hands off to the official stock ISO.

## Observed Apple installer flow

The Apple installer broadly performs the following sequence:

1. An unprivileged macOS application inspects the exact Apple model, APFS layout, FileVault state, power source, and available space.
2. It fetches an Ed25519-signed support catalog.
3. The catalog admits a specific device model and pins the installer engine, metadata, repair manifest, and full Omarchy payload by exact size and SHA-256.
4. A pinned, patched Asahi installer engine inventories the disk and produces a proposed plan.
5. The plan binds the model, current layout digest, selected extent, offsets, sizes, artifact identities, and required human steps.
6. The user reviews and approves that exact plan.
7. A root LaunchDaemon receives the handoff through authenticated XPC, verifies the machine-owner credential, imports the package into root-owned storage, and independently validates its contents.
8. The engine creates an Apple boot-policy stub and the EFI, boot, and root partitions, then writes a complete prebuilt Omarchy system.
9. Written files and raw partition contents are read back and compared with the source payload.
10. An append-only journal records intent events, checkpoints, evidence digests, phases, and the completion outcome.
11. The owner enters Apple Recovery and authorizes the new boot policy.
12. The machine boots through m1n1, U-Boot, GRUB, and finally Omarchy while retaining macOS.

## How its flow differs from Omarchy Install

| Omarchy Install | Apple installer |
| --- | --- |
| Runs the Tauri application elevated | Keeps the UI unprivileged and delegates mutation to a root helper |
| Confirmation is primarily tied to wizard state | Approval is bound to the exact candidate plan |
| Downloads the official Omarchy ISO | Downloads an engine, metadata, and model-specific final OS images |
| Creates temporary staging and cidata partitions | Creates the final Linux partitions directly |
| Reboots into the stock installer | Performs most installation before reboot |
| Linux erases Windows and the staging partitions | Preserves macOS and installs alongside it |
| Uses one-shot UEFI boot selection | Uses Apple's boot-policy and Recovery mechanisms |
| Maintains an atomic rollback-state document | Maintains an append-only intent/checkpoint/evidence transcript |
| Can reverse pre-reboot staging mutations | Often stops on ambiguous state and enters repair or manual recovery |
| Supports resumable ISO byte ranges | Resumes large payloads primarily at part granularity |

The Apple installer avoids the same-disk ISO and `copytoram` problem by writing the final operating system directly. That advantage comes from accepting a much larger maintenance burden: it owns a custom installation engine, full OS images, hardware-specific boot integration, and repair logic.

## Improvements worth considering

### 1. Seal the execution plan before confirmation

Before mutation, Omarchy Install could construct a canonical `InstallPlan` containing at least:

- Operation ID
- Physical disk identity
- Current partition-layout fingerprint
- Windows partition GUID and expected size
- Proposed partition offsets, sizes, types, labels, and GUIDs
- ESP identity
- ISO release, size, and digest
- Generated boot-entry description
- GRUB locator identity
- Cidata schema or content identity, excluding secrets
- Required manual actions
- Expected final pre-reboot layout

The backend would hash that canonical plan. The confirmation screen would approve the resulting plan identity, not merely the current frontend state. Immediately before execution, the backend would re-probe the machine and reject stale approval if the layout, target identity, release, or generated inputs changed.

Omarchy Install already journals exact disk identities and revalidates important state. Candidate-bound approval would connect those protections directly to what the user reviewed.

### 2. Investigate a small privileged Windows helper

The Apple app keeps its UI outside the privileged process. Its helper exposes a narrow operation, authenticates its client, imports files through a directory handle, rechecks permissions and types, and hashes artifacts again inside the privileged boundary.

Omarchy Install currently elevates the whole Tauri application, including its WebView and browser-facing surface. A possible Windows equivalent is:

```text
Normal UI process
  -> authenticated, narrow local IPC
    -> short-lived elevated native helper
      -> re-probe machine
      -> validate sealed plan and artifacts
      -> perform allowed mutations
      -> return structured results
```

This should begin as a threat-model and packaging study. A permanent installed service would conflict with the portable-app goal and would add its own attack surface. A transient elevated mode of the same signed executable may be more suitable, but it must not accept arbitrary commands, scripts, or paths.

### 3. Add cross-process exclusivity

The Apple project has both a per-user application lease and a single-flight guard in its root helper. Omarchy Install has in-process locking for some operations, but that does not necessarily stop another executable instance from starting a competing mutation sequence.

Potential design:

- A Windows named mutex for one active UI instance per user.
- A machine-wide mutation lease acquired by every destructive operation.
- An operation identity attached to the lease and journal.
- Read-only inspection and support export may remain available when safe.

The mutation lease matters more than merely disabling buttons in one window.

### 4. Model ambiguous mutation explicitly

The Apple engine records an intent before each mutation and a checkpoint afterward. If it finds intent without a checkpoint for a non-repeatable operation, it refuses to guess or replay it automatically.

Omarchy Install should retain its stronger rollback journal while considering more explicit states:

- Not started
- Intent durably recorded
- Completed but not yet verified
- Completed and verified
- Failed before mutation
- State ambiguous after interruption
- Rolled back and verified

Each operation should declare whether it is:

- Safe to retry
- Safe only after inspection
- Rollback-only
- Not automatically recoverable

Recovery should inspect actual disk and firmware state rather than equating “no completion record” with “nothing happened.”

### 5. Verify final state by reading it back

The Apple installer reopens written partitions and compares their contents with the source payload. Omarchy Install could apply the same principle to its own boundary:

- Rehash the ISO after it reaches the staging volume.
- Reopen and hash the staged EFI executable against the verified extracted source.
- Re-read and compare GRUB configuration and search-marker contents.
- Re-read and parse cidata files, checking expected non-secret values.
- Re-enumerate partitions and verify GUID, offset, size, filesystem, and label.
- Re-enumerate the BCD entry and confirm its device, path, description, and identifier.
- Read back firmware `bootsequence` after setting it and confirm the intended entry appears.

A successful write command is not equivalent to evidence that the intended final state exists.

### 6. Derive progress from durable state

The Apple helper streams its durable execution journal to the UI. Streaming is advisory; the final transcript is separately read and validated before success is reported.

For Omarchy Install, progress could become a projection of verified journal state:

- Completed phases come from durable records rather than UI assumptions.
- Restarting the app reconstructs the same state from disk.
- Progress messages never independently prove that a mutation completed.
- The final result is validated separately from the progress channel.

This should extend the existing journal, not introduce a competing progress database.

### 7. Strengthen physical qualification

The Apple runbook requires checking protected partitions, performing the first Linux boot, returning to macOS, and then performing a second Linux boot. The second boot helps detect accidental reliance on transient installation state.

The equivalent Omarchy Install matrix should cover:

- Failure and process termination around every mutation boundary
- Failed firmware handoff returning to Windows
- Rollback after each pre-reboot checkpoint
- First Omarchy boot
- Second Omarchy boot
- Removal of staging partitions after success
- Absence of stale temporary EFI files and firmware entries
- Successful boot without any state left over from a previous attempt
- Representative Windows versions, disk layouts, and OEM firmware

### 8. Consider a signed compatibility catalog later

The Apple catalog can admit or disable individual models, bind exact artifacts, expire releases, and reject catalog sequence rollback. Omarchy could eventually use something similar to disable a broken installer release or reject a known-dangerous hardware configuration without shipping a new executable.

This is not an automatic improvement. It creates an online control plane with signing-key operations, expiry and system-clock concerns, availability requirements, rollback policy, and incident-response obligations. Embedded policy plus the pinned Omarchy signing key may remain the safer v1 choice.

### 9. Preserve release provenance

The Apple engine records exact upstream repositories, tags, commits, submodules, build inputs, checksums, toolchain versions, overlay files, and build recipes. Omarchy Install does not build the official ISO, but the same idea can apply to release evidence:

- Exact installer source revision
- Embedded signing-key fingerprint
- Expected ISO release policy
- Dependency lockfiles and toolchain versions
- Windows build and signing identity
- Physical qualification results tied to the executable digest

This would make it easier to answer exactly what was tested and shipped.

## Ideas not previously developed as strongly

- User approval as a capability bound to the exact executable disk plan.
- Independent validation again inside the privileged boundary.
- Cross-process mutation exclusion rather than only UI busy state.
- Explicit ambiguous-mutation handling.
- Progress reconstructed from durable evidence.
- Destination read-back verification after successful commands.
- Binding hardware support, release identity, disk layout, and human steps into one candidate identity.
- Treating required firmware actions as part of the reviewed plan.
- Identity-based repair or replacement of a recognized previous installation.
- Requiring a second successful boot during physical qualification.

## UX flow observation adopted for evaluation

The Apple installer prepares and verifies its payload before presenting the destructive plan. That ordering is an observation, not evidence that the rest of its UI or implementation is correct.

Omarchy Install is adopting a related but independently designed flow:

1. The welcome screen chooses the latest official ISO or a local official ISO.
2. Clicking **Begin installation** starts acquisition and verification.
3. Machine checks and account configuration continue while that work runs in the background.
4. A persistent status strip reports real download and verification state.
5. The review step waits until the media is verified before exposing `ERASE WINDOWS`.
6. The media is checked again immediately before the first disk mutation.

This is intentionally not a direct copy. Unlike the Apple installer, Omarchy Install does not require a dedicated blocking download screen, and it retains the stronger typed erase phrase. The flow still needs independent usability testing, failure testing, and physical Windows qualification.

## What should not be copied directly

- Direct installation of custom final-system images.
- Maintaining a patched Asahi-derived installer engine for the Windows path.
- Apple-specific APFS, `bless`, One True Recovery, m1n1, or U-Boot behavior.
- A permanently installed privileged daemon without first justifying its lifecycle and attack surface.
- Its weaker part-level download resume in place of Omarchy Install's byte-range resume.
- Its repair and replace modes before the basic full-disk Windows replacement path is proven.
- Password handling designed for Apple machine-owner authorization; Omarchy's credentials have a different purpose.
- Assuming numerous unit tests or a written runbook establish broad hardware safety.

## Evidence and limitations of this review

- The repository contains an extensive Swift test suite, but it could not be executed on the Linux research host.
- All 89 Python engine tests passed locally on 2026-09-02.
- The physical-install runbook is a test procedure, not by itself proof of a successful qualification run.
- The source lock describes a reproducibility scope of two clean builds on the same host and identifies its validation artifact as unsigned.
- The copied reference directory does not retain independent Git metadata, so this review does not establish the project's complete history or provenance.

These limitations reinforce the central rule: useful engineering ideas must still be independently reviewed and tested before becoming part of Omarchy Install.

## Suggested order of investigation

1. Sealed, candidate-bound install plan.
2. Machine-wide and per-process execution leases.
3. Read-back verification for disk, ESP, cidata, BCD, and firmware writes.
4. Explicit ambiguous-state recovery semantics.
5. Progress derived from durable journal state.
6. Privileged-helper threat model and prototype.
7. Expanded physical qualification, including a second boot.
8. Optional signed compatibility catalog only if operationally justified.

This order is a research recommendation, not an implementation commitment.
