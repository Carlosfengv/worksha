import { DesktopError } from "@/shared/lib/desktop-command";
import {
  gatewayImpact,
  retentionInstant,
  retentionMetadata,
  type GatewayImpact,
  type RetentionMetadata,
  type RetentionRequest,
} from "./retention-result";

export const retentionSourceIssues = {
  source_absent: "未找到来源文件，无法确认历史是否为空。",
  source_changed: "来源文件已变化，请重新预览。",
  unsupported_schema: "来源格式尚不受支持，原记录已保留。",
  unavailable: "来源读取未完成，请重试。",
  limit: "来源超过本次预览限制，未返回部分数量。",
  invalid_input: "来源的时间范围尚未通过核对。",
} as const;
export type SourceReport<T> =
  | {
      status: "checked";
      readStartedAt: string;
      readFinishedAt: string;
      value: T;
    }
  | { status: "unverified"; reason: keyof typeof retentionSourceIssues };
export type ToolImpact = {
  records: number;
  expired: number;
  withinWindow: number;
  protected: GatewayImpact["protected"];
  unreadableRecords: number;
  classificationComplete: boolean;
  capacityLimit: 10000 | null;
};
export type RetentionInventory = RetentionMetadata & {
  scope: "product_owned_saved_metadata";
  remainingSources: "not_checked";
  gateway: SourceReport<GatewayImpact>;
  tools: SourceReport<ToolImpact>;
  toolHistory: "saved_only_prior_history_unknown";
};
const object = (v: unknown): Record<string, unknown> | null =>
  v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
const keys = (v: Record<string, unknown>, list: string[]) =>
  Object.keys(v).length === list.length &&
  list.every((k) => Object.hasOwn(v, k));
const count = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
function invalid(): never {
  throw new DesktopError({
    code: "retention_invalid_response",
    message: "综合留存预览回复尚未通过核对，请重新预览。",
    outcomeUnknown: false,
  });
}
function toolImpact(value: unknown): ToolImpact {
  const v = object(value),
    p = object(v?.protected);
  if (
    !v ||
    !p ||
    !keys(v, [
      "records",
      "expired",
      "withinWindow",
      "protected",
      "unreadableRecords",
      "classificationComplete",
      "capacityLimit",
    ]) ||
    !keys(p, [
      "active",
      "unknownOutcome",
      "unknownTime",
      "unsupportedSource",
    ]) ||
    !count(v.records) ||
    v.records > 1_000_000 ||
    !count(v.expired) ||
    !count(v.withinWindow) ||
    !count(v.unreadableRecords) ||
    !count(p.active) ||
    !count(p.unknownOutcome) ||
    !count(p.unknownTime) ||
    !count(p.unsupportedSource) ||
    v.expired +
      v.withinWindow +
      v.unreadableRecords +
      p.active +
      p.unknownOutcome +
      p.unknownTime +
      p.unsupportedSource !==
      v.records ||
    v.classificationComplete !==
      (v.unreadableRecords === 0 && p.unsupportedSource === 0) ||
    !(v.capacityLimit === null || v.capacityLimit === 10_000)
  )
    invalid();
  return v as ToolImpact;
}
export function retentionSourceReport<T>(
  value: unknown,
  parse: (value: unknown) => T,
  meta: RetentionMetadata,
): SourceReport<T> {
  const v = object(value);
  if (!v) invalid();
  if (v.status === "unverified") {
    if (
      !keys(v, ["status", "reason"]) ||
      typeof v.reason !== "string" ||
      !Object.hasOwn(retentionSourceIssues, v.reason)
    )
      invalid();
    return v as SourceReport<T>;
  }
  if (
    v.status !== "checked" ||
    !keys(v, ["status", "readStartedAt", "readFinishedAt", "value"]) ||
    !retentionInstant(v.readStartedAt) ||
    !retentionInstant(v.readFinishedAt) ||
    Date.parse(v.readStartedAt) < Date.parse(meta.capturedAt) ||
    Date.parse(v.readFinishedAt) < Date.parse(v.readStartedAt) ||
    Date.parse(v.readFinishedAt) > Date.parse(meta.validUntil)
  )
    invalid();
  return {
    status: "checked",
    readStartedAt: v.readStartedAt,
    readFinishedAt: v.readFinishedAt,
    value: parse(v.value),
  };
}
export function retentionInventory(
  value: unknown,
  request: RetentionRequest,
): RetentionInventory {
  const v = object(value);
  if (
    !v ||
    !keys(v, [
      "version",
      "storeId",
      "settingsRevision",
      "days",
      "capturedAt",
      "cutoff",
      "validUntil",
      "cleanupEnabled",
      "extraArchiveEnabled",
      "confirmationAvailable",
      "scope",
      "remainingSources",
      "aggregateRetention",
      "gateway",
      "tools",
      "toolHistory",
      "pendingWrites",
      "writerIssue",
    ]) ||
    v.scope !== "product_owned_saved_metadata" ||
    v.remainingSources !== "not_checked" ||
    v.toolHistory !== "saved_only_prior_history_unknown"
  )
    invalid();
  retentionMetadata(v, request);
  return {
    ...v,
    gateway: retentionSourceReport(v.gateway, gatewayImpact, v),
    tools: retentionSourceReport(v.tools, toolImpact, v),
  } as RetentionInventory;
}
