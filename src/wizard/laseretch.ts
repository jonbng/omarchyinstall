// Homepage laseretch from omarchy.org (references/omarchy-site/assets/js/modules/logo.js).
// Plays once, then holds the last frame. The <pre> is layout + fallback.

const WTE_WASM_URL = "/wte/laseretch.wasm";
const WTE_PLAYBACK_URL = "/wte/assets/playback-C457l4sF.js";
const EFFECT = "laseretch";
const ART_COLUMNS = 88;
const ART_ROWS = 20;
const CELL_ASPECT = 2;
const FONT_WAIT_MS = 1000;

type Playback = {
  restart: () => Promise<void>;
  stop: () => void;
};

type PlaybackCtor = new (opts: {
  canvas: HTMLCanvasElement;
  width: () => number;
  height: () => number;
  connected: () => boolean;
  input: () => string;
  effect: () => string;
  wasmUrl: () => ArrayBuffer;
  onFinished: () => void;
  frameRate: () => number;
}) => Playback;

let wasmBytes: ArrayBuffer | null = null;
let PlaybackClass: PlaybackCtor | null = null;

function prefersReducedMotion() {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

async function loadWasm() {
  if (wasmBytes) return wasmBytes;
  const response = await fetch(WTE_WASM_URL);
  if (!response.ok) throw new Error(`laseretch wasm ${response.status}`);
  wasmBytes = await response.arrayBuffer();
  return wasmBytes;
}

function afterFonts() {
  if (document.fonts?.ready == null) return Promise.resolve();
  return Promise.race([
    document.fonts.ready,
    new Promise<void>((resolve) => {
      window.setTimeout(resolve, FONT_WAIT_MS);
    }),
  ]);
}

function nativeGrid(host: HTMLElement) {
  const box = host.getBoundingClientRect();
  const cell = Math.max(
    1,
    Math.floor(Math.min(box.width / ART_COLUMNS, box.height / (ART_ROWS * CELL_ASPECT))),
  );
  return { width: cell * ART_COLUMNS, height: cell * ART_ROWS * CELL_ASPECT };
}

function scaleCanvas(
  canvas: HTMLCanvasElement,
  host: HTMLElement,
  nativeWidth: number,
  nativeHeight: number,
) {
  const box = host.getBoundingClientRect();
  if (box.width < 1 || box.height < 1) return;
  canvas.style.transform = `scale(${box.width / nativeWidth}, ${box.height / nativeHeight})`;
}

function watchSize(target: HTMLElement, onChange: () => void) {
  let frame = 0;
  const schedule = () => {
    if (frame !== 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      onChange();
    });
  };
  const observer = new ResizeObserver(schedule);
  observer.observe(target);
  return () => {
    if (frame !== 0) {
      cancelAnimationFrame(frame);
      frame = 0;
    }
    observer.disconnect();
  };
}

async function loadCanvasPlayback(): Promise<PlaybackCtor> {
  if (PlaybackClass) return PlaybackClass;
  // Runtime URL so Vite does not try to bundle the public/ file as source.
  const url = new URL(WTE_PLAYBACK_URL, window.location.href).href;
  const mod = (await import(/* @vite-ignore */ url)) as Record<string, unknown>;
  for (const value of Object.values(mod)) {
    if (
      typeof value === "function" &&
      value.prototype != null &&
      typeof value.prototype.restart === "function" &&
      typeof value.prototype.stop === "function"
    ) {
      PlaybackClass = value as PlaybackCtor;
      return PlaybackClass;
    }
  }
  throw new Error("CanvasPlayback not found");
}

function artFromPre(pre: HTMLPreElement) {
  let text = pre.textContent ?? "";
  if (text.startsWith("\n")) text = text.slice(1);
  return text.replace(/\n+$/, "");
}

function isCssBlack(value: string) {
  const v = value.replace(/\s/g, "").toLowerCase();
  return (
    v === "#000" ||
    v === "#000000" ||
    v === "black" ||
    v === "rgb(0,0,0)" ||
    v === "rgba(0,0,0,1)" ||
    v === "rgba(0,0,0,1.0)"
  );
}

