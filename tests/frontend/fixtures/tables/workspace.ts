import { appApi } from "@/app/api";
import { observationApi } from "@/features/observation/api";
import { settingsApi } from "@/features/settings/api";
import { modelServicesApi } from "@/features/model-services/api";
import type { ModelService } from "@/features/model-services/types";
import type { Provider } from "@/features/providers/types";
import type { GatewayClient } from "@/features/settings/types";
import { requestDashboard, requestRecord } from "../../request-fixture";
import { requests } from "./data";
import {
  installProviderOperationFixture,
  PROVIDER_FIXTURE_STORE_ID as storeId,
} from "../../provider-operation-fixture";
import { fixtureProvider } from "./data";

export const fixtureClient = "7bc3273c-8ca4-4b6b-9de9-5da3876ccbe1";
export const fixtureSecret = `la_${"a".repeat(64)}`;

export function installProviderLifecycleFixture() {
  const standalone = {
    ...fixtureProvider(0),
    name: "独立供应商",
    models: ["model-000"],
  };
  const providers = new Map<string, Provider>([[standalone.id, standalone]]);
  const services: ModelService[] = [];
  const snapshot = appApi.snapshot;
  appApi.snapshot = async () => {
    const original = await snapshot();
    return {
      ...original,
      agents: original.agents.map((agent) => ({
        ...agent,
        found: agent.id === "opencode-cli",
      })),
      providers: [...providers.values()],
      providerWriteProtocol: {
        version: 1,
        storeId,
        sequence: 0,
        receiptLimit: 1024,
        receiptBytes: 32768,
        pendingLimit: 1,
        pendingOperationIds: [],
      },
    };
  };
  window.providerOperationsFixture = installProviderOperationFixture({
    save: async (input) => {
      providers.set(input.id, {
        ...input,
        configured: true,
        revision: input.revision + 1,
      });
    },
    remove: async (input) => {
      if (
        services.some((service) =>
          service.models.some((model) => model.provider.id === input.id),
        )
      )
        throw Error("Unexpected deletion of a referenced provider");
      providers.delete(input.id);
    },
    connect: async () => {
      throw Error("Unexpected automatic connection");
    },
    catalog: async (action) =>
      action.kind === "read"
        ? {
            fetchedAt: new Date().toISOString(),
            sourceUrl: "http://127.0.0.1:9/v1/models",
            models: [
              { id: "test-default", name: "Test Default" },
              { id: "test-alternate", name: "Test Alternate" },
            ],
            truncated: false,
          }
        : null,
  });
  modelServicesApi.catalog = async () => ({
    services: [...services],
    attachments: [],
    writeProtocol: { version: 1, storeId, receiptLimit: 1024 },
  });
  modelServicesApi.verifyMember = async (input) => ({
    providerId: input.providerId,
    model: input.model,
    fingerprint: "fixture",
    state: "verified",
    checkedAt: new Date().toISOString(),
    message: "验证通过",
  });
  modelServicesApi.commit = async ({ action, operationId }) => {
    if (action.kind !== "save") throw Error("Unexpected model service action");
    const draft = action.draft;
    const service: ModelService = {
      id: draft.id,
      versionId: crypto.randomUUID(),
      revision: 1,
      name: draft.name,
      defaultModelId: draft.defaultModelId,
      publishedAt: new Date().toISOString(),
      models: draft.models.map((model) => ({
        id: model.id,
        name: model.name,
        capabilities: model.capabilities,
        provider: { ...providers.get(model.providerId!)!, model: model.model! },
      })),
    };
    services.push(service);
    return {
      apiVersion: 1,
      operationId,
      storeId,
      action: "save",
      serviceId: service.id,
      committedRevision: 1,
      service,
    };
  };
}

declare global {
  interface Window {
    workspaceReads: Record<string, number>;
    workspaceAuditClient: string | undefined;
    workspaceExported: string | undefined;
  }
}

export function installWorkspaceFixture() {
  const snapshot = appApi.snapshot;
  appApi.snapshot = async () => ({
    ...(await snapshot()),
    settingsWriteProtocol: { version: 1, storeId, receiptLimit: 128 },
  });
  const row = requestRecord(requests[0]);
  const requestRows = Array.from({ length: 30 }, (_, index) => ({
    ...row,
    id: `req-${index + 1}`,
    startedAt: new Date(Date.now() - index * 1_000).toISOString(),
    sessionId: index === 0 ? "workspace-linked-session" : null,
    sessionTitle:
      index === 0 ? "关联工作会话" : index === 2 ? fixtureClient : null,
    client:
      index === 1
        ? { id: fixtureClient, name: "已撤销 Mac", peer: "10.0.0.2" }
        : null,
  }));
  window.workspaceReads = {};
  observationApi.requestDetail = async (id) => {
    window.workspaceReads[id] = (window.workspaceReads[id] ?? 0) + 1;
    return {
      request: requestRows.find((row) => row.id === id) ?? null,
      asOf: new Date().toISOString(),
      sourceError: null,
    };
  };
  observationApi.requestDashboard = async (query) => {
    window.workspaceAuditClient = query.client;
    const matching = requestRows.filter(
      (row) => !query.client || row.client?.id === query.client,
    );
    const page = requestDashboard(query, matching);
    const bucket = page.buckets[0];
    return {
      ...page,
      buckets: [
        bucket,
        {
          ...bucket,
          start: bucket.end,
          end: "2026-10-19T02:00:00Z",
          samples: 0,
          value: 0,
        },
      ],
      snapshot: query.client ? `audit-${query.client}` : "workspace-snapshot",
    };
  };
  observationApi.exportRequests = async (id) => {
    window.workspaceExported = id;
    return { status: "saved", count: 1, fileName: "fixture.csv" };
  };
  const clients: GatewayClient[] = [
    {
      id: fixtureClient,
      name: "已撤销 Mac",
      agent: "claude",
      providerId: "anthropic",
      models: ["m"],
      concurrency: 2,
      active: 0,
      enabled: false,
      createdAt: new Date(0).toISOString(),
    },
  ];
  settingsApi.gatewayStatus = async () => ({
    listen: "127.0.0.1:43210",
    persistent: true,
    concurrency: 16,
    active: 0,
    clients: [...clients],
    writeProtocol: {
      version: 1,
      storeId,
      maxClients: 128,
      secretReplay: false,
    },
    clientUsage: {
      [fixtureClient]: {
        requests7d: 1,
        errors7d: 0,
        throttled429Count7d: 0,
        cost7d: {
          actual: [{ amount: 0.45, currency: "USD", calls: 1 }],
          estimated: [],
          unknownCalls: 0,
        },
        usage7d: {
          inputTokens: 100,
          outputTokens: 20,
          totalTokens: 120,
          cachedInputTokens: 0,
          cacheWriteInputTokens: 0,
          reasoningOutputTokens: null,
        },
        lastPeer: "10.0.0.2",
      },
    },
  });
  settingsApi.saveGatewayClientOperation = async ({ request }) => {
    if (request.action.kind !== "create") throw Error("unexpected action");
    clients.push({
      ...request.action.draft,
      id: request.operationId,
      enabled: true,
      active: 0,
      createdAt: new Date().toISOString(),
    });
    return {
      status: "receipt",
      key: fixtureSecret,
      sourceInstanceId: storeId,
      sourceCurrent: true,
      outcome: {
        request,
        receipt: {
          apiVersion: 1,
          storeId,
          operationId: request.operationId,
          clientId: request.operationId,
          action: "create",
          listen: "127.0.0.1:43210",
          completedAt: new Date().toISOString(),
        },
      },
    };
  };
}
