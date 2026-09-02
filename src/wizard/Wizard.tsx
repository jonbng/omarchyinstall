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
  fullNameError,
  emailError,
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
import {
  AbortButton,
  AbortDialog,
  closeWindow,
  startTitlebarDrag,
} from "./TitleControls";
import { KEYBOARDS, TIMEZONES, TIMEZONE_OPTIONS } from "./options";
import { SearchPicker } from "./SearchPicker";
import {
  isoAcquisitionRunning,
  type IsoAcquisitionState,
  type IsoSource,
  useIsoAcquisition,
} from "./useIsoAcquisition";
import "./wizard.css";

const STEPS = ["Welcome", "Machine", "Setup", "Review", "Install"] as const;
type Step = (typeof STEPS)[number];
type SetupQuestion =
  | "keyboard"
  | "username"
  | "fullName"
  | "email"
  | "password"
  | "hostname"
  | "timezone"
  | "encrypt";

const SETUP_QUESTIONS: SetupQuestion[] = [
  "keyboard",
  "username",
  "fullName",
  "email",
  "password",
  "hostname",
  "timezone",
  "encrypt",
];

const GIB = 1024 ** 3;
const RAM_RECOMMENDED = 14 * GIB;

const STEP_SLUG: Record<Step, string> = {
  Welcome: "welcome",
  Machine: "machine",
  Setup: "configure",
  Review: "review",
  Install: "install",
};

