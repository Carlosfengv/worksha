import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
import { routingQuotaLabels } from "@/features/accounts/display";
import type {
  PresetAction,
  PresetReply,
} from "../../../contracts/domain/provider-presets/types";
import presetActionSchema from "../../../contracts/domain/provider-presets/action.schema.json";
import presetReplySchema from "../../../contracts/domain/provider-presets/reply.schema.json";
import {
  builtinPresetAdvice,
  presetKey,
  validatePresetAdvice,
  type PresetOrigin,
} from "./preset-advice";
import type { DraftCatalogRequest } from "../../../contracts/domain/provider-draft-catalog/types";
import draftCatalogRequestSchema from "../../../contracts/domain/provider-draft-catalog/request.schema.json";
import type {
  DraftGrantAction,
  DraftGrantReceipt,
} from "../../../contracts/domain/provider-draft-grant/types";
import draftGrantActionSchema from "../../../contracts/domain/provider-draft-grant/action.schema.json";
import draftGrantReceiptSchema from "../../../contracts/domain/provider-draft-grant/receipt.schema.json";
import type {
  DraftRecoveryAction,
  DraftRecoveryEntry,
  DraftRecoveryReply,
} from "../../../contracts/domain/provider-draft-recovery/types";
import draftRecoveryActionSchema from "../../../contracts/domain/provider-draft-recovery/action.schema.json";
import draftRecoveryReplySchema from "../../../contracts/domain/provider-draft-recovery/reply.schema.json";
import type {
  SourceAction,
  SourceQuery,
  SourceState,
} from "../../../contracts/domain/provider-source/types";
import sourceStateSchema from "../../../contracts/domain/provider-source/state.schema.json";
import { parseSchema } from "@/shared/lib/contract-schema";
import type {
  Query as ProviderCatalogQuery,
  Request as ProviderCatalogRequest,
} from "../../../contracts/domain/provider-access/types";
import { parseProviderCatalogState, pluginsApi } from "@/features/plugins/api";
import type {
  Provider,
  ProviderImportCandidate,
  ProviderModelCatalog,
  ProviderStatus,
  ProviderTransaction,
  ProviderVerification,
  ProviderUsage,
  ProviderUsageQuery,
  DynamicChange,
} from "@/features/providers/types";
import { command, DesktopError } from "@/shared/lib/desktop-command";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { providerModelCatalog } from "./catalog-response";
export { providerModelCatalog } from "./catalog-response";
import { providerUsage } from "./usage-response";
import {
  selectedProviderCatalog,
  selectedProviderRoute,
  isExplicitProviderSource,
} from "./catalog-operation";
import {
  providerDynamicChange,
  providerImportCandidates,
  providerStatus,
  providerTransaction,
  providerValue,
  providerVerification,
} from "./status-response";
import { nullable, response, shape } from "./response-values";
import {
  acceptLocalTokenPreview,
  LocalTokenPreparationError,
  localTokenIdentity,
  localTokenProtocol,
  localTokenResult,
  requireLocalTokenSource,
  stageLocalToken,
  type LocalTokenHint,
  type LocalTokenIdentity,
  type LocalTokenSource,
} from "./local-token-operation";
import {
  nativeImportPreview,
  nativeImportOperation,
  nativeImported,
  type NativeImportRequest,
} from "./native-import-operation";
import {
  credentialDiscoveryPage,
  credentialDiscoveryCursor,
  type CredentialDiscoveryRequest,
} from "./credential-discovery";
import {
  legacyCredentialPreview,
  validLegacyDigest,
  type LegacyCredentialConfirmation,
} from "./legacy-credential-receipt";
import {
  providerProtocol,
  providerOperation,
  storedProviderRequest,
  type ProviderWriteRequest,
} from "./operation-receipt";
import {
  credentialOperation,
  isCredentialOperationId,
  credentialReceipt,
  type CredentialReceipt,
  type CredentialOperation,
  scopedCredentialProtocol,
  scopedCredentialOperation,
  credentialNamespace,
  credentialResultV3,
  type CredentialIdentityV3,
  type CredentialInitialization,
} from "./credential-receipt";

async function readProviderOperation(operationId: string, storeId: string) {
  await providersApi.writeProtocol(storeId);
  return command<unknown>("get_provider_operation", { operationId, storeId });
}

function credentialIdentity(storeId: string, operationId: string) {
  if (
    !isCredentialOperationId(storeId) ||
    !isCredentialOperationId(operationId)
  )
    throw new DesktopError({
      code: "invalid_credential_operation",
      outcomeUnknown: false,
      message: "凭据来源或操作编号无效，未发送请求。",
    });
}
function checkNamespace(namespaceId: string) {
  if (!isCredentialOperationId(namespaceId))
    throw new DesktopError({
      code: "invalid_credential_operation",
      outcomeUnknown: false,
      message: "原凭据恢复空间无效，未发送请求。",
    });
}
async function credentialNamespacePreflight(
  storeId: string,
  namespaceId: string,
) {
  checkNamespace(namespaceId);
  const protocol = await providersApi.credentialNamespace(storeId);
  if (protocol.namespaceId !== namespaceId)
    throw new DesktopError({
      code: "credential_namespace_changed",
      outcomeUnknown: false,
      message: "原凭据恢复空间已变化，未将原请求转到新空间。",
    });
  return protocol;
}

