import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { systemFeatureCatalog } from "@/features/plugins/system-features";
import { PluginManager } from "@/features/plugins/manager";
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
  manifest: (mode === "agent-peers" ? dshSource : rawSource) as Manifest,
  digest: "c".repeat(64),
  signingKey: "b".repeat(64),
  enabled: false,
  runtimeActive: false,
  publisherTrusted: true,
  activationEpoch: "0",
  grantRevision: "0",
  sourceAuthorized: false,
  unifiedAuthorized: false,
  persistMetadata: false,
  freshness: "unread",
};
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
        ? ["claude", "codex", "opencode"].map((instance) => ({
            id: "io.localagent.builtin.source." + instance,
            displayName: instance,
            version: "1.0.0",
            executionKind: "builtin" as const,
            desiredEnabled: true,
            runtimeActive: true,
            activationEpoch: "1",
            capabilities: [
              {
                address: { name: "session.source", version: 1, instance },
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
builtinPluginsApi.catalog = async () => structuredClone(builtinCatalog);
builtinPluginsApi.change = async (input) => {
  calls.push("builtin:change");
  builtinCatalog.composition = structuredClone(input.composition);
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
  return { apiVersion: "1.0.0", items: [structuredClone(source)] };
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
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="p-4">
      <PluginManager
        providers={[]}
        systemFeatures={systemFeatureCatalog(
          [
            {
              id: "builtin.model-services",
              page: "model-services",
              name: "模型服务",
            },
            ...(mode === "dependencies" || mode === "active-task"
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
    </main>
  </IconProvider>,
);
