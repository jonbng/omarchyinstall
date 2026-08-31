import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { HostInfo } from "./types";
import "./App.css";

function App() {
  const [host, setHost] = useState<HostInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<HostInfo>("host_info")
      .then(setHost)
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : String(err));
      });
  }, []);

  return (
    <main className="app">
      <header>
        <h1>Omarchy Install</h1>
        <p className="lede">Windows production app. Dev hosts compile against a stub.</p>
      </header>

      {error && <p className="banner error">{error}</p>}

      {host && !host.nativeWindows && (
        <p className="banner">
          Running on {host.os}/{host.arch}. Win32 code is not compiled in; production
          APIs will return “Windows only”.
        </p>
      )}

      <section className="panel">
        <h2>Host</h2>
        {host ? (
          <dl>
            <div>
              <dt>OS</dt>
              <dd>{host.os}</dd>
            </div>
            <div>
              <dt>Arch</dt>
              <dd>{host.arch}</dd>
            </div>
            <div>
              <dt>OS version</dt>
              <dd>{host.osVersion ?? "—"}</dd>
            </div>
            <div>
              <dt>Elevated</dt>
              <dd>{host.elevated ? "yes" : "no"}</dd>
            </div>
            <div>
              <dt>Win32</dt>
              <dd>{host.nativeWindows ? "native" : "stub"}</dd>
            </div>
          </dl>
        ) : (
          !error && <p className="muted">Reading host…</p>
        )}
      </section>
    </main>
  );
}

export default App;
