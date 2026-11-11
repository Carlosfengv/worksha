import { command } from "@/shared/lib/desktop-command";
import { DesktopError } from "@/shared/lib/desktop-command";
import {
  operationId,
  serviceReceipt,
  type ServiceRequest,
} from "./operation-receipt";
import type { ProviderTransaction } from "@/features/providers/types";
import type {
  RoutingProjectPage,
  RoutingProjectQuery,
} from "@/features/projects/types";
import {
  acceptLocalTokenPreview,
  LocalTokenPreparationError,
  requireLocalTokenSource,
  stageLocalToken,
  type LocalTokenSource,
} from "@/features/providers/local-token-operation";
import type {
  ModelService,
  ModelServiceCatalog,
  ModelServiceDraft,
  ServicePreview,
  ServicePreviewRequest,
} from "./types";
export const modelServicesApi = {
  async preview(input: ServicePreviewRequest): Promise<ServicePreview> {
    const result = await command<ServicePreview>("preview_model_service", {
      input,
    });
    if (
      result?.storeId !== input.storeId ||
      !Array.isArray(result.models) ||
      !Number.isFinite(Date.parse(result.checkedAt))
    ) {
      throw Error("发布预览无法核查，请重新读取原数据库。");
    }
    return result;
  },
  async requireWriteProtocol(expectedStoreId?: string): Promise<string> {
    const catalog = await modelServicesApi.catalog();
    if (
      catalog?.writeProtocol?.version !== 1 ||
      !operationId(catalog.writeProtocol.storeId) ||
      !Number.isSafeInteger(catalog.writeProtocol.receiptLimit) ||
      catalog.writeProtocol.receiptLimit < 1
    ) {
      throw new DesktopError({
        code: "write_protocol_unavailable",
        outcomeUnknown: false,
        message: "当前后台不支持模型服务结果核查；请升级后台后再发布或删除。",
      });
    }
    if (expectedStoreId && catalog.writeProtocol.storeId !== expectedStoreId)
      throw new DesktopError({
        code: "operation_store_changed",
        outcomeUnknown: false,
        message: "操作属于另一个本机数据库，请连接原后台后核查。",
      });
    return catalog.writeProtocol.storeId;
  },
  async verifyMember(input: {
    storeId: string;
    providerId: string;
    expectedRevision: number;
    model: string;
    upstreamAccount?: import("@/features/providers/types").UpstreamAccountRef;
  }) {
    return command<import("@/features/providers/types").ProviderVerification>(
      "verify_model_service_member",
      { input },
    );
  },
  async commit(request: ServiceRequest) {
    return serviceReceipt(
      await command("commit_model_service_operation", { request }),
      request,
    );
  },
  async operation(request: ServiceRequest) {
    return serviceReceipt(
      await command("get_model_service_operation", {
        operationId: request.operationId,
        storeId: request.storeId,
      }),
      request,
    );
  },
  catalog(): Promise<ModelServiceCatalog> {
    return command("get_model_services");
  },
  routingProjects(query: RoutingProjectQuery): Promise<RoutingProjectPage> {
    return command("query_model_service_routing_projects", { query });
  },
  save(draft: ModelServiceDraft): Promise<ModelService> {
    return command("save_model_service", { draft });
  },
  delete(
    serviceId: string,
    expectedRevision: number,
  ): Promise<{ deleted: boolean }> {
    return command("delete_model_service", { serviceId, expectedRevision });
  },
  prepareConnection(
    input: {
      agentId: string;
      expectedRevision: string;
      serviceId: string;
      expectedServiceRevision: number;
    },
    source?: LocalTokenSource,
    provider?: import("@/features/providers/types").Provider,
  ): Promise<ProviderTransaction> {
    const origin = requireLocalTokenSource(source);
    if (!provider) throw Error("模型服务的原默认目标尚未核查，请重新读取。");
    const hint = stageLocalToken(origin, {
      purpose: "model_service",
      agentId: input.agentId,
      expectedRevision: input.expectedRevision,
      providerId: provider.id,
      providerRevision: provider.revision,
      model: provider.model,
      serviceId: input.serviceId,
      serviceRevision: input.expectedServiceRevision,
    });
    return command<unknown>("prepare_model_service_connection", {
      input: { ...input, operation: origin.identity },
    })
      .then((value) => acceptLocalTokenPreview(value, origin, hint))
      .catch((reason) => {
        throw new LocalTokenPreparationError(reason, origin.identity);
      });
  },
};