const INSTALL_PHASES = [
  { id: "verify", label: "recheck installation media" },
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
  const [keyboardLabel, setKeyboardLabel] = useState("English (US)");
  const [setupQuestion, setSetupQuestion] = useState(0);
  const [eraseInput, setEraseInput] = useState("");
  const [secondConfirm, setSecondConfirm] = useState(false);
  const [bitlockerRiskAccepted, setBitlockerRiskAccepted] = useState(false);
  const iso = useIsoAcquisition();
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [journal, setJournal] = useState<StateJournal | null>(null);
  const [installPhase, setInstallPhase] = useState("idle");
  const [installReady, setInstallReady] = useState(false);
  const [rebooting, setRebooting] = useState(false);
  const [abortOpen, setAbortOpen] = useState(false);
  const [abortBusy, setAbortBusy] = useState(false);
  const [abortError, setAbortError] = useState<string | null>(null);
  const [version, setVersion] = useState("0.4.1");
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
  const bitlockerActive =
    probe?.bitlocker.some((volume) => !volume.fullyDecrypted) ?? false;
  const native = probe?.host.nativeWindows ?? false;
  const identityOk = identityFieldsOk(
    identity.username,
    identity.hostname,
    identity.password,
    password2,
  );
  const configurationOk =
    identityOk &&
    fullNameError(identity.fullName) == null &&
    emailError(identity.email) == null &&
    KEYBOARDS.some((keyboard) => keyboard.id === identity.keyboard) &&
    KEYBOARDS.some((keyboard) => keyboard.id === identity.keyboard && keyboard.label === keyboardLabel) &&
    TIMEZONES.includes(identity.timezone);
  const eraseOk = eraseInput.trim() === ERASE_PHRASE;
  const isoReady = iso.state.phase === "ready" && iso.state.result != null;

  const index = STEPS.indexOf(step);
  const currentQuestion = SETUP_QUESTIONS[setupQuestion];
  const setupQuestionOk = useMemo(() => {
    switch (currentQuestion) {
      case "keyboard":
        return KEYBOARDS.some((keyboard) => keyboard.id === identity.keyboard);
      case "username":
        return !usernameError(identity.username);
      case "fullName":
        return fullNameError(identity.fullName) == null;
      case "email":
        return emailError(identity.email) == null;
      case "password":
        return !passwordError(identity.password, password2) && identity.password === password2;
      case "hostname":
        return !hostnameError(identity.hostname);
      case "timezone":
        return TIMEZONES.includes(identity.timezone);
      case "encrypt":
        return true;
    }
  }, [currentQuestion, identity, password2]);
  const canContinue = useMemo(() => {
    switch (step) {
      case "Welcome":
        return iso.state.phase !== "selecting";
      case "Machine":
        return !!probe && !blocked && !probing && (!bitlockerActive || bitlockerRiskAccepted);
      case "Setup":
        return setupQuestionOk;
      case "Review":
        return isoReady && configurationOk && eraseOk && secondConfirm;
      case "Install":
        return false;
    }
  }, [step, probe, blocked, probing, bitlockerActive, bitlockerRiskAccepted, setupQuestionOk, iso.state.phase, isoReady, configurationOk, eraseOk, secondConfirm]);

  function goTo(name: Step) {
    const next = STEPS.indexOf(name);
    if (next < 0 || next >= index) return;
    setSecondConfirm(false);
    if (name === "Setup") setSetupQuestion(0);
    setStep(name);
  }

  function goNext() {
    if (!canContinue) return;
    if (step === "Welcome") {
      if (!isoReady && !iso.running) void iso.start();
      setStep("Machine");
      return;
    }
    if (step === "Setup" && setupQuestion < SETUP_QUESTIONS.length - 1) {
      setSetupQuestion((current) => current + 1);
      return;
    }
    const next = STEPS[index + 1];
    if (next) setStep(next);
  }

  function goBack() {
    if (step === "Setup" && setupQuestion > 0) {
      setSetupQuestion((current) => current - 1);
      return;
    }
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
  const [canScroll, setCanScroll] = useState(false);
  useEffect(() => {
    if (step !== "Setup" && step !== "Review") return;
    const root = stageRef.current;
    const field = root?.querySelector<HTMLInputElement | HTMLSelectElement>(
      "input:not([type=checkbox]), select",
    );
    field?.focus();
  }, [step, setupQuestion]);

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    stage.scrollTop = 0;
    const update = () => {
      setCanScroll(stage.scrollHeight - stage.scrollTop - stage.clientHeight > 8);
    };
    const observer = new ResizeObserver(update);
    observer.observe(stage);
    const body = stage.querySelector(".stage-body");
    if (body) observer.observe(body);
    const mutations = new MutationObserver(update);
    mutations.observe(stage, { childList: true, subtree: true, attributes: true, characterData: true });
    stage.addEventListener("scroll", update, { passive: true });
    const frame = requestAnimationFrame(update);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      mutations.disconnect();
      stage.removeEventListener("scroll", update);
    };
  }, [step, setupQuestion]);

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
    step === "Welcome"
      ? iso.running || isoReady
        ? "Continue setup"
        : iso.state.phase === "error"
          ? "Retry & continue"
          : "Begin installation"
      : step === "Review"
        ? isoReady ? "Erase Windows & prepare Omarchy" : "Waiting for verification…"
        : step === "Setup" && setupQuestion === SETUP_QUESTIONS.length - 1
          ? "Review installation"
          : "Next";

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

  function clearDestructiveConfirmation() {
    setEraseInput("");
    setSecondConfirm(false);
  }

  async function chooseLocalIso() {
    if (await iso.chooseLocal()) clearDestructiveConfirmation();
  }

  function chooseOfficialIso() {
    if (iso.useOfficial()) clearDestructiveConfirmation();
  }

  function handleMediaInvalid(error: string) {
    clearDestructiveConfirmation();
    iso.invalidate(error);
    setStep("Review");
  }

  const showIsoStatus =
    step !== "Welcome" &&
    step !== "Install" &&
    iso.state.phase !== "idle" &&
    iso.state.phase !== "selecting";

  return (
    <div className={`wizard ${showIsoStatus ? "has-media-status" : ""}`} data-step={step}>
      {runtimeMode() === "browser" && (
        <div className={`browser-fallback ${bridgeStatus}`} role="status">
          {bridgeStatus === "connected"
            ? "WebView2 is unavailable, so Omarchy Install is running securely in your browser. Keep this tab open."
            : "The local installer backend disconnected. Reopen OmarchyInstaller.exe to resume safely."}
        </div>
      )}
      <header
        className="waybar"
        data-tauri-drag-region
        onMouseDown={(event) => {
          if (event.button !== 0) return;
          const target = event.target as HTMLElement;
          if (target.closest("button, input, a, select, summary, [data-no-drag]")) return;
          event.preventDefault();
          void startTitlebarDrag().catch(() => undefined);
        }}
      >
        <div className="waybar-left" data-tauri-drag-region>
          <Mark />
          <span className="product">omarchy-install</span>
        </div>
        <ol className="crumbs" aria-label="Install progress" data-tauri-drag-region>
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
                  <span className="step-dot">{i < index ? "✓" : i + 1}</span>
                  <span>{STEP_SLUG[name]}</span>
                </button>
              </li>
            );
          })}
        </ol>
        <div className="waybar-right">
          <AbortButton
            onClick={() => {
              setAbortError(null);
              setAbortOpen(true);
            }}
          />
        </div>
      </header>

      {showIsoStatus && (
        <IsoStatusStrip state={iso.state} onRetry={() => void iso.start()} />
      )}

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
          {step === "Welcome" && (
            <Welcome
              media={iso.state}
              mediaRunning={iso.running}
              onChooseLocal={() => void chooseLocalIso()}
              onUseOfficial={chooseOfficialIso}
              onRetry={() => void iso.start()}
            />
          )}
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
              bitlockerRiskAccepted={bitlockerRiskAccepted}
              onBitlockerRiskAccepted={setBitlockerRiskAccepted}
            />
          )}
          {step === "Setup" && (
            <SetupStep
              identity={identity}
              password2={password2}
              keyboardLabel={keyboardLabel}
              question={setupQuestion}
              onChange={setIdentity}
              onPassword2={setPassword2}
              onKeyboardLabel={setKeyboardLabel}
            />
          )}
          {step === "Review" && !isoReady && (
            <MediaWaitStep state={iso.state} onRetry={() => void iso.start()} />
          )}
          {step === "Review" && isoReady && (
            <ConfirmStep
              probe={probe}
              identity={identity}
              keyboardLabel={keyboardLabel}
              eraseInput={eraseInput}
              secondConfirm={secondConfirm}
              onEraseInput={setEraseInput}
              onSecondConfirm={setSecondConfirm}
              media={iso.state}
              bitlockerActive={bitlockerActive}
            />
          )}
          {step === "Install" && (
            <MutateStep
              native={native}
              identity={identity}
              journal={journal}
              expectedIso={iso.state.result}
              allowBitlocker={bitlockerActive && bitlockerRiskAccepted}
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
              onMediaInvalid={handleMediaInvalid}
            />
          )}
        </div>
      </section>

      {canScroll && (
        <button
          type="button"
          className="scroll-hint"
          onClick={() => stageRef.current?.scrollBy({ top: 240, behavior: "smooth" })}
          aria-label="Scroll down for more"
        >
          <span>scroll</span>
          <span aria-hidden="true">↓</span>
        </button>
      )}

      <footer className="wizard-foot">
        {step !== "Welcome" && step !== "Install" ? (
          <button type="button" className="btn ghost" onClick={goBack}>
            Back
          </button>
        ) : (
          <span className="hint">
            {step === "Install"
              ? native ? "Preparing this PC" : "Dry run — no disk changes"
              : "No USB drive needed"}
          </span>
        )}
        {step !== "Install" && !(step === "Review" && !isoReady) && (
          <button
            type="button"
            className={`btn ${step === "Review" ? "danger" : "primary"}`}
            onClick={goNext}
            disabled={!canContinue}
          >
            {continueLabel}
          </button>
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

      <div className="version">v{version}</div>
    </div>
  );
}

