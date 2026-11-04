import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { AgentProviderSettings } from "@/features/providers/components/provider-config-control";
import { providersApi } from "@/features/providers/api";
import { modelServicesApi } from "@/features/model-services/api";
import { defaultCapabilities } from "@/features/model-services/types";
import type { Provider, ProviderStatus } from "@/features/providers/types";
import { ProviderActivationStatus } from "@/features/providers/components/provider-activation-status";
import { agentProducts } from "@/shared/types/agent";
import { AgentList } from "@/features/agents/components/agent-list";
import type { Snapshot } from "@/app/types";
import "@/styles/globals.css";

const provider: Provider = {
  id: "target-provider",
  name: "目标供应商",
  category: "custom",
  endpoint: "http://127.0.0.1:9/v1",
  protocol: "messages",
  model: "new-model",
  models: ["new-model"],
  configured: true,
  credentialRef: null,
  credentialLabel: null,
  revision: 1,
};
const grouped = new URLSearchParams(location.search).has("group");
const list = new URLSearchParams(location.search).has("list");
const dynamic = new URLSearchParams(location.search).has("dynamic");
const verifiedModels = new Set<string>();
if (grouped) provider.models.push("alternate-model");
const product =
  agentProducts.find(
    (p) => p.id === new URLSearchParams(location.search).get("agent"),
  ) ?? agentProducts[4];
const config = {
  agentId: product.configId,
  path: "/test/opencode.json",
  state: "ready",
  revision: "old-revision",
  mode: "api",
  endpoint: "http://127.0.0.1:9/old",
  model: "old-model",
  providerName: "原供应商",
  credential: "隔离测试配置",
  error: null,
  warnings: [],
};
const transaction: import("@/features/providers/types").ProviderTransaction = {
  id: "provider-transaction",
  agentId: product.configId,
  state: "pending",
  createdAt: "2026-10-19T03:00:00Z",
  appliedAt: null as string | null,
  before: { ...config },
  targetName: "目标供应商 · new-model",
  provider,
  model: "new-model",
  backupPath: "/test/backup.sealed",
  restoreOf: null,
  error: null,
};
let transactionPrepared =
  !list || new URLSearchParams(location.search).has("pending");
const binding: import("@/features/providers/types").DynamicBinding = {
  id: "fixture-binding",
  agentId: product.configId,
  scope: config.path,
  localEndpoint: "http://127.0.0.1:9/gateway/fixture-binding",
  credentialRef: "fixture-reference",
  generation: "1",
  createdAt: transaction.createdAt,
  target: {
    id: "version-one",
    bindingId: "fixture-binding",
    createdAt: transaction.createdAt,
    provider: { ...provider, name: "原供应商", model: "old-model" },
  },
};
if (dynamic) {
  config.endpoint = binding.localEndpoint;
  config.model = "local-agent-default";
  transaction.state = "applied";
  transaction.provider = binding.target.provider;
  transaction.model = "old-model";
  transaction.targetName = "原供应商 · old-model";
  transaction.routing = {
    id: binding.id,
    clientProtocol: "messages",
    upstreamProtocol: "messages",
    localEndpoint: binding.localEndpoint,
    models: [],
  };
}
const dynamicChanges: import("@/features/providers/types").DynamicChange[] = [];
if (grouped)
  transaction.routing = {
    id: "group-route",
    clientProtocol: "messages",
    upstreamProtocol: "messages",
    localEndpoint: "http://127.0.0.1:9/gateway/group",
    models: ["new-model", "alternate-model"],
  };
declare global {
  interface Window {
    providerSwitchFixture: {
      processes: number | null;
      failNext: boolean;
      actions: string[];
      finish: (() => void) | null;
      verified: boolean;
      failModel: string | null;
      readFailure: boolean;
      runtimeFailure: string | null;
    };
  }
}
const fixture = (window.providerSwitchFixture = {
  processes: new URLSearchParams(location.search).has("ready") ? 0 : 2,
  failNext: false,
  actions: [] as string[],
  finish: null as (() => void) | null,
  verified: !new URLSearchParams(location.search).has("expired"),
  failModel: null as string | null,
  readFailure: false,
  runtimeFailure: null as string | null,
});
const configs = list
  ? [...new Set(agentProducts.map((p) => p.configId))]
      .filter((id) => id !== "antigravity")
      .map((agentId) => ({ ...config, agentId }))
  : [config];
