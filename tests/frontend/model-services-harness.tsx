import { DesktopError } from "@/shared/lib/desktop-command";
import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
import {
  serviceReceipt,
  type ServiceRequest,
  type ServiceReceipt,
} from "@/features/model-services/operation-receipt";
import { createRoot } from "react-dom/client";
import { IconProvider } from "@/lib/icon-context";
import { ModelServicesPage } from "@/features/model-services/page";
import { ModelServiceSettings } from "@/features/model-services/components/agent-service-control";
import { modelServicesApi } from "@/features/model-services/api";
import {
  defaultCapabilities,
  defaultRoutingPolicy,
  type ModelService,
  type ModelServiceCatalog,
} from "@/features/model-services/types";
import { providersApi } from "@/features/providers/api";
import type {
  NativeConfig,
  Provider,
  ProviderStatus,
  ProviderTransaction,
  UpstreamRecoveryState,
} from "@/features/providers/types";
import "@/styles/globals.css";

const sources: Provider[] = ["Alpha", "Beta"].map((name, i) => ({
  id: `source-${name}`,
  name,
  category: "local",
  endpoint: "http://127.0.0.1:9",
  protocol: "messages",
  model: i ? "b" : "a",
  models: [i ? "b" : "a"],
  configured: true,
  credentialRef: null,
  credentialLabel: null,
  revision: 1,
}));
const service: ModelService = {
  id: "fixture-service",
  versionId: "fixture-version-one",
  revision: 1,
  name: "编程服务",
  defaultModelId: "model-pro",
  publishedAt: "2026-10-26T00:00:00Z",
  routingPolicy: defaultRoutingPolicy(),
  models: [
    {
      id: "model-pro",
      name: "Pro",
      provider: sources[0],
      capabilities: defaultCapabilities(),
    },
    {
      id: "model-flash",
      name: "Flash",
      provider: sources[1],
      capabilities: { ...defaultCapabilities(), contextWindow: 96000 },
    },
  ],
};
const params = new URLSearchParams(location.search);
if (params.has("nested")) {
  service.id = "620d7132-b3f3-4e56-8f5a-2c06c8282a01";
  service.versionId = "70130e70-ef47-4cd7-9b01-2da76b6101d5";
}
const agent = params.get("agent") ?? "opencode";
const original: NativeConfig = {
  agentId: agent as NativeConfig["agentId"],
  path: "/fixture/settings.json",
  state: "ready",
  revision: "original",
  mode: "native",
  endpoint: null,
  model: null,
  providerName: "原生配置",
  credential: "fixture",
  error: null,
  warnings: [],
};
const config = { ...original };
declare global {
  interface Window {
    modelServiceFixture: {
      actions: string[];
      failSave: boolean;
      holdVerification: boolean;
      releaseVerification: () => void;
      unmountView: () => void;
      loseCommitReply: boolean;
      dropBeforeCommit: boolean;
      failOperationRead: boolean;
      operationAttempts: string[];
      queryAttempts: number;
      receipts: Map<string, { request: string; receipt: ServiceReceipt }>;
      parseReceipt: (value: unknown, request: ServiceRequest) => ServiceReceipt;
      failRead: boolean;
      routingProjectVisible: boolean;
      revokeRoutingProject: () => void;
      failApply: boolean;
      previewQuota: string;
      previewRecovery: UpstreamRecoveryState | null;
      processes: number;
      inUse: boolean;
      catalog: ModelServiceCatalog;
      transactions: ProviderTransaction[];
      config: NativeConfig;
    };
  }
}
const fixture = (window.modelServiceFixture = {
  actions: [] as string[],
  failSave: false,
  holdVerification: false,
  releaseVerification: () => {},
  unmountView: () => {},
  loseCommitReply: false,
  dropBeforeCommit: false,
  failOperationRead: false,
  operationAttempts: [] as string[],
  queryAttempts: 0,
  receipts: new Map<string, { request: string; receipt: ServiceReceipt }>(),
  parseReceipt: serviceReceipt,
  failRead: false,
  routingProjectVisible: true,
  revokeRoutingProject() {
    this.routingProjectVisible = false;
    invalidateCapabilityAccess();
  },
  failApply: false,
  previewQuota: "recovering",
  previewRecovery: null as UpstreamRecoveryState | null,
  processes: params.has("running") ? 2 : 0,
  inUse: false,
  catalog: {
    writeProtocol: params.has("legacy-write")
      ? undefined
      : {
          version: 1,
          storeId: "00000000-0000-4000-8000-000000000001",
          receiptLimit: 256,
        },
    services: params.has("empty") ? [] : [service],
    attachments: [],
  } as ModelServiceCatalog,
  transactions: [] as ProviderTransaction[],
  config,
});
modelServicesApi.catalog = async () => {
  if (fixture.failRead) throw new Error("服务目录读取失败");
  return structuredClone(fixture.catalog);
};
modelServicesApi.routingProjects = async ({ query, selectedId }) => {
  const project = {
    id: `project:${"2".repeat(64)}`,
    name: "OpenCode 2.x 项目",
    path: "/fixture/opencode-v2",
    projectKind: "workspace" as const,
  };
  return {
    items:
      fixture.routingProjectVisible &&
      `${project.name} ${project.path}`
        .toLowerCase()
        .includes(query?.toLowerCase() ?? "")
        ? [project]
        : [],
    nextCursor: null,
    selected:
      fixture.routingProjectVisible && selectedId === project.id
        ? project
        : null,
  };
};
modelServicesApi.save = async (draft) => {
  fixture.actions.push("save");
  if (fixture.failSave) {
    fixture.failSave = false;
    throw new DesktopError({
      code: "conflict",
      outcomeUnknown: false,
      message: "模型服务已被修改，请重新读取后保存",
    });
  }
  const current = fixture.catalog.services.find((s) => s.id === draft.id);
  if ((current?.revision ?? 0) !== draft.revision) throw new Error("版本冲突");
  const saved: ModelService = {
    ...draft,
    revision: draft.revision + 1,
    versionId: crypto.randomUUID(),
    publishedAt: new Date().toISOString(),
    models: draft.models.map((m) => {
      const child = fixture.catalog.services.find(
        (service) => service.id === m.child?.serviceId,
      );
      return {
        id: m.id,
        name: m.name,
        capabilities: m.capabilities,
        ...(m.child ? { child: m.child } : {}),
        provider: child
          ? child.models.find((member) => member.id === child.defaultModelId)!
              .provider
          : { ...sources.find((p) => p.id === m.providerId)!, model: m.model },
      };
    }),
  };
  fixture.catalog.services = [
    ...fixture.catalog.services.filter((s) => s.id !== saved.id),
    saved,
  ];
  return structuredClone(saved);
};
modelServicesApi.delete = async (id) => {
  fixture.actions.push("delete");
  if (fixture.inUse)
    throw new DesktopError({
      code: "in_use",
      outcomeUnknown: false,
      message: "此服务仍被 Agent 接入，请先切换服务或恢复原配置",
    });
  fixture.catalog.services = fixture.catalog.services.filter(
    (s) => s.id !== id,
  );
  return { deleted: true };
};
const BACKEND = "fixture.model-service.backend";
if (params.has("write-recovery")) {
  const raw = sessionStorage.getItem(BACKEND);
  if (raw) {
    const state = JSON.parse(raw);
    fixture.catalog = state.catalog;
    fixture.receipts = new Map(state.receipts);
  }
}
const persistBackend = () => {
  if (params.has("write-recovery"))
    sessionStorage.setItem(
      BACKEND,
      JSON.stringify({
        catalog: fixture.catalog,
        receipts: [...fixture.receipts],
      }),
    );
};
modelServicesApi.commit = async (request) => {
  if (request.storeId !== fixture.catalog.writeProtocol?.storeId)
    throw new DesktopError({
      code: "operation_store_changed",
      message: "操作属于另一个数据库",
    });
  fixture.operationAttempts.push(request.operationId);
  const prior = fixture.receipts.get(request.operationId);
  if (prior) {
    if (prior.request !== JSON.stringify(request))
      throw new DesktopError({
        code: "operation_conflict",
        message: "操作编号已用于其他内容",
      });
    return serviceReceipt(structuredClone(prior.receipt), request);
  }
  if (fixture.dropBeforeCommit) {
    fixture.dropBeforeCommit = false;
    throw new DesktopError({
      code: "transport_connection",
      outcomeUnknown: true,
      message: "提交回复丢失",
    });
  }
  const service =
    request.action.kind === "save"
      ? await modelServicesApi.save(request.action.draft)
      : null;
  if (request.action.kind === "delete")
    await modelServicesApi.delete(
      request.action.serviceId,
      request.action.expectedRevision,
    );
  const receipt: ServiceReceipt = {
    storeId: request.storeId,
    apiVersion: 1,
    operationId: request.operationId,
    action: request.action.kind,
    serviceId:
      request.action.kind === "save"
        ? request.action.draft.id
        : request.action.serviceId,
    committedRevision:
      request.action.kind === "save"
        ? request.action.draft.revision + 1
        : request.action.expectedRevision + 1,
    service: service
      ? {
          ...service,
          routingPolicy: service.routingPolicy ?? defaultRoutingPolicy(),
        }
      : null,
  };
  fixture.receipts.set(request.operationId, {
    request: JSON.stringify(request),
    receipt,
  });
  persistBackend();
  if (fixture.loseCommitReply) {
    fixture.loseCommitReply = false;
    throw new DesktopError({
      code: "transport_connection",
      outcomeUnknown: true,
      message: "提交回复丢失",
    });
  }
  return serviceReceipt(structuredClone(receipt), request);
};
modelServicesApi.operation = async (request) => {
  fixture.queryAttempts++;
  if (request.storeId !== fixture.catalog.writeProtocol?.storeId)
    throw new DesktopError({
      code: "operation_store_changed",
      message: "操作属于另一个数据库",
    });
  if (fixture.failOperationRead) throw new Error("回执暂不可读取");
  const found = fixture.receipts.get(request.operationId);
  if (!found)
    throw new DesktopError({
      code: "operation_not_found",
      message: "尚未查到已提交回执",
    });
  return serviceReceipt(structuredClone(found.receipt), request);
};
modelServicesApi.prepareConnection = async (input) => {
  fixture.actions.push("prepare-service");
  const selected = fixture.catalog.services.find(
    (s) => s.id === input.serviceId,
  )!;
  const t: ProviderTransaction = {
    id: crypto.randomUUID(),
    agentId: original.agentId,
    state: "pending",
    createdAt: new Date().toISOString(),
    appliedAt: null,
    before: { ...config },
    targetName: `模型服务 · ${selected.name}`,
    provider: sources[0],
    model: "a",
    backupPath: "/fixture/backup.sealed",
    restoreOf: null,
    error: null,
    routing: {
      id: crypto.randomUUID(),
      clientProtocol: "messages",
      upstreamProtocol: "messages",
      localEndpoint: "http://127.0.0.1:9/gateway/fixture",
      models: [],
    },
  };
  fixture.catalog.attachments.push({
    bindingId: t.routing!.id,
    serviceId: selected.id,
    installed: structuredClone(selected),
    agentId: agent,
    localEndpoint: t.routing!.localEndpoint,
    attached: false,
    referenced: false,
    configurationUnknown: false,
    directorySyncRequired: false,
  });
  fixture.transactions.push(t);
  return structuredClone(t);
};
const verifications = new Map<
  string,
  ProviderStatus["verifications"][number]
