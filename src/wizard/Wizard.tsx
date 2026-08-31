import { useEffect, useMemo, useRef, useState } from "react";
import type {
  CidataIdentity,
  DiskMap,
  IsoProgress,
  MachineProbe,
  PartitionMap,
  StateJournal,
  VerifyResult,
} from "../types";
import { INSTALLER_HOLE_BYTES } from "../types";
import {
  abortCopy,
  abortKind,
  ERASE_PHRASE,
  formatBytes,
  partitionKind,
  reasonAction,
  reasonBody,
  reasonTitle,
} from "./copy";
import {
  hostnameError,
  identityOk as identityFieldsOk,
  installStartFromJournal,
  invokeError,
  passwordError,
  usernameError,
} from "./ipc";
import { AsciiLogo, Mark } from "./Logo";
import {
  getVersion,
  invoke,
  listen,
  onBridgeStatus,
  revealItemInDir,
  runtimeMode,
} from "./bridge";
import { previewProbe, runningOutsideTauri } from "./preview";
import { AbortButton, AbortDialog, closeWindow } from "./TitleControls";
import "./wizard.css";

const STEPS = ["Welcome", "Machine", "Identity", "Backup", "Confirm", "Install"] as const;
type Step = (typeof STEPS)[number];

const STEP_SLUG: Record<Step, string> = {
  Welcome: "welcome",
  Machine: "machine",
  Identity: "identity",
  Backup: "backup",
  Confirm: "confirm",
  Install: "install",
};

const KEYBOARDS = [
  { id: "us", label: "English (US)" },
  { id: "uk", label: "English (UK)" },
  { id: "de", label: "German" },
  { id: "fr", label: "French" },
  { id: "se", label: "Swedish" },
  { id: "dk", label: "Danish" },
  { id: "no", label: "Norwegian" },
  { id: "fi", label: "Finnish" },
  { id: "es", label: "Spanish" },
  { id: "it", label: "Italian" },
  { id: "pl", label: "Polish" },
  { id: "jp", label: "Japanese" },
];

const INSTALL_PHASES = [
  { id: "download", label: "download iso" },
  { id: "hash", label: "sha256" },
  { id: "signature", label: "gpg signature" },
  { id: "prepare", label: "installer partitions" },
  { id: "stage", label: "grub on esp" },
  { id: "cidata", label: "autoinstall cidata" },
  { id: "bootnext", label: "uefi bootnext" },
] as const;

function emptyIdentity(): CidataIdentity {
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  return {
    username: "",
    password: "",
    hostname: "omarchy",
    timezone,
    keyboard: "us",
    encrypt: true,
    fullName: null,
    email: null,
  };
}

