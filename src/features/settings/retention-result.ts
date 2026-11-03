import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 as uuid } from "@/shared/lib/uuid";
import type { Settings } from "./types";

export type RetentionRequest = Readonly<{
  storeId: string;
  expectedSettingsRevision: number;
  days: Settings["retentionDays"];
}>;
export type RetentionPreview = {
  version: 1;
  storeId: string;
  settingsRevision: number;
  days: Settings["retentionDays"];
  capturedAt: string;
  cutoff: string;
  validUntil: string;
  cleanupEnabled: false;
  extraArchiveEnabled: false;
  confirmationAvailable: false;
  scope: "committed_gateway_only";
  otherSources: "not_checked";
  aggregateRetention: "continuous";
  gateway: {
    attempts: number;
    logicalRequests: number;
    expired: number;
    withinWindow: number;
    protected: {
      active: number;
      unknownOutcome: number;
      unknownTime: number;
      unsupportedSource: number;
    };
    unreadableAttempts: number;
    classificationComplete: boolean;
  };
  pendingWrites: number;
  writerIssue: boolean;
};

const object = (v: unknown): Record<string, unknown> | null =>
  v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
const count = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
export const retentionInstant = (v: unknown): v is string =>
  typeof v === "string" &&
  /^\d{4}-\d{2}-\d{2}T/.test(v) &&
  /(?:Z|[+-]\d{2}:\d{2})$/.test(v) &&
  Number.isFinite(Date.parse(v));
const keys = (v: Record<string, unknown>, names: string[]) =>
  Object.keys(v).length === names.length &&
  names.every((name) => Object.hasOwn(v, name));
function invalid(): never {
  throw new DesktopError({
    code: "retention_invalid_response",
    message: "留存预览回复尚未通过核对，请重新预览。",
    outcomeUnknown: false,
  });
}
export function freezeRetentionRequest(
  value: RetentionRequest,
): RetentionRequest {
  if (
    !uuid(value.storeId) ||
    !count(value.expectedSettingsRevision) ||
    ![7, 30, 90].includes(value.days)
  )
    invalid();
  return Object.freeze({
    storeId: value.storeId,
    expectedSettingsRevision: value.expectedSettingsRevision,
    days: value.days,
  });
}
export function retentionPreview(
  value: unknown,
  request: RetentionRequest,
): RetentionPreview {
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
      "otherSources",
      "aggregateRetention",
      "gateway",
      "pendingWrites",
      "writerIssue",
    ]) ||
    v.scope !== "committed_gateway_only" ||
    v.otherSources !== "not_checked"
  )
    invalid();
  retentionMetadata(v, request);
  gatewayImpact(v.gateway);
  return v as RetentionPreview;
}

export type RetentionMetadata = Omit<
  RetentionPreview,
  "gateway" | "scope" | "otherSources"
>;
export type GatewayImpact = RetentionPreview["gateway"];

export function retentionMetadata(
  v: Record<string, unknown>,
  request: RetentionRequest,
): asserts v is Record<string, unknown> & RetentionMetadata {
  if (
    v.version !== 1 ||
    v.storeId !== request.storeId ||
    v.settingsRevision !== request.expectedSettingsRevision ||
    v.days !== request.days ||
    v.cleanupEnabled !== false ||
    v.extraArchiveEnabled !== false ||
    v.confirmationAvailable !== false ||
    v.aggregateRetention !== "continuous" ||
    !retentionInstant(v.capturedAt) ||
    !retentionInstant(v.cutoff) ||
    !retentionInstant(v.validUntil) ||
    Date.parse(v.validUntil) - Date.parse(v.capturedAt) !== 300_000 ||
    Date.parse(v.capturedAt) - Date.parse(v.cutoff) !==
      request.days * 86_400_000 ||
    !count(v.pendingWrites) ||
    typeof v.writerIssue !== "boolean"
  )
    invalid();
}

export function gatewayImpact(value: unknown): GatewayImpact {
  const g = object(value),
    p = object(g?.protected);
  if (
    !g ||
    !p ||
    !keys(g, [
      "attempts",
      "logicalRequests",
      "expired",
      "withinWindow",
      "protected",
      "unreadableAttempts",
      "classificationComplete",
    ]) ||
    !keys(p, [
      "active",
      "unknownOutcome",
      "unknownTime",
      "unsupportedSource",
    ]) ||
    !count(g.attempts) ||
    g.attempts > 1_000_000 ||
    !count(g.logicalRequests) ||
    !count(g.expired) ||
    !count(g.withinWindow) ||
    !count(g.unreadableAttempts) ||
    !count(p.active) ||
    !count(p.unknownOutcome) ||
    !count(p.unknownTime) ||
    !count(p.unsupportedSource) ||
    g.expired +
      g.withinWindow +
      p.active +
      p.unknownOutcome +
      p.unknownTime +
      p.unsupportedSource !==
      g.logicalRequests ||
    g.logicalRequests + g.unreadableAttempts > g.attempts ||
    g.classificationComplete !==
      (g.unreadableAttempts === 0 && p.unsupportedSource === 0)
  )
    invalid();
  return g as GatewayImpact;
}
