import {
  localConnectionsApi,
  type LocalBinding,
  type BudgetAttempt,
} from "@/features/agents/api";
import { modelServicesApi } from "@/features/model-services/api";

const storeId = "test-store";
const serviceId = "published-service";
const bindingId = "owned-binding";
const attempt: BudgetAttempt = {
  attemptKey: "attempt-one",
  observationId: "request-one",
  bindingId,
  revision: 2,
  createdAt: "2026-11-17T00:00:00Z",
  model: "fixture-model",
  providerId: "fixture-upstream",
  upstreamRequestId: "verified-request",
  tier: "primary",
  frozen: { tokens: 200, usdMicros: null },
  windows: [
    {
      kind: "daily",
      key: "2026-11-17",
      reserved: { tokens: 200, usdMicros: null },
    },
  ],
};

export function installLocalConnectionFixture() {
  let binding: LocalBinding | null = null;
  let waiting = true;
  modelServicesApi.catalog = async () => ({
    services: [
      {
        id: serviceId,
        versionId: "version-one",
        revision: 1,
        name: "测试模型服务",
        defaultModelId: "default-model",
        models: [],
        publishedAt: "2026-11-17T00:00:00Z",
      },
    ],
    attachments: [],
  });
  localConnectionsApi.list = async () => ({
    storeId,
    items: binding ? [binding] : [],
    pending: [],
  });
  localConnectionsApi.budgets = async () => ({
    storeId,
    items: binding
      ? [
          {
            bindingId,
            agentId: "local-chat-completions",
            model: "fixture-model",
            revoked: binding.revoked,
          },
        ]
      : [],
  });
  localConnectionsApi.budget = async () => ({
    storeId,
    policy: null,
    windows: [],
    attempts: waiting ? [attempt] : [],
  });
  localConnectionsApi.review = async (review) => {
    if (
      review.upstreamRequestId !== "verified-request" ||
      review.tokens?.kind !== "release" ||
      !review.source.trim()
    ) {
      throw Error("Unexpected review evidence");
    }
    waiting = false;
    return { attempt };
  };
  localConnectionsApi.write = async (input) => {
    if (input.kind === "create") {
      binding = {
        bindingId,
        serviceId,
        name: input.name,
        baseUrl: "http://127.0.0.1:10080/gateway/fixture/v1",
        keyVersion: 1,
        revoked: false,
      };
    } else if (input.kind === "rotate" && binding) {
      binding = { ...binding, keyVersion: binding.keyVersion + 1 };
    } else if (input.kind === "revoke" && binding) {
      binding = { ...binding, revoked: true };
    } else {
      throw Error("Unexpected local operation");
    }
    return {
      receipt: {
        bindingId,
        keyVersion: binding.keyVersion,
        operationId: input.query.operationId,
        state: binding.revoked ? "revoked" : "active",
      },
    };
  };
}