modelServicesApi.catalog = async () => ({
  writeProtocol: {
    version: 1,
    storeId: "00000000-0000-4000-8000-000000000001",
    receiptLimit: 256,
  },
  services: [
    {
      id: "fixture-service",
      versionId: "fixture-service-v1",
      revision: 1,
      name: "编程服务",
      defaultModelId: "fixture-model",
      models: [
        {
          id: "fixture-model",
          name: "默认编程模型",
          provider,
          capabilities: defaultCapabilities(),
        },
      ],
      publishedAt: transaction.createdAt,
    },
  ],
  attachments: [],
});
providersApi.providerStatus = async (): Promise<ProviderStatus> => {
  if (fixture.readFailure) throw new Error("隔离配置读取失败");
  return structuredClone({
    localTokenProtocol: {
      version: 2,
      schemaVersion: 15,
      storeId: "00000000-0000-4000-8000-000000000001",
      namespaceId: "00000000-0000-4000-8000-000000000002",
      namespaceState: "active",
      operationLimit: 4096,
      pendingLimit: 4,
      cancellationLimit: 4096,
      pending: new URLSearchParams(location.search).has("local-pending")
        ? [
            {
              version: 1,
              status: "pending",
              request: {
                identity: {
                  storeId: "00000000-0000-4000-8000-000000000001",
                  namespaceId: "00000000-0000-4000-8000-000000000002",
                  operationId: "00000000-0000-4000-8000-000000000003",
                },
                purpose: "dynamic_binding",
                agentId: "claude",
                configurationPath: "/fixture/.claude/settings.json",
                configurationPathHash: "a".repeat(64),
                expectedRevision: "a".repeat(64),
                provider,
                service: null,
              },
              ownerId: "00000000-0000-4000-8000-000000000003",
              credentialRef: "00000000-0000-4000-8000-000000000004",
              ownerSaved: false,
              previewId: null,
            },
          ]
        : [],
    },
    configs,
    transactions: transactionPrepared ? [transaction] : [],
    dynamicBindings: dynamic ? [binding] : [],
    dynamicChanges,
    backupProtection: "隔离配置副本",
    verifications: provider.models.map((modelId) => ({
      providerId: provider.id,
      model: modelId,
      fingerprint: "fixture",
      state:
        fixture.verified || verifiedModels.has(modelId)
          ? "verified"
          : "expired",
      checkedAt: "2026-10-19T03:00:00Z",
      message: "fixture",
    })),
    runtimes: configs.map((c) => ({
      agentId: c.agentId,
      version: "1.18.13",
      supported: fixture.runtimeFailure === null,
      processes: fixture.processes,
      reason: fixture.runtimeFailure ?? "",
    })),
  });
};
providersApi.prepareDynamicRoute = async (input) => {
  fixture.actions.push(`dynamic-prepare:${input.model}`);
  const c: import("@/features/providers/types").DynamicChange = {
    id: input.operationId,
    bindingId: binding.id,
    expectedGeneration: input.expectedGeneration,
    state: "pending",
    before: structuredClone(binding.target),
    target: { ...provider, model: input.model },
    published: null,
    createdAt: new Date().toISOString(),
  };
  dynamicChanges.unshift(c);
  return structuredClone(c);
};
providersApi.dynamicRouteAction = async (change, action) => {
  fixture.actions.push(`dynamic-${action}`);
  if (fixture.failNext) {
    fixture.failNext = false;
    throw new Error("目标版本已变化，请重新预览");
  }
  const c = dynamicChanges.find((c) => c.id === change.id)!;
  if (action === "commit") {
    c.state = "applied";
    c.published = {
      id: "version-two",
      bindingId: binding.id,
      provider: c.target,
      createdAt: c.createdAt,
    };
    binding.target = c.published;
    binding.generation = "2";
  }
  if (action === "cancel") c.state = "cancelled";
  return structuredClone(c);
};
providersApi.prepareProviderSwitch = async (
  agentId,
  revision,
  providerId,
  _providerRevision,
  model,
) => {
  fixture.actions.push(`prepare:${agentId}:${providerId}:${model}`);
  const before = configs.find((c) => c.agentId === agentId);
  if (!before || before.revision !== revision)
    throw new Error("配置版本已变化");
  transaction.agentId = agentId;
  transactionPrepared = true;
  transaction.state = "pending";
  transaction.before = { ...before };
  transaction.provider = providerId ? provider : null;
  transaction.model = model;
  return structuredClone(transaction);
};
providersApi.providerSwitchAction = async (_id, action) => {
  fixture.actions.push(action);
  if (fixture.failNext) {
    fixture.failNext = false;
    throw new Error("应用前发现 Agent 已重新启动，配置尚未写入");
  }
  if (action === "apply") {
    if (fixture.processes !== 0) {
      throw new Error("相关 Agent 仍在运行或进程状态无法确认，配置尚未写入");
    }
    await new Promise<void>((resolve) => {
      fixture.finish = resolve;
    });
    config.providerName = provider.name;
    config.model = provider.model;
    config.endpoint = provider.endpoint;
    transaction.state = "applied";
    transaction.appliedAt = "2026-10-19T03:01:00Z";
    transaction.activation = {
      state: "awaiting_restart",
      updatedAt: transaction.appliedAt,
    };
  } else if (action === "cancel") {
    transaction.state = "cancelled";
  } else if (action === "defer") {
    transaction.activation = {
      state: "deferred",
      updatedAt: "2026-10-19T03:01:00Z",
    };
  }
  return structuredClone(transaction);
};
providersApi.restartAgentProvider = async () => {
  fixture.actions.push("restart");
  if (fixture.failNext || fixture.processes == null || fixture.processes > 1) {
    fixture.failNext = false;
    throw new Error("还有其他相关进程，请先退出 CLI；配置未修改，桌面端未退出");
  }
  await new Promise<void>((resolve) => {
    fixture.finish = resolve;
  });
  config.providerName = provider.name;
  config.model = provider.model;
  config.endpoint = provider.endpoint;
  transaction.state = "applied";
  transaction.activation = {
    state: "launch_requested",
    updatedAt: "2026-10-19T03:01:00Z",
  };
  return structuredClone(transaction);
};
providersApi.verifyProvider = async (
  _id,
  _revision,
  model = provider.model,
) => {
  fixture.actions.push(grouped ? `verify:${model}` : "verify");
  if (fixture.failNext || fixture.failModel === model) {
    fixture.failNext = false;
    throw new Error("连接暂不可用，配置未修改");
  }
  verifiedModels.add(model);
  fixture.verified = provider.models.every((id) => verifiedModels.has(id));
  return {
    providerId: provider.id,
    model,
    fingerprint: "fixture",
    state: "verified",
    checkedAt: new Date().toISOString(),
    message: "verified",
  };
};
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <main className="mx-auto max-w-5xl p-4">
      {list ? (
        <AgentList
          data={
            {
              agents: [
                ...agentProducts.map((p) => ({
                  ...p,
                  found: true,
                  location: "/test",
                  connected: false,
                })),
                ...Array.from({ length: 6 }, (_, index) => ({
                  id: `extra-${index}`,
                  name: `只读 Agent ${index}`,
                  family: `extra-${index}`,
                  configId: `extra-${index}`,
                  found: false,
                  location: null,
                  connected: false,
                })),
              ],
              providers: [
                provider,
                {
                  ...provider,
                  id: "disabled-provider",
                  name: "模型已停用",
                  disabledModels: [provider.model],
                },
                {
                  ...provider,
                  id: "unconfigured",
                  name: "未配置供应商",
                  configured: false,
                },
              ],
              sessionSources: [],
              observationBindings: [],
            } as unknown as Snapshot
          }
        />
      ) : (
        <>
          <ProviderActivationStatus agentId={product.configId} />
          <AgentProviderSettings
            agentId={product.configId}
            agentProductId={product.id}
            providers={[provider]}
            refresh={async () => {}}
          />
        </>
      )}
    </main>
  </IconProvider>,
);
