import { useState } from "react";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { SessionSourceControl } from "@/features/sessions/components/session-source";
import { AgentProviderSettings } from "@/features/providers/components/provider-config-control";
import { sessionsApi } from "@/features/sessions/api";
import { providersApi } from "@/features/providers/api";
import type { SessionSource } from "@/features/sessions/types";
import "@/styles/globals.css";

let enabled = false;
declare global {
  interface Window {
    antigravitySourceWrites: boolean[];
  }
}
window.antigravitySourceWrites = [];
sessionsApi.setSessionSource = async (agentId, next) => {
  if (agentId !== "antigravity") throw Error("Incorrect source");
  window.antigravitySourceWrites.push(next);
  enabled = next;
};
providersApi.providerStatus = async () => ({
  configs: [],
  transactions: [],
  verifications: [],
  runtimes: [],
  backupProtection: "fixture",
});
function Harness() {
  const [source, setSource] = useState<SessionSource>({
    agentId: "antigravity",
    enabled: false,
    locations: [
      "/fixture/.gemini/antigravity/conversations",
      "/fixture/.gemini/config",
    ],
    status: "disabled",
    sessionCount: 0,
    scannedFiles: 0,
    totalFiles: 0,
    skippedFiles: 0,
    lastScan: null,
    error: null,
    versions: [],
    capabilities: [],
  });
  const refresh = async () => {
    setSource((old) => ({
      ...old,
      enabled,
      status: enabled ? "ready" : "disabled",
      sessionCount: enabled ? 1 : 0,
    }));
  };
  return (
    <IconProvider>
      <main className="space-y-6 p-4">
        <h1>Antigravity</h1>
        <AgentProviderSettings
          agentId="antigravity"
          agentProductId="antigravity"
          providers={[]}
          refresh={refresh}
        />
        <SessionSourceControl source={source} refresh={refresh} />
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