function initialStep(): Step {
  if (!import.meta.env.DEV || typeof window === "undefined") return "Welcome";
  const hash = window.location.hash.replace(/^#/, "");
  return (STEPS as readonly string[]).includes(hash) ? (hash as Step) : "Welcome";
}

export default function Wizard() {
  const [step, setStep] = useState<Step>(initialStep);
  const [probe, setProbe] = useState<MachineProbe | null>(null);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [probing, setProbing] = useState(true);
  const [identity, setIdentity] = useState<CidataIdentity>(emptyIdentity);
  const [password2, setPassword2] = useState("");
  const [eraseInput, setEraseInput] = useState("");
  const [secondConfirm, setSecondConfirm] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [journal, setJournal] = useState<StateJournal | null>(null);
  const [installPhase, setInstallPhase] = useState("idle");
  const [installReady, setInstallReady] = useState(false);
  const [rebooting, setRebooting] = useState(false);
  const [abortOpen, setAbortOpen] = useState(false);
  const [abortBusy, setAbortBusy] = useState(false);
  const [abortError, setAbortError] = useState<string | null>(null);
  const [version, setVersion] = useState("0.1.0");
  const [bridgeStatus, setBridgeStatus] = useState<"connected" | "disconnected">("connected");
  const allowClose = useRef(false);

  async function loadProbe() {
    setProbing(true);
    setProbeError(null);
    try {
      const next = await invoke<MachineProbe>("probe_machine");
      setProbe(next);
    } catch (err: unknown) {
      if (import.meta.env.DEV && runningOutsideTauri()) {
        setProbe(previewProbe());
      } else {
        setProbeError(invokeError(err));
      }
    } finally {
      setProbing(false);
    }
  }

  useEffect(() => {
    void loadProbe();
    void invoke<StateJournal | null>("load_install_state")
      .then((next) => {
        setJournal(next);
        if (next?.step === "bootNextSet" && initialStep() === "Welcome") {
          setStep("Install");
        }
      })
      .catch((error: unknown) => {
        setJournal(null);
        setActionError(invokeError(error));
      });
  }, []);

  useEffect(() => {
    void getVersion().then(setVersion).catch(() => undefined);
  }, []);

  useEffect(() => onBridgeStatus(setBridgeStatus), []);

  useEffect(() => {
    if (runtimeMode() !== "tauri") return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      unlisten = await win.onCloseRequested((event) => {
        if (allowClose.current) return;
        event.preventDefault();
        setAbortError(null);
        setAbortOpen(true);
      });
      if (cancelled) unlisten?.();
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const blocked = (probe?.blockingReasons.length ?? 0) > 0;
  const native = probe?.host.nativeWindows ?? false;
  const identityOk = identityFieldsOk(
    identity.username,
    identity.hostname,
    identity.password,
    password2,
  );
  const eraseOk = eraseInput.trim() === ERASE_PHRASE;

  const index = STEPS.indexOf(step);
  const canContinue = useMemo(() => {
    switch (step) {
      case "Welcome":
        return true;
      case "Machine":
        return !!probe && !blocked && !probing;
      case "Identity":
        return identityOk;
      case "Backup":
        return true;
      case "Confirm":
        return eraseOk && secondConfirm;
      case "Install":
        return false;
    }
  }, [step, probe, blocked, probing, identityOk, eraseOk, secondConfirm]);

  function goTo(name: Step) {
    const next = STEPS.indexOf(name);
    if (next < 0 || next >= index) return;
    setSecondConfirm(false);
    setStep(name);
  }

  function goNext() {
    if (!canContinue) return;
    const next = STEPS[index + 1];
    if (next) setStep(next);
  }

  function goBack() {
    const prev = STEPS[index - 1];
    if (prev && step !== "Install") {
      setSecondConfirm(false);
      setStep(prev);
    }
  }

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.defaultPrevented) return;
      const target = event.target as HTMLElement | null;
      if (target?.closest("button, a, summary")) return;
      if (event.key === "Escape") {
        if (abortOpen) return;
        if (index === 0 || step === "Install") return;
        event.preventDefault();
        goBack();
        return;
      }
      if (event.key !== "Enter" || step === "Install") return;
      if (!canContinue) return;
      event.preventDefault();
      goNext();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step, canContinue, index, abortOpen]);

  const stageRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (step !== "Identity" && step !== "Confirm") return;
    const root = stageRef.current;
    const field = root?.querySelector<HTMLInputElement | HTMLSelectElement>(
      "input:not([type=checkbox]), select",
    );
    field?.focus();
  }, [step]);

  async function onRelaunch() {
    setActionError(null);
    setBusy(true);
    try {
      await invoke("relaunch_elevated");
    } catch (err: unknown) {
      setActionError(invokeError(err));
      setBusy(false);
    }
  }

  async function onFirmware() {
    setActionError(null);
    setBusy(true);
    try {
      await invoke("reboot_to_firmware");
    } catch (err: unknown) {
      setActionError(invokeError(err));
      setBusy(false);
    }
  }

  const continueLabel =
    step === "Welcome" ? "Begin" : step === "Confirm" ? "Windows will be erased" : "Continue";

  const kind = abortKind({
    step,
    native,
    journal,
    phase: installPhase,
    ready: installReady,
    rebooting,
  });
  const abort = abortCopy(kind, native);

  async function confirmAbort() {
    if (!abort.confirm) {
      setAbortOpen(false);
      return;
    }
    setAbortError(null);
    if (kind === "quit") {
      allowClose.current = true;
      try {
        await closeWindow();
      } catch (err: unknown) {
        allowClose.current = false;
        setAbortError(invokeError(err));
      }
      return;
    }
    setAbortBusy(true);
    try {
      await invoke("abort_and_rollback");
      setJournal(null);
      setInstallReady(false);
      setInstallPhase("idle");
      allowClose.current = true;
      await closeWindow();
    } catch (err: unknown) {
      allowClose.current = false;
      setAbortError(invokeError(err));
      setAbortBusy(false);
    }
  }

  async function undoStaging() {
    await invoke("abort_and_rollback");
    setJournal(null);
    setInstallReady(false);
    setInstallPhase("idle");
  }

  return (
    <div className="wizard" data-step={step}>
      {runtimeMode() === "browser" && (
        <div className={`browser-fallback ${bridgeStatus}`} role="status">
          {bridgeStatus === "connected"
            ? "WebView2 is unavailable, so Omarchy Install is running securely in your browser. Keep this tab open."
            : "The local installer backend disconnected. Reopen OmarchyInstaller.exe to resume safely."}
        </div>
      )}
      <header className="waybar">
        <div className="waybar-left" data-tauri-drag-region>
          <Mark />
          <span className="product">omarchy-install</span>
        </div>
        <ol className="crumbs" aria-label="Install steps" data-tauri-drag-region>
          {STEPS.map((name, i) => {
            const state = name === step ? "current" : i < index ? "done" : "todo";
            return (
              <li key={name} className={state === "todo" ? undefined : state}>
                <button
                  type="button"
                  aria-current={name === step ? "step" : undefined}
                  disabled={i >= index}
                  onClick={() => goTo(name)}
                >
                  {STEP_SLUG[name]}
                </button>
              </li>
            );
          })}
        </ol>
        <div className="waybar-right">
          {probe && (
            <span className={`chip ${native ? "ok" : "dev"}`}>
              {native ? (probe.host.osVersion ?? "windows") : `dry run · ${probe.host.os}`}
            </span>
          )}
          <AbortButton
            onClick={() => {
              setAbortError(null);
              setAbortOpen(true);
            }}
          />
        </div>
      </header>

      <section className="stage" aria-live="polite" ref={stageRef}>
        {journal && (
          <p className="banner">
            {journal.step === "bootNextSet"
              ? "Installer staging is ready. Reboot from the install step, or undo."
              : journal.omarchyinstGuid
                ? "A previous staging journal exists. Finish identity if you want to continue, or undo."
                : "A previous staging journal exists."}
            <button
              type="button"
              className="btn ghost"
              onClick={() => {
                void undoStaging().catch((err: unknown) => setActionError(invokeError(err)));
              }}
            >
              Undo staging
            </button>
          </p>
        )}
        <div className="stage-body" key={step}>
          {step === "Welcome" && <Welcome />}
          {step === "Machine" && (
            <ProbeStep
              probe={probe}
              probing={probing}
              error={probeError}
              actionError={actionError}
              busy={busy}
              onRetry={() => void loadProbe()}
              onRelaunch={() => void onRelaunch()}
              onFirmware={() => void onFirmware()}
            />
          )}
          {step === "Identity" && (
            <IdentityStep
              identity={identity}
              password2={password2}
              onChange={setIdentity}
              onPassword2={setPassword2}
            />
          )}
          {step === "Backup" && <BackupStep />}
          {step === "Confirm" && (
            <ConfirmStep
              probe={probe}
              identity={identity}
              eraseInput={eraseInput}
              secondConfirm={secondConfirm}
              onEraseInput={setEraseInput}
              onSecondConfirm={setSecondConfirm}
            />
          )}
          {step === "Install" && (
            <MutateStep
              native={native}
              identity={identity}
              journal={journal}
              onStatus={(next) => {
                setInstallPhase(next.phase);
                setInstallReady(next.ready);
                if (next.rebooting) setRebooting(true);
              }}
              onJournal={setJournal}
              onClearPassword={() => {
                setIdentity((current) => ({ ...current, password: "" }));
                setPassword2("");
              }}
              onUndo={() => undoStaging()}
            />
          )}
        </div>
      </section>

      <footer className="wizard-foot">
        {step === "Welcome" ? (
          <button type="button" className="btn primary" onClick={goNext}>
            Begin
          </button>
        ) : (
          <>
            {step !== "Install" ? (
              <button
                type="button"
                className="btn ghost"
                onClick={goBack}
                disabled={index === 0}
              >
                Back
              </button>
            ) : (
              <span className="hint">
                {native ? "staging on this machine" : "dry run — host is untouched"}
              </span>
            )}
            {step !== "Install" && (
              <button
                type="button"
                className={`btn ${step === "Confirm" ? "danger" : "primary"}`}
                onClick={goNext}
                disabled={!canContinue}
              >
                {continueLabel}
              </button>
            )}
          </>
        )}
      </footer>

      {abortOpen && (
        <AbortDialog
          copy={abort}
          busy={abortBusy}
          error={abortError}
          onStay={() => {
            if (abortBusy) return;
            setAbortOpen(false);
          }}
          onConfirm={() => void confirmAbort()}
        />
      )}

      <div className="statusline">
        <span className="hl">omarchy-install {version}</span>
        <span>
          {index + 1}/{STEPS.length} {STEP_SLUG[step]}
        </span>
        <span>
          {probe
            ? `${probe.host.os}/${probe.host.arch}`
            : probing
              ? "probing…"
              : "host unknown"}
        </span>
        <span className="push">
          {blocked ? "blocked" : native ? "windows" : "dry run"}
          {" · "}
          esc back · return continue
        </span>
      </div>
    </div>
  );
}

