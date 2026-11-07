import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { LocalTokenRecovery } from "@/features/providers/components/local-token-recovery";
import { providersApi } from "@/features/providers/api";
import { modelServicesApi } from "@/features/model-services/api";
import {
  captureLocalTokenSource,
  localTokenProtocol,
  localTokenRecovery,
  localTokenResult,
  type LocalTokenHint,
  type LocalTokenOperation,
  type LocalTokenPurpose,
  type LocalTokenProtocol,
  type LocalTokenResult,
} from "@/features/providers/local-token-operation";
import { validatedProviderMetadata } from "@/features/providers/operation-receipt";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import type { Provider, ProviderTransaction } from "@/features/providers/types";
import "@/styles/globals.css";

const protocol: LocalTokenProtocol = {
  version: 2,
  schemaVersion: 16,
  storeId: "00000000-0000-4000-8000-000000000001",
  namespaceId: "00000000-0000-4000-8000-000000000002",
  namespaceState: "active",
  operationLimit: 4096,
  pendingLimit: 4,
  cancellationLimit: 4096,
  pending: [],
};
const source = captureLocalTokenSource(protocol),
  serviceId = "00000000-0000-4000-8000-000000000003";
const provider: Provider = {
  id: "anthropic",
  name: "原供应商",
  vendor: "anthropic",
  category: "local",
  protocol: "messages",
  endpoint: "http://127.0.0.1:32123",
  model: "模型",
  models: ["模型"],
  configured: true,
  revision: 1,
  credentialRef: null,
  credentialLabel: null,
};
let mode = "lost",
  operation: LocalTokenOperation | null = null,
  preview: ProviderTransaction | null = null,
  release: (() => void) | null = null;
