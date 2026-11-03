import { useCallback, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { useSnapshot } from "@/app/use-snapshot";
import type { Snapshot } from "@/app/types";
import { BackendConnection } from "@/features/settings/components/backend-connection";
import { SettingsPreferences } from "@/features/settings/components/settings-preferences";
import { GatewayPanel } from "@/features/settings/gateway-panel";
import type { GatewayStatus } from "@/features/settings/types";
import { providerStatusResource } from "@/features/providers/queries";
import { providersApi } from "@/features/providers/api";
import type {
  ProviderStatus,
  ProviderImportCandidate,
} from "@/features/providers/types";
import { NativeProviderImport } from "@/features/providers/components/native-provider-import";
import { FileDiff } from "@/features/projects/components/changes";
import { PluginOperationHistory } from "@/features/plugins/advanced-controls";
import { pluginsApi } from "@/features/plugins/api";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { useNativePage } from "@/features/sessions/hooks/use-native-page";
import { sessionsApi } from "@/features/sessions/api";
import type { NativePage, NativeSession } from "@/features/sessions/types";
import { ConversationReader } from "@/features/sessions/components/conversation-reader";
import { ReadingCache } from "@/features/sessions/reading-cache";
import { useOverview } from "@/features/observation/hooks/use-overview";
import { observationApi } from "@/features/observation/api";
import { RequestsPage } from "@/features/observation/requests-page";
import { AgentMemory } from "@/features/agents/components/agent-memory";
import { McpPage } from "@/features/mcp/page";
import { mcpApi } from "@/features/mcp/api";
import {
  activityConversation,
  activityOverview,
  activityQuery,
  activitySessions,
} from "./activity-fixture";
import { memoryFixture } from "./memory-context-fixture";
import { requestDashboard } from "./request-fixture";
import "@/styles/globals.css";

declare global {
  interface Window {
    readRecovery: {
      calls: string[];
      stage: number;
      connection: "success" | "reject" | "unconfirmed" | "malformed";
      connect: (() => void)[];
      held: { name: string; release: () => void }[];
      hold: string[];
      fail: string[];
      rawProviderStatus: () => Promise<ProviderStatus>;
    };
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
const params = new URLSearchParams(location.search);
const mode = params.get("view") ?? "settings";
window.readRecovery = {
  calls: [],
  stage: 0,
  connection: "success",
  connect: [],
  held: [],
  hold: params.has("hold") ? [params.get("hold")!] : [],
  fail: params.has("fail") ? [params.get("fail")!] : [],
  rawProviderStatus: () => providersApi.providerStatus(),
};
const fixture = window.readRecovery;
const stamp = () => new Date(Date.now() + fixture.stage).toISOString();
async function read<T>(name: string, value: T): Promise<T> {
  fixture.calls.push(name);
  if (fixture.fail.includes(name))
    throw Error("fixture read unavailable: " + name);
  if (fixture.hold.includes(name))
    return new Promise((resolve) =>
      fixture.held.push({ name, release: () => resolve(value) }),
    );
  return value;
}
const diagnostics = () => ({
  state: fixture.stage ? "connected" : "startup_failed",
  verifiedAt: fixture.stage ? Date.now() : null,
  canReconnect: true,
  events: [],
});
mockIPC((name, args) => {
  if (name === "reconnect_backend") {
    fixture.calls.push(name);
    return new Promise((resolve, reject) =>
      fixture.connect.push(() => {
        if (fixture.connection === "reject")
          return reject({
            code: "backend_identity_rejected",
            message: "fixture identity rejected",
            outcomeUnknown: false,
          });
        if (fixture.connection === "unconfirmed")
          return resolve({ ...diagnostics(), state: "disconnected" });
        if (fixture.connection === "malformed")
          return resolve({ unexpected: "private fixture payload" });
        fixture.stage += 1;
        resolve(diagnostics());
      }),
    );
  }
  if (name === "get_backend_diagnostics") return read(name, diagnostics());
  if (name === "get_snapshot")
    return read(name, {
      version: "snapshot-" + fixture.stage,
      databasePath: "/fixture/data",
      gatewayStatus: "ready",
      agents: [],
      providers: [],
      settings: {
        captureContent: false,
        retentionDays: fixture.stage ? 90 : 30,
      },
      settingsRevision: fixture.stage,
      settingsWriteProtocol: {
        version: 1,
        storeId: "00000000-0000-4000-8000-000000000001",
        receiptLimit: 256,
      },
      sessionSources: [],
      observationBindings: [],
    } satisfies Snapshot);
  if (name === "get_gateway_status")
    return read(name, {
      writeProtocol: {
        version: 1,
        storeId: "00000000-0000-4000-8000-000000000001",
        maxClients: 128,
        secretReplay: false,
      },
      listen: "192.168.1.10:43210",
      persistent: true,
      concurrency: 16,
      active: fixture.stage,
      clients: [
        {
          id: "11111111-1111-4111-8111-111111111111",
          name: "client-" + fixture.stage,
          agent: "claude",
          providerId: "fixture",
          models: ["fixture"],
          concurrency: 2,
          active: 0,
          enabled: true,
          createdAt: stamp(),
        },
      ],
    } satisfies GatewayStatus);
  if (name === "get_provider_status")
    return read(name, {
      configs: [],
      transactions: [],
      backupProtection: "provider-" + fixture.stage,
      verifications: [],
      runtimes: [],
    } satisfies ProviderStatus);
  if (name === "get_project_diff")
    return read(name, {
      asOf: stamp(),
      path: "file.rs",
      state: "ready",
      text: "diff-" + fixture.stage,
      reason: null,
    });
  if (name === "preview_native_provider_import")
    return read(name, {
      protocol: {
        version: 1,
        storeId: "71a3e175-52cf-41da-8631-c1fb652bb2a7",
        namespaceId: "6e183d0d-6268-4e1b-9abc-25d3e66f8249",
        namespaceState: "active",
        sequence: 0,
        operationLimit: 4096,
        pendingLimit: 4,
        pendingRequests: [],
      },
      candidates: [
        {
          id: "candidate",
          agentId: "claude",
          sourcePath: "/fixture/.claude/settings.json",
          sourceLabel: "Fixture",
          revision: String(fixture.stage).padStart(64, "a"),
          name: "Fixture native provider",
          vendor: "custom",
          endpoint: "https://fixture.invalid/v1",
          protocol: "messages",
          model: null,
          active: true,
          credentialSource: "fixture",
          canImport: true,
          reason: null,
          existingProviderId: null,
        } satisfies ProviderImportCandidate,
      ],
    });
  if (name === "query_agent_memory") {
    const parameters = args as { agentId: string; fileId: string | null };
    return memoryFixture(
      parameters.agentId,
      parameters.fileId ?? undefined,
    ).then((value) =>
      read(name, {
        ...value,
        selected: value.selected
          ? { ...value.selected, text: "memory-" + fixture.stage }
          : null,
      }),
    );
  }
  fixture.calls.push(name);
  throw Error("Unexpected business operation: " + name);
});
sessionsApi.sessions = (query): Promise<NativePage<NativeSession>> =>
  read("sessions", {
    items: [
      {
        ...activitySessions()[0],
        id: "page-" + fixture.stage + (query?.cursor ? "-next" : "-first"),
      },
    ],
    total: 2,
    nextCursor: query?.cursor ? null : "cursor-" + fixture.stage,
    summary: null,
    asOf: stamp(),
    coverage: "native_history",
    missingCounts: 0,
  });
sessionsApi.conversation = (id) =>
  read("conversation", {
    ...activityConversation(id),
    session: {
      ...activityConversation(id).session,
      title: "conversation-" + fixture.stage,
    },
  });
sessionsApi.refreshSessions = async () => {
  fixture.calls.push("unexpected_refresh_sessions");
  throw Error("Recovery must not replay indexing");
};
observationApi.overview = (query) => {
  const value = activityOverview(query);
  return read("overview", {
    ...value,
    asOf: stamp(),
    analysis: { ...value.analysis, asOf: stamp() },
  });
};
observationApi.requestDashboard = (query) =>
  read("requests", requestDashboard(query, []));
mcpApi.mcp = (query) =>
  read("mcp", {
    services: [],
    groups: [],
    usageSummary: {
      relatedTokens: null,
      relatedResponses: 0,
      tokenCoveredCalls: 0,
      tokenMissingCalls: 0,
    },
    facets: { models: [], projects: [] },
    calls: [],
    totalCalls: 0,
    nextCursor: null,
    selectedServiceId: null,
    selectedGroupId: null,
    sources: [],
    asOf: stamp(),
    historyAsOf: stamp(),
    coverage: "partial",
    warnings: [],
    appliedFilter: query,
  });
pluginsApi.extensionHistory = () => read("backups", { plans: [] });
function ProviderProbe({ name }: { name: string }) {
  const value = useSharedRead(providerStatusResource);
  return (
    <output data-testid={name}>
      {value.data?.backupProtection ?? "empty"}
    </output>
  );
}
function SettingsReaders() {
  const root = useSnapshot();
  const local = usePollingResource(
    "recovery-local",
    useCallback(() => read("local", "local-" + fixture.stage), []),
  );
  return (
    <>
      <output data-testid="root">{root.data?.version ?? "empty"}</output>
      <output data-testid="root-error">{root.error}</output>
      <output data-testid="local">{local.data}</output>
      {root.data && (
        <SettingsPreferences
          value={root.data.settings}
          revision={root.data.settingsRevision}
          protocol={root.data.settingsWriteProtocol}
          refresh={root.refresh}
        />
      )}
      <GatewayPanel
        providers={[]}
        storeId={root.data?.settingsWriteProtocol?.storeId}
      />
      <ProviderProbe name="provider-a" />
      <ProviderProbe name="provider-b" />
    </>
  );
}
function NativeReader() {
  const resource = useNativePage(
    "recovery-pages",
    useCallback((cursor?: string) => sessionsApi.sessions({ cursor }), []),
  );
  return (
    <>
      <output data-testid="native">
        {resource.data?.items.map((item) => item.id).join(",") ?? "empty"}
      </output>
      <button onClick={() => void resource.loadMore()}>next-page</button>
    </>
  );
}
function OverviewReader() {
  const resource = useOverview(activityQuery);
  return (
    <>
      <output data-testid="analysis">
        {resource.data?.analysis.asOf ?? "empty"}
      </output>
      <output data-testid="analysis-updating">
        {String(resource.updating)}
      </output>
    </>
  );
}
function BodyReader() {
  const cache = useRef(new ReadingCache());
  const [visible, setVisible] = useState(true);
  const summary = useCallback(() => {}, []);
  const id = activitySessions()[0].id;
  return (
    <>
      <button onClick={() => setVisible((v) => !v)}>toggle-reader</button>
      {visible && (
        <ConversationReader id={id} cache={cache.current} onSummary={summary} />
      )}
    </>
  );
}
function Harness() {
  const [connection, setConnection] = useState(true);
  return (
    <IconProvider>
      <main className="space-y-5 p-5">
        <button onClick={() => setConnection((v) => !v)}>
          toggle-connection
        </button>
        {connection && <BackendConnection />}
        {mode === "settings" && <SettingsReaders />}
        {mode === "native" && <NativeReader />}
        {mode === "overview" && <OverviewReader />}
        {mode === "conversation" && <BodyReader />}
        {mode === "memory" && <AgentMemory agentId="claude-code-cli" enabled />}
        {mode === "mcp" && <McpPage />}
        {mode === "requests" && <RequestsPage routeSearch="" />}
        {mode === "diff" && (
          <FileDiff
            projectId="project"
            file={{
              id: "file",
              path: "file.rs",
              previousPath: null,
              indexStatus: " ",
              worktreeStatus: "M",
              attribution: "unknown",
            }}
          />
        )}
        {mode === "backups" && <PluginOperationHistory />}
        {mode === "import" && (
          <NativeProviderImport
            onImported={async () => {
              throw Error("Recovery cannot import");
            }}
          />
        )}
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