function Welcome() {
  return (
    <div className="greeter-copy">
      <AsciiLogo />
      <p className="tagline">Beautiful, Fun &amp; Opinionated Linux by DHH</p>
      <h2>This installer erases Windows</h2>
      <p>
        Omarchy Install stages a real Linux installer on this disk, reboots into
        the official ISO, and that installer wipes the drive. Not dual-boot. Not
        Wubi. Linux will not ask again.
      </p>
      <p className="strong">Windows will be erased. All data on the selected disk will be gone.</p>
      <ul className="ticks">
        <li>No USB stick required.</li>
        <li>You confirm twice here. After reboot, Omarchy installs unattended.</li>
        <li>If anything fails before that reboot, Windows still boots and you can undo.</li>
      </ul>
    </div>
  );
}

type CheckRow = {
  id: string;
  label: string;
  detail: string;
  state: "ok" | "fail" | "warn";
};

function machineChecks(probe: MachineProbe): CheckRow[] {
  const boot =
    probe.disks.find((d) => d.deviceId === probe.recommendedDiskId) ??
    probe.disks.find((d) => d.isBoot);
  const bitlockerOk =
    probe.bitlocker.length === 0 || probe.bitlocker.every((v) => v.fullyDecrypted);
  const gptOk = (boot?.partitionStyle ?? "").toLowerCase() === "gpt";
  const elevatedOk = probe.host.elevated || !probe.host.nativeWindows;
  const efiOk = probe.efiVarsWritable || !probe.host.nativeWindows;
  const rst = probe.disks.some((d) => d.isRst);
  const dynamic = probe.disks.some((d) => d.isDynamic);
  const spaces = probe.disks.some((d) => d.isStorageSpaces);

  const rows: CheckRow[] = [
    {
      id: "uefi",
      label: "firmware",
      detail: probe.uefi ? "UEFI" : "legacy BIOS",
      state: probe.uefi ? "ok" : "fail",
    },
    {
      id: "gpt",
      label: "table",
      detail: boot?.partitionStyle?.toUpperCase() ?? "—",
      state: gptOk ? "ok" : "fail",
    },
    {
      id: "sb",
      label: "secure boot",
      detail: probe.secureBoot ? "on — turn off in firmware" : "off",
      state: probe.secureBoot ? "fail" : "ok",
    },
    {
      id: "ram",
      label: "ram",
      detail: `${formatBytes(probe.ramInstalledBytes)} installed · ${formatBytes(probe.ramTotalPhysBytes)} usable`,
      state: probe.ramOkForCopytoram ? "ok" : "fail",
    },
    {
      id: "bitlocker",
      label: "bitlocker",
      detail: bitlockerOk ? "fully decrypted" : "still encrypting",
      state: bitlockerOk ? "ok" : "fail",
    },
    {
      id: "admin",
      label: "privileges",
      detail: probe.host.elevated
        ? "administrator"
        : probe.host.nativeWindows
          ? "not elevated"
          : "dev host",
      state: elevatedOk ? "ok" : "fail",
    },
    {
      id: "efi",
      label: "efi vars",
      detail: probe.efiVarsWritable ? "writable" : "locked",
      state: efiOk ? "ok" : "fail",
    },
    {
      id: "tpm",
      label: "tpm",
      detail: probe.tpmPresent ? "present (ok)" : "none",
      state: "ok",
    },
  ];

  if (rst) {
    rows.push({ id: "rst", label: "rst / raid", detail: "intel rst / vmd", state: "fail" });
  }
  if (dynamic) {
    rows.push({ id: "dynamic", label: "dynamic", detail: "dynamic disk", state: "fail" });
  }
  if (spaces) {
    rows.push({ id: "spaces", label: "spaces", detail: "storage spaces", state: "fail" });
  }
  return rows;
}