>();
const verifyFixture = async (
  id: string,
  model?: string,
  hasAccount = false,
) => {
  fixture.actions.push(`verify:${id}:${model}`);
  if (fixture.holdVerification)
    await new Promise<void>((resolve) => {
      fixture.releaseVerification = () => {
        fixture.holdVerification = false;
        resolve();
      };
    });
  const needsAccount =
    params.has("require-account") && id === "source-Alpha" && !hasAccount;
  const result = {
    providerId: id,
    model,
    fingerprint: "fixture",
    state: needsAccount ? ("failed" as const) : ("verified" as const),
    checkedAt: new Date().toISOString(),
    message: needsAccount ? "测试来源要求使用已发布的账号" : "短请求验证通过",
  };
  verifications.set(`${id}:${model}`, result);
  return result;
};
providersApi.verifyProvider = (id, _revision, model) =>
  verifyFixture(id, model);
modelServicesApi.verifyMember = async (input) => {
  if (input.storeId !== fixture.catalog.writeProtocol?.storeId)
    throw new DesktopError({
      code: "operation_store_changed",
      message: "数据库已切换",
    });
  if (input.upstreamAccount)
    fixture.actions.push(`account:${input.upstreamAccount.id}`);
  return verifyFixture(input.providerId, input.model, !!input.upstreamAccount);
};
const fixtureUpstreamAccounts = [
  {
    id: "a8259fd0-66d1-493f-ac6f-12abc5437890",
    label: "主账号",
    state: "enabled",
  },
  {
    id: "b8259fd0-66d1-493f-ac6f-12abc5437890",
    label: "备用账号",
    state: "enabled",
  },
  {
    id: "c8259fd0-66d1-493f-ac6f-12abc5437890",
    label: "暂停账号",
    state: "paused",
  },
] as const;
if (params.has("require-account")) {
  service.routingPolicy = {
    ...defaultRoutingPolicy(),
    enabled: true,
    accountPools: [
      {
        modelId: service.defaultModelId,
        accounts: fixtureUpstreamAccounts
          .filter((account) => account.state === "enabled")
          .map(({ id }) => ({ id, revision: 1 })),
      },
    ],
  };
}
providersApi.upstreamAccount = async (providerId) => ({
  storeId: fixture.catalog.writeProtocol!.storeId,
  accounts:
    params.has("upstream-accounts") && providerId === "source-Alpha"
      ? fixtureUpstreamAccounts.map((a) => ({ ...a, revision: 1, providerId }))
      : [],
});
modelServicesApi.preview = async ({ storeId, draft, entry }) => {
  fixture.actions.push(`preview:${entry.kind}`);
  const preferred =
    entry.kind === "specified" ? entry.modelId : draft.defaultModelId;
  return {
    storeId,
    checkedAt: "2026-10-26T00:16:00Z",
    entryState: "configured",
    models: draft.models.map((model) => ({
      id: model.id,
      name: model.name,
      scope:
        model.id === preferred
          ? "preferred"
          : entry.kind === "specified"
            ? "capabilityMismatch"
            : draft.routingPolicy?.fallbackAcrossModels
              ? "fallback"
              : "outsideFallback",
      source: null,
      model: model.model,
      state: "configured",
      accounts:
        model.id === "model-pro"
          ? [
              {
                label: "主账号",
                position: 1,
                state: "configured",
                verification: "expired",
                verifiedAt: "2026-10-26T00:00:00Z",
                verifyOnPublish: true,
                cooldownKnown: true,
                cooldown: fixture.previewRecovery
                  ? null
                  : {
                      reason: "upstream_rate_limit",
                      untilMs: Date.parse("2026-10-26T00:18:00Z"),
                    },
                recovery: fixture.previewRecovery
                  ? {
                      reason: "upstream_rate_limit",
                      untilMs: Date.parse("2026-10-26T00:18:00Z"),
                      state: fixture.previewRecovery,
                    }
                  : null,
                quota: fixture.previewQuota,
              },
              {
                label: "备用账号",
                position: 2,
                state: "paused",
                verification: "unverified",
                verifiedAt: null,
                verifyOnPublish: false,
                cooldownKnown: true,
                cooldown: null,
                quota: null,
              },
            ]
          : [],
      child: null,
    })),
  };
};
providersApi.providerStatus = async (): Promise<ProviderStatus> => ({
  localTokenProtocol: {
    version: 2,
    schemaVersion: 16,
    storeId:
      fixture.catalog.writeProtocol?.storeId ??
      "00000000-0000-4000-8000-000000000001",
    namespaceId: "00000000-0000-4000-8000-000000000002",
    namespaceState: "active",
    operationLimit: 4096,
    pendingLimit: 4,
    cancellationLimit: 4096,
    pending: [],
  },
  configs: [{ ...config }],
  transactions: structuredClone(fixture.transactions),
  backupProtection: "fixture",
  runtimes: [
    {
      agentId: original.agentId,
      version: "1.18.13",
      supported: true,
      processes: fixture.processes,
      reason: "fixture",
    },
  ],
  verifications: [...verifications.values()],
});
providersApi.providerSwitchAction = async (id, action) => {
  fixture.actions.push(action);
  const t = fixture.transactions.find((t) => t.id === id)!;
  if (action === "apply") {
    if (fixture.failApply || fixture.processes > 0) {
      fixture.failApply = false;
      throw new Error("Agent 已重新启动，配置未写入");
    }
    t.state = "applied";
    if (t.restoreOf) {
      Object.assign(config, original);
      fixture.catalog.attachments = [];
    } else {
      Object.assign(config, {
        revision: "installed",
        model: "local-agent-default",
        endpoint:
          t.routing!.localEndpoint + (agent === "opencode" ? "/v1" : ""),
      });
      fixture.catalog.attachments = [
        {
          bindingId: t.routing!.id,
          serviceId: service.id,
          installed: service,
          agentId: agent,
          localEndpoint: config.endpoint!,
          attached: true,
          referenced: true,
          configurationUnknown: false,
          directorySyncRequired: false,
        },
      ];
    }
  } else if (action === "cancel") t.state = "cancelled";
  else if (action === "restore") {
    const restore = {
      ...t,
      id: crypto.randomUUID(),
      state: "pending",
      targetName: "恢复服务接入前配置",
      restoreOf: t.id,
    };
    fixture.transactions.push(restore);
    return structuredClone(restore);
  }
  return structuredClone(t);
};
if (params.has("pending")) {
  await modelServicesApi.prepareConnection({
    agentId: agent,
    expectedRevision: config.revision!,
    serviceId: service.id,
    expectedServiceRevision: service.revision,
  });
  fixture.actions.length = 0;
  if (params.has("legacy")) fixture.catalog.attachments = [];
}
const root = createRoot(document.getElementById("root")!);
fixture.unmountView = () => root.unmount();
root.render(
  <IconProvider>
    <main className="mx-auto max-w-5xl p-4">
      {params.has("agent") ? (
        <ModelServiceSettings
          agentId={agent}
          name={agent === "claude" ? "Claude Code CLI" : "OpenCode"}
          providers={sources}
        />
      ) : (
        <ModelServicesPage providers={sources} />
      )}
    </main>
  </IconProvider>,
);