function isoSourceName(source: IsoSource): string {
  if (source.kind === "official") return "Latest official Omarchy ISO";
  return source.filename ?? source.path.split(/[\\/]/).pop() ?? source.path;
}

function isoStatusLabel(state: IsoAcquisitionState): string {
  switch (state.phase) {
    case "idle":
      return state.source.kind === "official" ? "Ready to download" : "Ready to verify";
    case "selecting":
      return "Selecting an ISO…";
    case "preparing-local":
      return "Preparing selected ISO";
    case "download":
      return "Downloading official Omarchy ISO";
    case "hash":
      return "Checking the downloaded file";
    case "signature":
      return "Checking the publisher signature";
    case "ready":
      return "Installation media verified";
    case "error":
      return "Installation media needs attention";
  }
}

function IsoProgressBar({ state }: { state: IsoAcquisitionState }) {
  const running = isoAcquisitionRunning(state.phase);
  if (!running) return null;
  const pct = state.total && state.total > 0
    ? Math.min(100, Math.round((state.bytes / state.total) * 100))
    : null;
  return (
    <div
      className={`media-progress ${pct == null ? "indeterminate" : ""}`}
      role="progressbar"
      aria-label={isoStatusLabel(state)}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={pct ?? undefined}
    >
      <span style={{ width: pct == null ? "30%" : `${pct}%` }} />
    </div>
  );
}