function flagLabel(state: CheckRow["state"]): string {
  if (state === "ok") return "ok";
  if (state === "warn") return "..";
  return "!!";
}

function ProbeStep({
  probe,
  probing,
  error,
  actionError,
  busy,
  onRetry,
  onRelaunch,
  onFirmware,
}: {
  probe: MachineProbe | null;
  probing: boolean;
  error: string | null;
  actionError: string | null;
  busy: boolean;
  onRetry: () => void;
  onRelaunch: () => void;
  onFirmware: () => void;
}) {
  if (probing && !probe) {
    return (
      <p className="probe-wait">
        <span className="cursor" />
        reading firmware, disks, bitlocker, ram
      </p>
    );
  }
  if (error && !probe) {
    return (
      <div className="copy">
        <p className="kicker">probe</p>
        <p className="banner error">{error}</p>
        <button type="button" className="btn primary" onClick={onRetry}>
          Retry
        </button>
      </div>
    );
  }
  if (!probe) return null;

  const availWarn = probe.ramOkForCopytoram && probe.ramAvailBytes < 10 * 1024 ** 3;
  const boot =
    probe.disks.find((d) => d.deviceId === probe.recommendedDiskId) ??
    probe.disks.find((d) => d.isBoot);
  const checks = machineChecks(probe);

  return (
    <div className="probe">
      <div className="probe-lead">
        <p className="kicker">this machine</p>
        {probe.blockingReasons.length === 0 ? (
          <p className="banner ok">This PC can install Omarchy. Windows will be erased.</p>
        ) : (
          <p className="banner error">This PC is blocked until every item below is fixed.</p>
        )}
      </div>

      <ul className="checks">
        {checks.map((row) => (
          <li key={row.id} className={row.state}>
            <span className="check-flag">{flagLabel(row.state)}</span>
            <span className="check-key">{row.label}</span>
            <span className="check-val">{row.detail}</span>
          </li>
        ))}
      </ul>

      <div className="probe-side">
        {availWarn && (
          <p className="note">
            Windows currently has {formatBytes(probe.ramAvailBytes)} free. That does
            not block install — the RAM copy runs after Windows is gone.
          </p>
        )}
        {boot && <DiskCard disk={boot} linuxById={probe.linuxById} />}
      </div>

      {probe.blockingReasons.map((reason, i) => {
        const action = reasonAction(reason);
        return (
          <article key={`${reason.type}-${i}`} className="blocker">
            <div>
              <h3>{reasonTitle(reason)}</h3>
              <p>{reasonBody(reason)}</p>
            </div>
            {action === "relaunch" && (
              <button
                type="button"
                className="btn primary"
                disabled={busy || !probe.host.nativeWindows}
                onClick={onRelaunch}
              >
                Relaunch as Administrator
              </button>
            )}
            {action === "firmware" && (
              <button
                type="button"
                className="btn primary"
                disabled={busy || !probe.host.nativeWindows}
                onClick={onFirmware}
              >
                Reboot to firmware
              </button>
            )}
          </article>
        );
      })}

      {actionError && <p className="banner error">{actionError}</p>}

    </div>
  );
}

