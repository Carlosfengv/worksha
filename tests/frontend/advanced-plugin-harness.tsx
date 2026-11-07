import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import {
  AdvancedControls,
  PluginOperationHistory,
} from "@/features/plugins/advanced-controls";
import { pluginsApi } from "@/features/plugins/api";
import type { PluginStatus } from "@/features/plugins/types";
import type {
  ExtensionStatus,
  ExtensionPlan,
} from "@/features/plugins/advanced-types";
import type { Provider } from "@/features/providers/types";
import { DesktopError } from "@/shared/lib/desktop-command";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import { mockIPC } from "@tauri-apps/api/mocks";
import "@/styles/globals.css";

// Business stubs only. No browser-to-native bridge or real file / network access.
const mode = new URLSearchParams(location.search).get("mode") ?? "config";
const status: PluginStatus = {
  manifest: {
    manifestVersion: 1,
    id: "org.example.advanced",
    publisher: "org.example",
    version: "1.0.0",
    hostApi: "^2.0",
    displayName: "高级插件测试",
    runtime: { kind: "declarative" },
    capabilities: ["ui.contribution@1"],
    permissions: [{ kind: "ui.contribute" }],
    agent: null,
    providers: [],
    requires: [],
    settings: [],
    views: [],
  },
  digest: "a".repeat(64),
  signingKey: "b".repeat(64),
  enabled: true,
  runtimeStatus: "active",
  errorCode: null,
  activationEpoch: "1",
  grantRevision: "1",
  sourceAuthorized: false,
  connectionAuthorized: false,
  reportAuthorized: false,
  sourceLabel: null,
  connectionLabel: null,
  previousDigest: null,
  settings: {},
};
const providers: Provider[] = [
  {
    id: "fixture",
    name: "离线连接",
    endpoint: "http://127.0.0.1:12345/v1",
    model: "fixture",
    protocol: "responses",
    configured: true,
    credentialRef: null,
    credentialLabel: null,
    models: ["fixture"],
    revision: 1,
    category: "local",
  },
];
const calls: string[] = [];
declare global {
  interface Window {
    advancedPluginFixture: {
      recover: () => void;
      validateConnector: (
        integrity: Record<string, unknown>,
      ) => Promise<ExtensionStatus>;
    };
  }
}
const realExtensionStatus = pluginsApi.extensionStatus;
window.advancedPluginFixture = {
  recover: notifyBackendRecovery,
  validateConnector: async (integrity) => {
    Object.defineProperty(globalThis, "isTauri", {
      value: true,
      configurable: true,
    });
    mockIPC(() => ({
      ...structuredClone(data),
      connector: {
        host: "fixture",
        observationEnabled: true,
        transportConnected: false,
        tools: null,
        calls: 0,
        notifications: 0,
        lastError: null,
        content: "not-persisted",
        note: "fixture",
        ...integrity,
      },
    }));
    return realExtensionStatus({
      pluginId: status.manifest.id,
      digest: status.digest,
    });
  },
};
const record = (s: string) => {
  calls.push(s);
  document.querySelector("#calls")!.textContent = JSON.stringify(calls);
};
let data: ExtensionStatus = {
  apiVersion: "1.0.0",
  policy: {
    schemaVersion: 1,
    protocol:
      mode === "protocol"
        ? { baseProtocol: "responses", path: "/envelope" }
        : null,
    config:
      mode === "config"
        ? { fields: [{ pointer: "/env/API_MODEL", value: "model" }] }
        : null,
    connector: mode === "mcp" ? "mcp-stdio" : null,
  },
  resourceLabel: null,
  resourceTarget: null,
  resourceAuthorized: false,
  protocolBindings: [],
  plans: [],
  calls: [],
  connector: null,
};
const plan: ExtensionPlan = {
  planId: "fixture-plan",
  pluginId: status.manifest.id,
  digest: status.digest,
  path: "/temporary/selected.json",
  revision: "revision-1",
  providerId: "fixture",
  providerName: "离线连接",
  state: "prepared",
  createdAt: "2026-10-23T00:00:00Z",
  edits: [{ pointer: "/env/API_KEY", value: "${HOST_CREDENTIAL}" }],
  backupAvailable: false,
  canDiscard: true,
  credentialPlaceholder: "${HOST_CREDENTIAL}",
};
let connectorPolls = 0;
pluginsApi.extensionStatus = async () => {
  if (mode === "error") throw Error("扩展服务读取失败");
  if (
    data.connector &&
    new URLSearchParams(location.search).has("observation")
  ) {
    connectorPolls++;
    if (connectorPolls > 1) {
      data = {
        ...data,
        connector: {
          ...data.connector,
          transportConnected: true,
          tools: 3,
          calls: 1,
          observationIncomplete: false,
          unconfirmedCalls: 0,
        },
        calls: [
          {
            identity: "observed-echo",
            rpcId: 1,
            mcpConnection: "fixture",
            tool: "echo",
            status: "success",
            durationMs: 1,
            observedAt: "2026-10-24T00:00:00Z",
            agentSession: null,
            content: "not-persisted",
            modelTokens: null,
          },
        ],
      };
    }
  }
  return structuredClone(data);
};
pluginsApi.authorizeResource = async () => {
  record("authorize");
  data = {
    ...data,
    resourceAuthorized: true,
    resourceLabel: "/temporary/selected.json",
  };
  return { authorized: true };
};
pluginsApi.exportConnector = async () => {
  record("export");
  return { saved: true };
};
let historyReads = 0;
let historyPlans: ExtensionPlan[] = [
  { ...plan, state: "applied", backupAvailable: true, canDiscard: false },
];
if (new URLSearchParams(location.search).has("prepared")) historyPlans = [plan];
pluginsApi.extensionHistory = async () => {
  historyReads++;
  document.querySelector("#history-reads")!.textContent = String(historyReads);
  const fault = new URLSearchParams(location.search).get("historyError");
  if (
    (fault === "first" && historyReads === 1) ||
    (fault === "second" && historyReads === 2)
  )
    throw new DesktopError({
      code: "invalid_record",
      message:
        "插件扩展记录损坏或版本不兼容，请保留原文件与备份，核对后再操作。",
    });
  return {
    plans: structuredClone(historyPlans),
    planCapacity: {
      used: historyPlans.length,
      limit: 64,
      remaining: 64 - historyPlans.length,
    },
  };
};
pluginsApi.extensionAction = async (input) => {
  record(input.action.kind);
  switch (input.action.kind) {
    case "bind-protocol":
      data = { ...data, protocolBindings: [input.action.providerId] };
      break;
    case "unbind-protocol":
      data = { ...data, protocolBindings: [] };
      break;
    case "prepare-config":
      data = {
        ...data,
        plans: [plan],
        planCapacity: { used: 1, limit: 64, remaining: 63 },
      };
      break;
    case "discard-config":
      if (new URLSearchParams(location.search).has("discardError"))
        throw Error("预览状态已变化，请重新读取。");
      data = {
        ...data,
        plans: [],
        planCapacity: { used: 0, limit: 64, remaining: 64 },
      };
      historyPlans = [];
      break;
    case "apply-config":
      data = {
        ...data,
        plans: [
          {
            ...plan,
            state: "applied",
            backupAvailable: true,
            canDiscard: false,
          },
        ],
      };
      break;
    case "restore-config":
      historyPlans = historyPlans.map((entry) =>
        entry.planId === plan.planId ? { ...entry, state: "restored" } : entry,
      );
      data = {
        ...data,
        plans: [
          {
            ...plan,
            state: "restored",
            backupAvailable: true,
            canDiscard: false,
          },
        ],
      };
      break;
    case "start-connector":
      data = {
        ...data,
        connector: {
          host: "127.0.0.1:23456",
          transportConnected: false,
          observationEnabled: true,
          tools: null,
          notifications: 0,
          calls: 0,
          observationIncomplete: new URLSearchParams(location.search).has(
            "unconfirmed",
          )
            ? true
            : new URLSearchParams(location.search).has("legacy")
              ? undefined
              : false,
          unconfirmedCalls: new URLSearchParams(location.search).has(
            "unconfirmed",
          )
            ? 2
            : 0,
          lastError: null,
          content: "not-persisted",
          note: "waiting",
        },
      };
      break;
    case "stop-connector":
      data = { ...data, connector: null };
      break;
    default:
      break;
  }
  return {};
};
document.body.innerHTML =
  '<div id="root"></div><pre id="calls">[]</pre><pre id="history-reads">0</pre>';
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="mx-auto max-w-4xl space-y-6 p-6">
      {mode === "history" ? (
        <PluginOperationHistory />
      ) : (
        <AdvancedControls
          status={status}
          providers={providers}
          onChanged={() => {}}
        />
      )}
    </main>
  </IconProvider>,
);
