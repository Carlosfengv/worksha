import { builtinPluginsApi, fullPluginsApi } from "@/features/plugins/api";
import {
  modelServicesApi,
  routingPolicyApi,
} from "@/features/model-services/api";
import type { CompositionCatalog } from "../../contracts/plugins/builtin/types";
export const emptyBuiltinCatalog: CompositionCatalog = {
  schemaVersion: 1,
  storeId: "10000000-0000-4000-8000-000000000001",
  revision: "0",
  composition: { profile: "desktop", disabledPluginIds: [] },
  effectiveProfile: "desktop",
  host: {
    generation: "20000000-0000-4000-8000-000000000001",
    cleanupFailed: false,
    plugins: [],
  },
  problem: null,
};
export function setupPluginCenterPeers() {
  builtinPluginsApi.catalog = async () => structuredClone(emptyBuiltinCatalog);
  fullPluginsApi.list = async () => ({ apiVersion: "1.0.0", items: [] });
  modelServicesApi.catalog = async () => ({
    services: [],
    attachments: [],
    writeProtocol: {
      version: 1,
      storeId: emptyBuiltinCatalog.storeId,
      receiptLimit: 100,
    },
  });
  routingPolicyApi.inventory = async () => ({
    kind: "inventory",
    storeId: emptyBuiltinCatalog.storeId,
    packages: [],
    bindings: [],
    scopes: [],
    publishes: [],
    unavailableServiceIds: [],
  });
}

export function builtinBusinessPlugins(): CompositionCatalog["host"]["plugins"] {
  return [
    ["builtin.providers", "Providers", []],
    ["builtin.agents", "Agents", ["builtin.providers"]],
    ["builtin.model-services", "模型服务", ["builtin.providers"]],
    ["builtin.sessions", "Sessions", []],
    ["builtin.requests", "请求", []],
    ["builtin.overview", "总览", ["builtin.sessions", "builtin.requests"]],
    ["builtin.mcp", "MCP", ["builtin.sessions"]],
  ].map(([id, displayName, deps]) => ({
    id: id as string,
    displayName: displayName as string,
    version: "1.0.0",
    executionKind: "builtin",
    desiredEnabled: true,
    runtimeActive: true,
    activationEpoch: "1",
    capabilities: [
      {
        address: {
          name: "business.feature",
          version: 1,
          instance: id as string,
        },
        metadata: {
          kind: "command" as const,
          resourceScope: "application" as const,
        },
      },
    ],
    requires: (deps as string[]).map((id) => ({
      name: "business.feature",
      version: 1,
      instance: id,
    })),
    optionalRequires: [],
    resources: [],
    problem: null,
  }));
}
