import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
import objectContributionReplySchema from "../../../contracts/plugins/objects/object-contribution-reply.schema.json";
import type {
  ObjectContributionAction,
  ObjectContributionReply,
} from "../../../contracts/plugins/objects/types";
import { command } from "@/shared/lib/desktop-command";
import type { State as ProviderCatalogState } from "../../../contracts/domain/provider-access/types";
import providerCatalogStateSchema from "../../../contracts/domain/provider-access/state.schema.json";
import { parseDiagnostics } from "./diagnostics";
import { parseContributions } from "./contributions";
import { object, parseContract, parseSchema } from "./contract";
export function parseProviderCatalogState(value: unknown) {
  return parseSchema<ProviderCatalogState>(providerCatalogStateSchema, value);
}
export const objectContributionsApi = {
  async perform(
    input: ObjectContributionAction,
  ): Promise<ObjectContributionReply> {
    if (input.kind === "grant" || input.kind === "revoke")
      invalidateCapabilityAccess();
    const reply = parseSchema<ObjectContributionReply>(
      objectContributionReplySchema,
      await command<unknown>("perform_object_contribution", { input }).finally(
        () => {
          if (input.kind === "grant" || input.kind === "revoke")
            invalidateCapabilityAccess();
        },
      ),
    );
    const invalid = () => {
      throw new Error("对象扩展响应与当前请求不一致，请重新读取。");
    };
    if (input.kind === "catalog") {
      if (reply.kind !== "catalog" || reply.catalog.schemaVersion !== 1)
        invalid();
    } else if (input.kind === "grant" || input.kind === "revoke") {
      if (
        reply.kind !== "status" ||
        reply.status.pluginId !== input.request.slot.pluginId ||
        reply.status.digest !== input.request.slot.digest
      )
        invalid();
    } else if (input.kind === "issue") {
      if (
        reply.kind !== "reference" ||
        reply.reference.contextIdentity !==
          input.request.expectedContextIdentity ||
        reply.reference.slot.pluginId !== input.request.slot.pluginId ||
        reply.reference.slot.digest !== input.request.slot.digest ||
        reply.reference.slot.slotId !== input.request.slot.slotId ||
        reply.reference.target !== input.request.object.target
      )
        invalid();
    } else if (input.kind === "read") {
      if (reply.kind !== "summary" || reply.objectRef !== input.query.objectRef)
        invalid();
    } else if (
      reply.kind !== "released" ||
      reply.objectRef !== input.query.objectRef
    )
      invalid();
    return reply;
  },
};
import policySchema from "../../../contracts/plugins/extensions/policy.schema.json";
import fullCatalogSchema from "../../../contracts/plugins/sources/catalog.schema.json";
import fullReviewSchema from "../../../contracts/plugins/sources/review.schema.json";
import fullStatusSchema from "../../../contracts/plugins/sources/status.schema.json";
import type {
  Catalog as FullCatalog,
  Review as FullReview,
  Install as FullInstall,
  Status as FullStatus,
  StateChange as FullStateChange,
} from "../../../contracts/plugins/sources/types";

export const fullPluginsApi = {
  async list() {
    return parseSchema<FullCatalog>(
      fullCatalogSchema,
      await command<unknown>("list_full_plugins"),
    );
  },
  async inspect() {
    const value = await command<unknown>("inspect_full_plugin_package");
    return value === null
      ? null
      : parseSchema<FullReview>(fullReviewSchema, value);
  },
  async install(input: FullInstall) {
    invalidateCapabilityAccess();
    return parseSchema<FullStatus>(
      fullStatusSchema,
      await command<unknown>("install_full_plugin_package", { input }),
    );
  },
  async change(input: FullStateChange) {
    invalidateCapabilityAccess();
    return parseSchema<FullStatus>(
      fullStatusSchema,
      await command<unknown>("change_full_plugin_state", { input }).finally(
        invalidateCapabilityAccess,
      ),
    );
  },
};
import type {
  ExtensionQuery,
  ExtensionRequest,
  ExtensionStatus,
  ExtensionPlan,
  ExtensionHistory,
  PlanCapacity,
} from "./advanced-types";
import type {
  ActionRequest,
  InstallRequest,
  PackageReview,
  PluginList,
  PluginStatus,
  PluginView,
  StateRequest,
  ViewQuery,
} from "./types";