function segClass(p: PartitionMap): string {
  const kind = partitionKind(p.typeGuid);
  if (kind === "EFI system") return "esp";
  if (kind === "MSR") return "msr";
  if (kind === "Windows") return "win";
  if (kind === "Recovery") return "rec";
  return "other";
}

function DiskCard({ disk, linuxById }: { disk: DiskMap; linuxById: string | null }) {
  const total = Math.max(disk.sizeBytes, 1);
  return (
    <div className="disk">
      <header>
        <h3>Windows disk</h3>
        <span className="chip ok">boot</span>
      </header>
      <p className="muted mono">{disk.deviceId}</p>
      {linuxById && <p className="muted mono">{linuxById}</p>}
      <p>
        {formatBytes(disk.sizeBytes)} · {disk.partitionStyle.toUpperCase()}
        {disk.bus ? ` · ${disk.bus}` : ""}
      </p>
      <p className="muted">
        {disk.maxShrinkBytes != null
          ? `shrinkable ${formatBytes(disk.maxShrinkBytes)}`
          : "shrinkable unknown"}
      </p>
      <div className="disk-bar" aria-hidden="true">
        {disk.partitions.map((p, i) => (
          <div
            key={p.gptGuid ?? String(i)}
            className={`seg ${segClass(p)}`}
            style={{ flexGrow: Math.max(p.sizeBytes / total, 0.015) }}
            title={`${p.letter ?? partitionKind(p.typeGuid) ?? p.label ?? "partition"} ${formatBytes(p.sizeBytes)}`}
          />
        ))}
        <div
          className="seg planned"
          style={{ flexGrow: Math.max(INSTALLER_HOLE_BYTES / total, 0.04) }}
          title={`OMARCHYINST + cidata ${formatBytes(INSTALLER_HOLE_BYTES)}`}
        />
      </div>
      <ol className="parts">
        {disk.partitions.map((p, i) => (
          <li key={p.gptGuid ?? String(i)}>
            <span>{p.letter ?? partitionKind(p.typeGuid) ?? p.label ?? "partition"}</span>
            <span className="muted">
              {formatBytes(p.sizeBytes)}
              {p.fs ? ` ${p.fs}` : ""}
            </span>
          </li>
        ))}
        <li className="planned">
          <span>OMARCHYINST + cidata (planned)</span>
          <span>{formatBytes(INSTALLER_HOLE_BYTES)}</span>
        </li>
      </ol>
    </div>
  );
}

