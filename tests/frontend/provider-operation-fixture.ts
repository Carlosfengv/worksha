import { mockIPC } from "@tauri-apps/api/mocks";
import {
  discoveryFixturePage,
  DISCOVERY_FIXTURE_ID,
} from "./credential-discovery-fixture";
import { DesktopError } from "@/shared/lib/desktop-command";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";
import {
  builtinPresetAdvice,
  presetKey,
} from "@/features/providers/preset-advice";
import {
  sameProviderRequest,
  type ProviderOperation,
  type ProviderWriteRequest,
} from "@/features/providers/operation-receipt";
import type {
  Provider,
  ProviderVerification,
} from "@/features/providers/types";
export const PROVIDER_FIXTURE_STORE_ID = "c9e027fe-4536-4d08-8a55-74e6cbaf6313";
export const PROVIDER_FIXTURE_OTHER_STORE_ID =
  "6087597b-0625-42ea-8fd0-e51f6ad16d01";
export const PROVIDER_FIXTURE_NAMESPACE_ID =
  "7025064b-5ff9-4f40-8b55-946614da682b";
export const PROVIDER_FIXTURE_OTHER_NAMESPACE_ID =
  "f776744f-9a29-4498-9588-fbf3bfb07444";
interface Backend {
  sequence: number;
  probes: number;
  commits: number;
  operations: Record<string, ProviderOperation>;
  namespaceId: string;
  namespaceState: "active" | "unverified";
  legacyState: "fresh" | "imported" | "unverified";
  legacyClaims?: Record<
    string,
    {
      sourceDigest: string;
      stage: "prepared" | "fenced" | "committed";
      phase: "ready" | "cancelled";
    }
  >;
}
interface Persisted {
  storeId: string;
  stores: Record<string, Backend>;
}
export function installProviderOperationFixture(options: {
  presets?: (
    action: import("../../contracts/domain/provider-presets/types").PresetAction,
  ) => Promise<unknown>;
  beforeProtocolReply?: () => void;
  draftGrant?: (
    action: import("../../contracts/domain/provider-draft-grant/types").DraftGrantAction,
  ) => Promise<unknown>;
  draftRecovery?: (
    action: import("../../contracts/domain/provider-draft-recovery/types").DraftRecoveryAction,
  ) => Promise<unknown>;
  catalog?: (
    action: import("../../contracts/domain/provider-draft-catalog/types").DraftCatalogAction,
  ) => Promise<unknown>;
  save: (provider: Provider) => Promise<void>;
  connect: (
    provider: Provider,
  ) => Promise<{ provider: Provider; verification: ProviderVerification }>;
  mode?: string | null;
}) {
  const key = `fixture.provider-backend.v1:${location.pathname}${location.search}`;
  const fresh = (id = PROVIDER_FIXTURE_STORE_ID): Backend => ({
    sequence: 0,
    probes: 0,
    commits: 0,
    operations: {},
    legacyClaims:
      options.mode === "discovery-prepared" ||
      options.mode === "discovery-committed"
        ? {
            [DISCOVERY_FIXTURE_ID]: {
              sourceDigest: "a".repeat(64),
              stage:
                options.mode === "discovery-prepared"
                  ? "prepared"
                  : "committed",
              phase: "ready",
            },
          }
        : undefined,
    namespaceId:
      id === PROVIDER_FIXTURE_STORE_ID
        ? PROVIDER_FIXTURE_NAMESPACE_ID
        : PROVIDER_FIXTURE_OTHER_NAMESPACE_ID,
    namespaceState: options.mode?.startsWith("credential-unverified")
      ? "unverified"
      : "active",
    legacyState: options.mode?.startsWith("credential-unverified")
      ? "unverified"
      : "fresh",
  });
  let persisted: Persisted = {
    storeId: PROVIDER_FIXTURE_STORE_ID,
    stores: { [PROVIDER_FIXTURE_STORE_ID]: fresh() },
  };
  try {
    const raw = sessionStorage.getItem(key);
    if (raw) persisted = JSON.parse(raw) as Persisted;
  } catch {
    /* Synthetic fixture state only. */
  }
  const persist = () => sessionStorage.setItem(key, JSON.stringify(persisted));
  let release: (() => void) | undefined;
  let discoveryReads = 0;
  const control = {
    failInitialization: options.mode === "credential-unverified-init-error",
    commands: [] as { name: string; args: unknown }[],
    loseReplies:
      options.mode === "operation-lost" ||
      options.mode === "operation-query-unavailable" ||
      options.mode === "operation-pending",
    failQueries: options.mode === "operation-query-unavailable",
    corrupt: null as
      "version" | "request" | "provider" | "status" | "verification" | null,
    switchStore(id: string) {
      persisted.storeId = id;
      persisted.stores[id] ??= fresh(id);
      persist();
      notifyBackendRecovery();
    },
    switchNamespace(id: string) {
      persisted.stores[persisted.storeId].namespaceId = id;
      persist();
      notifyBackendRecovery();
    },
    release() {
      release?.();
      release = undefined;
    },
    snapshot() {
      return structuredClone(persisted);
    },
  };
  const fail = (
    code: string,
    message: string,
    outcomeUnknown = false,
  ): never => {
    throw { code, message, outcomeUnknown };
  };
  const outgoing = (operation: ProviderOperation) => {
    const value = structuredClone(operation);
    if (control.corrupt === "version")
      (value as { apiVersion: number }).apiVersion = 2;
    if (control.corrupt === "request")
      value.request.storeId = PROVIDER_FIXTURE_OTHER_STORE_ID;
    if (control.corrupt === "provider" && value.provider)
      value.provider.revision = Number.MAX_SAFE_INTEGER + 1;
    if (control.corrupt === "status")
      (value as { status: string }).status = "future";
    if (control.corrupt === "verification" && value.verification)
      value.verification.fingerprint = "invalid";
    return value;
  };
  Object.defineProperty(globalThis, "isTauri", {
    value: true,
    configurable: true,
  });
  mockIPC(async (name, args) => {
    if (name === "discover_provider_credentials") {
      control.commands.push({ name, args });
      const request = (args as { request: Record<string, unknown> }).request;
      if (
        Object.keys(request).sort().join(",") !== "cursor,namespaceId,storeId"
      )
        return fail("invalid_credential_operation", "发现请求字段不匹配");
      const source = persisted.stores[persisted.storeId];
      if (request.storeId !== persisted.storeId)
        return fail("credential_store_changed", "原后台已变化");
      if (request.namespaceId !== source.namespaceId)
        return fail("credential_namespace_changed", "原恢复空间已变化");
      if (options.mode === "discovery-fail")
        return fail(
          "credential_discovery_unavailable",
          "后台记录暂时无法完整读取",
        );
      const read = ++discoveryReads;
      const value = discoveryFixturePage(request, options.mode, read);
      if (
        options.mode === "discovery-hold" ||
        (options.mode === "discovery-restore-hold" && read === 2)
      )
        await new Promise<void>((done) => {
          release = done;
        });
      return value;
    }
    if (
      name === "preview_legacy_provider_credential" ||
      name === "confirm_legacy_provider_credential"
    ) {
      control.commands.push({ name, args });
      const input = (
        args as {
          request: {
            storeId: string;
            namespaceId: string;
            operationId: string;
            sourceDigest?: string;
            confirmed?: boolean;
          };
        }
      ).request;
      const fields = Object.keys(input).sort().join(",");
      const expectedFields =
        name === "preview_legacy_provider_credential"
          ? "namespaceId,operationId,storeId"
          : "confirmed,namespaceId,operationId,sourceDigest,storeId";
      if (fields !== expectedFields)
        return fail("invalid_credential_operation", "旧记录请求字段不匹配");
      if (options.mode === "legacy-weak-backend")
        return fail(
          "credential_protocol_unavailable",
          "当前后台尚未提供旧凭据持久防重，未发送密钥或确认。",
        );
      const backend = persisted.stores[persisted.storeId];
      if (input.storeId !== persisted.storeId)
        return fail("credential_store_changed", "旧记录原后台已变化");
      if (input.namespaceId !== backend.namespaceId)
        return fail("credential_namespace_changed", "旧记录原空间已变化");
      const claims = (backend.legacyClaims ??= {});
      let claim = claims[input.operationId];
      if (name === "confirm_legacy_provider_credential") {
        if (options.mode === "legacy-fail-before")
          return fail(
            "credential_recovery_unavailable",
            "迁移结果尚未确认",
            true,
          );
        if (!input.confirmed || input.sourceDigest !== "a".repeat(64))
          return fail("credential_legacy_source_changed", "原材料已变化");
        const wasFenced = claim?.stage === "fenced";
        claim = claims[input.operationId] = {
          sourceDigest: input.sourceDigest,
          stage: "fenced",
          phase: options.mode === "legacy-cancelled" ? "cancelled" : "ready",
        };
        persist();
        if (options.mode === "legacy-hold")
          await new Promise<void>((resolve) => {
            release = resolve;
          });
        if (options.mode === "legacy-pending" && !wasFenced)
          return fail(
            "credential_migration_pending",
            "旧凭据迁移尚未完成，请继续恢复",
            true,
          );
        claim.stage = "committed";
        persist();
        if (options.mode === "legacy-lost")
          return fail("transport_connection", "模拟迁移回复丢失", true);
      }
      return {
        version: 2,
        storeId: input.storeId,
        namespaceId: input.namespaceId,
        operationId: input.operationId,
        sourceDigest: claim?.sourceDigest ?? "a".repeat(64),
        stage: claim?.stage ?? "unclaimed",
        phase: claim?.phase ?? "ready",
        label: claim?.phase === "cancelled" ? null : "Legacy fixture",
      };
    }
    if (
      name === "get_provider_credential_protocol_v3" ||
      name === "initialize_provider_credential_namespace"
    ) {
      control.commands.push({ name, args });
      const input = (
        name === "initialize_provider_credential_namespace"
          ? (
              args as {
                request: {
                  storeId: string;
                  namespaceId: string;
                  choice: string;
                };
              }
            ).request
          : args
      ) as { storeId: string; namespaceId?: string; choice?: string };
      if (input.storeId !== persisted.storeId)
        return fail("credential_store_changed", "凭据来源已变化");
      if (options.mode === "credential-v3-unsupported")
        return fail(
          "credential_protocol_unavailable",
          "后台尚未支持原恢复空间核查",
        );
      const backend = persisted.stores[persisted.storeId];
      if (name === "initialize_provider_credential_namespace") {
        if (input.namespaceId !== backend.namespaceId)
          return fail("credential_namespace_changed", "原恢复空间已变化");
        if (control.failInitialization)
          return fail(
            "credential_recovery_unavailable",
            "新 Key 保存准备暂不可用",
          );
        if (
          options.mode === "credential-unverified-restore-error" &&
          input.choice === "restore"
        )
          return fail(
            "credential_namespace_unverified",
            "原恢复材料暂不可用，已保留原记录",
          );
        if (options.mode === "credential-unverified-init-hold")
          await new Promise<void>((resolve) => {
            release = resolve;
          });
        backend.namespaceState = "active";
        if (input.choice === "restore") backend.legacyState = "imported";
        persist();
        if (options.mode === "credential-unverified-init-lost")
          return fail("transport_connection", "模拟初始化回复丢失", true);
      }
      return {
        version: 3,
        storeId: input.storeId,
        namespaceId: backend.namespaceId,
        namespaceState: backend.namespaceState,
        legacyState: backend.legacyState,
        operationLimit: 4096,
      };
    }
    if (name === "get_provider_credential_protocol") {
      const input = args as { storeId: string };
      if (input.storeId !== persisted.storeId)
        return fail("credential_store_changed", "凭据来源已变化");
      return { version: 2, storeId: input.storeId, operationLimit: 4096 };
    }
    control.commands.push({ name, args });
    const backend = persisted.stores[persisted.storeId];
    if (name === "get_provider_write_protocol") {
      options.beforeProtocolReply?.();
      if (options.mode === "operation-unsupported") return { version: 0 };
      return {
        version: 1,
        storeId: persisted.storeId,
        sequence: backend.sequence,
        receiptLimit: 256,
        receiptBytes: 16 * 1024 * 1024,
        pendingLimit: 4,
        pendingOperationIds: Object.values(backend.operations)
          .filter((op) => op.status === "pending")
          .map((op) => op.request.operationId),
      };
    }
    let input = args as Record<string, unknown>;
    if (name === "query_provider_presets") {
      const action =
        input.input as import("../../contracts/domain/provider-presets/types").PresetAction;
      if (action.storeId !== persisted.storeId)
        return fail("operation_store_changed", "预设属于原数据库");
      if (options.presets) return options.presets(action);
      if (action.kind === "list")
        return {
          kind: "listed",
          storeId: action.storeId,
          items: structuredClone(builtinPresetAdvice),
          issues: [],
        };
      const advice = builtinPresetAdvice.find(
        (p) => presetKey(p.origin) === presetKey(action.origin),
      );
      if (!advice) return fail("plugin_disabled", "此预设暂不可选");
      return {
        kind: "resolved",
        storeId: action.storeId,
        advice: structuredClone(advice),
      };
    }
    if (name === "perform_provider_draft_recovery") {
      if (!options.draftRecovery)
        return fail("fixture_missing", "未声明草稿授权恢复夹具");
      return options.draftRecovery(
        input.input as import("../../contracts/domain/provider-draft-recovery/types").DraftRecoveryAction,
      );
    }
    if (name === "perform_provider_draft_grant") {
      if (!options.draftGrant)
        return fail("fixture_missing", "未声明草稿授权夹具");
      return options.draftGrant(
        input.input as import("../../contracts/domain/provider-draft-grant/types").DraftGrantAction,
      );
    }
    if (name === "perform_provider_draft_catalog") {
      if (!options.catalog)
        return fail("fixture_missing", "未声明草稿目录夹具");
      return options.catalog(
        input.input as import("../../contracts/domain/provider-draft-catalog/types").DraftCatalogAction,
      );
    }
    if (name === "perform_provider_catalog_source") {
      const action = input.input as {
        kind: string;
        query: {
          providerId: string;
          expectedRevision: string;
          storeId: string;
        };
      };
      if (action.kind !== "state" || action.query.storeId !== persisted.storeId)
        return fail("operation_store_changed", "合成来源不匹配");
      return {
        providerId: action.query.providerId,
        expectedRevision: action.query.expectedRevision,
        sourceRevision: null,
        kind: "builtin",
        choice: null,
        selection: null,
        authorized: true,
        policy: null,
        errorCode: null,
      };
    }
    if (name === "perform_provider_source_operation") {
      const action =
        input.input as import("../../contracts/domain/provider-source-operation/types").SourceOperationAction;
      const request = action.request.operation;
      if (action.kind === "query") {
        input = { storeId: request.storeId, operationId: request.operationId };
        name = "get_provider_operation";
      } else {
        input = { request };
        name = "commit_provider_operation";
      }
    }
    if (name === "get_provider_operation") {
      if (input.storeId !== persisted.storeId)
        return fail("operation_store_changed", "操作属于其他数据库");
      if (control.failQueries)
        return fail("service_unavailable", "合成查询暂不可用");
      const operation = backend.operations[String(input.operationId)];
      return operation
        ? outgoing(operation)
        : fail("operation_not_found", "未查到原操作");
    }
    if (
      name !== "commit_provider_operation" &&
      name !== "cancel_provider_operation"
    )
      return fail(
        "unsupported_fixture_command",
        `Unexpected fixture command ${name}`,
      );
    const request = structuredClone(input.request) as ProviderWriteRequest;
    if (request.storeId !== persisted.storeId)
      return fail("operation_store_changed", "操作属于其他数据库");
    const old = backend.operations[request.operationId];
    if (old && !sameProviderRequest(old.request, request))
      return fail("operation_conflict", "同号输入不同");
    if (name === "cancel_provider_operation") {
      if (!old && request.expectedSequence !== backend.sequence)
        return fail("operation_not_found", "未查到过期原操作");
      const operation =
        old ??
        ({
          apiVersion: 1,
          sequence: ++backend.sequence,
          status: "cancelled",
          request,
          provider: null,
          verification: null,
          error: null,
        } satisfies ProviderOperation);
      if (operation.status === "pending") operation.status = "cancelled";
      backend.operations[request.operationId] = operation;
      persist();
      return outgoing(operation);
    }
    if (old) return outgoing(old);
    if (options.mode === "operation-admission-rejected")
      return fail("capacity", "合成容量拒绝");
    if (
      options.mode === "operation-sequence-race" &&
      control.commands.filter((row) => row.name === "commit_provider_operation")
        .length === 1
    ) {
      backend.sequence++;
      persist();
    }
    if (request.expectedSequence !== backend.sequence)
      return fail("operation_expired", "旧操作序号不可重放");
    const operation: ProviderOperation = {
      apiVersion: 1,
      sequence: ++backend.sequence,
      status: "pending",
      request,
      provider: null,
      verification: null,
      error: null,
    };
    backend.operations[request.operationId] = operation;
    persist();
    const perform = async () => {
      try {
        let saved: Provider;
        let verification: ProviderVerification | null = null;
        if (request.kind === "connect") {
          backend.probes++;
          persist();
          const connected = await options.connect(request.provider);
          saved = connected.provider;
          verification = {
            ...connected.verification,
            fingerprint: "f".repeat(64),
          };
          if (options.mode === "operation-pending")
            await new Promise<void>((resolve) => {
              release = resolve;
            });
        } else {
          await options.save(request.provider);
          saved = {
            ...request.provider,
            configured: true,
            revision: request.provider.revision + 1,
          };
        }
        if (operation.status === "cancelled") return operation;
        backend.commits++;
        operation.status = "committed";
        operation.provider = saved;
        operation.verification = verification;
      } catch (reason) {
        if (operation.status !== "cancelled") {
          const error =
            reason instanceof DesktopError ? reason : new DesktopError(reason);
          operation.status = "failed";
          operation.error = { code: error.code, message: error.message };
        }
      }
      persist();
      return operation;
    };
    if (options.mode === "operation-pending") {
      void perform();
      return fail("transport_timeout", "合成回复丢失，原请求仍在处理", true);
    }
    const completed = await perform();
    if (control.loseReplies)
      return fail("transport_timeout", "合成提交回复丢失", true);
    return outgoing(completed);
  });
  return control;
}
declare global {
  interface Window {
    providerOperationsFixture: ReturnType<
      typeof installProviderOperationFixture
    >;
  }
}
