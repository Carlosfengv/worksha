import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { NativeProviderImport } from "@/features/providers/components/native-provider-import";
import { DesktopError } from "@/shared/lib/desktop-command";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import type {
  NativeImportOperation,
  NativeImportRequest,
} from "@/features/providers/native-import-operation";
import type {
  Provider,
  ProviderImportCandidate,
} from "@/features/providers/types";
import "@/styles/globals.css";

const STORE = "71a3e175-52cf-41da-8631-c1fb652bb2a7",
  NAMESPACE = "6e183d0d-6268-4e1b-9abc-25d3e66f8249";
const mode = new URLSearchParams(location.search).get("mode") ?? "normal";
const KEY = "fixture.native-import.backend";
const state: {
  sequence: number;
  operations: Record<string, NativeImportOperation>;
} = JSON.parse(sessionStorage.getItem(KEY) ?? '{"sequence":0,"operations":{}}');
const persist = () => sessionStorage.setItem(KEY, JSON.stringify(state));
let revision = "a".repeat(64);
const candidate = (): ProviderImportCandidate => ({
  id: "claude-fixture",
  agentId: "claude",
  sourcePath: "/fixture/.claude/settings.json",
  sourceLabel: "fixture",
  revision,
  name: "Fixture native provider",
  vendor: "custom",
  endpoint: "https://fixture.invalid/v1",
  protocol: "messages",
  model: "fixture-model",
  active: true,
  credentialSource: "fixture",
  canImport: true,
  reason: null,
  existingProviderId: null,
});
function operation(request: NativeImportRequest): NativeImportOperation {
  const provider: Provider = {
    id: "custom-" + request.operation.operationId,
    name: "Fixture native provider",
    vendor: "custom",
    endpoint: "https://fixture.invalid/v1",
    protocol: "messages",
    model: request.model ?? "fixture-model",
    configured: true,
    credentialRef: crypto.randomUUID(),
    credentialLabel: "Claude Code · 本机导入 API Key",
    models: [request.model ?? "fixture-model"],
    disabledModels: [],
    modelCatalog: null,
    revision: 0,
    category: "custom",
  };
  return {
    version: 1,
    sequence: request.operation.expectedSequence + 1,
    status: "pending",
    request,
    provider,
    reused: false,
    createsCredential: true,
  };
}
if (mode === "server-pending" && !Object.keys(state.operations).length) {
  const request: NativeImportRequest = {
    candidateId: "claude-fixture",
    expectedRevision: revision,
    model: "fixture-model",
    operation: {
      storeId: STORE,
      namespaceId: NAMESPACE,
      operationId: crypto.randomUUID(),
      expectedSequence: 0,
    },
  };
  const pending = operation(request);
  state.operations[request.operation.operationId] = pending;
  state.sequence = 1;
  persist();
}
declare global {
  interface Window {
    nativeImportFixture: {
      calls: string[];
      submits: NativeImportRequest[];
      queries: NativeImportRequest[];
      cancels: NativeImportRequest[];
      refreshes: number;
      rotate: () => void;
      recover: () => void;
      release?: () => void;
      unmount?: () => void;
      state: typeof state;
    };
  }
}
window.nativeImportFixture = {
  calls: [],
  submits: [],
  queries: [],
  cancels: [],
  refreshes: 0,
  rotate: () => {
    revision = "b".repeat(64);
  },
  recover: notifyBackendRecovery,
  state,
};
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
if (mode === "bad-storage")
  localStorage.setItem(
    "local-agent.native-import.v1.bad",
    "original-unreadable-bytes",
  );
if (mode === "storage-fails") {
  const original = Storage.prototype.setItem;
  Storage.prototype.setItem = function (key, value) {
    if (key.startsWith("local-agent.native-import.v1."))
      throw new DOMException("quota", "QuotaExceededError");
    original.call(this, key, value);
  };
}
const unknown = () =>
  new DesktopError({
    code: "transport_connection",
    outcomeUnknown: true,
    message: "回复丢失",
  });
mockIPC(async (name, args) => {
  const f = window.nativeImportFixture;
  f.calls.push(name);
  if (name === "preview_native_provider_import")
    return {
      protocol: {
        version: mode === "weak-protocol" ? 2 : 1,
        storeId: STORE,
        namespaceId: NAMESPACE,
        namespaceState: "active",
        sequence: mode === "full" ? 4096 : state.sequence,
        operationLimit: 4096,
        pendingLimit: 4,
        pendingRequests: Object.values(state.operations)
          .filter((o) => o.status === "pending")
          .map((o) => o.request),
      },
      candidates: [candidate()],
    };
  const request = (args as { input: NativeImportRequest }).input,
    id = request.operation.operationId;
  if (name === "get_native_provider_import") {
    f.queries.push(request);
    if (mode === "expired" && !state.operations[id])
      return {
        version: 1,
        sequence: request.operation.expectedSequence + 1,
        status: "expired",
        request,
        provider: null,
        reused: false,
        createsCredential: false,
      };
    if (!state.operations[id]) throw unknown();
    return state.operations[id];
  }
  if (name === "cancel_native_provider_import") {
    f.cancels.push(request);
    const current = state.operations[id] ?? operation(request);
    if (current.status !== "committed") current.status = "cancelled";
    state.operations[id] = current;
    state.sequence = Math.max(state.sequence, current.sequence);
    persist();
    return current;
  }
  if (name === "import_native_provider") {
    f.submits.push(request);
    let current = state.operations[id];
    if (!current) {
      if (mode === "expired") {
        const occupied = operation({
          ...request,
          operation: { ...request.operation, operationId: crypto.randomUUID() },
        });
        occupied.status = "committed";
        occupied.provider!.revision = 1;
        state.operations[occupied.request.operation.operationId] = occupied;
        state.sequence++;
        persist();
        throw new DesktopError({
          code: "native_import_expired",
          outcomeUnknown: false,
          message: "原请求序号已由另一项操作占用",
        });
      }
      current = operation(request);
      state.operations[id] = current;
      state.sequence++;
      persist();
      if (mode === "pending" || mode === "source-change" || mode === "reload")
        throw unknown();
    }
    if (mode === "hold")
      await new Promise<void>((resolve) => {
        f.release = resolve;
      });
    if (current.status === "cancelled") throw new Error("cancelled");
    current.status = "committed";
    current.provider!.revision = 1;
    persist();
    if (mode === "lost-success" && f.submits.length === 1) throw unknown();
    if (mode === "bad-result" && f.submits.length === 1)
      return {
        provider: { ...current.provider, id: "custom-" + crypto.randomUUID() },
        reused: false,
      };
    return { provider: current.provider, reused: false };
  }
  throw new Error("Unexpected fixture command: " + name);
});
const root = createRoot(document.getElementById("root")!);
window.nativeImportFixture.unmount = () => root.unmount();
root.render(
  <NativeProviderImport
    onImported={async () => {
      window.nativeImportFixture.refreshes++;
      if (mode === "refresh-fails") throw new Error("refresh failed");
    }}
  />,
);
