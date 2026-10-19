export type HostInfo = {
  os: string;
  arch: string;
  elevated: boolean;
  nativeWindows: boolean;
  osVersion: string | null;
};

export type BlockingReason =
  | { type: "notElevated" }
  | { type: "notUefi" }
  | { type: "notGpt"; diskId: string }
  | { type: "secureBoot" }
  | { type: "bitLocker"; mount: string | null }
  | {
      type: "ram";
      haveInstalled: number;
      haveTotalPhys: number;
      needInstalled: number;
      needTotalPhys: number;
    }
  | { type: "efiVarsLocked" }
  | { type: "probeIncomplete"; component: string; detail: string }
  | { type: "missingEsp"; diskId: string }
  | { type: "ambiguousEsp"; diskId: string; count: number }
  | { type: "rst"; diskId: string }
  | { type: "dynamic"; diskId: string }
  | { type: "storageSpaces"; diskId: string }
  | { type: "shrinkTooSmall"; have: number; need: number };

export type BitlockerVolume = {
  deviceId: string | null;
  diskId: string | null;
  mount: string | null;
  protectionStatus: number;
  conversionStatus: number;
  fullyDecrypted: boolean;
};

export type PartitionMap = {
  gptGuid: string | null;
  typeGuid: string | null;
  letter: string | null;
  label: string | null;
  sizeBytes: number;
  offsetBytes: number;
  fs: string | null;
};

export type DiskMap = {
  deviceId: string;
  sizeBytes: number;
  partitionStyle: string;
  bus: string | null;
  isBoot: boolean;
  isRst: boolean;
  isDynamic: boolean;
  isStorageSpaces: boolean;
  maxShrinkBytes: number | null;
  partitions: PartitionMap[];
};

export type MachineProbe = {
  host: HostInfo;
  uefi: boolean;
  secureBoot: boolean;
  efiVarsWritable: boolean;
  ramInstalledBytes: number;
  ramTotalPhysBytes: number;
  ramAvailBytes: number;
  ramOkForCopytoram: boolean;
  tpmPresent: boolean;
  recommendedDiskId: string | null;
  targetEsp: {
    diskId: string;
    diskGuid: string;
    diskNumber: number;
    partitionGuid: string;
    volumeGuid: string;
  } | null;
  linuxById: string | null;
  bitlocker: BitlockerVolume[];
  disks: DiskMap[];
  blockingReasons: BlockingReason[];
};

export type CidataIdentity = {
  username: string;
  password: string;
  hostname: string;
  timezone: string;
  keyboard: string;
  encrypt: boolean;
  fullName: string | null;
  email: string | null;
};

export type IsoProgress = {
  phase: "download" | "hash" | "signature";
  bytes: number;
  total: number | null;
};

export type VerifyResult = {
  sha256: string;
  bytes: number;
};

export type LocalIsoSelection = {
  path: string;
  filename: string;
  bytes: number;
};

export type PrepareResult = {
  omarchyinstGuid: string;
  omarchyinstPartuuid: string;
  cidataGuid: string;
  oldCSizeBytes: number;
  newCSizeBytes: number;
  partitionBytes: number;
};

export type InstallPlan = {
  version: number;
  targetDiskGuid: string;
  targetDiskNumber: number;
  targetDiskDeviceId: string;
  windowsPartitionGuid: string;
  windowsPartitionOffsetBytes: number;
  oldCSizeBytes: number;
  newCSizeBytes: number;
  espPartitionGuid: string;
  espVolumeGuid: string;
  omarchyinstOffsetBytes: number;
  omarchyinstSizeBytes: number;
  cidataOffsetBytes: number;
  cidataSizeBytes: number;
  linuxDevice: string;
  isoSha256: string;
  isoSizeBytes: number;
  bootContractVersion: number;
  cidataContractVersion: number;
  acknowledgementVersion: number;
  destructiveEraseAcknowledged: boolean;
  bitlockerRiskAcknowledged: boolean;
};

export type InstallPlanPreview = {
  plan: InstallPlan;
  digest: string;
};

export type InstallProgress = {
  sequence: number;
  phase: "prepare" | "stage" | "cidata" | "bootnext";
  status: "started" | "completed" | "failed";
  operationId: string | null;
  planDigest: string | null;
  detail: string | null;
};

export type RecoveryDisposition =
  | "fresh"
  | "safeToResume"
  | "safeToRetry"
  | "rollbackRequired"
  | "manualReconciliationRequired"
  | "completed";

export type RecoveryAssessment = {
  disposition: RecoveryDisposition;
  reason: string;
  operationId: string | null;
  step: string | null;
  pendingOperation: string | null;
};

export type StageResult = {
  espGuid: string;
  searchFilename: string;
  grubCfgSha256: string;
};

export type CidataResult = {
  cidataGuid: string;
  linuxDevice: string;
  encrypt: boolean;
};

export type BootNextResult = {
  bootId: string;
  bcdFirmwareId: string | null;
  appendedBootOrder: boolean;
};

export type StateJournal = {
  version: number;
  step: string;
  operationId: string;
  pendingOperation: string | null;
  targetDiskGuid: string | null;
  targetDiskNumber: number | null;
  windowsPartitionGuid: string | null;
  espPartitionGuid: string | null;
  espVolumeGuid: string | null;
  omarchyinstGuid: string | null;
  cidataGuid: string | null;
  linuxDevice: string | null;
  isoSha256?: string | null;
  searchFilename: string | null;
  bootId: string | null;
  planDigest?: string | null;
};

export const INSTALLER_HOLE_BYTES = 8 * 1024 ** 3 + 512 * 1024 ** 2;
export const OMARCHYINST_BYTES = 8 * 1024 ** 3;
export const CIDATA_BYTES = 512 * 1024 ** 2;

export const ESP_TYPE = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}";
export const WINRE_TYPE = "{de94bba4-06d1-4d40-a16a-bfd50179d6ac}";
export const BASIC_DATA_TYPE = "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}";
