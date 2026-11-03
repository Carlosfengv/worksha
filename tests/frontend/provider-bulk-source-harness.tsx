import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { ProviderModelInventory } from "@/features/providers/components/provider-model-inventory";
import {
  providerSourceApi,
  providerCatalogApi,
  scopedProviderCatalogApi,
  providerSourceOperationApi,
  providersApi,
} from "@/features/providers/api";
import { pluginsApi, fullPluginsApi } from "@/features/plugins/api";
import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import type { Provider } from "@/features/providers/types";
import type {
  SourceState,
  SourceAction,
} from "../../contracts/domain/provider-source/types";
import "@/styles/globals.css";
const storeId = "c9e027fe-4536-4d08-8a55-74e6cbaf6313";
const a: Provider = {
  id: "custom-5d160843-43f9-46b9-8486-a3e652cedb49",
  name: "外部来源 A",
  vendor: "custom",
  endpoint: "http://127.0.0.1:11434/v1",
  protocol: "responses",
  model: "registered-a",
  models: ["registered-a"],
  disabledModels: [],
  credentialRef: null,
  credentialLabel: null,
  configured: true,
  revision: 1,
  category: "local",
};
const b: Provider = {
  ...a,
  id: "custom-69c2a053-bffc-4d5e-b5eb-c22c7bc2c2d9",
  name: "内置来源 B",
  model: "registered-b",
  models: ["registered-b"],
};
const query = { storeId, providerId: a.id, expectedRevision: "1" };
const policy = {
  schemaVersion: 1,
  baseProtocol: "responses",
  path: "/catalog",
};
const choice = { pluginId: "org.example.bulk", digest: "a".repeat(64) };
const selection = {
  ...choice,
  bindingId: "owned-bulk-grant",
  target: { kind: "saved" as const, providerId: a.id, expectedRevision: "1" },
};
const original: SourceState = {
  providerId: a.id,
  expectedRevision: "1",
  kind: "external",
  sourceRevision: "b".repeat(64),
  choice,
  selection,
  authorized: true,
  policy,
  errorCode: null,
};
const control = {
  state: structuredClone(original),
  failState: false,
  holdState: false,
  failRead: false,
  holdRead: false,
  hide: false,
  mutationStarted: false,
  releaseRead: null as (() => void) | null,
  releaseState: null as (() => void) | null,
  releaseMutation: null as (() => void) | null,
  legacyReads: 0,
  reads: [] as { providerId: string; kind: string; token: string | null }[],
  states: 0,
  calls: [] as { name: string; args: unknown }[],
  error: "",
  active: (() => {}) as (value: boolean) => void,
  invalidate: invalidateCapabilityAccess,
  recover: notifyBackendRecovery,
  async mutate(kind: string) {
    const operation = {
      storeId,
      operationId: crypto.randomUUID(),
      expectedSequence: 0,
      kind: "save" as const,
      provider: a,
    };
    try {
      if (kind === "source")
        await providerSourceApi.select(
          query,
          control.state.sourceRevision ?? null,
          null,
        );
      else if (kind === "catalog")
        await providerCatalogApi.change({
          ...choice,
          action: { kind: "unbind", bindingId: selection.bindingId },
        });
      else if (kind === "scoped")
        await scopedProviderCatalogApi.change({
          storeId,
          request: {
            ...choice,
            action: { kind: "unbind", bindingId: selection.bindingId },
          },
        });
      else if (kind === "write")
        await providerSourceOperationApi.perform("commit", operation, {
          source: { kind: "builtin" },
          expectedSourceRevision: control.state.sourceRevision ?? null,
        });
      else if (kind === "native-write")
        await providersApi.commitOperation(operation);
      else if (kind === "full-plugin")
        await fullPluginsApi.change({
          action: "disable",
          package: { pluginId: choice.pluginId, digest: choice.digest },
        });
      else
        await pluginsApi.change({
          pluginId: choice.pluginId,
          digest: choice.digest,
          action: "disable",
        });
    } catch (reason) {
      control.error = reason instanceof Error ? reason.message : String(reason);
    }
  },
};
declare global {
  interface Window {
    bulkSourceFixture: typeof control;
  }
}
window.bulkSourceFixture = control;
Object.defineProperty(document, "visibilityState", {
  configurable: true,
  get: () => (control.hide ? "hidden" : "visible"),
});
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
providersApi.providerUsage = async () => {
  throw Error("此独立目录夹具未接入用量");
};
mockIPC(async (name, args) => {
  control.calls.push({ name, args: structuredClone(args) });
  if (name === "get_provider_write_protocol")
    return {
      version: 1,
      storeId,
      sequence: 0,
      receiptLimit: 256,
      receiptBytes: 2097152,
      pendingLimit: 4,
      pendingOperationIds: [],
    };
  if (name === "fetch_provider_models") {
    control.legacyReads++;
    throw Error("bulk must not bypass its remembered source");
  }
  if (name === "perform_provider_catalog_source") {
    const input = (args as { input: SourceAction }).input;
    if (input.kind !== "select") {
      if (input.query.storeId !== storeId)
        throw { code: "operation_store_changed", message: "原数据库已变化" };
      if (input.kind === "state") {
        control.states++;
        if (input.query.providerId === a.id) {
          if (control.holdState)
            await new Promise<void>((done) => {
              control.releaseState = done;
            });
          if (control.failState)
            throw {
              code: "catalog_source_unavailable",
              message: "目录来源尚不可核查",
            };
          return structuredClone(control.state);
        }
        return {
          providerId: b.id,
          expectedRevision: "1",
          sourceRevision: null,
          kind: "builtin",
          authorized: true,
        };
      }
      const current =
        input.query.providerId === a.id
          ? structuredClone(control.state)
          : { kind: "builtin", sourceRevision: null, authorized: true };
      if (
        !current.authorized ||
        !["builtin", "external"].includes(current.kind)
      )
        throw { code: "permission_denied", message: "目录未授权" };
      if ((input.sourceRevision ?? null) !== current.sourceRevision)
        throw { code: "catalog_source_changed", message: "目录来源已变化" };
      control.reads.push({
        providerId: input.query.providerId,
        kind: current.kind,
        token: current.sourceRevision,
      });
      if (control.holdRead && input.query.providerId === a.id) {
        control.holdRead = false;
        await new Promise<void>((done) => {
          control.releaseRead = done;
        });
      }
      if (control.failRead && input.query.providerId === a.id)
        throw { code: "catalog_upstream", message: "目录上游暂时不可用" };
      return {
        models: [
          {
            id:
              input.query.providerId === b.id
                ? "native-b-candidate"
                : current.kind === "builtin"
                  ? "native-a-candidate"
                  : current.sourceRevision === "c".repeat(64)
                    ? "external-new-candidate"
                    : "external-a-candidate",
            name: "Owned directory model",
            contextWindow: 98765,
          },
        ],
        sourceUrl:
          a.endpoint + (current.kind === "builtin" ? "/models" : "/catalog"),
        fetchedAt: new Date().toISOString(),
        truncated: false,
      };
    }
  }
  control.mutationStarted = true;
  await new Promise<void>((done) => {
    control.releaseMutation = done;
  });
  throw {
    code: "transport_timeout",
    outcomeUnknown: true,
    message: "受管写入回复未确认",
  };
});
function Harness() {
  const [active, setActive] = useState(true);
  control.active = setActive;
  return (
    <IconProvider>
      <main className="p-4">
        <ProviderModelInventory
          providers={[a, b]}
          storeId={storeId}
          active={active}
        />
      </main>
    </IconProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
