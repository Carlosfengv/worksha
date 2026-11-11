import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import { IconProvider } from "@/lib/icon-context";
import type { Snapshot } from "@/app/types";
import { AgentsPage } from "@/features/agents/page";
import { bundledDshAgent } from "@/features/agents/bundled-source";
import { agentConnectionApi } from "@/features/agents/api";
import { domainApi } from "@/features/domain/api";
import { sessionsApi } from "@/features/sessions/api";
import { AgentDirectoryProvider } from "@/shared/hooks/use-agent-directory";
import { agentProducts, type AgentDiscovery } from "@/shared/types/agent";
import { systemFeatureCatalog } from "@/features/plugins/system-features";
import { PluginManager } from "@/features/plugins/manager";
import { createProjectsPlugin } from "@/features/projects/plugin";
import { providersApi } from "@/features/providers/api";
import type { ProviderOperation } from "@/features/providers/operation-receipt";
import {
  builtinPluginsApi,
  fullPluginsApi,
  pluginsApi,
  pluginUploadsApi,
} from "@/features/plugins/api";
import {
  modelServicesApi,
  routingPolicyApi,
} from "@/features/model-services/api";
import {
  builtinBusinessPlugins,
  emptyBuiltinCatalog,
} from "./plugin-center-peers";
import { compositionSelection } from "@/features/plugins/builtin-plan";
import type { CompositionCatalog } from "../../contracts/plugins/builtin/types";
import raw from "../../sdk/plugins/examples/aurora-agent/package/manifest.json";
import rawSource from "../../sdk/plugins/examples/full-history/package/manifest.json";
import rawPolicy from "../../sdk/plugins/examples/pure-policy/package/manifest.json";
import dshSource from "../../sdk/plugins/examples/dsh-desktop/package/manifest.json";
import type { Manifest, PluginStatus } from "@/features/plugins/types";
import type { Status as SourceStatus } from "../../contracts/plugins/sources/types";
import type {
  PackageStatus as PolicyStatus,
  Reply,
} from "../../contracts/plugins/routing/types";
import "@/styles/globals.css";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
const mode = new URLSearchParams(location.search).get("mode") ?? "plugin";
const agentParity = mode.startsWith("agent-parity");
const projectsPlugin = createProjectsPlugin(() => null);
const storeId = emptyBuiltinCatalog.storeId;
const calls: string[] = [];
declare global {
  interface Window {
    centerCalls: string[];
    changeCenterBackend: () => void;
  }
}
window.centerCalls = calls;
window.changeCenterBackend = notifyBackendRecovery;
const manifest = raw as Manifest;
let plugin: PluginStatus = {
  manifest,
  digest: "a".repeat(64),
  signingKey: "b".repeat(64),
  enabled: false,
  runtimeStatus: "installed",
  activationEpoch: "1",
  grantRevision: "0",
  sourceAuthorized: false,
  connectionAuthorized: false,
  reportAuthorized: false,
  settings: {},
};
let source: SourceStatus = {
  manifest: (mode === "agent-peers" || mode === "bundled-dsh" || agentParity
    ? dshSource
    : rawSource) as Manifest,
  digest: "c".repeat(64),
  signingKey: "b".repeat(64),
  enabled: mode === "agent-parity-source",
  runtimeActive: mode === "agent-parity-source",
  publisherTrusted: true,
  activationEpoch: "0",
  grantRevision: "0",
  sourceAuthorized: false,
  unifiedAuthorized: false,
  persistMetadata: false,
  freshness: "unread",
};
let sourceInstalled = mode !== "bundled-dsh" && mode !== "agent-parity";
let policy: PolicyStatus = {
  manifest: rawPolicy as Manifest,
  digest: "d".repeat(64),
  signingKey: "b".repeat(64),
  enabled: false,
  active: false,
  publisherTrusted: true,
  epoch: "0",
};
const builtinCatalog: CompositionCatalog = {
  ...structuredClone(emptyBuiltinCatalog),
  impact: {
    activeTasks: mode === "active-task",
    modelServices: ["开发服务", "团队服务"],
  },
  host: {
    ...emptyBuiltinCatalog.host,
    plugins: [
      ...builtinBusinessPlugins(),
      ...(mode === "agent-peers"
        ? [
            ...["claude", "codex", "opencode"].map((instance) => ({
              id: "io.localagent.builtin.source." + instance,
              displayName: instance,
              name: "session.source",
              instance,
            })),
            ...["CLI", "Desktop"].map((client) => ({
              id: "opencode-" + client.toLowerCase(),
              displayName: "OpenCode " + client,
              name: "agent.client-profile",
              instance: "opencode-" + client.toLowerCase(),
            })),
          ].map(({ id, displayName, name, instance }) => ({
            id,
            displayName,
            version: "1.0.0",
            executionKind: "builtin" as const,
            desiredEnabled: true,
            runtimeActive: true,
            activationEpoch: "1",
            capabilities: [
              {
                address: { name, version: 1, instance },
                metadata: {
                  kind: "query" as const,
                  resourceScope: "application" as const,
                },
              },
            ],
            requires: [],
            optionalRequires: [],
            resources: [],
          }))
        : []),
    ],
  },
};
if (mode === "recovery") {
  builtinCatalog.effectiveProfile = "recovery";
  builtinCatalog.composition = {
    profile: "recovery",
    disabledPluginIds: [
      "builtin.agents",
      "builtin.model-services",
      "builtin.providers",
    ],
  };
  builtinCatalog.host.plugins.push({
    id: "project.source",
    displayName: "项目目录",
    version: "1.0.0",
    executionKind: "builtin",
    desiredEnabled: false,
    runtimeActive: false,
    activationEpoch: "1",
    capabilities: [
      {
        address: { name: "project.source", version: 1, instance: "native" },
        metadata: { kind: "query", resourceScope: "project" },
      },
    ],
    requires: [
      { name: "business.feature", version: 1, instance: "builtin.projects" },
    ],
    optionalRequires: [],
    resources: [],
  });
  for (const plugin of builtinCatalog.host.plugins)
    plugin.runtimeActive = plugin.desiredEnabled = false;
  const operation: ProviderOperation = {
    apiVersion: 1,
    sequence: 1,
    status: "pending",
    request: {
      storeId,
      operationId: "10000000-0000-4000-8000-000000000021",
      expectedSequence: 0,
      kind: "save",
      provider: {
        id: "custom-10000000-0000-4000-8000-000000000022",
        name: "待核查服务商",
        endpoint: "https://example.invalid/v1",
        protocol: "responses",
        model: "fixture",
        models: ["fixture"],
        configured: false,
        credentialRef: null,
        credentialLabel: null,
        revision: 0,
        category: "cloud",
      },
    },
    provider: null,
    verification: null,
    error: null,
  };
  providersApi.writeProtocol = async () => ({
    version: 1,
    storeId,
    sequence: 1,
    receiptLimit: 256,
    receiptBytes: 1048576,
    pendingLimit: 4,
    pendingOperationIds: [operation.request.operationId],
  });
  providersApi.pendingOperation = async () => operation;
  providersApi.cancelOperation = async () => ({
    ...operation,
    status: "cancelled",
  });
}
builtinPluginsApi.catalog = async () => structuredClone(builtinCatalog);
builtinPluginsApi.change = async (input) => {
  calls.push("builtin:change");
  builtinCatalog.composition = structuredClone(input.composition);
  builtinCatalog.effectiveProfile = input.composition.profile;
  builtinCatalog.revision = String(BigInt(builtinCatalog.revision) + 1n);
  builtinCatalog.host.generation = crypto.randomUUID();
  const selected = compositionSelection(
    builtinCatalog.host.plugins,
    input.composition,
  );
  for (const plugin of builtinCatalog.host.plugins)
    plugin.runtimeActive = plugin.desiredEnabled = selected.has(plugin.id);
  return {
    receipt: {
      operationId: input.operationId,
      storeId,
      revision: builtinCatalog.revision,
      composition: structuredClone(input.composition),
    },
    catalog: structuredClone(builtinCatalog),
  };
};
pluginsApi.list = async () => ({
  apiVersion: "1.0.0",
  runtime: "fixture",
  developmentMode: false,
  items: [structuredClone(plugin)],
});
fullPluginsApi.list = async () => {
  if (mode === "partial") throw Error("此目录读取失败");
  return {
    apiVersion: "1.0.0",
    items: sourceInstalled
      ? [
          structuredClone(source),
          ...(mode === "agent-parity-versions"
            ? [
                {
                  ...structuredClone(source),
                  digest: "e".repeat(64),
                  manifest: { ...source.manifest, version: "1.1.0" },
                },
              ]
            : []),
        ]
      : [],
  };
};
modelServicesApi.catalog = async () => ({
  services: [],
  attachments: [],
  writeProtocol: { version: 1, storeId, receiptLimit: 100 },
});
routingPolicyApi.inventory = async () => ({
  kind: "inventory",
  storeId,
  packages: [structuredClone(policy)],
  bindings: [],
  scopes: [],
  unavailableServiceIds: [],
  publishes: [],
});
pluginUploadsApi.inspect = async () => {
  calls.push("inspect");
  if (mode === "cancel") return null;
  if (mode === "invalid") throw Error("插件能力不受支持，准入检查未通过");
  const p = mode === "source" ? source : mode === "policy" ? policy : plugin;
  const review = {
    packageRef: "fixture-review",
    digest: p.digest,
    manifest: p.manifest,
    signingKey: p.signingKey!,
    trustedPublisher: false,
  };
  if (mode === "source")
    return {
      kind: "source",
      review: { ...review, compressedBytes: "100", expandedBytes: "200" },
    };
  if (mode === "policy")
    return {
      kind: "policy",
      review: { ...review, compressedBytes: "100", expandedBytes: "200" },
    };
  return {
    kind: "plugin",
    review: {
      ...review,
      compressedBytes: 100,
      expandedBytes: 200,
      development: false,
    },
  };
};
pluginUploadsApi.inspectBundledDsh = async () => {
  calls.push("inspect:bundled-dsh");
  return {
    kind: "source",
    review: {
      packageRef: "fixture-bundled-dsh",
      digest: source.digest,
      manifest: source.manifest,
      signingKey: source.signingKey!,
      trustedPublisher: false,
      compressedBytes: "100",
      expandedBytes: "200",
    },
  };
};
pluginsApi.install = async () => {
  calls.push("plugin:install");
  if (mode === "context-changed") notifyBackendRecovery();
  return structuredClone(plugin);
};
pluginsApi.change = async (input) => {
  calls.push("plugin:" + input.action);
  if (mode === "enable-failed") throw Error("初始化失败");
  plugin = {
    ...plugin,
    enabled: input.action === "enable",
    runtimeStatus: input.action === "enable" ? "active" : "disabled",
  };
  return structuredClone(plugin);
};
fullPluginsApi.install = async () => {
  calls.push("source:install");
  sourceInstalled = true;
  return structuredClone(source);
};
fullPluginsApi.change = async (input) => {
  calls.push("source:" + input.action);
  source = {
    ...source,
    enabled: input.action === "enable",
    runtimeActive: input.action === "enable",
  };
  return structuredClone(source);
};
routingPolicyApi.perform = async (input): Promise<Reply> => {
  calls.push("policy:" + input.kind);
  if (input.kind === "state") {
    policy = {
      ...policy,
      enabled: input.action === "enable",
      active: input.action === "enable",
    };
  }
  return { kind: "package", status: structuredClone(policy) };
};
pluginsApi.extensionHistory = async () => ({ plans: [], unknownPlanIds: [] });
if (agentParity) {
  providersApi.providerStatus = async () => ({
    configs: [],
    transactions: [],
    backupProtection: "fixture",
    verifications: [],
    runtimes: [],
  });
  agentConnectionApi.status = async () => ({ state: "authorization-required" });
  sessionsApi.sessions = async () => ({
    items: [],
    total: 0,
    nextCursor: null,
    summary: null,
    asOf: "2026-11-08T00:00:00Z",
    coverage: "not_connected",
    missingCounts: 0,
  });
  domainApi.authorizeSource = async ({ pluginId, digest }) => {
    if (
      pluginId !== source.manifest.id ||
      digest !== source.digest ||
      !source.runtimeActive
    )
      throw Error("错误的来源授权目标");
    calls.push("source:authorize-file");
    source = {
      ...source,
      sourceAuthorized: true,
      sourceLabel: "fixture.jsonl",
    };
  };
}