export const providersApi = {
  async upstreamAccount(
    providerId: string,
    input: import("./types").UpstreamAccountAction,
  ) {
    const reply = await command<{
      storeId: string;
      accounts: import("./types").UpstreamAccount[];
    }>("perform_upstream_account", { providerId, input });
    if (
      !isCredentialOperationId(reply?.storeId) ||
      ("storeId" in input && reply.storeId !== input.storeId) ||
      !Array.isArray(reply.accounts) ||
      reply.accounts.length > 32 ||
      reply.accounts.some(
        (a) =>
          !a ||
          a.providerId !== providerId ||
          !isCredentialOperationId(a.id) ||
          !Number.isSafeInteger(a.revision) ||
          a.revision < 1 ||
          typeof a.label !== "string" ||
          !["enabled", "paused", "revoked"].includes(a.state) ||
          (a.routingQuota !== undefined &&
            !Object.hasOwn(routingQuotaLabels, a.routingQuota)) ||
          (a.verifications != null &&
            (!Array.isArray(a.verifications) ||
              a.verifications.some(
                (entry) =>
                  !entry ||
                  typeof entry.model !== "string" ||
                  !entry.model ||
                  ![
                    "verified",
                    "expired",
                    "failed",
                    "outdated",
                    "unknown",
                  ].includes(entry.state) ||
                  (entry.checkedAt !== null &&
                    (typeof entry.checkedAt !== "string" ||
                      !Number.isFinite(Date.parse(entry.checkedAt)))),
              ))) ||
          (a.cooldowns != null &&
            (!Array.isArray(a.cooldowns) ||
              a.cooldowns.some(
                (entry) =>
                  !entry ||
                  (entry.model !== null && typeof entry.model !== "string") ||
                  !Number.isSafeInteger(entry.untilMs) ||
                  entry.untilMs <= 0 ||
                  typeof entry.reason !== "string",
              ))),
      )
    )
      throw Error("上游账号响应无法核查，请刷新原后台。");
    if (input.kind !== "list")
      window.dispatchEvent(new Event("upstream-accounts-changed"));
    return reply;
  },
  async localTokenProtocol() {
    return localTokenProtocol(
      await command<unknown>("get_local_token_protocol"),
    );
  },
  async queryLocalToken(input: LocalTokenIdentity, hint?: LocalTokenHint) {
    const epoch = backendReadEpoch();
    const value = await command<unknown>("get_local_token_operation", {
      input: localTokenIdentity(input),
    });
    requireLocalTokenSource({ identity: input, epoch, active: true });
    return localTokenResult(value, input, hint);
  },
  async continueLocalToken(input: LocalTokenIdentity, hint?: LocalTokenHint) {
    const epoch = backendReadEpoch();
    const value = await command<unknown>("continue_local_token_operation", {
      input: localTokenIdentity(input),
    });
    requireLocalTokenSource({ identity: input, epoch, active: true }, true);
    return localTokenResult(value, input, hint);
  },
  async cancelLocalToken(input: LocalTokenIdentity, hint?: LocalTokenHint) {
    const epoch = backendReadEpoch();
    const value = await command<unknown>("cancel_local_token_operation", {
      input: localTokenIdentity(input),
    });
    requireLocalTokenSource({ identity: input, epoch, active: true }, true);
    return localTokenResult(value, input, hint);
  },
  async discoverCredentials(request: CredentialDiscoveryRequest) {
    if (!isCredentialOperationId(request.storeId))
      throw new DesktopError({
        code: "invalid_credential_operation",
        outcomeUnknown: false,
        message: "凭据发现来源无效，未发送请求。",
      });
    checkNamespace(request.namespaceId);
    if (request.cursor !== null && !credentialDiscoveryCursor(request.cursor))
      throw Error("凭据发现游标无效");
    const value = await command<unknown>("discover_provider_credentials", {
      request: {
        storeId: request.storeId,
        namespaceId: request.namespaceId,
        cursor: request.cursor,
      },
    });
    return credentialDiscoveryPage(value, request);
  },
  async previewLegacyCredential(request: CredentialIdentityV3) {
    credentialIdentity(request.storeId, request.operationId);
    const protocol = await credentialNamespacePreflight(
      request.storeId,
      request.namespaceId,
    );
    if (protocol.namespaceState !== "active")
      throw new DesktopError({
        code: "credential_namespace_unverified",
        outcomeUnknown: false,
        message: "请先核查当前凭据恢复空间。",
      });
    const identity = {
      storeId: request.storeId,
      namespaceId: request.namespaceId,
      operationId: request.operationId,
    };
    const value = await command<unknown>("preview_legacy_provider_credential", {
      request: identity,
    });
    if (
      value &&
      typeof value === "object" &&
      "version" in value &&
      value.version !== 2
    )
      throw new DesktopError({
        code: "credential_protocol_unavailable",
        outcomeUnknown: false,
        message:
          "当前后台尚未提供旧凭据持久防重，请正常退出旧版服务后升级；未发送确认。",
      });
    return legacyCredentialPreview(value, identity);
  },
  async confirmLegacyCredential(request: LegacyCredentialConfirmation) {
    if (request.confirmed !== true || !validLegacyDigest(request.sourceDigest))
      throw new DesktopError({
        code: "invalid_credential_operation",
        outcomeUnknown: false,
        message: "旧记录确认或材料摘要无效，未发送迁移。",
      });
    const preview = await providersApi.previewLegacyCredential(request);
    if (
      request.confirmed !== true ||
      preview.sourceDigest !== request.sourceDigest
    )
      throw new DesktopError({
        code: "credential_legacy_source_changed",
        outcomeUnknown: false,
        message: "原材料或确认来源已变化，请重新核查。",
      });
    const result = legacyCredentialPreview(
      await command<unknown>("confirm_legacy_provider_credential", {
        request: {
          storeId: request.storeId,
          namespaceId: request.namespaceId,
          operationId: request.operationId,
          sourceDigest: request.sourceDigest,
          confirmed: true,
        },
      }),
      request,
    );
    if (
      result.sourceDigest !== request.sourceDigest ||
      result.stage !== "committed"
    )
      throw new DesktopError({
        code: "transport_invalid_response",
        outcomeUnknown: true,
        message: "旧记录迁移尚未确认，请保留原来源和材料核查。",
      });
    return result;
  },
  async credentialNamespace(storeId: string | undefined) {
    if (!isCredentialOperationId(storeId))
      throw new DesktopError({
        code: "credential_origin_missing",
        outcomeUnknown: false,
        message: "凭据原数据库尚未核查。",
      });
    return credentialNamespace(
      await command<unknown>("get_provider_credential_protocol_v3", {
        storeId,
      }),
      storeId,
    );
  },
  async initializeCredentialNamespace(request: CredentialInitialization) {
    await credentialNamespacePreflight(request.storeId, request.namespaceId);
    const result = credentialNamespace(
      await command<unknown>("initialize_provider_credential_namespace", {
        request,
      }),
      request.storeId,
    );
    if (
      result.namespaceId !== request.namespaceId ||
      result.namespaceState !== "active" ||
      (request.choice === "restore" && result.legacyState === "unverified")
    )
      throw new DesktopError({
        code: "transport_invalid_response",
        outcomeUnknown: true,
        message: "尚未确认原恢复空间的核查结果，请保留原来源重新读取。",
      });
    return result;
  },
  async createCredentialV3(
    request: CredentialIdentityV3 & { label: string; secret: string },
  ) {
    credentialIdentity(request.storeId, request.operationId);
    const protocol = await credentialNamespacePreflight(
      request.storeId,
      request.namespaceId,
    );
    if (protocol.namespaceState !== "active")
      throw new DesktopError({
        code: "credential_namespace_unverified",
        outcomeUnknown: false,
        message: "请先核查原凭据恢复材料或明确建立新空间，未发送密钥。",
      });
    const result = credentialResultV3(
      await command<unknown>("create_provider_credential_v3", { request }),
      request,
    );
    if (
      !result.receipt ||
      result.status !== "ready" ||
      result.receipt.credentialLabel !== request.label.trim()
    )
      throw new DesktopError({
        code: "transport_invalid_response",
        outcomeUnknown: true,
        message: "凭据结果未能对应原名称，请按原恢复空间和编号核查。",
      });
    return result.receipt;
  },
  async credentialOperationV3(request: CredentialIdentityV3) {
    credentialIdentity(request.storeId, request.operationId);
    await credentialNamespacePreflight(request.storeId, request.namespaceId);
    return credentialResultV3(
      await command<unknown>("get_provider_credential_operation_v3", {
        request,
      }),
      request,
    );
  },
  async cancelCredentialOperationV3(request: CredentialIdentityV3) {
    credentialIdentity(request.storeId, request.operationId);
    await credentialNamespacePreflight(request.storeId, request.namespaceId);
    return credentialResultV3(
      await command<unknown>("cancel_provider_credential_operation_v3", {
        request,
      }),
      request,
    );
  },
  async credentialProtocol(storeId: string | undefined) {
    if (!isCredentialOperationId(storeId))
      throw new DesktopError({
        code: "credential_origin_missing",
        outcomeUnknown: false,
        message: "尚未核查凭据的原数据库，请连接原后台后操作。",
      });
    return scopedCredentialProtocol(
      await command<unknown>("get_provider_credential_protocol", { storeId }),
      storeId,
    );
  },
  async createScopedCredential(
    storeId: string,
    label: string,
    secret: string,
    operationId: string,
  ) {
    credentialIdentity(storeId, operationId);
    await providersApi.credentialProtocol(storeId);
    const result = scopedCredentialOperation(
      await command<unknown>("create_scoped_provider_credential", {
        storeId,
        operationId,
        label,
        secret,
      }),
      storeId,
      operationId,
    );
    if (
      !result.receipt ||
      result.status !== "ready" ||
      result.receipt.credentialLabel !== label.trim()
    )
      throw new DesktopError({
        code: "transport_invalid_response",
        outcomeUnknown: true,
        message: "凭据保存回复未能对应原名称，请按原库和编号核查。",
      });
    return result.receipt;
  },
  async scopedCredentialOperation(storeId: string, operationId: string) {
    credentialIdentity(storeId, operationId);
    await providersApi.credentialProtocol(storeId);
    return scopedCredentialOperation(
      await command<unknown>("get_scoped_provider_credential_operation", {
        storeId,
        operationId,
      }),
      storeId,
      operationId,
    );
  },
  async cancelScopedCredentialOperation(storeId: string, operationId: string) {
    credentialIdentity(storeId, operationId);
    await providersApi.credentialProtocol(storeId);
    return scopedCredentialOperation(
      await command<unknown>("cancel_scoped_provider_credential_operation", {
        storeId,
        operationId,
      }),
      storeId,
      operationId,
    );
  },
  async writeProtocol(expectedStoreId?: string) {
    if (!expectedStoreId)
      throw new DesktopError({
        code: "operation_origin_missing",
        outcomeUnknown: false,
        message: "尚未核查此配置的来源，请刷新原后台后再操作。",
      });
    const protocol = providerProtocol(
      await command<unknown>("get_provider_write_protocol"),
    );
    if (protocol.storeId !== expectedStoreId)
      throw new DesktopError({
        code: "operation_store_changed",
        outcomeUnknown: false,
        message: "草稿属于另一个本机数据库，请连接原后台后核查。",
      });
    return protocol;
  },
  async commitOperation(request: ProviderWriteRequest) {
    invalidateCapabilityAccess();
    await providersApi.writeProtocol(request.storeId);
    return providerOperation(
      await command<unknown>("commit_provider_operation", { request }),
      request,
    );
  },
  async operation(request: ProviderWriteRequest) {
    return providerOperation(
      await readProviderOperation(request.operationId, request.storeId),
      request,
    );
  },
  async cancelOperation(request: ProviderWriteRequest) {
    invalidateCapabilityAccess();
    await providersApi.writeProtocol(request.storeId);
    return providerOperation(
      await command<unknown>("cancel_provider_operation", { request }),
      request,
    );
  },
  async pendingOperation(operationId: string, storeId: string) {
    const value = await readProviderOperation(operationId, storeId);
    const request =
      value && typeof value === "object" && "request" in value
        ? storedProviderRequest(value.request)
        : null;
    if (
      !request ||
      request.operationId !== operationId ||
      request.storeId !== storeId
    )
      throw new DesktopError({
        code: "invalid_response",
        outcomeUnknown: false,
        message: "后台待核查操作身份不一致，未采用。",
      });
    return providerOperation(value, request);
  },
  connectProvider(provider: Provider): Promise<{
    provider: Provider;
    verification: ProviderVerification;
    catalog: ProviderModelCatalog | null;
  }> {
    invalidateCapabilityAccess();
    return command<unknown>("connect_provider", { provider }).then((value) =>
      response(
        shape({
          provider: providerValue,
          verification: providerVerification,
          catalog: nullable(providerModelCatalog),
        }),
        value,
        "供应商连接",
        true,
      ),
    );
  },
  prepareDynamicConnection(
    draft: {
      agentId: string;
      expectedRevision: string;
      providerId: string;
      expectedProviderRevision: number;
      model: string;
    },
    source?: LocalTokenSource,
  ): Promise<ProviderTransaction> {
    const origin = requireLocalTokenSource(source);
    const hint = stageLocalToken(origin, {
      purpose: "dynamic_binding",
      agentId: draft.agentId,
      expectedRevision: draft.expectedRevision,
      providerId: draft.providerId,
      providerRevision: draft.expectedProviderRevision,
      model: draft.model,
    });
    return command<unknown>("prepare_dynamic_connection", {
      draft: { ...draft, operation: origin.identity },
    })
      .then((value) => acceptLocalTokenPreview(value, origin, hint))
      .catch((reason) => {
        throw new LocalTokenPreparationError(reason, origin.identity);
      });
  },
  prepareDynamicRoute(input: {
    operationId: string;
    bindingId: string;
    expectedGeneration: string;
    providerId: string;
    expectedProviderRevision: number;
    model: string;
  }): Promise<DynamicChange> {
    return command<unknown>("prepare_dynamic_route", { input }).then((value) =>
      providerDynamicChange(value, {
        id: input.operationId,
        bindingId: input.bindingId,
        expectedGeneration: input.expectedGeneration,
        target: {
          id: input.providerId,
          revision: input.expectedProviderRevision,
          model: input.model,
        },
      }),
    );
  },
  dynamicRouteAction(
    change: DynamicChange,
    action: "commit" | "cancel" | "restore",
  ): Promise<DynamicChange> {
    return command<unknown>("dynamic_route_action", {
      changeId: change.id,
      action,
    }).then((value) =>
      providerDynamicChange(
        value,
        action === "restore"
          ? { bindingId: change.bindingId, target: change.before.provider }
          : {
              id: change.id,
              bindingId: change.bindingId,
              expectedGeneration: change.expectedGeneration,
              target: change.target,
            },
      ),
    );
  },
  providerUsage(query: ProviderUsageQuery): Promise<ProviderUsage> {
    return command<unknown>("query_provider_usage", { query }).then((value) =>
      providerUsage(value, query),
    );
  },
  providerModels(input: {
    endpoint: string;
    protocol: Provider["protocol"];
    vendor: string;
    credentialRef: string | null;
  }): Promise<ProviderModelCatalog> {
    return command<unknown>("fetch_provider_models", { input }).then(
      providerModelCatalog,
    );
  },
  providerStatus(): Promise<ProviderStatus> {
    return command<unknown>("get_provider_status").then(providerStatus);
  },
  async requireCompatibleAgent(agentId: string): Promise<void> {
    const status = await providersApi.providerStatus();
    const runtime = status.runtimes.find((item) => item.agentId === agentId);
    if (!runtime?.supported) {
      throw new Error(
        runtime?.reason || "无法确认 Agent 的配置兼容性，请重试。",
      );
    }
  },
  discoverProviderImports(): Promise<ProviderImportCandidate[]> {
    return command<unknown>("discover_provider_imports").then(
      providerImportCandidates,
    );
  },
  async previewNativeImport() {
    return nativeImportPreview(
      await command<unknown>("preview_native_provider_import"),
    );
  },
  async nativeImportResult(input: NativeImportRequest, cancel = false) {
    const value = cancel
      ? await command<unknown>("cancel_native_provider_import", { input })
      : await command<unknown>("get_native_provider_import", { input });
    return nativeImportOperation(value, input);
  },
  async importNativeProvider(
    input: NativeImportRequest,
  ): Promise<{ provider: Provider; reused: boolean }> {
    invalidateCapabilityAccess();
    const result = nativeImported(
      await command<unknown>("import_native_provider", { input }),
    );
    if (
      !result.reused &&
      result.provider.id !== "custom-" + input.operation.operationId
    )
      throw new DesktopError({
        code: "native_import_result_unknown",
        outcomeUnknown: true,
        message: "导入回复与原编号不匹配，请查询原结果。",
      });
    return result;
  },
  async createProviderCredential(
    label: string,
    secret: string,
    operationId: string,
  ): Promise<CredentialReceipt> {
    if (!isCredentialOperationId(operationId))
      throw new Error("凭据操作编号无效，未提交保存。");
    // Confirm protocol support before sending a secret. Older services may
    // share the runtime identity version but lack the operation endpoints.
    try {
      await providersApi.credentialOperation(operationId);
    } catch (reason) {
      if (!(
        reason instanceof DesktopError && reason.code === "operation_not_found"
      ))
        throw reason;
    }
    return credentialReceipt(
      await command("create_provider_credential", {
        label,
        secret,
        operationId,
      }),
      operationId,
    );
  },
  async credentialOperation(operationId: string): Promise<CredentialOperation> {
    return credentialOperation(
      await command("get_provider_credential_operation", { operationId }),
      operationId,
    );
  },
  async cancelCredentialOperation(
    operationId: string,
  ): Promise<CredentialOperation> {
    return credentialOperation(
      await command("cancel_provider_credential_operation", { operationId }),
      operationId,
    );
  },
  verifyProvider(
    providerId: string,
    expectedRevision: number,
    model?: string,
  ): Promise<ProviderVerification> {
    return command<unknown>("verify_provider", {
      providerId,
      expectedRevision,
      model,
    }).then((value) => {
      const result = providerVerification(value);
      if (
        result.providerId !== providerId ||
        (model !== undefined && result.model !== model)
      )
        throw new DesktopError({
          code: "transport_invalid_response",
          outcomeUnknown: false,
          message: "模型验证回复与目标连接或模型不一致，未采用。",
        });
      return result;
    });
  },
  prepareProviderSwitch(
    agentId: string,
    expectedRevision: string,
    providerId: string | null,
    expectedProviderRevision?: number,
    model?: string,
    source?: LocalTokenSource,
  ): Promise<ProviderTransaction> {
    const origin = providerId ? requireLocalTokenSource(source) : null;
    const hint =
      providerId && origin
        ? stageLocalToken(origin, {
            purpose: "fixed_route",
            agentId,
            expectedRevision,
            providerId,
            providerRevision: expectedProviderRevision,
            model,
          })
        : null;
    return command<unknown>("prepare_provider_switch", {
      draft: {
        agentId,
        expectedRevision,
        providerId,
        expectedProviderRevision,
        model,
        ...(origin ? { operation: origin.identity } : {}),
      },
    })
      .then((value) =>
        origin && hint
          ? acceptLocalTokenPreview(value, origin, hint)
          : providerTransaction(value, {
              agentId,
              beforeRevision: expectedRevision,
            }),
      )
      .catch((reason) => {
        throw origin
          ? new LocalTokenPreparationError(reason, origin.identity)
          : reason;
      });
  },
  providerSwitchAction(
    transactionId: string,
    action: "apply" | "cancel" | "restore" | "defer",
  ): Promise<ProviderTransaction> {
    return command<unknown>("provider_switch_action", {
      transactionId,
      action,
    }).then((value) =>
      providerTransaction(
        value,
        action === "restore"
          ? { restoreOf: transactionId }
          : { id: transactionId },
      ),
    );
  },
  restartAgentProvider(
    transactionId: string,
    agentProductId: string,
  ): Promise<ProviderTransaction> {
    return command<unknown>("restart_agent_provider", {
      transactionId,
      agentProductId,
    }).then((value) => providerTransaction(value, { id: transactionId }));
  },
  async saveProvider(provider: Provider): Promise<void> {
    invalidateCapabilityAccess();
    await command("save_provider", { provider });
  },
};

