export type RuntimeMode = "tauri" | "browser" | "preview";

type EventEnvelope<T> = { payload: T };
type Listener<T> = (event: EventEnvelope<T>) => void;
type BridgeStatus = "connected" | "disconnected";

const statusListeners = new Set<(status: BridgeStatus) => void>();
let bootstrap: Promise<void> | null = null;
let heartbeat: number | null = null;

export function runtimeMode(): RuntimeMode {
  if (typeof window === "undefined") return "preview";
  if ("__TAURI_INTERNALS__" in window) return "tauri";
  if (window.location.hostname === "127.0.0.1" && window.location.protocol === "http:") {
    return "browser";
  }
  return "preview";
}

export function onBridgeStatus(listener: (status: BridgeStatus) => void) {
  statusListeners.add(listener);
  return () => {
    statusListeners.delete(listener);
  };
}

function setStatus(status: BridgeStatus) {
  for (const listener of statusListeners) listener(status);
}

async function browserSession() {
  if (bootstrap) return bootstrap;
  bootstrap = (async () => {
    const params = new URLSearchParams(window.location.hash.replace(/^#/, ""));
    const fragmentToken = params.get("token");
    const savedToken = sessionStorage.getItem("omarchy-bootstrap");
    const token = fragmentToken ?? savedToken;

    if (token) {
      sessionStorage.setItem("omarchy-bootstrap", token);
      const response = await fetch("/api/session", {
        method: "POST",
        headers: { Authorization: `Bearer ${token}` },
        credentials: "same-origin",
      });
      if (!response.ok) throw new Error("The secure browser session could not be established.");
      sessionStorage.removeItem("omarchy-bootstrap");
      history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
    } else {
      const response = await fetch("/api/heartbeat", {
        method: "POST",
        credentials: "same-origin",
      });
      if (!response.ok) throw new Error("This browser session has expired. Reopen OmarchyInstaller.exe.");
    }

    setStatus("connected");
    if (heartbeat == null) {
      heartbeat = window.setInterval(() => {
        void fetch("/api/heartbeat", { method: "POST", credentials: "same-origin" })
          .then((response) => setStatus(response.ok ? "connected" : "disconnected"))
          .catch(() => setStatus("disconnected"));
      }, 10_000);
    }
  })().catch((error) => {
    setStatus("disconnected");
    bootstrap = null;
    throw error;
  });
  return bootstrap;
}

export async function invoke<T = void>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (runtimeMode() === "tauri") {
    const api = await import("@tauri-apps/api/core");
    return api.invoke<T>(command, args);
  }
  if (runtimeMode() !== "browser") {
    throw new Error("Tauri IPC is unavailable in browser preview mode");
  }

  await browserSession();
  let response: Response;
  try {
    response = await fetch(`/api/invoke/${encodeURIComponent(command)}`, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(args),
    });
  } catch (error) {
    setStatus("disconnected");
    throw error;
  }
  if (!response.ok) {
    setStatus("disconnected");
    throw new Error(`Local backend returned HTTP ${response.status}`);
  }
  const result = (await response.json()) as
    | { ok: true; value: T }
    | { ok: false; error: string };
  if (!result.ok) throw new Error(result.error);
  setStatus("connected");
  return result.value;
}

export async function listen<T>(eventName: string, listener: Listener<T>): Promise<() => void> {
  if (runtimeMode() === "tauri") {
    const api = await import("@tauri-apps/api/event");
    return api.listen<T>(eventName, listener);
  }
  if (runtimeMode() !== "browser") return () => undefined;

  await browserSession();
  const source = new EventSource("/api/events", { withCredentials: true });
  const receive = (event: MessageEvent<string>) => {
    listener({ payload: JSON.parse(event.data) as T });
  };
  source.addEventListener(eventName, receive as EventListener);
  source.onopen = () => setStatus("connected");
  source.onerror = () => setStatus("disconnected");
  return () => source.close();
}

export async function getVersion(): Promise<string> {
  if (runtimeMode() === "tauri") {
    const api = await import("@tauri-apps/api/app");
    return api.getVersion();
  }
  if (runtimeMode() === "browser") return invoke<string>("_version");
  return "0.4.1";
}

export async function closeApp() {
  if (runtimeMode() === "tauri") {
    await invoke("exit_app");
    return;
  }
  if (runtimeMode() === "browser") {
    await invoke("_shutdown");
    window.close();
  }
}

export async function revealItemInDir(path: string) {
  if (runtimeMode() !== "tauri") return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}
