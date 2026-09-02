import type { BlockingReason } from "../types";

export const ERASE_PHRASE = "ERASE WINDOWS";

export function formatBytes(n: number): string {
  const gib = 1024 ** 3;
  const mib = 1024 ** 2;
  if (n >= gib) {
    const v = n / gib;
    return `${v >= 10 ? v.toFixed(0) : v.toFixed(1)} GiB`;
  }
  if (n >= mib) {
    return `${(n / mib).toFixed(0)} MiB`;
  }
  return `${n} B`;
}

export function partitionKind(typeGuid: string | null): string | null {
  if (!typeGuid) return null;
  const g = typeGuid.replace(/[{}]/g, "").toLowerCase();
  if (g === "c12a7328-f81f-11d2-ba4b-00a0c93ec93b") return "EFI system";
  if (g === "e3c9e316-0b5c-4db8-817d-f92df00215ae") return "MSR";
  if (g === "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7") return "Windows";
  if (g === "de94bba4-06d1-4d40-a16a-bfd50179d6ac") return "Recovery";
  return null;
}

export function reasonTitle(reason: BlockingReason): string {
  switch (reason.type) {
    case "notElevated":
      return "Administrator required";
    case "notUefi":
      return "UEFI required";
    case "notGpt":
      return "GPT required";
    case "secureBoot":
      return "Secure Boot is on";
    case "bitLocker":
      return "BitLocker is still encrypting";
    case "ram":
      return "Not enough RAM";
    case "efiVarsLocked":
      return "Cannot write EFI variables";
    case "probeIncomplete":
      return "Safety probe incomplete";
    case "missingEsp":
      return "EFI system partition not found";
    case "ambiguousEsp":
      return "Multiple EFI partitions found";
    case "rst":
      return "Intel RST / RAID";
    case "dynamic":
      return "Dynamic disk";
    case "storageSpaces":
      return "Storage Spaces";
    case "shrinkTooSmall":
      return "Not enough shrinkable space";
  }
}

export function reasonBody(reason: BlockingReason): string {
  switch (reason.type) {
    case "notElevated":
      return "Omarchy Install must run as Administrator to probe firmware and later change partitions.";
    case "notUefi":
      return "This PC is using legacy BIOS. Omarchy Install only supports UEFI.";
    case "notGpt":
      return `The Windows disk (${reason.diskId}) is not GPT.`;
    case "secureBoot":
      return "The Omarchy ISO boots unsigned GRUB. Turn Secure Boot off in firmware. This is an ISO limitation, not a bug in this app.";
    case "bitLocker":
      return `BitLocker is not fully decrypted${reason.mount ? ` on ${reason.mount}` : ""}. Turn it off in Windows and wait until decryption finishes. Suspending BitLocker is not enough.`;
    case "ram":
      return `This PC has ${formatBytes(reason.haveInstalled)} installed (${formatBytes(reason.haveTotalPhys)} usable). Copying the live installer into RAM needs ${formatBytes(reason.needInstalled)} installed and about ${formatBytes(reason.needTotalPhys)} usable.`;
    case "efiVarsLocked":
      return "Windows refused EFI variable access. Re-run as Administrator. Without this, the one-shot installer boot entry cannot be set.";
    case "probeIncomplete":
      return `The ${reason.component} check did not complete. No disk changes are allowed until it succeeds.`;
    case "missingEsp":
      return `No EFI system partition was found on the Windows boot disk (${reason.diskId}).`;
    case "ambiguousEsp":
      return `${reason.count} EFI system partitions were found on the Windows boot disk (${reason.diskId}); automatic selection is unsafe.`;
    case "rst":
      return `Intel RST / VMD / RAID on ${reason.diskId} is not supported in v1.`;
    case "dynamic":
      return `Dynamic disks (${reason.diskId}) are not supported.`;
    case "storageSpaces":
      return `Storage Spaces (${reason.diskId}) is not supported.`;
    case "shrinkTooSmall":
      return `Windows can only free ${formatBytes(reason.have)}. The installer partition needs ${formatBytes(reason.need)}. Turn off hibernation and Fast Startup, then retry.`;
  }
}

export type AbortKind = "quit" | "rollback" | "last-chance" | "too-late";

export type AbortCopy = {
  kicker: string;
  title: string;
  body: string;
  confirm: string | null;
  cancel: string;
};

export function abortKind(opts: {
  step: string;
  native: boolean;
  journal: { omarchyinstGuid: string | null; cidataGuid: string | null; bootId: string | null } | null;
  phase: string;
  ready: boolean;
  rebooting: boolean;
}): AbortKind {
  if (!opts.native) return "quit";
  if (opts.rebooting) return "too-late";
  if (opts.ready) return "last-chance";
  const staged = !!(
    opts.journal?.omarchyinstGuid ||
    opts.journal?.cidataGuid ||
    opts.journal?.bootId
  );
  const mutating =
    opts.step === "Install" &&
    (opts.phase === "prepare" ||
      opts.phase === "stage" ||
      opts.phase === "cidata" ||
      opts.phase === "bootnext" ||
      opts.phase === "done");
  if (staged || mutating) return "rollback";
  return "quit";
}

export function abortCopy(kind: AbortKind, native: boolean): AbortCopy {
  switch (kind) {
    case "quit":
      return {
        kicker: "abort",
        title: "Abort omarchy-install?",
        body: native
          ? "The Windows installation has not been changed. A partial ISO download may remain in the cache and will resume next time."
          : "This is a dry run. Exiting does not change this machine.",
        confirm: "Exit",
        cancel: "Stay",
      };
    case "rollback":
      return {
        kicker: "abort",
        title: "Undo staging and exit?",
        body: "This app already started changing the disk — installer partition, ESP files, or BootNext. Abort will try to put Windows back. You can still boot Windows until you reboot into Omarchy.",
        confirm: "Undo and exit",
        cancel: "Stay",
      };
    case "last-chance":
      return {
        kicker: "last chance",
        title: "Abort before reboot?",
        body: "BootNext is set. After reboot, the official installer wipes this disk and this app cannot undo. Abort now rolls staging back. Stay if you still want to reboot.",
        confirm: "Undo and exit",
        cancel: "Stay",
      };
    case "too-late":
      return {
        kicker: "too late",
        title: "Too late to abort from here",
        body: "Reboot into the Omarchy installer was requested. If the live environment starts, it will wipe the disk — there is no Windows rollback. If you land back in Windows, open this app again and undo staging.",
        confirm: null,
        cancel: "Dismiss",
      };
  }
}

export type ReasonAction = "relaunch" | "firmware";

export function reasonAction(reason: BlockingReason): ReasonAction | null {
  switch (reason.type) {
    case "notElevated":
    case "efiVarsLocked":
      return "relaunch";
    case "secureBoot":
      return "firmware";
    default:
      return null;
  }
}