function IdentityStep({
  identity,
  password2,
  onChange,
  onPassword2,
}: {
  identity: CidataIdentity;
  password2: string;
  onChange: (next: CidataIdentity) => void;
  onPassword2: (v: string) => void;
}) {
  const patch = (partial: Partial<CidataIdentity>) => onChange({ ...identity, ...partial });
  const userErr = identity.username.length > 0 ? usernameError(identity.username) : null;
  const hostErr = identity.hostname.length > 0 ? hostnameError(identity.hostname) : null;
  const passErr =
    identity.password.length > 0 || password2.length > 0
      ? passwordError(identity.password, password2)
      : null;
  const kb = KEYBOARDS.find((k) => k.id === identity.keyboard)?.label ?? identity.keyboard;

  return (
    <form className="form" onSubmit={(e) => e.preventDefault()}>
      <p className="kicker">identity</p>
      <p>
        These become the Omarchy user. After reboot the official installer uses
        them automatically — Linux will not ask again.
      </p>
      {identity.username && (
        <p className="identity-preview">
          {identity.username}@{identity.hostname}
          {" · "}
          {kb}
          {" · "}
          {identity.encrypt ? "luks on" : "unencrypted"}
        </p>
      )}
      <label className="form-row">
        <span>keyboard</span>
        <select
          value={identity.keyboard}
          onChange={(e) => patch({ keyboard: e.target.value })}
        >
          {KEYBOARDS.map((k) => (
            <option key={k.id} value={k.id}>
              {k.label}
            </option>
          ))}
        </select>
      </label>
      <label className="form-row">
        <span>username</span>
        <input
          autoComplete="username"
          spellCheck={false}
          value={identity.username}
          onChange={(e) => patch({ username: e.target.value })}
        />
      </label>
      {userErr && <p className="field-error">{userErr}</p>}
      <label className="form-row">
        <span>password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={identity.password}
          onChange={(e) => patch({ password: e.target.value })}
        />
      </label>
      <label className="form-row">
        <span>confirm</span>
        <input
          type="password"
          autoComplete="new-password"
          value={password2}
          onChange={(e) => onPassword2(e.target.value)}
        />
      </label>
      {passErr && <p className="field-error">{passErr}</p>}
      <label className="form-row">
        <span>hostname</span>
        <input
          spellCheck={false}
          value={identity.hostname}
          onChange={(e) => patch({ hostname: e.target.value })}
        />
      </label>
      {hostErr && <p className="field-error">{hostErr}</p>}
      <label className="form-row">
        <span>timezone</span>
        <input
          spellCheck={false}
          value={identity.timezone}
          onChange={(e) => patch({ timezone: e.target.value })}
        />
      </label>
      <label className="form-row check">
        <span>encrypt</span>
        <span className="check">
          <input
            type="checkbox"
            checked={identity.encrypt}
            onChange={(e) => patch({ encrypt: e.target.checked })}
          />
          Encrypt the disk (recommended)
        </span>
      </label>
      <details className="optional">
        <summary>optional git identity</summary>
        <label className="form-row">
          <span>name</span>
          <input
            value={identity.fullName ?? ""}
            onChange={(e) => patch({ fullName: e.target.value ? e.target.value : null })}
          />
        </label>
        <label className="form-row">
          <span>email</span>
          <input
            value={identity.email ?? ""}
            onChange={(e) => patch({ email: e.target.value ? e.target.value : null })}
          />
        </label>
      </details>
    </form>
  );
}

function BackupStep() {
  return (
    <div className="copy">
      <p className="kicker">last chance</p>
      <h2>This app does not back anything up</h2>
      <p>
        If there is anything on this Windows install you still want, copy it off
        now. Photos, browser profiles, BitLocker recovery keys, game saves — none
        of that comes along.
      </p>
      <p className="banner">
        Windows will be erased. There is no undo after reboot.
      </p>
    </div>
  );
}

function ConfirmStep({
  probe,
  identity,
  eraseInput,
  secondConfirm,
  onEraseInput,
  onSecondConfirm,
}: {
  probe: MachineProbe | null;
  identity: CidataIdentity;
  eraseInput: string;
  secondConfirm: boolean;
  onEraseInput: (v: string) => void;
  onSecondConfirm: (v: boolean) => void;
}) {
  const disk =
    probe?.disks.find((d) => d.deviceId === probe.recommendedDiskId) ??
    probe?.disks.find((d) => d.isBoot);
  const matched = eraseInput.trim() === ERASE_PHRASE;
  const kb = KEYBOARDS.find((k) => k.id === identity.keyboard)?.label ?? identity.keyboard;

  return (
    <div className="copy">
      <p className="kicker">confirm</p>
      <h2>Type {ERASE_PHRASE} to continue</h2>
      <p>
        After reboot, Omarchy will wipe this entire disk automatically. Linux
        will not ask again.
      </p>
      <dl className="summary-grid">
        <dt>user</dt>
        <dd>
          {identity.username || "—"}@{identity.hostname}
        </dd>
        <dt>disk</dt>
        <dd className="mono">
          {disk ? `${disk.deviceId} · ${formatBytes(disk.sizeBytes)}` : "—"}
        </dd>
        <dt>luks</dt>
        <dd>{identity.encrypt ? "on" : "off"}</dd>
        <dt>layout</dt>
        <dd>{kb}</dd>
      </dl>
      {disk && <DiskCard disk={disk} linuxById={probe?.linuxById ?? null} />}
      <label className="prompt-line">
        <span className="ps">&gt;</span>
        <input
          className={`confirm-input ${matched ? "match" : ""}`}
          value={eraseInput}
          onChange={(e) => {
            onEraseInput(e.target.value);
            onSecondConfirm(false);
          }}
          placeholder={ERASE_PHRASE}
          autoComplete="off"
          spellCheck={false}
        />
      </label>
      {matched && (
        <label className="check">
          <input
            type="checkbox"
            checked={secondConfirm}
            onChange={(e) => onSecondConfirm(e.target.checked)}
          />
          I understand Windows will be erased and Linux will not ask again.
        </label>
      )}
    </div>
  );
}

