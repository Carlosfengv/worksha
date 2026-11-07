import { setupPluginCenterPeers } from "./plugin-center-peers";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { PluginManager } from "@/features/plugins/manager";
import { pluginsApi, parsePluginList } from "@/features/plugins/api";
import { parseContract } from "@/features/plugins/contract";
import type { Manifest, PluginStatus } from "@/features/plugins/types";
import raw from "../../sdk/plugins/examples/aurora-agent/package/manifest.json";
import "@/styles/globals.css";
setupPluginCenterPeers();

const mode = new URLSearchParams(location.search).get("mode") ?? "ready";
const current: PluginStatus = {
  manifest: parseContract<Manifest>("manifest", raw),
  digest: "d".repeat(64),
  signingKey: "a".repeat(64),
  enabled: true,
  runtimeStatus: mode === "stale" ? "degraded" : "active",
  errorCode: mode === "stale" ? "invalid_input" : null,
  activationEpoch: "1",
  grantRevision: "1",
  sourceAuthorized: mode !== "revoked",
  connectionAuthorized: false,
  reportAuthorized: false,
  sourceLabel: null,
  connectionLabel: null,
  previousDigest: null,
  settings: { "title-style": "original" },
};
const at = "2026-10-24T01:00:00Z";
const row = {
  pluginId: current.manifest.id,
  digest: current.digest,
  activity:
    mode === "unknown"
      ? null
      : {
          since: at,
          attempts: "9007199254740993",
          succeeded: "9007199254740989",
          failed: "3",
          interrupted: "1",
          timeouts: "1",
          resourceLimits: "1",
          workerCrashes: "1",
          inFlight: "0",
          lastOperation: "refresh",
          lastFinishedAt: at,
          lastSuccessAt: at,
          lastErrorCode: mode === "stale" ? "invalid_input" : null,
          lastDurationMs: "27",
        },
  snapshot: {
    state:
      mode === "empty"
        ? "confirmed-empty"
        : mode === "unknown"
          ? "not-read"
          : mode === "revoked"
            ? "awaiting-authorization"
            : mode === "stale"
              ? "stale"
              : "available",
    observedAt: mode === "unknown" || mode === "revoked" ? null : at,
    records:
      mode === "unknown" || mode === "revoked"
        ? null
        : mode === "empty"
          ? "0"
          : "513",
  },
};
const diagnostics = {
  version: 1,
  available: true,
  scope: "service-lifetime",
  since: at,
  dropped: mode === "gap" ? "2" : "0",
  items: [row],
};
if (mode === "broken" && row.activity) row.activity.attempts = "-1";
const mutations: string[] = [];
declare global {
  interface Window {
    diagnosticMutations: string[];
  }
}
window.diagnosticMutations = mutations;
pluginsApi.list = async () =>
  parsePluginList({
    items: [current],
    apiVersion: "1.0.0",
    developmentMode: false,
    runtime: "wasmtime-36.0.19",
    ...(mode === "old" ? {} : { diagnostics }),
  });
pluginsApi.extensionHistory = async () => ({ plans: [] });
pluginsApi.action = async () => {
  mutations.push("action");
  return {};
};
pluginsApi.inspect = async () => {
  mutations.push("inspect");
  return null;
};
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="p-6">
      <PluginManager providers={[]} />
    </main>
  </IconProvider>,
);
