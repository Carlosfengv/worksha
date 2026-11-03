import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { IconProvider } from "@/lib/icon-context";
import { Button } from "@/components/ui/button";
import { useProviderWrite } from "@/features/providers/hooks/use-provider-write";
import { ProviderWriteStatus } from "@/features/providers/components/provider-write-status";
import { ProviderOperationRecovery } from "@/features/providers/components/provider-operation-recovery";
import {
  freezeProviderRequest,
  sameProviderRequest,
  type ProviderWriteRequest,
  type ProviderOperation,
} from "@/features/providers/operation-receipt";
import {
  rememberProviderOperation,
  providerRecoveryRecords,
} from "@/features/providers/pending-operations";
import type {
  ProviderWriteRoute,
  ProviderCatalogSelection,
} from "@/features/providers/catalog-operation";
import type { Provider } from "@/features/providers/types";
import { DesktopError } from "@/shared/lib/desktop-command";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import "@/styles/globals.css";

const mode = new URLSearchParams(location.search).get("mode") ?? "lost";
const storeId = "c9e027fe-4536-4d08-8a55-74e6cbaf6313";
const otherStoreId = "6087597b-0625-42ea-8fd0-e51f6ad16d01";
const savedKey = "fixture.catalog-backend:" + location.search;
type Backend = {
  sequence: number;
  probes: number;
  metadata: number;
  operations: Record<string, ProviderOperation>;
  catalogs: Record<string, ProviderWriteRoute>;
};
let backend: Backend = {
  sequence: 0,
  probes: 0,
  metadata: 0,
  operations: {},
  catalogs: {},
};
const previous = sessionStorage.getItem(savedKey);
if (previous) backend = JSON.parse(previous) as Backend;
const persist = () => sessionStorage.setItem(savedKey, JSON.stringify(backend));
const provider: Provider = {
  id: "custom-5d160843-43f9-46b9-8486-a3e652cedb49",
  name: "目录恢复连接",
  vendor: "custom",
  endpoint: "http://127.0.0.1:11434/v1",
  protocol: "responses",
  model: "owned-model",
  models: ["owned-model"],
  disabledModels: [],
  modelCatalog: null,
  revision: 0,
  configured: false,
  credentialRef: null,
  credentialLabel: null,
  category: "local",
};
const catalog: ProviderCatalogSelection = {
  pluginId: "org.example.catalog",
  digest: "a".repeat(64),
  bindingId:
    "a".repeat(64) +
    "-" +
    "b".repeat(64) +
    ":1644cc2c-7d97-4e63-9d15-a79bed26b749",
  target: {
    kind: "draft",
    label: provider.name,
    endpoint: provider.endpoint,
    protocol: provider.protocol,
    vendor: provider.vendor ?? "custom",
    credentialRef: null,
  },
};
const route: ProviderWriteRoute = mode.includes("unified")
  ? {
      source: mode.includes("builtin")
        ? { kind: "builtin" }
        : { kind: "external", catalog },
      expectedSourceRevision: "b".repeat(64),
    }
  : catalog;