function IsoStatusStrip({
  state,
  onRetry,
}: {
  state: IsoAcquisitionState;
  onRetry: () => void;
}) {
  const pct = state.total && state.total > 0
    ? Math.min(100, Math.round((state.bytes / state.total) * 100))
    : null;
  return (
    <aside className={`media-strip ${state.phase}`} aria-live="polite">
      <div className="media-strip-inner">
        <span className="media-state-mark" aria-hidden="true">
          {state.phase === "ready" ? "✓" : state.phase === "error" ? "!" : "↓"}
        </span>
        <div className="media-strip-copy">
          <strong>{isoStatusLabel(state)}</strong>
          <span>
            {isoSourceName(state.source)}
            {isoAcquisitionRunning(state.phase) && state.total != null
              ? ` · ${formatBytes(state.bytes)} of ${formatBytes(state.total)}${pct != null ? ` · ${pct}%` : ""}`
              : state.phase === "ready" && state.result
                ? ` · ${formatBytes(state.result.bytes)}`
                : ""}
          </span>
          <IsoProgressBar state={state} />
        </div>
        {state.phase === "error" && (
          <button type="button" className="btn ghost compact" onClick={onRetry}>
            Retry
          </button>
        )}
      </div>
    </aside>
  );
}

function Welcome({
  media,
  mediaRunning,
  onChooseLocal,
  onUseOfficial,
  onRetry,
}: {
  media: IsoAcquisitionState;
  mediaRunning: boolean;
  onChooseLocal: () => void;
  onUseOfficial: () => void;
  onRetry: () => void;
}) {
  return (
    <div className="greeter-copy">
      <AsciiLogo />
      <h1 className="hero-title">Install Omarchy. No USB required.</h1>
      <p className="hero-copy">Answer a few questions, reboot, and the official installer handles the rest.</p>
      <div className="journey" aria-label="What happens next">
        <div><span>1</span><strong>Check</strong><small>We verify this PC</small></div>
        <div><span>2</span><strong>Configure</strong><small>Account, keyboard and timezone</small></div>
        <div><span>3</span><strong>Install</strong><small>Reboot and finish automatically</small></div>
      </div>
      <div className={`welcome-media ${media.phase}`}>
        <div className="welcome-media-copy">
          <span>Installation media</span>
          <strong className={media.source.kind === "local" ? "mono" : undefined}>
            {isoSourceName(media.source)}
          </strong>
          <small>
            {media.phase === "idle"
              ? media.source.kind === "official"
                ? "The roughly 6 GiB download starts when you begin."
                : "This file will be checked when you begin."
              : isoStatusLabel(media)}
          </small>
          {media.error && <small className="media-error">{media.error}</small>}
          <IsoProgressBar state={media} />
        </div>
        <div className="welcome-media-actions">
          {media.source.kind === "local" ? (
            <button type="button" className="btn ghost compact" disabled={mediaRunning} onClick={onUseOfficial}>
              Use latest
            </button>
          ) : null}
          <button
            type="button"
            className="btn ghost compact"
            disabled={mediaRunning || media.phase === "selecting"}
            onClick={onChooseLocal}
          >
            {media.phase === "selecting" ? "Selecting…" : "Choose local ISO"}
          </button>
          {media.phase === "error" && (
            <button type="button" className="btn primary compact" onClick={onRetry}>
              Retry
            </button>
          )}
        </div>
      </div>
      <p className="erase-note"><strong>Windows and everything on its disk will be erased.</strong></p>
    </div>
  );
}

function MediaWaitStep({
  state,
  onRetry,
}: {
  state: IsoAcquisitionState;
  onRetry: () => void;
}) {
  const failed = state.phase === "error";
  return (
    <div className="media-wait">
      <p className="kicker">installation media</p>
      <h1>{failed ? "The ISO is not ready yet" : "Finishing the installation media"}</h1>
      <p className="review-lead">
        {failed
          ? "Fix or retry the media step before reviewing the disk erase. Your Windows installation has not been changed."
          : "Your setup is saved. The erase confirmation will appear after the ISO has downloaded and passed both integrity checks."}
      </p>
      <div className={`media-wait-card ${failed ? "error" : ""}`}>
        <strong>{isoStatusLabel(state)}</strong>
        <span className="mono">{isoSourceName(state.source)}</span>
        {state.error && <p>{state.error}</p>}
        <IsoProgressBar state={state} />
      </div>
      {failed || state.phase === "idle" ? (
        <div className="actions">
          <button type="button" className="btn primary" onClick={onRetry}>
            {failed ? "Retry" : "Start download"}
          </button>
        </div>
      ) : null}
    </div>
  );
}