import { parseSchema } from "@/shared/lib/contract-schema";
import actionSchema from "../../../contracts/plugins/routing/action.schema.json";
import replySchema from "../../../contracts/plugins/routing/reply.schema.json";
import type { Action, Reply } from "../../../contracts/plugins/routing/types";
export async function performRoutingPolicy(
  input: Action,
): Promise<Reply | null> {
  parseSchema<Action>(actionSchema, input);
  const value = await command<unknown>("perform_routing_policy", { input });
  if (value === null && input.kind === "inspect") return null;
  const reply = parseSchema<Reply>(replySchema, value);
  const expected = {
    list: "inventory",
    inspect: "review",
    install: "package",
    state: "package",
    bind: "binding",
    unbind: "unbound",
    publish: "publish",
    "query-publish": "publish",
    "resume-publish": "publish",
    "cancel-publish": "publish",
  } as const;
  if (reply.kind !== expected[input.kind])
    throw Error("策略响应类型已变化，请重新读取。");
  const store =
    input.kind === "publish" ? input.request.storeId : input.storeId;
  if (
    ("storeId" in reply && reply.storeId !== store) ||
    (reply.kind === "binding" &&
      (input.kind !== "bind" ||
        reply.binding.storeId !== store ||
        reply.binding.serviceId !== input.serviceId ||
        reply.binding.serviceVersionId !== input.serviceVersionId ||
        reply.binding.package.pluginId !== input.package.pluginId ||
        reply.binding.package.digest !== input.package.digest ||
        reply.binding.failureMode !== input.failureMode)) ||
    (reply.kind === "publish" &&
      (reply.status.storeId !== store ||
        reply.status.operationId !==
          (input.kind === "publish"
            ? input.request.operationId
            : "operationId" in input
              ? input.operationId
              : "")))
  )
    throw Error("策略响应范围已变化，请保留原操作编号核查。");
  if (
    reply.kind === "package" &&
    ((input.kind === "install" && reply.status.digest !== input.digest) ||
      (input.kind === "state" &&
        (reply.status.manifest.id !== input.package.pluginId ||
          (input.action !== "rollback" &&
            reply.status.digest !== input.package.digest))))
  )
    throw Error("策略包身份已变化，请重新读取。");
  if (
    reply.kind === "inventory" &&
    (reply.bindings.some((b) => b.storeId !== store) ||
      reply.scopes.some((s) => s.storeId !== store) ||
      reply.publishes.some(
        (p) =>
          p.storeId !== store ||
          p.request.storeId !== store ||
          p.request.operationId !== p.operationId,
      ))
  )
    throw Error("策略目录包含其他数据库的记录，请重新读取。");
  if (
    reply.kind === "unbound" &&
    (input.kind !== "unbind" || reply.serviceId !== input.serviceId)
  )
    throw Error("策略绑定范围已变化，请重新读取。");
  if (!["list", "inspect", "query-publish"].includes(input.kind))
    window.dispatchEvent(new Event("routing-policy-changed"));
  return reply;
}
export async function routingPolicyInventory(storeId: string) {
  const reply = await performRoutingPolicy({ kind: "list", storeId });
  if (reply?.kind !== "inventory") throw Error("策略目录不可读取。");
  return reply;
}

export const routingPolicyApi = {
  perform: performRoutingPolicy,
  inventory: routingPolicyInventory,
};

import recoveryActionSchema from "../../../contracts/domain/routing-policy-recovery/action.schema.json";
import recoveryReplySchema from "../../../contracts/domain/routing-policy-recovery/reply.schema.json";
import type {
  PolicyRecoveryAction,
  PolicyRecoveryReply,
} from "../../../contracts/domain/routing-policy-recovery/types";

export async function performRoutingPolicyRecovery(
  input: PolicyRecoveryAction,
) {
  parseSchema<PolicyRecoveryAction>(recoveryActionSchema, input);
  const reply = parseSchema<PolicyRecoveryReply>(
    recoveryReplySchema,
    await command<unknown>("perform_routing_policy_recovery", { input }),
  );
  const storeId =
    input.kind === "list"
      ? input.storeId
      : input.kind === "inspect"
        ? input.scope.storeId
        : input.reference.scope.storeId;
  if (reply.kind === "listed" && input.kind === "list") {
    const ids = reply.entries.map((e) => e.scope.serviceId);
    if (
      reply.storeId !== storeId ||
      reply.entries.length > 256 ||
      new Set(ids).size !== ids.length ||
      reply.entries.some(
        // Existing service spellings are opaque identities. Canonical v4 is
        // required only for new host-issued references and revisions below.
        (e) => e.scope.storeId !== storeId || !e.scope.serviceId,
      )
    )
      throw Error("恢复目录范围已变化，请重新读取。");
  } else if (reply.kind === "inspected" && input.kind === "inspect") {
    const value = reply.inspection;
    if (
      value.entry.scope.storeId !== storeId ||
      value.entry.scope.serviceId !== input.scope.serviceId ||
      (value.reference &&
        (value.reference.scope.storeId !== storeId ||
          value.reference.scope.serviceId !== input.scope.serviceId ||
          !operationId(value.reference.referenceId))) ||
      ((value.canRestoreNative || value.canRestorePending) && !value.reference)
    )
      throw Error("恢复核查身份已变化，请重新核查。");
  } else if (reply.kind === "resolved" && input.kind === "resolve") {
    if (
      reply.reference.scope.storeId !== storeId ||
      reply.reference.scope.serviceId !== input.reference.scope.serviceId ||
      reply.reference.referenceId !== input.reference.referenceId ||
      reply.mode.kind !== input.mode.kind ||
      !operationId(reply.revision)
    )
      throw Error("恢复结果身份不一致，请保留原核查编号。");
  } else throw Error("恢复响应类型不一致，请保留原核查编号。");
  return reply;
}