type CatalogChange = Omit<ProviderCatalogRequest, "action"> & {
  action: Exclude<ProviderCatalogRequest["action"], { kind: "read" }>;
};
type CatalogRead = Omit<ProviderCatalogRequest, "action"> & {
  action: Extract<ProviderCatalogRequest["action"], { kind: "read" }>;
};
const performProviderCatalog = (input: ProviderCatalogRequest) => {
  if (input.action.kind !== "read") invalidateCapabilityAccess();
  return command<unknown>("perform_provider_catalog", { input });
};
function catalogProviderRequest(
  input: import("../../../contracts/domain/provider-connection/types").ConnectionRequest,
) {
  const operation = storedProviderRequest(input.operation);
  const catalog = operation
    ? selectedProviderCatalog(input.catalog, operation)
    : null;
  if (!operation || !catalog)
    throw new DesktopError({
      code: "invalid_input",
      outcomeUnknown: false,
      message: "目录操作的原请求或选择无法核查，本次未提交。",
    });
  return { operation, catalog };
}
export const providerCatalogApi = {
  async state(query: ProviderCatalogQuery) {
    return parseProviderCatalogState(
      await command<unknown>("query_provider_catalog", { query }),
    );
  },
  async change(input: CatalogChange) {
    return parseProviderCatalogState(await performProviderCatalog(input));
  },
  async read(input: CatalogRead) {
    return providerModelCatalog(await performProviderCatalog(input));
  },
  async adapters() {
    return (await pluginsApi.list()).items.filter((p) =>
      p.manifest.capabilities.includes("provider.models@1"),
    );
  },
  async commit(
    input: import("../../../contracts/domain/provider-connection/types").ConnectionRequest,
  ) {
    const frozen = catalogProviderRequest(input);
    await providersApi.writeProtocol(frozen.operation.storeId);
    invalidateCapabilityAccess();
    return providerOperation(
      await command<unknown>("commit_catalog_provider_operation", {
        input: frozen,
      }),
      frozen.operation,
    );
  },
  async operation(
    input: import("../../../contracts/domain/provider-connection/types").ConnectionRequest,
  ) {
    const frozen = catalogProviderRequest(input);
    await providersApi.writeProtocol(frozen.operation.storeId);
    return providerOperation(
      await command<unknown>("get_catalog_provider_operation", {
        input: frozen,
      }),
      frozen.operation,
    );
  },
};
const scopedProviderCatalog = (
  input: import("../../../contracts/domain/provider-connection/types").ScopedRequest,
) => {
  if (input.request.action.kind !== "read") invalidateCapabilityAccess();
  return command<unknown>("perform_scoped_provider_catalog", { input });
};
export const scopedProviderCatalogApi = {
  async change(
    input: import("../../../contracts/domain/provider-connection/types").ScopedRequest,
  ) {
    return parseProviderCatalogState(await scopedProviderCatalog(input));
  },
  async read(
    input: import("../../../contracts/domain/provider-connection/types").ScopedRequest,
  ) {
    return providerModelCatalog(await scopedProviderCatalog(input));
  },
};