type CheckRow = {
  id: string;
  label: string;
  detail: string;
  state: "ok" | "fail" | "warn";
};

function bitlockerOk(probe: MachineProbe): boolean {
  return probe.bitlocker.length === 0 || probe.bitlocker.every((volume) => volume.fullyDecrypted);
}

function machineChecks(probe: MachineProbe): CheckRow[] {
  const boot =
    probe.disks.find((d) => d.deviceId === probe.recommendedDiskId) ??
    probe.disks.find((d) => d.isBoot);
  const isBitlockerOk = bitlockerOk(probe);
  const gptOk = (boot?.partitionStyle ?? "").toLowerCase() === "gpt";
  const elevatedOk = probe.host.elevated || !probe.host.nativeWindows;
  const efiOk = probe.efiVarsWritable || !probe.host.nativeWindows;
  const rst = probe.disks.some((d) => d.isRst);
  const dynamic = probe.disks.some((d) => d.isDynamic);
  const spaces = probe.disks.some((d) => d.isStorageSpaces);
  const lowMemory =
    probe.ramOkForCopytoram &&
    (probe.ramInstalledBytes < RAM_RECOMMENDED || probe.ramTotalPhysBytes < RAM_RECOMMENDED);

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
      detail: `${formatBytes(probe.ramInstalledBytes)} installed · ${formatBytes(probe.ramTotalPhysBytes)} usable${lowMemory ? " · 14 GiB recommended" : ""}`,
      state: probe.ramOkForCopytoram ? (lowMemory ? "warn" : "ok") : "fail",
    },
    {
      id: "bitlocker",
      label: "bitlocker",
      detail: isBitlockerOk ? "fully decrypted" : "enabled — turn off recommended",
      state: isBitlockerOk ? "ok" : "warn",
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
  bitlockerRiskAccepted,
  onBitlockerRiskAccepted,
}: {
  probe: MachineProbe | null;
  probing: boolean;
  error: string | null;
  actionError: string | null;
  busy: boolean;
  onRetry: () => void;
  onRelaunch: () => void;
  onFirmware: () => void;
  bitlockerRiskAccepted: boolean;
  onBitlockerRiskAccepted: (accepted: boolean) => void;
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

  const availWarn = probe.ramOkForCopytoram && probe.ramAvailBytes < 10 * GIB;
  const lowMemoryWarn =
    probe.ramOkForCopytoram &&
    (probe.ramInstalledBytes < RAM_RECOMMENDED || probe.ramTotalPhysBytes < RAM_RECOMMENDED);
  const boot =
    probe.disks.find((d) => d.deviceId === probe.recommendedDiskId) ??
    probe.disks.find((d) => d.isBoot);
  const checks = machineChecks(probe);
  const ready = probe.blockingReasons.length === 0;

  return (
    <div className="probe">
      <div className={`system-result ${ready ? "ready" : "blocked"}`}>
        <span className="result-icon">{ready ? "✓" : "!"}</span>
        <div>
          <p className="kicker">System check</p>
          <h1>{ready ? "This PC is ready." : "This PC needs attention."}</h1>
          <p>{ready ? "Everything needed to install Omarchy looks good." : "Fix the items below, then run the check again."}</p>
        </div>
      </div>

      <div className="system-overview">
        <div><span>Disk</span><strong>{boot ? formatBytes(boot.sizeBytes) : "Unknown"}</strong><small>{boot?.bus ?? "Windows drive"}</small></div>
        <div><span>Memory</span><strong>{formatBytes(probe.ramInstalledBytes)}</strong><small>{lowMemoryWarn ? "14 GiB recommended" : "Ready"}</small></div>
        <div><span>Firmware</span><strong>{probe.uefi ? "UEFI" : "Legacy BIOS"}</strong><small>{probe.secureBoot ? "Secure Boot on" : "Secure Boot off"}</small></div>
        <div><span>BitLocker</span><strong>{bitlockerOk(probe) ? "Off" : "On"}</strong><small>{bitlockerOk(probe) ? "Ready" : "Recovery risk"}</small></div>
      </div>

      {(lowMemoryWarn || availWarn) && (
        <p className="note">Memory is within the supported range, but 14 GiB or more is recommended.</p>
      )}

      {!bitlockerOk(probe) && (
        <article className="blocker warning">
          <div>
            <h3>Turn off BitLocker (recommended)</h3>
            <p>
              Continuing with BitLocker may trigger a recovery-key prompt if installer
              staging or the reboot handoff fails. Windows rollback is not guaranteed.
            </p>
            <label className="check risk-acceptance">
              <input
                type="checkbox"
                checked={bitlockerRiskAccepted}
                onChange={(event) => onBitlockerRiskAccepted(event.target.checked)}
              />
              Continue with BitLocker enabled and accept the recovery risk.
            </label>
          </div>
        </article>
      )}

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

      <details className="optional technical-details">
        <summary>Technical details</summary>
        <ul className="checks">
          {checks.map((row) => (
            <li key={row.id} className={row.state}>
              <span className="check-flag">{flagLabel(row.state)}</span>
              <span className="check-key">{row.label}</span>
              <span className="check-val">{row.detail}</span>
            </li>
          ))}
        </ul>
        {boot && <DiskCard disk={boot} linuxById={probe.linuxById} />}
      </details>
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

function SetupStep({
  identity,
  password2,
  keyboardLabel,
  question,
  onChange,
  onPassword2,
  onKeyboardLabel,
}: {
  identity: CidataIdentity;
  password2: string;
  keyboardLabel: string;
  question: number;
  onChange: (next: CidataIdentity) => void;
  onPassword2: (v: string) => void;
  onKeyboardLabel: (label: string) => void;
}) {
  const patch = (partial: Partial<CidataIdentity>) => onChange({ ...identity, ...partial });
  const current = SETUP_QUESTIONS[question];
  const error =
    current === "username" && identity.username.length > 0
      ? usernameError(identity.username)
      : current === "fullName" && (identity.fullName?.length ?? 0) > 0
        ? fullNameError(identity.fullName)
        : current === "email" && (identity.email?.length ?? 0) > 0
          ? emailError(identity.email)
      : current === "hostname" && identity.hostname.length > 0
        ? hostnameError(identity.hostname)
        : current === "password" && (identity.password.length > 0 || password2.length > 0)
          ? passwordError(identity.password, password2)
          : null;

  const titles: Record<SetupQuestion, string> = {
    keyboard: "Select keyboard layout",
    username: "Select username",
    fullName: "Set Git author name",
    email: "Set Git author email",
    password: "Set account password",
    hostname: "Set computer name",
    timezone: "Select timezone",
    encrypt: "Configure disk encryption",
  };

  const hints: Record<SetupQuestion, string> = {
    keyboard: "This layout is also used when entering the disk passphrase.",
    username: "Linux username · lowercase letters, numbers, hyphens and underscores.",
    fullName: "Written to Git commits as user.name.",
    email: "Written to Git commits as user.email.",
    password: "Used for login, sudo, and disk encryption when enabled.",
    hostname: "Identifies this computer on the local network.",
    timezone: "Controls the system clock and regional time.",
    encrypt: "Encryption protects the installation when the computer is powered off.",
  };

  return (
    <form className="setup" onSubmit={(e) => e.preventDefault()}>
      <div className="question-count">System configuration · {question + 1}/{SETUP_QUESTIONS.length}</div>
      <div className="question-progress" aria-hidden="true">
        {SETUP_QUESTIONS.map((item, index) => (
          <span key={item} className={index <= question ? "active" : undefined} />
        ))}
      </div>
      <h1>{titles[current]}</h1>
      <p className="question-hint">{hints[current]}</p>

      {current === "keyboard" && (
        <SearchPicker
          label="Keyboard layout"
          value={identity.keyboard}
          selectedLabel={keyboardLabel}
          options={KEYBOARDS}
          onSearch={() => patch({ keyboard: "" })}
          onChange={(keyboard) => {
            patch({ keyboard: keyboard.id });
            onKeyboardLabel(keyboard.label);
          }}
        />
      )}

      {current === "username" && <label className="field-card">
        <span>Username</span>
        <input
          autoComplete="username"
          spellCheck={false}
          placeholder="e.g. jane"
          value={identity.username}
          onChange={(e) => patch({ username: e.target.value })}
        />
      </label>}

      {current === "fullName" && <label className="field-card">
        <span>Full name</span>
        <input
          autoComplete="name"
          placeholder="e.g. Jane Doe"
          value={identity.fullName ?? ""}
          onChange={(e) => patch({ fullName: e.target.value })}
        />
      </label>}

      {current === "email" && <label className="field-card">
        <span>Email address</span>
        <input
          type="email"
          autoComplete="email"
          spellCheck={false}
          placeholder="e.g. jane@example.com"
          value={identity.email ?? ""}
          onChange={(e) => patch({ email: e.target.value || null })}
        />
      </label>}

      {current === "password" && <div className="field-stack">
        <label className="field-card">
          <span>Password</span>
          <input
            type="password"
            autoComplete="new-password"
            autoFocus
            value={identity.password}
            onChange={(e) => patch({ password: e.target.value })}
          />
        </label>
        <label className="field-card">
          <span>Confirm password</span>
          <input
            type="password"
            autoComplete="new-password"
            value={password2}
            onChange={(e) => onPassword2(e.target.value)}
          />
        </label>
      </div>}

      {current === "hostname" && <label className="field-card">
        <span>Computer name</span>
        <input
          spellCheck={false}
          value={identity.hostname}
          onChange={(e) => patch({ hostname: e.target.value })}
        />
      </label>}

      {current === "timezone" && (
        <SearchPicker
          label="Timezone"
          value={identity.timezone}
          options={TIMEZONE_OPTIONS}
          onSearch={() => patch({ timezone: "" })}
          onChange={(timezone) => patch({ timezone: timezone.id })}
        />
      )}

      {current === "encrypt" && <div className="choice-grid">
        <button type="button" className={identity.encrypt ? "choice selected" : "choice"} onClick={() => patch({ encrypt: true })}>
          <span className="choice-icon">✓</span>
          <span><strong>Encrypt my disk</strong><small>Recommended</small></span>
        </button>
        <button type="button" className={!identity.encrypt ? "choice selected" : "choice"} onClick={() => patch({ encrypt: false })}>
          <span className="choice-icon">○</span>
          <span><strong>Leave it unencrypted</strong><small>Faster startup, less protection</small></span>
        </button>
      </div>}

      {error && <p className="field-error standalone">{error}</p>}
    </form>
  );
}

function ConfirmStep({
  probe,
  identity,
  keyboardLabel,
  eraseInput,
  secondConfirm,
  onEraseInput,
  onSecondConfirm,
  media,
  bitlockerActive,
}: {
  probe: MachineProbe | null;
  identity: CidataIdentity;
  keyboardLabel: string;
  eraseInput: string;
  secondConfirm: boolean;
  onEraseInput: (v: string) => void;
  onSecondConfirm: (v: boolean) => void;
  media: IsoAcquisitionState;
  bitlockerActive: boolean;
}) {
  const disk =
    probe?.disks.find((d) => d.deviceId === probe.recommendedDiskId) ??
    probe?.disks.find((d) => d.isBoot);
  const matched = eraseInput.trim() === ERASE_PHRASE;

  return (
    <div className="review">
      <p className="kicker">Ready to install</p>
      <h1>Review your setup</h1>
      <p className="review-lead">Omarchy will replace Windows on this PC.</p>
      {bitlockerActive && (
        <p className="banner">
          BitLocker is still on. Keep your recovery key nearby.
        </p>
      )}
      <dl className="review-grid">
        <dt>User</dt>
        <dd>
          {identity.username || "—"}@{identity.hostname}
        </dd>
        <dt>Target</dt>
        <dd className="mono">
          {disk ? `${disk.deviceId} · ${formatBytes(disk.sizeBytes)}` : "—"}
        </dd>
        <dt>Security</dt>
        <dd>{identity.encrypt ? "Disk encrypted" : "Not encrypted"}</dd>
        <dt>Keyboard</dt>
        <dd>{keyboardLabel}</dd>
        <dt>Install media</dt>
        <dd>
          {isoSourceName(media.source)} · <span className="verified-copy">verified</span>
        </dd>
        <dt>Git</dt>
        <dd>
          {identity.fullName}
          {identity.email ? ` <${identity.email}>` : ""}
        </dd>
      </dl>

      <details className="optional review-details">
        <summary>Installation details</summary>
        <div className="iso-source">
          <span className={media.source.kind === "local" ? "mono" : undefined}>
            {media.source.kind === "local" ? media.source.path : isoSourceName(media.source)}
          </span>
          <span className="verified-copy">
            {media.result ? `${formatBytes(media.result.bytes)} · SHA-256 verified · signed` : "Verified"}
          </span>
        </div>
        {media.result && <p className="mono media-digest">{media.result.sha256}</p>}
        {disk && <DiskCard disk={disk} linuxById={probe?.linuxById ?? null} />}
      </details>

      <div className="danger-zone">
        <strong>Everything on the Windows disk will be erased.</strong>
        <p>Back up anything you want to keep, then type <b>{ERASE_PHRASE}</b>.</p>
        <label className="confirm-field">
          <span className="sr-only">Type {ERASE_PHRASE}</span>
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
        <label className={`check final-check ${matched ? "visible" : ""}`}>
          <input
            type="checkbox"
            checked={secondConfirm}
            disabled={!matched}
            onChange={(e) => onSecondConfirm(e.target.checked)}
          />
          My files are backed up and I understand this cannot be undone.
        </label>
      </div>
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
  const current = order.indexOf(phase);
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
  expectedIso,
  allowBitlocker,
  onStatus,
  onJournal,
  onClearPassword,
  onUndo,
  onMediaInvalid,
}: {
  native: boolean;
  identity: CidataIdentity;
  journal: StateJournal | null;
  expectedIso: VerifyResult | null;
  allowBitlocker: boolean;
  onStatus: (next: { phase: string; ready: boolean; rebooting: boolean }) => void;
  onJournal: (next: StateJournal | null) => void;
  onClearPassword: () => void;
  onUndo: () => Promise<void>;
  onMediaInvalid: (error: string) => void;
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
  const [mediaInvalid, setMediaInvalid] = useState(false);
  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;
  const identityRef = useRef(identity);
  identityRef.current = identity;
  const onJournalRef = useRef(onJournal);
  onJournalRef.current = onJournal;
  const onClearPasswordRef = useRef(onClearPassword);
  onClearPasswordRef.current = onClearPassword;
  const expectedIsoRef = useRef(expectedIso);
  expectedIsoRef.current = expectedIso;
  const onMediaInvalidRef = useRef(onMediaInvalid);
  onMediaInvalidRef.current = onMediaInvalid;

  useEffect(() => {
    onStatusRef.current({ phase, ready, rebooting });
  }, [phase, ready, rebooting]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      unlisten = await listen<IsoProgress>("iso://progress", (event) => {
        if (cancelled) return;
        setPhase("verify");
        setBytes(event.payload.bytes);
        setTotal(event.payload.total);
      });
      let checkingMedia = false;
      try {
        setError(null);
        setMediaInvalid(false);
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
          setError("Re-enter the user password in Setup, then retry.");
          setPhase("cidata");
          return;
        }
        if (start === "prepare") {
          checkingMedia = true;
          setPhase("verify");
          const expected = expectedIsoRef.current;
          if (!expected) {
            throw new Error("The installation media is no longer approved. Verify it again before continuing.");
          }
          const result = await invoke<VerifyResult>("verify_iso");
          if (cancelled) return;
          if (result.sha256 !== expected.sha256 || result.bytes !== expected.bytes) {
            throw new Error("The installation media changed after review. Verify it again before continuing.");
          }
          checkingMedia = false;
          setSha(result.sha256);
          setBytes(result.bytes);
          setTotal(result.bytes);
          setPhase("prepare");
          await invoke("prepare_installer_partition", { allowBitlocker });
          if (cancelled) return;
        }
        if (start === "prepare" || start === "stage") {
          setPhase("stage");
          await invoke("stage_bootloader");
          if (cancelled) return;
        }
        if (start === "prepare" || start === "stage" || start === "cidata") {
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
          setMediaInvalid(checkingMedia);
        }
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
    // The approved media and identity are read through refs; retry is `tick`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick, allowBitlocker]);

  const pct =
    total && total > 0 ? Math.min(100, Math.round((bytes / total) * 100)) : null;
  const label =
    phase === "verify"
      ? "Rechecking installation media"
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
              <span>
                {item.label}
              </span>
              {item.id === "verify" && working && phase === "verify" && (
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
            {mediaInvalid ? (
              <button
                type="button"
                className="btn primary"
                onClick={() => onMediaInvalidRef.current(error)}
              >
                Return to media check
              </button>
            ) : (
              <>
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
              </>
            )}
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
