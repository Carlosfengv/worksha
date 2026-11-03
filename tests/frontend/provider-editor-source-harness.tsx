import { useState } from "react";
import { createRoot } from "react-dom/client";
import { ProviderEditor } from "@/features/providers/components/provider-editor";
import { ProviderOperationRecovery } from "@/features/providers/components/provider-operation-recovery";
import {
  providerCatalogApi,
  providerSourceApi,
  scopedProviderCatalogApi,
  providersApi,
} from "@/features/providers/api";
import type { Provider } from "@/features/providers/types";
import type { PluginStatus } from "@/features/plugins/types";
import type {
  State,
  Target,
} from "../../contracts/domain/provider-access/types";
import {
  installProviderOperationFixture,
  PROVIDER_FIXTURE_STORE_ID,
} from "./provider-operation-fixture";
import { providerRecoveryRecords } from "@/features/providers/pending-operations";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
import type { DraftCatalogRequest } from "../../contracts/domain/provider-draft-catalog/types";
import type {
  DraftGrantScope,
  DraftGrantReceipt,
} from "../../contracts/domain/provider-draft-grant/types";
import type { Selection } from "../../contracts/domain/provider-connection/types";
import type { SourceRequest } from "../../contracts/domain/provider-source-operation/types";
import type { DraftRecoveryEntry } from "../../contracts/domain/provider-draft-recovery/types";
import "@/styles/globals.css";
const mode = new URLSearchParams(location.search).get("mode");
const saved = mode?.startsWith("saved");
const digest = "a".repeat(64),
  pluginId = "org.example.editor-catalog";
const provider: Provider = {
  id: "custom-5d160843-43f9-46b9-8486-a3e652cedb49",
  name: "目录来源测试",
  endpoint: "http://127.0.0.1:11434/v1",
  protocol: "responses",
  vendor: "openai",
  model: "owned-model",
  models: ["owned-model"],
  disabledModels: [],
  modelCatalog: null,
  credentialRef: null,
  credentialLabel: null,
  configured: !!saved,
  revision: saved ? 1 : 0,
  category: "local",
};
const policy = { schemaVersion: 1, baseProtocol: "responses", path: "catalog" };
const control = {
  calls: [] as string[],
  binds: [] as Target[],
  unbinds: [] as string[],
  reads: 0,
  nativeReads: 0,
  saves: [] as Provider[],
  sourceSelections: [] as Parameters<typeof providerSourceApi.select>[],
  records: providerRecoveryRecords,
  operations: null as ReturnType<typeof installProviderOperationFixture> | null,
  release: null as (() => void) | null,
  recover: notifyBackendRecovery,
  draftRequests: [] as DraftCatalogRequest[],
  draftChecks: 0,
  token: "b".repeat(64),
  rejectCheck: false,
  holdCheck: false,
  holdRead: false,
  holdBind: false,
  invalidate: invalidateCapabilityAccess,
  legacyReads: 0,
  badCheckReply: false,
  retireOnProtocol: false,
  ownerCalls: [] as string[],
  loseAcquireReply: false,
  holdAttach: false,
  recoveryCalls: [] as string[],
  holdRecovery: null as "list" | "inspect" | "finish" | null,
  badRecoveryReply: false,
  dropRequestCopies: () => {
    for (const record of owners.values()) record.request = null;
  },
  seedRecoveryOwners: (count: number) => {
    for (let index = 0; index < count; index++)
      owners.set(crypto.randomUUID(), {
        scope: {
          catalog: { pluginId, digest },
          draft: {
            endpoint: provider.endpoint,
            protocol: "responses",
            vendor: "openai",
            credentialRef: null,
          },
          label: `分页草稿归属 ${index + 1}`,
          allowPrivateNetwork: true,
        },
        selection: null,
        shared: false,
        operationId: null,
        storeId: PROVIDER_FIXTURE_STORE_ID,
        request: null,
      });
  },
  badOwnerReply: false,
  closeEditor: () => {},
  currentBindings: () => structuredClone(bindings),
  revokeOwnedGrants: () => {
    for (const [id] of owners) revokedOwners.add(id);
    bindings = [];
    invalidateCapabilityAccess();
  },
};
let bindings: State["bindings"] = [];
const owners = new Map<
  string,
  {
    scope: DraftGrantScope;
    selection: Selection | null;
    shared: boolean;
    operationId: string | null;
    storeId: string;
    request: SourceRequest | null;
  }
