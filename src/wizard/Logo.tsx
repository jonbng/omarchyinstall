import { useEffect, useRef, useState } from "react";
import { startLaseretch } from "./laseretch";

export const OMARCHY_ASCII = ` ▄██████▄    ▄▄▄▄███▄▄▄▄      ▄████████    ▄████████  ▄████████    ▄█    █▄    ▄██   ▄
███    ███ ▄██▀▀▀███▀▀▀██▄   ███    ███   ███    ███ ███    ███   ███    ███   ███   ██▄
███    ███ ███   ███   ███   ███    ███   ███    ███ ███    █▀    ███    ███   ███▄▄▄███
███    ███ ███   ███   ███   ███    ███  ▄███▄▄▄▄██▀ ███         ▄███▄▄▄▄███▄▄ ▀▀▀▀▀▀███
███    ███ ███   ███   ███ ▀███████████ ▀▀███▀▀▀▀▀   ███        ▀▀███▀▀▀▀███▀  ▄██   ███
███    ███ ███   ███   ███   ███    ███ ▀███████████ ███    █▄    ███    ███   ███   ███
███    ███ ███   ███   ███   ███    ███   ███    ███ ███    ███   ███    ███   ███   ███
 ▀██████▀   ▀█   ███   █▀    ███    █▀    ███    ███ ████████▀    ███    █▀     ▀█████▀
                                          ███    ███


         ▄█  ███▄▄▄▄      ▄████████     ███        ▄████████  ▄█        ▄█
        ███  ███▀▀▀██▄   ███    ███ ▀█████████▄   ███    ███ ███       ███
        ███▌ ███   ███   ███    █▀     ▀███▀▀██   ███    ███ ███       ███
        ███▌ ███   ███   ███            ███   ▀   ███    ███ ███       ███
        ███▌ ███   ███ ▀███████████     ███     ▀███████████ ███       ███
        ███  ███   ███          ███     ███       ███    ███ ███       ███
        ███  ███   ███    ▄█    ███     ███       ███    ███ ███▌    ▄ ███▌    ▄
        █▀    ▀█   █▀   ▄████████▀     ▄████▀     ███    █▀  █████▄▄██ █████▄▄██
                                                             ▀         ▀`;

export function Mark({ size = 22 }: { size?: number }) {
  return (
    <svg
      className="mark"
      width={size}
      height={size}
      viewBox="0 0 1200 1200"
      aria-hidden="true"
    >
      <path
        fill="currentColor"
        fillRule="evenodd"
        clipRule="evenodd"
        d="m1200 1200h-480v-80h400v-1040h-479.996v160h-400v720h720v-720h-80v-80h159.996v880h-400v160h-640v-1200h1200zm-1120-80h480v-80h-400l.004-400h-80.004zm0-560h80.004v-400h400v-80h-480.004z"
      />
    </svg>
  );
}

export function AsciiLogo() {
  const hostRef = useRef<HTMLDivElement>(null);
  const preRef = useRef<HTMLPreElement>(null);
  const [mode, setMode] = useState<"pending" | "live" | "static">("pending");

  useEffect(() => {
    const host = hostRef.current;
    const pre = preRef.current;
    if (!host || !pre) return;

    const fallback = window.setTimeout(() => {
      setMode((current) => (current === "pending" ? "static" : current));
    }, 2000);

    const stop = startLaseretch(pre, host, {
      onLive() {
        window.clearTimeout(fallback);
        setMode("live");
      },
      onFail() {
        window.clearTimeout(fallback);
        setMode("static");
      },
    });

    return () => {
      window.clearTimeout(fallback);
      stop();
    };
  }, []);

  return (
    <div
      ref={hostRef}
      className={`ascii ascii--${mode}`}
      aria-label="Omarchy"
    >
      <pre ref={preRef} className="ascii-base">
        {OMARCHY_ASCII}
      </pre>
    </div>
  );
}