function AgentParity() {
  const [hash, setHash] = useState(location.hash);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    const update = () => setHash(location.hash);
    window.addEventListener("hashchange", update);
    return () => window.removeEventListener("hashchange", update);
  }, []);
  const [path, search] = hash.split("?");
  const encodedId = path.split("/")[2];
  const agents: AgentDiscovery[] = agentProducts.slice(-1).map((agent) => ({
    ...agent,
    found: true,
    connected: false,
    location: null,
  }));
  if ((revision || mode === "agent-parity-source") && source.runtimeActive)
    agents.push({
      ...bundledDshAgent,
      found: true,
      connected: source.unifiedAuthorized,
      connectionAdapter: "agent.connection@1",
    });
  const data: Snapshot = {
    agents,
    providers: [],
    sessionSources: [],
    observationBindings: [],
    version: "fixture",
    databasePath: "/fixture/data",
    gatewayStatus: "not_configured",
    settings: { captureContent: false, retentionDays: 30 },
  };
  return (
    <AgentDirectoryProvider products={agents}>
      <AgentsPage
        data={data}
        selectedId={encodedId ? decodeURIComponent(encodedId) : undefined}
        selectedTab={new URLSearchParams(search).get("tab") ?? undefined}
        refresh={async () => setRevision((value) => value + 1)}
      />
    </AgentDirectoryProvider>
  );
}

createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="p-4">
      {agentParity ? (
        <AgentParity />
      ) : (
        <PluginManager
          providers={[]}
          systemFeatures={systemFeatureCatalog(
            [
              ...projectsPlugin.pages.map((page) => ({
                id: projectsPlugin.id,
                page: page.id,
                name: page.label,
                details: page.systemFeature,
              })),
              {
                id: "builtin.model-services",
                page: "model-services",
                name: "模型服务",
              },
              ...(mode === "dependencies" ||
              mode === "active-task" ||
              mode === "recovery"
                ? [
                    {
                      id: "builtin.providers",
                      page: "providers",
                      name: "Providers",
                    },
                    { id: "builtin.agents", page: "agents", name: "Agents" },
                    {
                      id: "builtin.sessions",
                      page: "sessions",
                      name: "Sessions",
                    },
                  ]
                : []),
            ],
            "1.0.0",
          )}
        />
      )}
    </main>
  </IconProvider>,
);
