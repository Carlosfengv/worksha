import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { SessionsPage } from "@/features/sessions/page";
import { AgentDetail } from "@/features/agents/components/agent-detail";
import { createUiRegistry } from "@/app/plugin-registry";
import type { Snapshot } from "@/app/types";
import type {
  Conversation,
  NativePage,
  NativeSession,
  SessionQuery,
} from "@/features/sessions/types";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import {
  activityOverview,
  activityQuery,
  activitySessions,
} from "./activity-fixture";
import { requestDashboard, requestRecord } from "./request-fixture";
import type { RequestQuery } from "@/features/observation/request-types";
import "@/styles/globals.css";
import "@/styles/app.css";

declare global {
  interface Window {
    sessionNav: {
      queries: SessionQuery[];
      conversations: string[];
      anchors: (string | null)[];
      hold: boolean;
      releases: (() => void)[];
      error: string | null;
      version: number;
      empty: boolean;
      partial: boolean;
      connected: boolean;
      recover: () => void;
    };
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
window.sessionNav = {
  queries: [],
  conversations: [],
  anchors: [],
  hold: false,
  releases: [],
  error: null,
  version: 0,
  empty: false,
  partial: false,
  connected: true,
  recover: () => {
    window.sessionNav.version++;
    notifyBackendRecovery();
  },
};
const fixture = window.sessionNav;
const params = new URLSearchParams(location.search);
const registry = createUiRegistry([
  {
    id: "fixture",
    version: 1,
    pages: (["sessions", "plugins", "settings"] as const).map((id) => ({
      id,
      label: id,
      icon: "message-circle",
      render: () => null,
    })),
  },
]);
const observedId = "codex:00000000-0000-4000-8000-000000000001";
function session(index: number): NativeSession {
  return {
    ...activitySessions()[0],
    id: `claude:session-${index}`,
    nativeId: `session-${index}`,
    agentId: "claude",
    agentProductId: "claude-code-cli",
    agentProducts: ["claude-code-cli"],
    title: `Session ${index}${fixture.version ? ` revision-${fixture.version}` : ""}`,
    projectPath: `/fixture/project-${index % 3}`,
    projectName: `Project ${index % 3}`,
    projectId: null,
    projectHidden: index % 17 === 16,
    modelErrorCount: 0,
    latestFailureId: null,
  };
}
function page(query: SessionQuery): NativePage<NativeSession> {
  if (params.has("gateway") && query.includeGateway)
    return {
      items: [
        session(0),
        {
          ...session(1),
          id: observedId,
          agentId: "codex",
          agentProductId: "codex-desktop",
          agentProducts: ["codex-desktop"],
          title: "Codex 测试会话",
          status: "observed",
          projectPath: null,
          projectName: null,
          modelCalls: 1,
        },
      ],
      total: 2,
      nextCursor: null,
      summary: null,
      asOf: "2026-10-28T01:00:00Z",
      coverage: "native_history",
      missingCounts: 0,
    };
  const offset = query.cursor ? Number(query.cursor.split(":")[1]) : 0;
  if (query.cursor && !query.cursor.startsWith(`revision-${fixture.version}:`))
    throw { code: "stale_snapshot", message: "synthetic stale cursor" };
  const total =
    query.query === "none" || fixture.empty || !fixture.connected
      ? 0
      : Number(params.get("total") ?? 10000);
  const start = query.query ? 5000 : 0;
  return {
    items: Array.from({ length: Math.min(40, total - offset) }, (_, n) =>
      session(start + offset + n),
    ),
    total,
    nextCursor:
      offset + 40 < total ? `revision-${fixture.version}:${offset + 40}` : null,
    summary: null,
    asOf: "2026-10-28T01:00:00Z",
    coverage: !fixture.connected
      ? "not_connected"
      : fixture.partial
        ? "partial"
        : "native_history",
    missingCounts: 0,
  };
}
function conversation(id: string): Conversation {
  const index = Number(id.split("session-")[1]);
  return {
    session: session(index),
    total: 4,
    nextCursor: null,
    asOf: "2026-10-28T01:00:00Z",
    coverage: "native_history",
    warnings: [],
    items: Array.from({ length: 4 }, (_, n) => ({
      id: `${id}:event-${n}`,
      nativeId: `event-${n}`,
      turnId: "fixture",
      kind: params.has("trace") && n === 2 ? "model" : "message",
      role: "user",
      name: params.has("trace") && n === 2 ? "关联模型响应" : "message",
      timestamp: null,
      endTime: null,
      status: "recorded",
      text: "synthetic conversation ".repeat(150),
      input: null,
      output: null,
      model: null,
      modelResponseId: null,
      provider: null,
      usage: null,
      rawUsage: null,
      callId: null,
      server: null,
      toolSource: null,
      durationMs: null,
      truncated: false,
      sourceLine: n,
      failure: null,
      agentProductId: null,
    })),
  };
}
mockIPC(async (name, args) => {
  const input = (args ?? {}) as Record<string, unknown>;
  if (name === "query_sessions") {
    const query = input.query as SessionQuery;
    fixture.queries.push(query);
    const result = page(query);
    if (fixture.hold)
      await new Promise<void>((resolve) => fixture.releases.push(resolve));
    if (fixture.error)
      throw { code: fixture.error, message: "synthetic list unavailable" };
    return result;
  }
  if (name === "get_conversation") {
    const id = String(input.sessionId);
    fixture.conversations.push(id);
    fixture.anchors.push(
      typeof input.anchor === "string" ? input.anchor : null,
    );
    return conversation(id);
  }
  if (name === "query_request_dashboard" && params.has("gateway")) {
    const query = input.query as RequestQuery;
    const row = activityOverview(activityQuery).gatewayActivities[0].request;
    return requestDashboard(
      query,
      query.session === observedId
        ? [
            requestRecord({
              ...row,
              sessionId: observedId,
              model: "fixture-gateway-model",
            }),
          ]
        : [],
    );
  }
  if (name === "refresh_sessions") return;
  if (name === "list_plugins")
    return {
      items: [],
      developmentMode: false,
      apiVersion: "1.0.0",
      runtime: "fixture",
    };
  throw Error(`unexpected fixture command: ${name}`);
});
const snapshot: Snapshot = {
  version: "fixture",
  databasePath: "/fixture/data",
  gatewayStatus: "ready",
  agents: [],
  providers: [],
  settings: { captureContent: false, retentionDays: 30 },
  settingsRevision: 0,
  observationBindings: [],
  sessionSources: [
    {
      agentId: "claude",
      enabled: true,
      locations: ["/fixture"],
      status: "ready",
      sessionCount: 10000,
      scannedFiles: 1,
      totalFiles: 1,
      skippedFiles: 0,
      lastScan: null,
      error: null,
      versions: [],
      capabilities: [],
    },
  ],
};
function Harness() {
  const [route, setRoute] = useState(location.hash);
  useEffect(() => {
    const change = () => setRoute(location.hash);
    window.addEventListener("hashchange", change);
    return () => window.removeEventListener("hashchange", change);
  }, []);
  const sessionRoute = registry.readRoute(route);
  return (
    <IconProvider>
      <main className="flex h-screen flex-col">
        {params.has("agent") ? (
          <AgentDetail
            agent={{
              id: "claude-code-cli",
              family: "claude",
              name: "Fixture Agent",
              configId: "fixture",
              found: true,
              location: null,
              connected: false,
            }}
            source={snapshot.sessionSources[0]}
            data={snapshot}
            tab={
              new URLSearchParams(route.split("?")[1]).get("tab") ?? "sessions"
            }
            refresh={async () => {}}
          />
        ) : (
          <SessionsPage
            data={snapshot}
            navigate={() => {}}
            selectedId={sessionRoute.objectId}
            initialAgent={sessionRoute.agent}
            initialView={sessionRoute.view}
            anchor={sessionRoute.anchor}
            returnTo={sessionRoute.returnTo}
          />
        )}
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