function phaseState(
  id: string,
  phase: string,
  ready: boolean,
  windowsOnly: boolean,
  failed: boolean,
): "ok" | "run" | "fail" | "skip" | "todo" {
  const order = INSTALL_PHASES.map((p) => p.id) as readonly string[];
  const current = order.indexOf(phase === "error" ? "download" : phase);
  const mine = order.indexOf(id);
  if (windowsOnly) {
    return mine >= order.indexOf("prepare") ? "skip" : "ok";
  }
  if (ready) return "ok";
  if (failed && mine === current) return "fail";
  if (mine < current) return "ok";
  if (mine === current) return failed ? "fail" : "run";
  return "todo";
}

function MutateStep({
  native,
  identity,
  journal,
  onStatus,
  onJournal,
  onClearPassword,
  onUndo,
}: {
  native: boolean;
  identity: CidataIdentity;
  journal: StateJournal | null;
  onStatus: (next: { phase: string; ready: boolean; rebooting: boolean }) => void;
  onJournal: (next: StateJournal | null) => void;
  onClearPassword: () => void;
  onUndo: () => Promise<void>;
}) {
  const [phase, setPhase] = useState<string>("working");
  const [bytes, setBytes] = useState(0);
  const [total, setTotal] = useState<number | null>(null);
  const [sha, setSha] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [windowsOnly, setWindowsOnly] = useState(false);
  const [ready, setReady] = useState(false);
  const [tick, setTick] = useState(0);
  const [rebooting, setRebooting] = useState(false);
  const [busyAction, setBusyAction] = useState(false);
  const [bundlePath, setBundlePath] = useState<string | null>(null);
  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;
  const identityRef = useRef(identity);
  identityRef.current = identity;
  const onJournalRef = useRef(onJournal);
  onJournalRef.current = onJournal;
  const onClearPasswordRef = useRef(onClearPassword);
  onClearPasswordRef.current = onClearPassword;

  useEffect(() => {
    onStatusRef.current({ phase, ready, rebooting });
  }, [phase, ready, rebooting]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      unlisten = await listen<IsoProgress>("iso://progress", (event) => {
        if (cancelled) return;
        setPhase(event.payload.phase);
        setBytes(event.payload.bytes);
        setTotal(event.payload.total);
      });
      try {
        setError(null);
        setWindowsOnly(false);
        setReady(false);
        setBundlePath(null);
        const current =
          (await invoke<StateJournal | null>("load_install_state").catch(() => null)) ?? journal;
        if (cancelled) return;
        if (current) onJournalRef.current(current);
        const start = installStartFromJournal(current?.step);
        if (start === "done") {
          setPhase("done");
          setReady(true);
          return;
        }
        if (start === "cidata" && identityRef.current.password.length < 1) {
          setError("Re-enter the user password on the identity step, then retry.");
          setPhase("cidata");
          return;
        }
        if (start === "download") {
          setPhase("download");
          await invoke("download_iso");
          if (cancelled) return;
          setPhase("hash");
          const result = await invoke<VerifyResult>("verify_iso");
          if (cancelled) return;
          setSha(result.sha256);
          setBytes(result.bytes);
          setTotal(result.bytes);
          setPhase("prepare");
          await invoke("prepare_installer_partition");
          if (cancelled) return;
        }
        if (start === "download" || start === "stage") {
          setPhase("stage");
          await invoke("stage_bootloader");
          if (cancelled) return;
        }
        if (start === "download" || start === "stage" || start === "cidata") {
          setPhase("cidata");
          await invoke("write_cidata", { identity: identityRef.current });
          if (cancelled) return;
        }
        setPhase("bootnext");
        await invoke("set_boot_next");
        if (cancelled) return;
        const next = await invoke<StateJournal | null>("load_install_state").catch(() => null);
        if (next) onJournalRef.current(next);
        onClearPasswordRef.current();
        setPhase("done");
        setReady(true);
      } catch (err: unknown) {
        if (cancelled) return;
        const message = invokeError(err);
        if (message.toLowerCase().includes("only available on windows")) {
          setWindowsOnly(true);
          setPhase("prepare");
        } else {
          setError(message);
        }
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
    // journal is read at start via IPC; retry is `tick`
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick]);

  const pct =
    total && total > 0 ? Math.min(100, Math.round((bytes / total) * 100)) : null;
  const label =
    phase === "download"
      ? "Downloading official ISO"
      : phase === "hash"
        ? "Checking sha256"
        : phase === "signature"
          ? "Checking GPG signature"
          : phase === "prepare"
            ? "Creating installer partitions"
            : phase === "stage"
              ? "Planting GRUB on the ESP"
              : phase === "cidata"
                ? "Writing autoinstall cidata"
                : phase === "bootnext"
                  ? "Setting one-shot BootNext"
                  : phase === "done" && ready
                    ? "Ready to reboot into Omarchy"
                    : windowsOnly
                      ? "ISO verified — disk staging is Windows-only"
                      : error
                        ? "Staging failed"
                        : "Starting…";

  const working =
    !error &&
    !ready &&
    !windowsOnly &&
    phase !== "idle" &&
    phase !== "stopped";

  return (
    <div className="copy">
      <p className="kicker">install</p>
      <h2>{label}</h2>
      <ul className="log">
        {INSTALL_PHASES.map((item) => {
          const state = phaseState(item.id, phase, ready, windowsOnly, !!error);
          return (
            <li key={item.id} className={state}>
              <span>{state === "todo" ? "··" : state}</span>
              <span>{item.label}</span>
              {item.id === "download" && working && phase === "download" && (
                <span>
                  {formatBytes(bytes)}
                  {total != null ? ` / ${formatBytes(total)}` : ""}
                  {pct != null ? `  ${pct}%` : ""}
                </span>
              )}
            </li>
          );
        })}
      </ul>
      {working && (
        <div
          className={`progress ${pct == null ? "indeterminate" : ""}`}
          role="progressbar"
          aria-valuenow={pct ?? undefined}
        >
          <span style={{ width: pct != null ? `${pct}%` : "30%" }} />
        </div>
      )}
      {sha && <p className="mono">{sha}</p>}
      {windowsOnly && (
        <p className="banner">
          prepare_installer_partition, stage_bootloader, write_cidata, and set_boot_next
          are Windows-only. This host ran those commands; they returned “Windows only”.
          Windows has not been erased.
        </p>
      )}
      {ready && (
        <>
          <p className="banner ok">
            BootNext is set. After reboot, Omarchy will wipe this disk automatically.
          </p>
          <p className="strong">Last chance to undo from this app.</p>
          <div className="actions">
            <button
              type="button"
              className="btn danger"
              disabled={busyAction}
              onClick={() => {
                setBusyAction(true);
                setRebooting(true);
                void invoke("reboot_to_installer")
                  .catch((err: unknown) => {
                    setRebooting(false);
                    setError(invokeError(err));
                  })
                  .finally(() => setBusyAction(false));
              }}
            >
              Reboot into Omarchy installer
            </button>
            <button
              type="button"
              className="btn ghost"
              disabled={busyAction}
              onClick={() => {
                setBusyAction(true);
                void onUndo()
                  .then(() => {
                    setReady(false);
                    setPhase("stopped");
                  })
                  .catch((err: unknown) => setError(invokeError(err)))
                  .finally(() => setBusyAction(false));
              }}
            >
              Undo staging
            </button>
          </div>
        </>
      )}
      {error && (
        <>
          <p className="banner error">{error}</p>
          {bundlePath && <p className="mono">{bundlePath}</p>}
          <div className="actions">
            <button type="button" className="btn primary" onClick={() => setTick((n) => n + 1)}>
              Retry
            </button>
            <button
              type="button"
              className="btn ghost"
              disabled={busyAction}
              onClick={() => {
                setBusyAction(true);
                void onUndo()
                  .then(() => {
                    setError(null);
                    setReady(false);
                    setPhase("stopped");
                  })
                  .catch((err: unknown) => setError(invokeError(err)))
                  .finally(() => setBusyAction(false));
              }}
            >
              Undo staging
            </button>
            <button
              type="button"
              className="btn ghost"
              disabled={busyAction}
              onClick={() => {
                setBusyAction(true);
                void (async () => {
                  const path = await invoke<string>("export_support_bundle");
                  setBundlePath(path);
                  try {
                    await revealItemInDir(path);
                  } catch {
                    /* path is shown even if reveal is unavailable */
                  }
                })()
                  .catch((err: unknown) => setError(invokeError(err)))
                  .finally(() => setBusyAction(false));
              }}
            >
              Export support bundle
            </button>
          </div>
        </>
      )}
      {phase === "stopped" && !error && (
        <>
          <p className="banner">Staging was undone. Retry starts over from the ISO.</p>
          <div className="actions">
            <button type="button" className="btn primary" onClick={() => setTick((n) => n + 1)}>
              Retry
            </button>
          </div>
        </>
      )}
      {!native && !error && phase !== "stopped" && (
        <p className="note">
          Dry run: this host is untouched. Mutate IPC writes a stub journal under OmarchyInstall
          only.
        </p>
      )}
    </div>
  );
}