const performProviderSource = (input: SourceAction) =>
  command<unknown>("perform_provider_catalog_source", { input });
function providerSourceState(
  value: unknown,
  query: SourceQuery,
  write = false,
) {
  return response(
    (value) => {
      const state = parseSchema<SourceState>(sourceStateSchema, value);
      if (
        state.providerId !== query.providerId ||
        state.expectedRevision !== query.expectedRevision
      )
        throw Error("source identity");
      if (
        state.sourceRevision != null &&
        !/^[a-f0-9]{64}$/.test(state.sourceRevision)
      )
        throw Error("source revision");
      if (state.kind === "builtin") {
        if (
          !state.authorized ||
          state.choice != null ||
          state.selection != null ||
          state.policy != null ||
          state.errorCode != null
        )
          throw Error("builtin authority");
      } else {
        if (!state.sourceRevision || !state.choice)
          throw Error("source choice");
        if (state.authorized) {
          const selection = state.selection;
          if (
            state.kind !== "external" ||
            !selection ||
            !state.policy ||
            state.errorCode != null ||
            selection.pluginId !== state.choice.pluginId ||
            selection.digest !== state.choice.digest ||
            selection.target.kind !== "saved" ||
            selection.target.providerId !== query.providerId ||
            selection.target.expectedRevision !== query.expectedRevision
          )
            throw Error("source authority");
        } else if (state.selection != null) throw Error("source authority");
      }
      return state;
    },
    value,
    "目录来源",
    write,
  );
}
const performDraftCatalog = (
  kind: "read" | "check",
  request: DraftCatalogRequest,
) =>
  command<unknown>("perform_provider_draft_catalog", {
    input: {
      kind,
      request: parseSchema<DraftCatalogRequest>(
        draftCatalogRequestSchema,
        request,
      ),
    },
  });
