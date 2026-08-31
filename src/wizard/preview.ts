import type { MachineProbe } from "../types";

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

/** Fixture matching the Linux stub, used when `bun run dev` is opened outside Tauri. */
export function previewProbe(): MachineProbe {
  const cSize = 400 * GIB;
  return {
    host: {
      os: "linux",
      arch: "x86_64",
      elevated: false,
      nativeWindows: false,
      osVersion: null,
    },
    uefi: true,
    secureBoot: false,
    efiVarsWritable: true,
    ramInstalledBytes: 16 * GIB,
    ramTotalPhysBytes: 15 * GIB + 512 * MIB,
    ramAvailBytes: 6 * GIB,
    ramOkForCopytoram: true,
    tpmPresent: true,
    recommendedDiskId: String.raw`\\.\PHYSICALDRIVE0`,
    targetEsp: {
      diskId: String.raw`\\.\PHYSICALDRIVE0`,
      diskGuid: "{dddddddd-dddd-4ddd-8ddd-dddddddddddd}",
      diskNumber: 0,
      partitionGuid: "{11111111-1111-1111-1111-111111111111}",
      volumeGuid: "\\\\?\\Volume{11111111-1111-1111-1111-111111111111}\\",
    },
    linuxById: "/dev/disk/by-id/nvme-VENDOR_DISK_1234",
    bitlocker: [
      {
        deviceId: "\\\\?\\Volume{33333333-3333-3333-3333-333333333333}\\",
        diskId: String.raw`\\.\PHYSICALDRIVE0`,
        mount: "C:",
        protectionStatus: 0,
        conversionStatus: 0,
        fullyDecrypted: true,
      },
    ],
    disks: [
      {
        deviceId: String.raw`\\.\PHYSICALDRIVE0`,
        sizeBytes: 512 * GIB,
        partitionStyle: "gpt",
        bus: "NVMe",
        isBoot: true,
        isRst: false,
        isDynamic: false,
        isStorageSpaces: false,
        maxShrinkBytes: 80 * GIB,
        partitions: [
          {
            gptGuid: "{11111111-1111-1111-1111-111111111111}",
            typeGuid: "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}",
            letter: null,
            label: "SYSTEM",
            sizeBytes: 100 * MIB,
            offsetBytes: MIB,
            fs: "fat32",
          },
          {
            gptGuid: "{22222222-2222-2222-2222-222222222222}",
            typeGuid: "{e3c9e316-0b5c-4db8-817d-f92df00215ae}",
            letter: null,
            label: null,
            sizeBytes: 16 * MIB,
            offsetBytes: 101 * MIB,
            fs: null,
          },
          {
            gptGuid: "{33333333-3333-3333-3333-333333333333}",
            typeGuid: "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}",
            letter: "C:",
            label: "Windows",
            sizeBytes: cSize,
            offsetBytes: 117 * MIB,
            fs: "ntfs",
          },
          {
            gptGuid: "{44444444-4444-4444-4444-444444444444}",
            typeGuid: "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}",
            letter: null,
            label: "WinRE",
            sizeBytes: 800 * MIB,
            offsetBytes: 117 * MIB + cSize,
            fs: "ntfs",
          },
        ],
      },
    ],
    blockingReasons: [],
  };
}

export function runningOutsideTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    !("__TAURI_INTERNALS__" in window) &&
    window.location.hostname !== "127.0.0.1"
  );
}