const protocol = () => ({
  version: 1 as const,
  storeId: control.backendStoreId,
  sequence: backend.sequence,
  receiptLimit: 256,
  receiptBytes: 16777216,
  pendingLimit: 4,
  pendingOperationIds: Object.values(backend.operations)
    .filter((op) => op.status === "pending")
    .map((op) => op.request.operationId),
});
function makeOperation(
  request: ProviderWriteRequest,
  status: ProviderOperation["status"],
): ProviderOperation {
  const saved =
    status === "committed"
      ? {
          ...request.provider,
          configured: true,
          revision: request.provider.revision + 1,
        }
      : null;
  return {
    apiVersion: 1,
    sequence: request.expectedSequence + 1,
    status,
    request,
    provider: saved,
    verification:
      saved && request.kind === "connect"
        ? {
            providerId: saved.id,
            model: saved.model,
            fingerprint: "f".repeat(64),
            state: "verified",
            checkedAt: "2026-11-01T00:00:00Z",
            message: "offline fixture",
          }
        : null,
    error: null,
  };
}
const control = {
  backendStoreId: storeId,
  commands: [] as { name: string; args: unknown }[],
  failQueries:
    mode === "lost" || mode === "save-lost" || mode.startsWith("unified-"),
  holdQuery: false,
  finishQuery: null as (() => void) | null,
  loseReply: true,
  backend: () => structuredClone(backend),
  records: providerRecoveryRecords,
  recover: notifyBackendRecovery,
  otherStoreId,
};
declare global {
  interface Window {
    catalogRecoveryFixture: typeof control;
  }
}
window.catalogRecoveryFixture = control;
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
if (mode.startsWith("seed") && Object.keys(backend.operations).length === 0) {
  const request = freezeProviderRequest(protocol(), "connect", provider);
  backend.operations[request.operationId] = makeOperation(
    request,
    mode === "seed-late" ? "committed" : "pending",
  );
  backend.catalogs[request.operationId] = route;
  backend.sequence++;
  if (mode === "seed-late") control.holdQuery = true;
  rememberProviderOperation(request, route);
  persist();
}
mockIPC(async (name, args) => {
  control.commands.push({ name, args: structuredClone(args) });
  if (name === "get_provider_write_protocol") return protocol();
  if (name === "commit_provider_operation" || name === "get_provider_operation")
    throw new DesktopError({
      code: "unexpected_native_transport",
      outcomeUnknown: true,
      message: "外部恢复不得进入原生提交或查询",
    });
  if (name === "perform_provider_source_operation") {
    const action = (
      args as {
        input: import("../../contracts/domain/provider-source-operation/types").SourceOperationAction;
      }
    ).input;
    if (
      JSON.stringify({
        source: action.request.source,
        expectedSourceRevision: action.request.expectedSourceRevision,
      }) !== JSON.stringify(route)
    )
      throw Error("changed explicit source");
    name =
      action.kind === "commit"
        ? "commit_catalog_provider_operation"
        : "get_catalog_provider_operation";
    args = { input: { operation: action.request.operation, catalog } };
  }
  const frozen =
    name === "cancel_provider_operation"
      ? (args as { request: ProviderWriteRequest }).request
      : (args as { input: { operation: ProviderWriteRequest } }).input
          .operation;
  if (frozen.storeId !== control.backendStoreId)
    throw new DesktopError({
      code: "operation_store_changed",
      outcomeUnknown: false,
      message: "原数据库已变化",
    });
  if (
    name === "commit_catalog_provider_operation" ||
    name === "get_catalog_provider_operation"
  ) {
    const selected = (args as { input: { catalog: ProviderCatalogSelection } })
      .input.catalog;
    if (JSON.stringify(selected) !== JSON.stringify(catalog))
      throw Error("wrong catalog choice");
  }
  let op = backend.operations[frozen.operationId];
  if (op && !sameProviderRequest(op.request, frozen))
    throw Error("changed original request");
  if (name === "commit_catalog_provider_operation") {
    if (!op) {
      op = backend.operations[frozen.operationId] = makeOperation(
        frozen,
        "committed",
      );
      backend.catalogs[frozen.operationId] = route;
      backend.sequence++;
      backend.metadata++;
      if (frozen.kind === "connect") backend.probes++;
      persist();
    }
    if (control.loseReply)
      throw new DesktopError({
        code: "transport_connection",
        outcomeUnknown: true,
        message: "合成回复丢失",
      });
  } else if (name === "get_catalog_provider_operation") {
    if (control.holdQuery)
      await new Promise<void>((resolve) => (control.finishQuery = resolve));
    if (control.failQueries)
      throw new DesktopError({
        code: "service_unavailable",
        outcomeUnknown: true,
        message: "原结果暂不可读取",
      });
  } else if (name === "cancel_provider_operation") {
    op = backend.operations[frozen.operationId] = makeOperation(
      frozen,
      "cancelled",
    );
    persist();
  } else throw Error("Unexpected fixture command " + name);
  if (!op)
    throw new DesktopError({
      code: "operation_not_found",
      outcomeUnknown: true,
      message: "原结果尚不可核查",
    });
  return structuredClone(op);
});
function Harness() {
  const [resolved, setResolved] = useState(0);
  const write = useProviderWrite(storeId, () => setResolved((n) => n + 1));
  return (
    <main className="space-y-4 p-4">
      <p>已核查结果 {resolved}</p>
      <Button
        disabled={write.busy || write.unresolved}
        onClick={() =>
          void write.start(
            mode === "save-lost" || mode.startsWith("unified-")
              ? "save"
              : "connect",
            provider,
            undefined,
            async () =>
              mode === "invalid-choice"
                ? (undefined as unknown as ProviderCatalogSelection)
                : route,
          )
        }
      >
        提交目录原请求
      </Button>
      <ProviderWriteStatus write={write} />
      {write.state.kind === "failed" && (
        <p role="alert">{write.state.message}</p>
      )}
      <ProviderOperationRecovery
        storeId={storeId}
        onUpdated={async () => setResolved((n) => n + 1)}
      />
    </main>
  );
}
createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <Harness />
  </IconProvider>,
);