export const providerDraftGrantApi = {
  async perform(input: DraftGrantAction) {
    parseSchema<DraftGrantAction>(draftGrantActionSchema, input);
    invalidateCapabilityAccess();
    return response(
      (value) => {
        const result = parseSchema<DraftGrantReceipt>(
          draftGrantReceiptSchema,
          value,
        );
        const storeId =
          input.kind === "open" ? input.storeId : input.owner.storeId;
        if (
          result.owner.storeId !== storeId ||
          (input.kind !== "open" &&
            result.owner.ownerId !== input.owner.ownerId) ||
          !/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(
            result.owner.ownerId,
          ) ||
          (["prepared", "released"].includes(result.state)
            ? result.selection != null
            : !result.selection)
        )
          throw Error("draft grant identity");
        if (input.kind === "open" && result.state !== "prepared")
          throw Error("draft grant preparation");
        if (
          input.kind === "acquire" &&
          !["acquired", "borrowed"].includes(result.state)
        )
          throw Error("draft grant authority");
        if (input.kind === "attach" && result.state !== "pending")
          throw Error("draft grant pin");
        if (
          input.kind === "release" &&
          !["pending", "released"].includes(result.state)
        )
          throw Error("draft grant release");
        return result;
      },
      await command<unknown>("perform_provider_draft_grant", { input }),
      "草稿目录授权",
    );
  },
};
const draftRecoveryId = (value: string) =>
  /^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(value);