const calls: { name: string; args: unknown }[] = [];
let cancellation: LocalTokenResult["cancellation"] = null;
function makePreview() {
  if (!operation) throw Error("fixture missing operation");
  operation.status = "committed";
  operation.ownerSaved = true;
  operation.previewId = source.identity.operationId;
  protocol.pending = [];
  preview = {
    id: source.identity.operationId,
    agentId: "claude",
    state: "pending",
    createdAt: "2026-10-28T00:00:00Z",
    appliedAt: null,
    before: {
      agentId: "claude",
      path: "/fixture/.claude/settings.json",
      state: "absent",
      revision: null,
      mode: "native",
      endpoint: null,
      model: null,
      providerName: "原生登录",
      credential: "未读取",
      error: null,
      warnings: [],
    },
    targetName: "原供应商 · 模型",
    provider,
    model: provider.model,
    backupPath: "/fixture/backup.sealed",
    restoreOf: null,
    error: null,
    localTokenOperation: source.identity,
    routing: {
      id: operation.ownerId,
      clientProtocol: "messages",
      upstreamProtocol: "messages",
      localEndpoint: "http://127.0.0.1:32124/gateway/fixture",
      models: [],
    },
  };
  return structuredClone(preview);
}
const result = () => ({
  version: 2,
  identity: source.identity,
  operation: structuredClone(operation),
  preview: structuredClone(preview),
  credential: null,
  cancellation,
  canContinue: operation?.status === "pending" && !preview,
  sourceMessage: "原配置与目标已核对",
});
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
mockIPC(async (name, args) => {
  calls.push({ name, args: structuredClone(args) });
  if (name === "get_local_token_protocol") return structuredClone(protocol);
  if (
    [
      "prepare_provider_switch",
      "prepare_dynamic_connection",
      "prepare_model_service_connection",
    ].includes(name)
  ) {
    if (cancellation)
      throw { code: "local_token_cancelled", message: "原编号已阻止后续创建" };
    if (mode === "before-admission")
      throw {
        code: "transport_connection",
        outcomeUnknown: true,
        message: "发送结果未知，请保留原编号",
      };
    if (operation)
      throw {
        code: "local_token_result_unknown",
        outcomeUnknown: true,
        message: "原编号待核查",
      };
    const purpose: LocalTokenPurpose =
      name === "prepare_provider_switch"
        ? "fixed_route"
        : name === "prepare_dynamic_connection"
          ? "dynamic_binding"
          : "model_service";
    operation = {
      version: 1,
      status: "pending",
      request: {
        identity: source.identity,
        purpose,
        agentId: "claude",
        configurationPath: "/fixture/.claude/settings.json",
        configurationPathHash: "a".repeat(64),
        expectedRevision: "absent",
        provider,
        service:
          purpose === "model_service"
            ? {
                id: serviceId,
                versionId: "00000000-0000-4000-8000-000000000004",
                revision: 1,
              }
            : null,
      },
      ownerId: source.identity.operationId,
      credentialRef: "00000000-0000-4000-8000-000000000005",
      ownerSaved: false,
      previewId: null,
    };
    protocol.pending = [operation];
    if (mode === "hold")
      await new Promise<void>((resolve) => {
        release = resolve;
      });
    if (mode === "success" || mode === "hold") return makePreview();
    if (mode === "preview-lost") makePreview();
    throw {
      code: "local_token_result_unknown",
      outcomeUnknown: true,
      message: "回复丢失，请保留原编号核查",
    };
  }
  if (name === "get_local_token_operation") {
    if (mode === "query-hold")
      await new Promise<void>((resolve) => {
        release = resolve;
      });
    if (mode === "unknown")
      throw {
        code: "local_token_result_unknown",
        outcomeUnknown: true,
        message: "尚未查到，原编号已保留",
      };
    const value = result();
    if (mode === "foreign")
      value.identity = {
        ...source.identity,
        storeId: "00000000-0000-4000-8000-000000000099",
      };
    if (mode === "removed") {
      value.canContinue = false;
      Object.assign(value, {
        credential: {
          operationId: source.identity.operationId,
          status: "removed",
          receipt: null,
        },
      });
    }
    return value;
  }
  if (name === "continue_local_token_operation") {
    makePreview();
    return result();
  }
  if (name === "cancel_local_token_operation") {
    if (mode === "unknown")
      throw {
        code: "local_token_result_unknown",
        outcomeUnknown: true,
        message: "停止结果尚不可核查，原编号已保留",
      };
    if (!operation) {
      cancellation = { version: 1, identity: source.identity };
      if (mode === "cancel-foreign")
        cancellation = {
          version: 1,
          identity: { ...source.identity, namespaceId: crypto.randomUUID() },
        };
      if (mode === "cancel-hold")
        await new Promise<void>((resolve) => {
          release = resolve;
        });
      if (mode === "cancel-lost")
        throw {
          code: "transport_connection",
          outcomeUnknown: true,
          message: "停止回复丢失，请查询原编号",
        };
      return result();
    }
    operation.status = "cancelled";
    protocol.pending = [];
    if (preview) preview.state = "cancelled";
    return result();
  }
  throw Error("unexpected fixture command: " + name);
});
async function submit(purpose: LocalTokenPurpose) {
  try {
    if (purpose === "fixed_route")
      await providersApi.prepareProviderSwitch(
        "claude",
        "absent",
        provider.id,
        provider.revision,
        provider.model,
        source,
      );
    else if (purpose === "dynamic_binding")
      await providersApi.prepareDynamicConnection(
        {
          agentId: "claude",
          expectedRevision: "absent",
          providerId: provider.id,
          expectedProviderRevision: provider.revision,
          model: provider.model,
        },
        source,
      );
    else
      await modelServicesApi.prepareConnection(
        {
          agentId: "claude",
          expectedRevision: "absent",
          serviceId,
          expectedServiceRevision: 1,
        },
        source,
        provider,
      );
    return "预览已确认";
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}
const root = createRoot(document.getElementById("root")!);
function Harness() {
  const [message, setMessage] = useState(""),
    [adopted, setAdopted] = useState("");
  useEffect(
    () => () => {
      source.active = false;
    },
    [],
  );
  return (
    <main className="space-y-4 p-6">
      <button onClick={() => void submit("fixed_route").then(setMessage)}>
        准备入口
      </button>
      <p role="status">{message}</p>
      <LocalTokenRecovery
        protocol={protocol}
        agentId="claude"
        onPreview={(value) => setAdopted(value.id)}
      />
      <p data-testid="adopted">{adopted}</p>
    </main>
  );
}
root.render(<Harness />);
const fixture = {
  submit,
  setMode(value: string) {
    mode = value;
  },
  release() {
    release?.();
  },
  reconnect() {
    notifyBackendRecovery();
  },
  unmount() {
    root.unmount();
  },
  snapshot() {
    return structuredClone({
      calls,
      protocol,
      operation,
      preview,
      cancellation,
      recovery: localTokenRecovery(),
      identity: source.identity,
    });
  },
  modelValid(model: string) {
    return !!validatedProviderMetadata({
      ...provider,
      model,
      models: [model],
      disabledModels: [],
    });
  },
  protocolValid(value: unknown) {
    try {
      localTokenProtocol(value);
      return true;
    } catch {
      return false;
    }
  },
  resultValid(value: unknown, hint?: LocalTokenHint) {
    try {
      localTokenResult(value, source.identity, hint);
      return true;
    } catch {
      return false;
    }
  },
};
declare global {
  interface Window {
    localTokenFixture: typeof fixture;
  }
}
window.localTokenFixture = fixture;