function nightColor(host: HTMLElement) {
  const fromCss = getComputedStyle(host).getPropertyValue("--bg").trim();
  if (fromCss.startsWith("#") || fromCss.startsWith("rgb")) return fromCss;
  const painted = getComputedStyle(document.body).backgroundColor;
  return painted && painted !== "rgba(0, 0, 0, 0)" ? painted : "#1a1b26";
}

/** Playback clears the canvas with packed black. Remap that fill to the UI night color. */
function remapBlackClear(canvas: HTMLCanvasElement, night: string) {
  const original = canvas.getContext.bind(canvas);
  canvas.getContext = ((id: string, options?: CanvasRenderingContext2DSettings) => {
    const ctx = original(id, options);
    if (id !== "2d" || !(ctx instanceof CanvasRenderingContext2D)) return ctx;
    const tagged = ctx as CanvasRenderingContext2D & { __omarchyNight?: boolean };
    if (tagged.__omarchyNight) return ctx;
    tagged.__omarchyNight = true;
    const desc = Object.getOwnPropertyDescriptor(
      CanvasRenderingContext2D.prototype,
      "fillStyle",
    );
    if (!desc?.set || !desc.get) return ctx;
    const set = desc.set;
    const get = desc.get;
    Object.defineProperty(ctx, "fillStyle", {
      configurable: true,
      enumerable: true,
      get() {
        return get.call(ctx);
      },
      set(value: string | CanvasGradient | CanvasPattern) {
        if (typeof value === "string" && isCssBlack(value)) set.call(ctx, night);
        else set.call(ctx, value);
      },
    });
    return ctx;
  }) as HTMLCanvasElement["getContext"];
}

export function startLaseretch(
  pre: HTMLPreElement,
  host: HTMLElement,
  hooks: { onLive: () => void; onFail: () => void },
): () => void {
  if (prefersReducedMotion()) {
    hooks.onFail();
    return () => undefined;
  }

  const input = artFromPre(pre);
  if (input.trim() === "") {
    hooks.onFail();
    return () => undefined;
  }

  let stopped = false;
  let playback: Playback | null = null;
  let stopWatching = () => undefined as void;
  let removeError = () => undefined as void;

  const fail = () => {
    if (stopped) return;
    stopped = true;
    removeError();
    stopWatching();
    playback?.stop();
    host.querySelector(".ascii__wte")?.remove();
    hooks.onFail();
  };

  afterFonts()
    .then(loadWasm)
    .then(loadCanvasPlayback)
    .then((CanvasPlayback) => {
      if (stopped) return;
      const box = pre.getBoundingClientRect();
      if (box.width < 8 || box.height < 8) {
        fail();
        return;
      }

      const holder = document.createElement("span");
      holder.className = "ascii__wte";

      const canvas = document.createElement("canvas");
      canvas.setAttribute("aria-hidden", "true");
      remapBlackClear(canvas, nightColor(host));
      const native = nativeGrid(pre);
      canvas.style.width = `${native.width}px`;
      canvas.style.height = `${native.height}px`;
      scaleCanvas(canvas, pre, native.width, native.height);

      const bytes = wasmBytes;
      if (bytes == null) {
        fail();
        return;
      }

      playback = new CanvasPlayback({
        canvas,
        width: () => native.width,
        height: () => native.height,
        connected: () => canvas.isConnected && !stopped,
        input: () => input,
        effect: () => EFFECT,
        wasmUrl: () => bytes,
        onFinished() {},
        frameRate: () => 720,
      });

      stopWatching = watchSize(pre, () => {
        scaleCanvas(canvas, pre, native.width, native.height);
      });

      const onError = (event: ErrorEvent) => {
        const message = String(event.message ?? event.error ?? "");
        if (!/memory access out of bounds|RuntimeError|CompileError|WebAssembly/i.test(message)) {
          return;
        }
        fail();
      };
      window.addEventListener("error", onError);
      removeError = () => window.removeEventListener("error", onError);

      holder.append(canvas);
      host.append(holder);
      hooks.onLive();
      void playback.restart().catch(fail);
    })
    .catch(fail);

  return () => {
    if (stopped) return;
    stopped = true;
    removeError();
    stopWatching();
    playback?.stop();
    host.querySelector(".ascii__wte")?.remove();
  };
}
