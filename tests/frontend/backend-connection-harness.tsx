import { useEffect, useState } from "react";
import { subscribeBackendRecovery } from "@/shared/lib/backend-recovery";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { BackendConnection } from "@/features/settings/components/backend-connection";
import {
  backendDiagnostics,
  type BackendDiagnostics,
} from "@/features/settings/backend-result";
import "@/styles/globals.css";
declare global {
  interface Window {
    backendFixture: {
      calls: string[];
      refreshes: number;
      pending: (() => void)[];
      oldRead: (() => void) | null;
    };
    parseBackendFixture: typeof backendDiagnostics;
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
const mode = new URLSearchParams(location.search).get("mode");
window.backendFixture = { calls: [], refreshes: 0, pending: [], oldRead: null };
window.parseBackendFixture = backendDiagnostics;
const events = Array.from({ length: 32 }, (_, index) => ({
  id: crypto.randomUUID(),
  operationId: crypto.randomUUID(),
  at: Date.now() - index * 1000,
  phase: "process" as const,
  category: "child_exited" as const,
  exitCode: index === 0 ? 7 : null,
  signal: null,
}));
let current: BackendDiagnostics = {
  state: mode === "closing" ? "stopping" : "startup_failed",
  verifiedAt: null,
  canReconnect: mode !== "closing",
  events,
};
let reads = 0;
mockIPC((name) => {
  window.backendFixture.calls.push(name);
  if (name === "get_backend_diagnostics") {
    reads++;
    if (mode === "malformed")
      return {
        ...current,
        secret: "private-payload-sentinel",
        events: [{ secret: "private-payload-sentinel" }],
      };
    if (mode === "old-read" && reads === 2)
      return new Promise((resolve) => {
        const old = current;
        window.backendFixture.oldRead = () => resolve(old);
      });
    return current;
  }
  if (name === "reconnect_backend")
    return new Promise((resolve, reject) => {
      window.backendFixture.pending.push(() => {
        if (mode === "rejected") {
          reject(
            JSON.stringify({
              code: "backend_identity_rejected",
              message: "后台身份不匹配，未连接。",
              outcomeUnknown: false,
            }),
          );
          return;
        }
        current = { ...current, state: "connected", verifiedAt: Date.now() };
        resolve(
          mode === "unconfirmed"
            ? { ...current, state: "disconnected" }
            : current,
        );
      });
    });
  throw new Error("Unexpected business command: " + name);
});
function DataObserver() {
  const [error, setError] = useState(false);
  useEffect(
    () =>
      subscribeBackendRecovery(() => {
        window.backendFixture.refreshes++;
        if (mode === "refresh-error") setError(true);
      }),
    [],
  );
  return error ? (
    <p role="alert">页面数据刷新未完成，请再次刷新数据。</p>
  ) : null;
}
function Harness() {
  const [mounted, setMounted] = useState(true);
  return (
    <IconProvider>
      <main className="max-w-3xl space-y-4 p-6">
        <button onClick={() => setMounted((v) => !v)}>切换挂载</button>
        {mounted && (
          <>
            <BackendConnection />
            <DataObserver />
          </>
        )}
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
