import { useEffect, useRef } from "react";
import type { AbortCopy } from "./copy";
import { closeApp } from "./bridge";

export async function closeWindow() {
  await closeApp();
}

export function AbortButton({ onClick }: { onClick: () => void }) {
  return (
    <button type="button" className="abort-btn" onClick={onClick}>
      abort
    </button>
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
