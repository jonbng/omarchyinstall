import { useEffect, useRef, useState } from "react";
import type { AbortCopy } from "./copy";
import { closeApp, runtimeMode } from "./bridge";

export async function closeWindow() {
  await closeApp();
}

export async function handleTitlebarMouseDown(clickCount: number) {
  if (runtimeMode() !== "tauri") return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const window = getCurrentWindow();
  if (clickCount > 1) await window.toggleMaximize();
  else await window.startDragging();
}

export function WindowControls({ onClose }: { onClose: () => void }) {
  const [maximized, setMaximized] = useState(false);
  const native = runtimeMode() === "tauri";

  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const window = getCurrentWindow();
      const update = async () => {
        const next = await window.isMaximized();
        if (!cancelled) setMaximized(next);
      };
      await update();
      unlisten = await window.onResized(() => void update().catch(() => undefined));
      if (cancelled) unlisten?.();
    })().catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [native]);

  return (
    <div className="window-controls" data-no-drag>
      {native && (
        <>
          <button
            type="button"
            className="window-control"
            aria-label="Minimize"
            title="Minimize"
            onClick={() => {
              void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
                getCurrentWindow().minimize(),
              ).catch(() => undefined);
            }}
          >
            <span className="minimize-icon" aria-hidden="true" />
          </button>
          <button
            type="button"
            className="window-control"
            aria-label={maximized ? "Restore" : "Maximize"}
            title={maximized ? "Restore" : "Maximize"}
            onClick={() => {
              void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
                getCurrentWindow().toggleMaximize(),
              ).catch(() => undefined);
            }}
          >
            <span className={maximized ? "restore-icon" : "maximize-icon"} aria-hidden="true" />
          </button>
        </>
      )}
      <button
        type="button"
        className="window-control close-control"
        aria-label="Close"
        title="Close"
        onClick={onClose}
      >
        <span aria-hidden="true">×</span>
      </button>
    </div>
  );
}

export function AbortDialog({
  copy,
  busy,
  error,
  onStay,
  onConfirm,
}: {
  copy: AbortCopy;
  busy: boolean;
  error: string | null;
  onStay: () => void;
  onConfirm: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    if (!node.open) node.showModal();
    const onCancel = (event: Event) => {
      event.preventDefault();
      if (!busy) onStay();
    };
    node.addEventListener("cancel", onCancel);
    return () => node.removeEventListener("cancel", onCancel);
  }, [busy, onStay]);

  return (
    <dialog ref={ref} className="abort-dialog" aria-labelledby="abort-title">
      <p className="kicker">{copy.kicker}</p>
      <h2 id="abort-title">{copy.title}</h2>
      <p>{copy.body}</p>
      {error && <p className="banner error">{error}</p>}
      <div className="abort-actions">
        <button type="button" className="btn ghost" onClick={onStay} disabled={busy} autoFocus>
          {copy.cancel}
        </button>
        {copy.confirm && (
          <button
            type="button"
            className="btn danger"
            onClick={onConfirm}
            disabled={busy}
          >
            {busy ? "Working…" : copy.confirm}
          </button>
        )}
      </div>
    </dialog>
  );
}