>();
const revokedOwners = new Set<string>();
if (mode === "preexisting-grant")
  bindings.push({
    bindingId: crypto.randomUUID(),
    digest,
    label: provider.name,
    endpoint: provider.endpoint,
    protocol: provider.protocol,
    authorized: true,
    allowPrivateNetwork: true,
    providerId: null,
    expectedRevision: null,
  });
const catalogState = (): State => ({
  canBind: true,
  policy,
  bindings: structuredClone(bindings),
  errorCode: null,
});
providerCatalogApi.adapters = async () => [
  {
    manifest: {
      id: pluginId,
      displayName: "外部测试目录",
      capabilities: ["provider.models@1"],
    },
    digest,
    enabled: true,
  } as PluginStatus,
];
providerCatalogApi.state = async () => catalogState();
providerSourceApi.state = async (query) => {
  control.calls.push("source-state");
  if (mode === "saved-unreadable") throw Error("原来源文件尚不可核查");
  return {
    providerId: query.providerId,
    expectedRevision: query.expectedRevision,
    sourceRevision: control.token,
    kind: mode === "saved-changed" ? "changed" : "external",
    choice: { pluginId, digest },
    selection: null,
    authorized: false,
    policy: mode === "saved-changed" ? null : policy,
    errorCode:
      mode === "saved-changed"
        ? "catalog_source_changed"
        : "catalog_source_authorization",
  };
};
providerSourceApi.select = async (query, sourceRevision, catalog) => {
  control.sourceSelections.push([query, sourceRevision, catalog]);
  return {
    providerId: query.providerId,
    expectedRevision: query.expectedRevision,
    sourceRevision: "c".repeat(64),
    kind: "external",
    choice: { pluginId, digest },
    selection: catalog,
    authorized: true,
    policy,
    errorCode: null,
  };
};
scopedProviderCatalogApi.change = async (input) => {
  invalidateCapabilityAccess();
  control.calls.push(input.request.action.kind);
  const action = input.request.action;
  if (action.kind === "bind") {
    control.binds.push(action.target);
    const target = action.target;
    const binding = {
      bindingId: crypto.randomUUID(),
      digest,
      label: target.kind === "draft" ? target.label : provider.name,
      endpoint: target.kind === "draft" ? target.endpoint : provider.endpoint,
      protocol: target.kind === "draft" ? target.protocol : provider.protocol,
      authorized: true,
      allowPrivateNetwork: action.allowPrivateNetwork,
      providerId: target.kind === "saved" ? target.providerId : null,
      expectedRevision:
        target.kind === "saved" ? target.expectedRevision : null,
    };
    bindings = [binding];
    if (mode === "bind-hold" || control.holdBind)
      await new Promise<void>((resolve) => {
        control.release = resolve;
      });
  } else if (action.kind === "unbind") {
    control.unbinds.push(action.bindingId);
    bindings = bindings.filter((b) => b.bindingId !== action.bindingId);
  }
  return catalogState();
};
scopedProviderCatalogApi.read = async () => {
  control.reads++;
  return {
    models: [{ id: "external-model", name: "外部目录模型" }],
    sourceUrl: provider.endpoint + "/catalog",
    fetchedAt: "2026-11-01T00:00:00Z",
    truncated: false,
  };
};
providersApi.providerModels = async () => {
  control.legacyReads++;
  control.nativeReads++;
  return {
    models: [{ id: "native-model", name: "内置目录模型" }],
    sourceUrl: provider.endpoint + "/models",
    fetchedAt: "2026-11-01T00:00:00Z",
    truncated: false,
  };
};
control.operations = installProviderOperationFixture({
  draftRecovery: async (action) => {
    control.recoveryCalls.push(action.kind);
    const storeId =
      action.kind === "list" ? action.storeId : action.owner.storeId;
    if (storeId !== control.operations!.snapshot().storeId)
      throw Error("此归属属于原数据库，本次未执行");
    const entry = (id: string, inspect = false): DraftRecoveryEntry => {
      const record = owners.get(id)!;
      const operation = record.operationId
        ? control.operations!.snapshot().stores[record.storeId].operations[
            record.operationId
          ]
        : null;
      return {
        owner: { ownerId: id, storeId: record.storeId },
        label: record.scope.label,
        pluginId,
        digest,
        endpointOrigin: new URL(record.scope.draft.endpoint).origin,
        ownership: !record.selection
          ? "prepared"
          : record.shared
            ? "shared"
            : "managed",
        operationId: record.operationId,
        state: !inspect
          ? record.operationId
            ? "unchecked"
            : "unattached"
          : !record.operationId
            ? "unattached"
            : (operation?.status ?? "not-found"),
        canFinish:
          inspect &&
          (!record.operationId ||
            (!!operation && operation.status !== "pending")),
      };
    };
    const result =
      action.kind === "list"
        ? {
            kind: "listed",
            storeId,
            entries: [...owners.entries()]
              .filter(([, r]) => r.storeId === storeId)
              .map(([id]) => entry(id)),
          }
        : action.kind === "inspect"
          ? {
              kind: "inspected",
              inspection: {
                entry: entry(action.owner.ownerId, true),
                request:
                  owners.get(action.owner.ownerId)!.request?.operation ?? null,
              },
            }
          : null;
    if (control.holdRecovery === action.kind)
      await new Promise<void>((resolve) => {
        control.release = resolve;
      });
    if (
      control.badRecoveryReply &&
      result &&
      "inspection" in result &&
      result.inspection
    )
      result.inspection.entry.owner.storeId = crypto.randomUUID();
    if (action.kind !== "finish") return result;
    const record = owners.get(action.owner.ownerId);
    const released = !record || entry(action.owner.ownerId, true).canFinish;
    if (record && released) {
      if (
        record.selection &&
        !record.shared &&
        ![...owners.values()].some(
          (r) =>
            r !== record &&
            r.selection?.bindingId === record.selection?.bindingId,
        )
      ) {
        control.unbinds.push(record.selection.bindingId);
        bindings = bindings.filter(
          (b) => b.bindingId !== record.selection?.bindingId,
        );
      }
      owners.delete(action.owner.ownerId);
    }
    return { kind: "finished", owner: action.owner, released };
  },
  draftGrant: async (action): Promise<DraftGrantReceipt> => {
    control.ownerCalls.push(action.kind);
    if (action.kind === "open") {
      const owner = { storeId: action.storeId, ownerId: crypto.randomUUID() };
      owners.set(owner.ownerId, {
        scope: action.scope,
        selection: null,
        shared: false,
        operationId: null,
        storeId: owner.storeId,
        request: null,
      });
      return { owner, state: "prepared", selection: null };
    }
    const owner = action.owner,
      record = owners.get(owner.ownerId);
    if (!record) {
      if (action.kind === "release")
        return { owner, state: "released", selection: null };
      throw Error("草稿归属已结束");
    }
    if (action.kind === "acquire") {
      if (revokedOwners.has(owner.ownerId))
        throw Error("旧归属已撤销，请重新授权");
      if (!record.selection) {
        const d = record.scope.draft;
        const target: Target = {
          kind: "draft",
          label: record.scope.label,
          ...d,
        };
        let binding = bindings.find(
          (b) =>
            b.endpoint === d.endpoint &&
            b.protocol === d.protocol &&
            !b.providerId,
        );
        if (binding) {
          const existingId = binding.bindingId;
          record.shared = ![...owners.values()].some(
            (r) => !r.shared && r.selection?.bindingId === existingId,
          );
        } else {
          control.binds.push(target);
          binding = {
            bindingId: crypto.randomUUID(),
            digest,
            label: record.scope.label,
            endpoint: d.endpoint,
            protocol: d.protocol,
            authorized: true,
            allowPrivateNetwork: record.scope.allowPrivateNetwork,
            providerId: null,
            expectedRevision: null,
          };
          bindings.push(binding);
        }
        record.selection = {
          ...record.scope.catalog,
          target,
          bindingId: binding.bindingId,
        };
      }
      if (mode === "bind-hold" || control.holdBind)
        await new Promise<void>((resolve) => {
          control.release = resolve;
        });
      if (control.loseAcquireReply) throw Error("合成授权回复丢失");
      return {
        owner: control.badOwnerReply
          ? { ...owner, storeId: crypto.randomUUID() }
          : owner,
        state: record.shared ? "borrowed" : "acquired",
        selection: record.selection,
      };
    }
    if (action.kind === "attach") {
      record.operationId = action.request.operation.operationId;
      record.request = structuredClone(action.request);
      if (control.holdAttach)
        await new Promise<void>((resolve) => {
          control.release = resolve;
        });
      return { owner, state: "pending", selection: record.selection };
    }
    if (record.operationId) {
      const backend = control.operations?.snapshot().stores[owner.storeId];
      const operation = backend?.operations[record.operationId];
      if (!operation || operation.status === "pending")
        return { owner, state: "pending", selection: record.selection };
    }
    if (
      record.selection &&
      !record.shared &&
      ![...owners.values()].some(
        (r) =>
          r !== record &&
          r.selection?.bindingId === record.selection?.bindingId,
      )
    ) {
      control.unbinds.push(record.selection.bindingId);
      bindings = bindings.filter(
        (b) => b.bindingId !== record.selection?.bindingId,
      );
    }
    owners.delete(owner.ownerId);
    return { owner, state: "released", selection: null };
  },
  beforeProtocolReply: () => {
    if (control.retireOnProtocol) {
      control.retireOnProtocol = false;
      control.invalidate();
    }
  },
  catalog: async ({ kind, request }) => {
    if (kind === "check") {
      control.draftChecks++;
      if (control.holdCheck)
        await new Promise<void>((resolve) => {
          control.release = resolve;
        });
    } else {
      control.draftRequests.push(structuredClone(request));
      if (control.holdRead)
        await new Promise<void>((resolve) => {
          control.release = resolve;
        });
    }
    if (
      control.rejectCheck ||
      (request.origin.kind === "saved" &&
        request.origin.sourceRevision !== control.token)
    )
      throw Error("原目录来源已变化");
    if (kind === "check")
      return control.badCheckReply ? { authorized: true } : null;
    const external = request.source.kind === "external";
    if (external) control.reads++;
    else control.nativeReads++;
    return {
      models: [
        {
          id: external ? "external-model" : "native-model",
          name: external ? "外部目录模型" : "内置目录模型",
        },
      ],
      sourceUrl: request.draft.endpoint + (external ? "/catalog" : "/models"),
      fetchedAt: "2026-11-01T00:00:00Z",
      truncated: false,
    };
  },
  mode:
    mode === "lost"
      ? "operation-query-unavailable"
      : mode === "ownership-rejected"
        ? "operation-admission-rejected"
        : mode === "ownership-pending"
          ? "operation-pending"
          : undefined,
  save: async (p) => {
    control.saves.push(p);
  },
  connect: async (p) => {
    control.saves.push(p);
    return {
      provider: { ...p, configured: true, revision: p.revision + 1 },
      verification: {
        providerId: p.id,
        fingerprint: "f".repeat(64),
        model: p.model,
        state: "verified",
        checkedAt: "2026-11-01T00:00:00Z",
        message: "Synthetic verification",
      },
    };
  },
});
Object.assign(window, { editorSourceFixture: control });
declare global {
  interface Window {
    editorSourceFixture: typeof control;
  }
}
function Harness() {
  const [open, setOpen] = useState(true);
  control.closeEditor = () => setOpen(false);
  return (
    <main className="p-4">
      {open ? (
        <ProviderEditor
          provider={provider}
          storeId={PROVIDER_FIXTURE_STORE_ID}
          onClose={() => setOpen(false)}
          onSaved={async () => setOpen(false)}
        />
      ) : (
        <p>连接已保存或关闭</p>
      )}
      <ProviderOperationRecovery
        storeId={PROVIDER_FIXTURE_STORE_ID}
        onUpdated={async () => {}}
      />
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);
