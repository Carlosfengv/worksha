import { useState } from "react";
import { createRoot } from "react-dom/client";
import { ProviderEditor } from "@/features/providers/components/provider-editor";
import { providersApi } from "@/features/providers/api";
import type { Provider } from "@/features/providers/types";
import { DesktopError } from "@/shared/lib/desktop-command";
import {
  credentialReceipt,
  type CredentialReceipt,
  type ScopedCredentialOperation,
  type CredentialOperationV3,
} from "@/features/providers/credential-receipt";
import "@/styles/globals.css";
import {
  installProviderOperationFixture,
  PROVIDER_FIXTURE_STORE_ID,
  PROVIDER_FIXTURE_OTHER_STORE_ID,
} from "./provider-operation-fixture";
import { ProviderOperationRecovery } from "@/features/providers/components/provider-operation-recovery";
import { createProviderPresetFixture } from "./provider-presets-fixture";

declare global {
  interface Window {
    autoConnectFixture: {
      credentials: { label: string; secret: string; operationId: string }[];
      credentialChecks: string[];
      credentialCancels: string[];
      connections: Provider[];
      manualSaves: Provider[];
      modelRequests: Parameters<typeof providersApi.providerModels>[0][];
      saved: number;
      release?: () => void;
      unmount?: () => void;
      materialize?: () => void;
      sourceChange?: (storeId: string | undefined) => void;
    };
  }
}
window.autoConnectFixture = {
  credentials: [],
  credentialChecks: [],
  credentialCancels: [],
  connections: [],
  manualSaves: [],
  modelRequests: [],
  saved: 0,
};
const fixture = window.autoConnectFixture;
const mode = new URLSearchParams(location.search).get("mode");
const receipts = new Map<string, CredentialReceipt>();
const receipt = (operationId: string, label: string): CredentialReceipt => ({
  operationId,
  credentialRef: "e1b71318-84c0-4f5d-b399-aee182f69ade",
  credentialLabel: label,
});
const provider: Provider = {
  id: "custom-3da06dc7-60bc-4c9b-8831-c3632690012b",
  name: "",
  endpoint: "",
  vendor: "custom",
  protocol: "responses",
  model: "",
  models: [],
  configured: false,
  credentialRef: null,
  credentialLabel: null,
  revision: 0,
  category: "custom",
};
if (new URLSearchParams(location.search).has("configured")) {
  Object.assign(provider, {
    name: "Existing provider",
    vendor: "deepseek",
    endpoint: "https://fixture.invalid/v1",
    model: "fixture-chat",
    models: ["fixture-chat"],
    configured: true,
    credentialRef: "9f765378-a884-4b50-b04c-cf1b14b4a82c",
    credentialLabel: "Existing Key",
    modelCatalog: {
      fetchedAt: "2026-10-30T00:00:00Z",
      sourceUrl: "https://fixture.invalid/v1/models",
      endpoint: "https://fixture.invalid/v1",
      protocol: provider.protocol,
      credentialRef: "9f765378-a884-4b50-b04c-cf1b14b4a82c",
    },
  } satisfies Partial<Provider>);
}
providersApi.createScopedCredential = async (
  storeId,
  label,
  secret,
  operationId,
) => {
  const previous = fixture.credentials.find(
    (row) => row.operationId === operationId,
  );
  fixture.credentials.push({ label, secret, operationId });
  if (previous && previous.secret !== secret)
    throw new DesktopError({
      code: "operation_conflict",
      message: "本次操作的密钥不同，请输入原密钥。",
    });
  const saved = receipt(operationId, label);
  if (mode === "credential-hold")
    await new Promise<void>((resolve) => {
      fixture.release = resolve;
    });
  if (mode === "credential-lost" || mode === "credential-query")
    receipts.set(operationId, saved);
  if (
    mode === "credential-error" ||
    mode === "credential-cancelled-late" ||
    mode === "credential-lost" ||
    mode === "credential-query" ||
    (mode === "credential-retry" && !previous)
  )
    throw new DesktopError({
      code: "credential_unavailable",
      outcomeUnknown: true,
      message: "钥匙串暂不可用，请重试。",
    });
  if (mode === "credential-malformed")
    return {
      ...credentialReceipt(
        { ...saved, operationId: crypto.randomUUID() },
        operationId,
      ),
      version: 2,
      storeId,
    };
  receipts.set(operationId, saved);
  return { ...saved, version: 2, storeId };
};
providersApi.scopedCredentialOperation = async (
  storeId,
  operationId,
): Promise<ScopedCredentialOperation> => {
  fixture.credentialChecks.push(operationId);
  if (mode === "credential-error")
    throw new DesktopError({
      code: "operation_not_found",
      message: "尚未查到",
    });
  if (mode === "credential-query" && fixture.credentialChecks.length === 1)
    throw new DesktopError({
      code: "transport_connection",
      message: "结果暂不可读",
    });
  const saved =
    mode === "credential-restored" || mode === "credential-check-hold"
      ? receipt(operationId, "新连接 API Key")
      : receipts.get(operationId);
  if (mode === "credential-check-hold")
    await new Promise<void>((resolve) => {
      fixture.release = resolve;
    });
  return saved
    ? {
        version: 2,
        storeId,
        operationId,
        status: "ready",
        receipt: { ...saved, version: 2, storeId },
      }
    : { version: 2, storeId, operationId, status: "pending", receipt: null };
};
providersApi.cancelScopedCredentialOperation = async (
  storeId,
  operationId,
): Promise<ScopedCredentialOperation> => {
  fixture.credentialCancels.push(operationId);
  if (mode === "credential-cancelled-late" && !receipts.has(operationId)) {
    fixture.materialize = () =>
      receipts.set(operationId, receipt(operationId, "新连接 API Key"));
    return {
      version: 2,
      storeId,
      operationId,
      status: "cancelled_pending",
      receipt: null,
    };
  }
  const saved = receipts.get(operationId);
  return saved
    ? {
        version: 2,
        storeId,
        operationId,
        status: "ready",
        receipt: { ...saved, version: 2, storeId },
      }
    : { version: 2, storeId, operationId, status: "cancelled", receipt: null };
};
const v3Reference = "54261267-b628-4be6-ab05-462c312a251c";
providersApi.createCredentialV3 = async (input) => {
  const saved = await providersApi.createScopedCredential(
    input.storeId,
    input.label,
    input.secret,
    input.operationId,
  );
  return {
    ...saved,
    version: 3,
    namespaceId: input.namespaceId,
    credentialRef: v3Reference,
  };
};
providersApi.credentialOperationV3 = async (
  input,
): Promise<CredentialOperationV3> => {
  const claim =
    window.providerOperationsFixture.snapshot().stores[input.storeId]
      ?.legacyClaims?.[input.operationId];
  if (claim) {
    fixture.credentialChecks.push(input.operationId);
    if (claim.stage !== "committed" || mode === "legacy-query-unavailable")
      throw new DesktopError({
        code: "credential_migration_pending",
        outcomeUnknown: true,
        message: "旧凭据结果尚待核查",
      });
    return {
      version: 3,
      ...input,
      status: claim.phase,
      receipt:
        claim.phase === "ready"
          ? {
              version: 3,
              ...input,
              credentialRef: "e1b71318-84c0-4f5d-b399-aee182f69ade",
              credentialLabel: "Legacy fixture",
            }
          : null,
    };
  }
  const saved = await providersApi.scopedCredentialOperation(
    input.storeId,
    input.operationId,
  );
  return {
    ...saved,
    version: 3,
    namespaceId: input.namespaceId,
    receipt: saved.receipt
      ? {
          ...saved.receipt,
          version: 3,
          namespaceId: input.namespaceId,
          credentialRef: v3Reference,
        }
      : null,
  };
};
providersApi.cancelCredentialOperationV3 = async (
  input,
): Promise<CredentialOperationV3> => {
  const saved = await providersApi.cancelScopedCredentialOperation(
    input.storeId,
    input.operationId,
  );
  return {
    ...saved,
    version: 3,
    namespaceId: input.namespaceId,
    receipt: saved.receipt
      ? {
          ...saved.receipt,
          version: 3,
          namespaceId: input.namespaceId,
          credentialRef: v3Reference,
        }
      : null,
  };
};
providersApi.connectProvider = async (input) => {
  fixture.connections.push(structuredClone(input));
  if (mode === "hold")
    await new Promise<void>((resolve) => {
      fixture.release = resolve;
    });
  if (mode === "unknown" || (mode === "retry" && !input.model))
    throw new DesktopError({
      code: "needs_model",
      message: "目录不可读取，补充模型 ID 后重试；连接方式仍会自动识别。",
    });
  const connected = {
    ...input,
    name: input.name || "fixture.invalid",
    endpoint: "https://fixture.invalid/v1",
    protocol: "chat-completions" as const,
    model: input.model || "fixture-model",
    models: [input.model || "fixture-model"],
    configured: true,
    revision: input.revision + 1,
  };
  return {
    provider: connected,
    verification: {
      providerId: connected.id,
      model: connected.model,
      fingerprint: "fixture",
      state: "verified",
      checkedAt: "2026-10-26T00:00:00Z",
      message: "已自动识别并完成短请求验证。",
    },
    catalog: null,
  };
};
providersApi.saveProvider = async (input) => {
  fixture.manualSaves.push(input);
};
providersApi.providerModels = async (input) => {
  fixture.modelRequests.push(input);
  if (mode === "models-error") throw Error("目录读取失败，请重试。");
  if (mode === "models-hold")
    await new Promise<void>((resolve) => {
      fixture.release = resolve;
    });
  return {
    models:
      mode === "models-empty"
        ? []
        : [
            { id: "fixture-chat", name: "Fixture Chat" },
            { id: "fixture-reasoner", name: "Fixture Reasoner" },
          ],
    fetchedAt: "2026-10-30T00:00:00Z",
    sourceUrl: input.endpoint.replace(/\/$/, "") + "/models",
    truncated: false,
  };
};
const presetFixture = createProviderPresetFixture();
window.providerPresetsFixture = presetFixture.control;
window.providerOperationsFixture = installProviderOperationFixture({
  presets: new URLSearchParams(location.search).has("presets")
    ? presetFixture.handle
    : undefined,
  catalog: async (action) => {
    if (action.kind === "check") return null;
    const draft = action.request.draft;
    if (!["responses", "messages", "chat-completions"].includes(draft.protocol))
      throw Error("Invalid fixture protocol");
    return providersApi.providerModels({
      ...draft,
      protocol: draft.protocol as Provider["protocol"],
      credentialRef: draft.credentialRef ?? null,
    });
  },
  mode,
  save: (input) => providersApi.saveProvider(input),
  connect: (input) => providersApi.connectProvider(input),
});
if (mode === "credential-origin-b")
  window.providerOperationsFixture.switchStore(PROVIDER_FIXTURE_OTHER_STORE_ID);
function Harness() {
  const [open, setOpen] = useState(true);
  const [storeId, setStoreId] = useState<string | undefined>(
    mode === "operation-origin-missing"
      ? undefined
      : mode === "credential-origin-b"
        ? PROVIDER_FIXTURE_OTHER_STORE_ID
        : PROVIDER_FIXTURE_STORE_ID,
  );
  fixture.sourceChange = setStoreId;
  fixture.unmount = () => setOpen(false);
  return (
    <main className="p-4">
      <p>{fixture.saved ? "连接已保存" : "添加连接"}</p>
      <ProviderOperationRecovery
        storeId={storeId}
        onUpdated={async () => {
          fixture.saved++;
        }}
      />
      {open && (
        <ProviderEditor
          provider={provider}
          storeId={storeId}
          onClose={() => setOpen(false)}
          onSaved={async () => {
            fixture.saved++;
            if (mode === "operation-refresh-error" && fixture.saved === 1)
              throw new Error("合成列表刷新失败");
            setOpen(false);
          }}
        />
      )}
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
