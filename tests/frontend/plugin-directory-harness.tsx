import { useState } from "react";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { AgentDirectoryProvider } from "@/shared/hooks/use-agent-directory";
import { agentProducts, type AgentDiscovery } from "@/shared/types/agent";
import { RequestFilters } from "@/features/observation/components/request-filters";
import { AgentDetail } from "@/features/agents/components/agent-detail";
import { sessionsApi } from "@/features/sessions/api";
import type { SessionSource } from "@/features/sessions/types";
import type { Snapshot } from "@/app/types";
import "@/styles/globals.css";

declare global {
  interface Window {
    directoryWrites: string[];
    directoryReads: string[];
  }
}
window.directoryWrites = [];
window.directoryReads = [];
sessionsApi.sessions = async (query) => {
  window.directoryReads.push(query.agent ?? "all");
  return {
    items: [],
    total: 0,
    nextCursor: null,
    summary: null,
    asOf: "2026-10-22T00:00:00Z",
    coverage: "not_connected",
    missingCounts: 0,
  };
};
sessionsApi.setSessionSource = async (id, enabled) => {
  window.directoryWrites.push(`${id}:${enabled}`);
};
const initialSource: SessionSource = {
  agentId: "fixture-reader",
  enabled: false,
  locations: ["/fixture/readonly"],
  status: "disabled",
  sessionCount: 0,
  scannedFiles: 0,
  totalFiles: 0,
  skippedFiles: 0,
  lastScan: null,
  error: null,
  versions: [],
  capabilities: [],
};
function Harness() {
  const [name, setName] = useState("Fixture Reader");
  const [params, setParams] = useState(new URLSearchParams());
  const [source, setSource] = useState(initialSource);
  const custom: AgentDiscovery = {
    id: "fixture-reader",
    family: "fixture-reader",
    configId: "fixture-reader",
    name,
    found: true,
    connected: false,
    location: "/fixture/readonly",
  };
  const products = [...agentProducts, custom];
  const data: Snapshot = {
    version: "fixture",
    databasePath: "/fixture/data",
    gatewayStatus: "not_configured",
    agents: [custom],
    providers: [],
    settings: { captureContent: false, retentionDays: 30 },
    sessionSources: [source],
    observationBindings: [],
  };
  return (
    <IconProvider>
      <AgentDirectoryProvider products={products}>
        <main className="space-y-6 p-4">
          <button onClick={() => setName("Renamed Reader")}>更新目录</button>
          <RequestFilters
            params={params}
            data={null}
            onChange={(patch) =>
              setParams((old) => {
                const next = new URLSearchParams(old);
                for (const [key, value] of Object.entries(patch)) {
                  if (value) next.set(key, value);
                  else next.delete(key);
                }
                return next;
              })
            }
          />
          <output data-testid="agent-query">{params.get("agent")}</output>
          <AgentDetail
            agent={custom}
            source={source}
            data={data}
            tab="connection"
            refresh={async () => {
              setSource((old) => ({
                ...old,
                enabled: !old.enabled,
                status: old.enabled ? "disabled" : "ready",
              }));
            }}
          />
        </main>
      </AgentDirectoryProvider>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