const status = (value: unknown) => parseContract<PluginStatus>("status", value);
const queryExtension = (query: ExtensionQuery) =>
  command<unknown>("query_plugin_extension", { query });
function view(value: unknown): PluginView {
  if (!object(value) || typeof value.confirmedEmpty !== "boolean")
    throw new Error("插件视图响应无效。");
  parseContract("view", value.view);
  status(value.status);
  if (value.items !== undefined) {
    if (!Array.isArray(value.items)) throw new Error("插件记录响应无效。");
    for (const item of value.items) {
      if (!object(item) || typeof item.sessionId !== "string")
        throw new Error("插件记录身份无效。");
      parseContract("session", item.record);
    }
  }
  if (value.quota) parseContract("quota", value.quota);
  if (value.report) parseContract("report", value.report);
  if (
    value.snapshot !== undefined &&
    value.snapshot !== null &&
    typeof value.snapshot !== "string"
  )
    throw new Error("插件快照无效。");
  for (const key of ["quotaExpired", "reportExpired", "needsRefresh"])
    if (value[key] !== undefined && typeof value[key] !== "boolean")
      throw new Error("插件状态响应无效。");
  if (value.summary !== undefined) {
    const summary = value.summary;
    if (
      !object(summary) ||
      ![
        "knownUsageRecords",
        "inputTokens",
        "outputTokens",
        "totalTokens",
      ].every(
        (key) =>
          typeof summary[key] === "string" &&
          /^(0|[1-9][0-9]*)$/.test(summary[key] as string),
      )
    )
      throw new Error("插件汇总响应无效。");
  }
  if (
    value.total !== undefined &&
    (!Number.isSafeInteger(value.total) || Number(value.total) < 0)
  )
    throw new Error("插件分页响应无效。");
  if (
    value.nextCursor !== undefined &&
    value.nextCursor !== null &&
    typeof value.nextCursor !== "string"
  )
    throw new Error("插件游标响应无效。");
  return value as unknown as PluginView;
}
export function parsePluginList(value: unknown): PluginList {
  if (
    !object(value) ||
    !Array.isArray(value.items) ||
    typeof value.developmentMode !== "boolean" ||
    value.apiVersion !== "1.0.0" ||
    typeof value.runtime !== "string"
  )
    throw new Error("插件目录响应无效。");
  let diagnostics: PluginList["diagnostics"] = null;
  const items = value.items.map(status);
  let contributions: PluginList["contributions"] = null;
  let contributionsError: string | undefined;
  if (value.contributions !== undefined) {
    try {
      contributions = parseContributions(value.contributions, items);
    } catch (reason) {
      contributionsError =
        reason instanceof Error ? reason.message : "插件详情声明暂不可用。";
    }
  }
  let diagnosticsError: string | undefined;
  if (value.diagnostics !== undefined) {
    try {
      diagnostics = parseDiagnostics(value.diagnostics);
    } catch (reason) {
      diagnosticsError =
        reason instanceof Error ? reason.message : "运行诊断暂不可用。";
    }
  }
  return {
    diagnostics,
    contributions,
    contributionsError,
    diagnosticsError,
    items,
    developmentMode: value.developmentMode,
    apiVersion: value.apiVersion,
    runtime: value.runtime,
  };
}
export const pluginsApi = {
  async extensionStatus(query: ExtensionQuery): Promise<ExtensionStatus> {
    const v = await queryExtension(query);
    if (
      !object(v) ||
      v.apiVersion !== "1.0.0" ||
      !Array.isArray(v.plans) ||
      !Array.isArray(v.calls) ||
      !Array.isArray(v.protocolBindings) ||
      typeof v.resourceAuthorized !== "boolean" ||
      (v.resourceTarget !== null && typeof v.resourceTarget !== "string") ||
      (v.resourceLabel !== null && typeof v.resourceLabel !== "string")
    )
      throw new Error("高级扩展状态无效。");
    parseSchema(policySchema, v.policy);
    parsePlans(v.plans);
    parsePlanCapacity(v.planCapacity);
    if (
      v.calls.some(
        (c) =>
          !object(c) ||
          typeof c.identity !== "string" ||
          typeof c.tool !== "string" ||
          typeof c.status !== "string" ||
          (c.durationMs !== null && typeof c.durationMs !== "number"),
      )
    )
      throw new Error("MCP 观测记录无效。");
    if (
      v.connector !== null &&
      (!object(v.connector) ||
        typeof v.connector.observationEnabled !== "boolean" ||
        typeof v.connector.transportConnected !== "boolean")
    )
      throw new Error("MCP 连接状态无效。");
    if (object(v.connector)) {
      const { observationIncomplete, unconfirmedCalls } = v.connector;
      if (
        (observationIncomplete !== undefined &&
          typeof observationIncomplete !== "boolean") ||
        (unconfirmedCalls !== undefined &&
          (!Number.isSafeInteger(unconfirmedCalls) ||
            Number(unconfirmedCalls) < 0)) ||
        (observationIncomplete !== undefined &&
          unconfirmedCalls !== undefined &&
          observationIncomplete !== Number(unconfirmedCalls) > 0)
      )
        throw new Error("MCP 观测完整性状态无效。");
    }
    return v as unknown as ExtensionStatus;
  },
  async extensionHistory(): Promise<ExtensionHistory> {
    const query = { pluginId: "", digest: "" };
    const v = await queryExtension(query);
    if (!object(v) || !Array.isArray(v.plans))
      throw new Error("配置恢复记录无效。");
    return {
      plans: parsePlans(v.plans),
      planCapacity: parsePlanCapacity(v.planCapacity),
    };
  },
  async extensionAction(input: ExtensionRequest) {
    return command<unknown>("perform_plugin_extension", { input });
  },
  async authorizeResource(query: ExtensionQuery) {
    invalidateCapabilityAccess();
    return command<unknown>("authorize_plugin_resource", { query });
  },
  async exportConnector(query: ExtensionQuery) {
    return command<unknown>("export_plugin_connector", { query });
  },
  async list(): Promise<PluginList> {
    return parsePluginList(await command<unknown>("list_plugins"));
  },
  async inspect(): Promise<PackageReview | null> {
    const value = await command<unknown>("inspect_plugin_package");
    return value === null
      ? null
      : parseContract<PackageReview>("review", value);
  },
  async install(input: InstallRequest) {
    invalidateCapabilityAccess();
    return status(await command<unknown>("install_plugin_package", { input }));
  },
  async change(input: StateRequest) {
    invalidateCapabilityAccess();
    return status(
      await command<unknown>("change_plugin_state", { input }).finally(
        invalidateCapabilityAccess,
      ),
    );
  },
  async authorizeSource(pluginId: string, digest: string) {
    invalidateCapabilityAccess();
    const value = await command<unknown>("authorize_plugin_source", {
      pluginId,
      digest,
    }).finally(invalidateCapabilityAccess);
    return value === null ? null : status(value);
  },
  async query(query: ViewQuery) {
    return view(await command<unknown>("query_plugin_view", { query }));
  },
  async action(input: ActionRequest) {
    return command<unknown>("perform_plugin_action", { input });
  },
};
function parsePlans(v: unknown[]): ExtensionPlan[] {
  if (
    v.some(
      (p) =>
        !object(p) ||
        ![
          "planId",
          "pluginId",
          "digest",
          "path",
          "revision",
          "providerId",
          "providerName",
          "state",
          "createdAt",
        ].every((k) => typeof p[k] === "string") ||
        !Array.isArray(p.edits) ||
        typeof p.backupAvailable !== "boolean" ||
        (p.canDiscard !== undefined && typeof p.canDiscard !== "boolean") ||
        (p.canDiscard === true &&
          (p.state !== "prepared" || p.backupAvailable !== false)),
    )
  )
    throw new Error("配置计划记录无效。");
  return v as ExtensionPlan[];
}

function parsePlanCapacity(value: unknown): PlanCapacity | undefined {
  if (value === undefined) return undefined;
  if (
    !object(value) ||
    ![value.used, value.limit, value.remaining].every(
      (n) => typeof n === "number" && Number.isSafeInteger(n) && n >= 0,
    ) ||
    Number(value.limit) === 0 ||
    Number(value.remaining) !==
      Math.max(0, Number(value.limit) - Number(value.used))
  ) {
    throw new Error("配置计划容量信息无效。");
  }
  return value as unknown as PlanCapacity;
}
