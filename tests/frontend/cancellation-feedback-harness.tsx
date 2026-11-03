import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { AgentServiceControl } from "@/features/model-services/components/agent-service-control";
import { ProviderConfigControl } from "@/features/providers/components/provider-config-control";
import { defaultCapabilities } from "@/features/model-services/types";
import {
  localTokenRecovery,
  rememberLocalToken,
  type LocalTokenIdentity,
  type LocalTokenProtocol,
  type LocalTokenPurpose,
} from "@/features/providers/local-token-operation";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import type { Provider, ProviderStatus } from "@/features/providers/types";
import type { ModelServiceCatalog } from "@/features/model-services/types";
import "@/styles/globals.css";

const purpose = (new URLSearchParams(location.search).get("purpose") ??
  "model_service") as LocalTokenPurpose;
const protocol: LocalTokenProtocol = {
  version: 2,
  schemaVersion: 15,
  storeId: "00000000-0000-4000-8000-000000000001",
  namespaceId: "00000000-0000-4000-8000-000000000002",
  namespaceState: "active",
  operationLimit: 4096,
  pendingLimit: 4,
  cancellationLimit: 4096,
  pending: [],
};
const provider: Provider = {
  id: "anthropic",
  name: "隔离供应商",
  vendor: "anthropic",
  category: "local",
  endpoint: "http://127.0.0.1:9",
  protocol: "messages",
  model: "fixture-model",
  models: ["fixture-model", "other-model"],
  configured: true,
  credentialRef: null,
  credentialLabel: null,
  revision: 1,
};
const status: ProviderStatus = {
  localTokenProtocol: protocol,
  configs: [
    {
      agentId: "claude",
      path: "/fixture/.claude/settings.json",
      state: "ready",
      revision: "a".repeat(64),
      mode: "native",
      endpoint: null,
      model: null,
      providerName: "原生登录",
      credential: "fixture",
      error: null,
      warnings: [],
    },
  ],
  transactions: [],
  dynamicBindings: [],
  dynamicChanges: [],
  backupProtection: "fixture",
  runtimes: [
    {
      agentId: "claude",
      supported: true,
      processes: 0,
      version: "fixture",
      reason: "fixture",
    },
  ],
  verifications: [],
};
const catalog: ModelServiceCatalog = {
  writeProtocol: { version: 1, storeId: protocol.storeId, receiptLimit: 256 },
  services: [
    {
      id: "00000000-0000-4000-8000-000000000003",
      versionId: "00000000-0000-4000-8000-000000000004",
      revision: 1,
      name: "隔离服务",
      defaultModelId: "fixture-model",
      publishedAt: "2026-10-28T00:00:00Z",
      models: [
        {
          id: "fixture-model",
          name: "默认模型",
          provider,
          capabilities: defaultCapabilities(),
        },
      ],
    },
  ],
  attachments: [],
};
const calls: { name: string; args: unknown }[] = [];
const cancelled = new Set<string>();
let cancelMode = "fail",
  failVerification = false,
  release: (() => void) | null = null;
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
mockIPC(async (name, args) => {
  calls.push({ name, args: structuredClone(args) });
  if (name === "get_local_token_protocol") return structuredClone(protocol);
  if (name === "verify_provider") {
    return {
      providerId: provider.id,
      model: (args as { model?: string }).model ?? provider.model,
      fingerprint: "fixture",
      checkedAt: "2026-10-28T00:00:00Z",
      state: failVerification ? "failed" : "verified",
      message: failVerification ? "新的模型验证失败" : "fixture",
    };
  }
  if (
    [
      "prepare_provider_switch",
      "prepare_dynamic_connection",
      "prepare_model_service_connection",
    ].includes(name)
  )
    throw {
      code: "local_token_result_unknown",
      outcomeUnknown: true,
      message: "原准备请求失败，请核查原编号",
    };
  if (
    name === "cancel_local_token_operation" ||
    name === "get_local_token_operation"
  ) {
    const identity = (args as { input: LocalTokenIdentity }).input;
    if (name === "cancel_local_token_operation") {
      if (cancelMode === "fail")
        throw {
          code: "local_token_result_unknown",
          message: "停止结果尚不可核查",
        };
      cancelled.add(identity.operationId);
      if (cancelMode === "hold")
        await new Promise<void>((resolve) => {
          release = resolve;
        });
      if (cancelMode === "lost")
        throw {
          code: "transport_connection",
          outcomeUnknown: true,
          message: "停止回复丢失",
        };
    }
    if (!cancelled.has(identity.operationId))
      throw { code: "local_token_result_unknown", message: "原编号仍待核查" };
    return {
      version: 2,
      identity,
      operation: null,
      credential: null,
      preview: null,
      cancellation: {
        version: 1,
        identity:
          cancelMode === "foreign"
            ? { ...identity, namespaceId: crypto.randomUUID() }
            : identity,
      },
      canContinue: false,
      sourceMessage: "原编号后续创建已阻止",
    };
  }
  throw Error("unexpected command " + name);
});
const root = createRoot(document.getElementById("root")!);
root.render(
  <IconProvider>
    <main className="space-y-4 p-6">
      {purpose === "model_service" ? (
        <AgentServiceControl
          agentId="claude"
          name="Claude Code CLI"
          providers={[provider]}
          status={status}
          catalog={catalog}
          reload={() => {}}
        />
      ) : (
        <ProviderConfigControl
          status={status}
          providers={[provider]}
          reload={() => {}}
          onlyAgent="claude"
          agentProductId="claude-code-cli"
          {...(purpose === "fixed_route"
            ? {
                renderPicker: (
                  open: (
                    config: ProviderStatus["configs"][number],
                    id: string,
                  ) => void,
                ) => (
                  <button onClick={() => open(status.configs[0], provider.id)}>
                    准备配置路由
                  </button>
                ),
              }
            : {})}
        />
      )}
    </main>
  </IconProvider>,
);
const fixture = {
  setCancelMode(value: string) {
    cancelMode = value;
  },
  failVerification() {
    failVerification = true;
  },
  release() {
    release?.();
  },
  reconnect() {
    notifyBackendRecovery();
  },
  addOlderHint() {
    const identity = { ...protocol, operationId: crypto.randomUUID() };
    const origin = {
      storeId: identity.storeId,
      namespaceId: identity.namespaceId,
      operationId: identity.operationId,
    };
    rememberLocalToken({
      version: 1,
      identity: origin,
      purpose,
      agentId: "claude",
      expectedRevision: "a".repeat(64),
      providerId: provider.id,
      providerRevision: provider.revision,
      model: provider.model,
      ...(purpose === "model_service"
        ? { serviceId: catalog.services[0].id, serviceRevision: 1 }
        : {}),
    });
    return origin;
  },
  snapshot() {
    return structuredClone({
      calls,
      recovery: localTokenRecovery(),
      cancelled: [...cancelled],
    });
  },
};
declare global {
  interface Window {
    cancellationFeedback: typeof fixture;
  }
}
window.cancellationFeedback = fixture;
