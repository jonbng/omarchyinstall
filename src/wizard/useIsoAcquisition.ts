import { useCallback, useEffect, useRef, useState } from "react";
import type { IsoProgress, LocalIsoSelection, VerifyResult } from "../types";
import { invoke, listen } from "./bridge";
import { invokeError } from "./ipc";

export type IsoSource =
  | { kind: "official" }
  | { kind: "local"; path: string; filename: string | null };

export type IsoAcquisitionPhase =
  | "idle"
  | "selecting"
  | "preparing-local"
  | "download"
  | "hash"
  | "signature"
  | "ready"
  | "error";

export type IsoAcquisitionState = {
  source: IsoSource;
  phase: IsoAcquisitionPhase;
  bytes: number;
  total: number | null;
  result: VerifyResult | null;
  error: string | null;
};

const INITIAL_STATE: IsoAcquisitionState = {
  source: { kind: "official" },
  phase: "idle",
  bytes: 0,
  total: null,
  result: null,
  error: null,
};

export function isoAcquisitionRunning(phase: IsoAcquisitionPhase): boolean {
  return phase === "preparing-local" || phase === "download" || phase === "hash" || phase === "signature";
}

export function useIsoAcquisition() {
  const [state, setState] = useState<IsoAcquisitionState>(INITIAL_STATE);
  const stateRef = useRef(state);
  const activeRef = useRef(false);
  const pendingRef = useRef<Promise<VerifyResult | null> | null>(null);
  stateRef.current = state;

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<IsoProgress>("iso://progress", (event) => {
      if (cancelled || !activeRef.current) return;
      setState((current) => ({
        ...current,
        phase: event.payload.phase,
        bytes: event.payload.bytes,
        total: event.payload.total,
        error: null,
      }));
    }).then((next) => {
      unlisten = next;
      if (cancelled) unlisten();
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const start = useCallback((): Promise<VerifyResult | null> => {
    if (pendingRef.current) return pendingRef.current;
    const source = stateRef.current.source;
    activeRef.current = true;
    setState((current) => ({
      ...current,
      phase: source.kind === "local" ? "preparing-local" : "download",
      bytes: 0,
      total: null,
      result: null,
      error: null,
    }));

    const pending = (async () => {
      try {
        if (source.kind === "local") {
          const selected = await invoke<LocalIsoSelection>("prepare_local_iso", {
            path: source.path,
          });
          setState((current) => ({
            ...current,
            source: { kind: "local", path: selected.path, filename: selected.filename },
            phase: "hash",
            bytes: 0,
            total: selected.bytes,
          }));
        } else {
          await invoke("download_iso");
          setState((current) => ({ ...current, phase: "hash" }));
        }
        const result = await invoke<VerifyResult>("verify_iso");
        setState((current) => ({
          ...current,
          phase: "ready",
          bytes: result.bytes,
          total: result.bytes,
          result,
          error: null,
        }));
        return result;
      } catch (error: unknown) {
        setState((current) => ({
          ...current,
          phase: "error",
          result: null,
          error: invokeError(error),
        }));
        return null;
      } finally {
        activeRef.current = false;
        pendingRef.current = null;
      }
    })();
    pendingRef.current = pending;
    return pending;
  }, []);

  const chooseLocal = useCallback(async (): Promise<boolean> => {
    if (activeRef.current) return false;
    const previous = stateRef.current;
    setState((current) => ({ ...current, phase: "selecting", error: null }));
    try {
      const path = await invoke<string | null>("pick_local_iso");
      if (!path) {
        setState(previous);
        return false;
      }
      setState({
        source: { kind: "local", path, filename: null },
        phase: "idle",
        bytes: 0,
        total: null,
        result: null,
        error: null,
      });
      return true;
    } catch (error: unknown) {
      setState({
        ...previous,
        phase: "error",
        result: null,
        error: invokeError(error),
      });
      return false;
    }
  }, []);

  const useOfficial = useCallback((): boolean => {
    if (activeRef.current || stateRef.current.source.kind === "official") return false;
    setState(INITIAL_STATE);
    return true;
  }, []);

  const invalidate = useCallback((error: string) => {
    activeRef.current = false;
    pendingRef.current = null;
    setState((current) => ({
      ...current,
      phase: "error",
      result: null,
      error,
    }));
  }, []);

  return {
    state,
    running: isoAcquisitionRunning(state.phase),
    start,
    chooseLocal,
    useOfficial,
    invalidate,
  };
}