function draftRecoveryEntry(entry: DraftRecoveryEntry, storeId: string) {
  if (
    entry.owner.storeId !== storeId ||
    !draftRecoveryId(entry.owner.ownerId) ||
    (entry.operationId != null && !draftRecoveryId(entry.operationId)) ||
    (entry.state === "unattached" && entry.operationId != null) ||
    (!["unchecked", "unattached", "unavailable"].includes(entry.state) &&
      !entry.operationId) ||
    (entry.canFinish &&
      !["unattached", "committed", "failed", "cancelled"].includes(entry.state))
  )
    throw Error("draft recovery identity");
  if (
    entry.pluginId == null ||
    entry.digest == null ||
    entry.endpointOrigin == null
  ) {
    if (
      entry.ownership !== "missing" ||
      entry.state !== "unchecked" ||
      entry.pluginId != null ||
      entry.digest != null ||
      entry.endpointOrigin != null
    )
      throw Error("draft recovery scope");
  } else {
    const origin = new URL(entry.endpointOrigin);
    if (
      !/^[a-f0-9]{64}$/.test(entry.digest) ||
      origin.origin !== entry.endpointOrigin
    )
      throw Error("draft recovery scope");
  }
  return entry;
}
async function performDraftRecovery(input: DraftRecoveryAction) {
  parseSchema<DraftRecoveryAction>(draftRecoveryActionSchema, input);
  const storeId = input.kind === "list" ? input.storeId : input.owner.storeId;
  if (
    !draftRecoveryId(storeId) ||
    (input.kind !== "list" && !draftRecoveryId(input.owner.ownerId))
  )
    throw Error("草稿授权恢复身份无效。");
  if (input.kind === "finish") invalidateCapabilityAccess();
  return response(
    (value) => {
      const result = parseSchema<DraftRecoveryReply>(
        draftRecoveryReplySchema,
        value,
      );
      if (input.kind === "list") {
        if (
          result.kind !== "listed" ||
          result.storeId !== storeId ||
          result.entries.length > 256
        )
          throw Error("draft recovery inventory");
        const owners = new Set<string>();
        for (const entry of result.entries) {
          draftRecoveryEntry(entry, storeId);
          if (owners.has(entry.owner.ownerId))
            throw Error("duplicate draft owner");
          owners.add(entry.owner.ownerId);
        }
      } else {
        const owner =
          result.kind === "finished"
            ? result.owner
            : result.kind === "inspected"
              ? result.inspection.entry.owner
              : null;
        if (
          !owner ||
          owner.storeId !== storeId ||
          owner.ownerId !== input.owner.ownerId ||
          (input.kind === "inspect"
            ? result.kind !== "inspected"
            : result.kind !== "finished")
        )
          throw Error("draft recovery owner");
        if (result.kind === "inspected") {
          const { entry, request } = result.inspection;
          draftRecoveryEntry(entry, storeId);
          if (entry.state === "unchecked")
            throw Error("uninspected draft owner");
          if (request != null) {
            const original = storedProviderRequest(request);
            if (
              !original ||
              original.storeId !== storeId ||
              original.operationId !== entry.operationId ||
              entry.state === "unattached"
            )
              throw Error("draft recovery original request");
          }
        }
      }
      return result;
    },
    await command<unknown>("perform_provider_draft_recovery", { input }),
    "草稿授权恢复",
    input.kind === "finish",
  );
}
export const providerDraftRecoveryApi = {
  async list(storeId: string) {
    const result = await performDraftRecovery({ kind: "list", storeId });
    if (result.kind !== "listed") throw Error("草稿授权列表无法核查。");
    return result.entries;
  },
  async inspect(
    owner: import("../../../contracts/domain/provider-draft-grant/types").DraftGrantOwner,
  ) {
    const result = await performDraftRecovery({ kind: "inspect", owner });
    if (result.kind !== "inspected") throw Error("草稿授权归属无法核查。");
    return {
      entry: result.inspection.entry,
      request:
        result.inspection.request == null
          ? null
          : storedProviderRequest(result.inspection.request),
    };
  },
  async finish(
    owner: import("../../../contracts/domain/provider-draft-grant/types").DraftGrantOwner,
  ) {
    const result = await performDraftRecovery({ kind: "finish", owner });
    if (result.kind !== "finished") throw Error("草稿授权结束结果待核查。");
    return result.released;
  },
};
async function performPresetQuery(input: PresetAction) {
  parseSchema<PresetAction>(presetActionSchema, input);
  if (
    !/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(
      input.storeId,
    )
  )
    throw Error("预设查询的原数据库身份无效。");
  return response(
    (value) => {
      const reply = parseSchema<PresetReply>(presetReplySchema, value);
      if (reply.storeId !== input.storeId) throw Error("预设属于其他数据库。");
      if (input.kind === "list") {
        if (
          reply.kind !== "listed" ||
          reply.items.length > builtinPresetAdvice.length + 64 * 16 ||
          reply.issues.length > 64
        )
          throw Error("预设列表不完整或超出容量。");
        const keys = new Set<string>();
        for (const item of reply.items) {
          validatePresetAdvice(item);
          const key = presetKey(item.origin);
          if (keys.has(key)) throw Error("预设身份重复。");
          keys.add(key);
        }
        if (!builtinPresetAdvice.every((p) => keys.has(presetKey(p.origin))))
          throw Error("内置预设列表不完整。");
        for (const issue of reply.issues) {
          if (
            !issue.errorCode ||
            issue.errorCode.length > 100 ||
            (issue.pluginId == null) !== (issue.digest == null) ||
            (issue.digest != null && !/^[a-f0-9]{64}$/.test(issue.digest))
          )
            throw Error("预设来源错误格式无效。");
        }
      } else {
        if (
          reply.kind !== "resolved" ||
          presetKey(reply.advice.origin) !== presetKey(input.origin)
        )
          throw Error("预设核查身份不匹配。");
        validatePresetAdvice(reply.advice);
      }
      return reply;
    },
    await command<unknown>("query_provider_presets", { input }),
    "供应商预设",
  );
}
export const providerPresetsApi = {
  async list(storeId: string) {
    const reply = await performPresetQuery({ kind: "list", storeId });
    if (reply.kind !== "listed") throw Error("预设列表暂不可核查。");
    return {
      items: reply.items.map(validatePresetAdvice),
      issues: reply.issues,
    };
  },
  async resolve(storeId: string, origin: PresetOrigin) {
    const reply = await performPresetQuery({
      kind: "resolve",
      storeId,
      origin,
    });
    if (reply.kind !== "resolved") throw Error("所选预设暂不可核查。");
    return validatePresetAdvice(reply.advice);
  },
};
export const providerDraftCatalogApi = {
  async read(request: DraftCatalogRequest) {
    return response(
      providerModelCatalog,
      await performDraftCatalog("read", request),
      "草稿模型目录",
    );
  },
  async check(request: DraftCatalogRequest) {
    return response(
      (value) => {
        if (value !== null) throw Error("draft authority response");
      },
      await performDraftCatalog("check", request),
      "草稿目录范围",
    );
  },
};
export const providerSourceApi = {
  async state(query: SourceQuery) {
    return providerSourceState(
      await performProviderSource({ kind: "state", query }),
      query,
    );
  },
  async select(
    query: SourceQuery,
    sourceRevision: string | null,
    catalog:
      | import("../../../contracts/domain/provider-connection/types").Selection
      | null,
  ) {
    invalidateCapabilityAccess();
    const result = providerSourceState(
      await performProviderSource({
        kind: "select",
        query,
        sourceRevision,
        catalog,
      }),
      query,
      true,
    );
    window.dispatchEvent(new Event("provider-catalog-source-changed"));
    return result;
  },
  async read(query: SourceQuery, sourceRevision: string | null) {
    return providerModelCatalog(
      await performProviderSource({ kind: "read", query, sourceRevision }),
    );
  },
};

export const providerSourceOperationApi = {
  async perform(
    kind: "commit" | "query",
    operation: ProviderWriteRequest,
    route: import("./catalog-operation").ExplicitProviderSource,
  ) {
    const request = storedProviderRequest(operation);
    const selected = request ? selectedProviderRoute(route, request) : null;
    if (!request || !selected || !isExplicitProviderSource(selected))
      throw new DesktopError({
        code: "invalid_input",
        outcomeUnknown: false,
        message: "来源操作无法核查，本次未提交。",
      });
    await providersApi.writeProtocol(request.storeId);
    if (kind === "commit") invalidateCapabilityAccess();
    return providerOperation(
      await command<unknown>("perform_provider_source_operation", {
        input: { kind, request: { operation: request, ...selected } },
      }),
      request,
    );
  },
};
