import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 } from "@/shared/lib/uuid";
import type { ModelService, ModelServiceDraft, RoutingPolicy } from "./types";
export type ServiceAction =
  | { kind: "save"; draft: ModelServiceDraft }
  | { kind: "delete"; serviceId: string; expectedRevision: number };
export interface ServiceRequest {
  storeId: string;
  operationId: string;
  action: ServiceAction;
}
export interface ServiceReceipt {
  storeId: string;
  apiVersion: 1;
  operationId: string;
  action: "save" | "delete";
  serviceId: string;
  committedRevision: number;
  service: ModelService | null;
}
export const operationId = isUuidV4;
const object = (v: unknown): Record<string, unknown> | null =>
  v !== null && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
const text = (v: unknown, max = 200): v is string =>
  typeof v === "string" && !!v.trim() && v.length <= max && !/\p{Cc}/u.test(v);
const revision = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
function invalid(): never {
  throw new DesktopError({
    code: "transport_invalid_response",
    outcomeUnknown: true,
    message: "未能核对模型服务回执，请保留原编号查询结果。",
  });
}
export function actionIdentity(action: ServiceAction) {
  return action.kind === "save"
    ? { serviceId: action.draft.id, expectedRevision: action.draft.revision }
    : {
        serviceId: action.serviceId,
        expectedRevision: action.expectedRevision,
      };
}
function capabilities(value: unknown) {
  const v = object(value);
  return (
    !!v &&
    revision(v.contextWindow) &&
    v.contextWindow >= 2048 &&
    v.contextWindow <= 2000000 &&
    revision(v.maxOutputTokens) &&
    v.maxOutputTokens >= 16 &&
    v.maxOutputTokens <= 65536 &&
    v.maxOutputTokens < v.contextWindow &&
    typeof v.tools === "boolean" &&
    v.thinking === false &&
    Array.isArray(v.inputModalities) &&
    v.inputModalities.length === 1 &&
    v.inputModalities[0] === "text"
  );
}
function policy(value: unknown): value is RoutingPolicy {
  const v = object(value);
  return (
    !!v &&
    typeof v.enabled === "boolean" &&
    [v.order, v.disabledModels].every(
      (a) => Array.isArray(a) && a.length <= 32 && a.every((x) => text(x, 128)),
    ) &&
    typeof v.fallbackOnFailure === "boolean" &&
    typeof v.fallbackOnRateLimit === "boolean" &&
    revision(v.maxAttempts) &&
    v.maxAttempts >= 1 &&
    v.maxAttempts <= 5 &&
    revision(v.deadlineSeconds) &&
    v.deadlineSeconds >= 5 &&
    v.deadlineSeconds <= 300 &&
    revision(v.cooldownSeconds) &&
    v.cooldownSeconds >= 1 &&
    v.cooldownSeconds <= 3600 &&
    Array.isArray(v.limits) &&
    v.limits.length <= 32 &&
    v.limits.every((row) => {
      const r = object(row);
      return (
        !!r &&
        text(r.modelId, 128) &&
        revision(r.maxConcurrent) &&
        r.maxConcurrent <= 1024 &&
        revision(r.requestsPerMinute) &&
        r.requestsPerMinute <= 1000000
      );
    })
  );
}
export function serviceReceipt(
  value: unknown,
  request: ServiceRequest,
): ServiceReceipt {
  const v = object(value),
    identity = actionIdentity(request.action);
  if (
    !operationId(request.storeId) ||
    !operationId(request.operationId) ||
    !v ||
    v.storeId !== request.storeId ||
    v.apiVersion !== 1 ||
    v.operationId !== request.operationId ||
    v.action !== request.action.kind ||
    v.serviceId !== identity.serviceId ||
    !revision(v.committedRevision) ||
    v.committedRevision !== identity.expectedRevision + 1
  )
    invalid();
  if (request.action.kind === "delete") {
    if (v.service !== null) invalid();
  } else {
    const s = object(v.service);
    if (
      !s ||
      s.id !== identity.serviceId ||
      s.revision !== v.committedRevision ||
      !text(s.versionId, 128) ||
      !text(s.name, 80) ||
      !text(s.publishedAt, 64) ||
      !Number.isFinite(Date.parse(s.publishedAt)) ||
      !Array.isArray(s.models) ||
      !s.models.length ||
      s.models.length > 32 ||
      !policy(s.routingPolicy)
    )
      invalid();
    const ids = new Set<string>();
    for (const row of s.models) {
      const m = object(row),
        p = object(m?.provider);
      if (
        !m ||
        !text(m.id, 128) ||
        ids.has(m.id) ||
        !text(m.name, 80) ||
        !capabilities(m.capabilities) ||
        !p ||
        !text(p.id) ||
        !text(p.name) ||
        !text(p.model) ||
        !text(p.endpoint, 2048) ||
        !["messages", "responses", "chat-completions"].includes(
          String(p.protocol),
        ) ||
        typeof p.configured !== "boolean" ||
        !revision(p.revision)
      )
        invalid();
      ids.add(m.id);
    }
    if (typeof s.defaultModelId !== "string" || !ids.has(s.defaultModelId))
      invalid();
  }
  return v as unknown as ServiceReceipt;
}
// Project only publication parameters; Provider objects, endpoints, credentials
// and arbitrary props must never enter the recovery record.
export function freezeRequest(
  action: ServiceAction,
  storeId: string,
  id: string = crypto.randomUUID(),
): ServiceRequest {
  if (!operationId(id) || !operationId(storeId)) invalid();
  if (action.kind === "delete")
    return {
      storeId,
      operationId: id,
      action: {
        kind: "delete",
        serviceId: action.serviceId,
        expectedRevision: action.expectedRevision,
      },
    };
  const d = action.draft;
  return {
    storeId,
    operationId: id,
    action: {
      kind: "save",
      draft: {
        id: d.id,
        revision: d.revision,
        name: d.name,
        defaultModelId: d.defaultModelId,
        routingPolicy: d.routingPolicy
          ? {
              enabled: d.routingPolicy.enabled,
              order: [...d.routingPolicy.order],
              disabledModels: [...d.routingPolicy.disabledModels],
              fallbackOnFailure: d.routingPolicy.fallbackOnFailure,
              fallbackOnRateLimit: d.routingPolicy.fallbackOnRateLimit,
              maxAttempts: d.routingPolicy.maxAttempts,
              deadlineSeconds: d.routingPolicy.deadlineSeconds,
              cooldownSeconds: d.routingPolicy.cooldownSeconds,
              limits: d.routingPolicy.limits.map((l) => ({
                modelId: l.modelId,
                maxConcurrent: l.maxConcurrent,
                requestsPerMinute: l.requestsPerMinute,
              })),
            }
          : undefined,
        models: d.models.map((m) => ({
          id: m.id,
          name: m.name,
          providerId: m.providerId,
          model: m.model,
          capabilities: {
            contextWindow: m.capabilities.contextWindow,
            maxOutputTokens: m.capabilities.maxOutputTokens,
            tools: m.capabilities.tools,
            thinking: m.capabilities.thinking,
            inputModalities: [...m.capabilities.inputModalities],
          },
        })),
      },
    },
  };
}
export function storedRequest(value: unknown): ServiceRequest | null {
  const v = object(value),
    a = object(v?.action);
  if (!v || !operationId(v.storeId) || !operationId(v.operationId) || !a)
    return null;
  if (
    a.kind === "delete" &&
    text(a.serviceId, 128) &&
    revision(a.expectedRevision)
  ) {
    const request = freezeRequest(
      {
        kind: "delete",
        serviceId: a.serviceId,
        expectedRevision: a.expectedRevision,
      },
      v.storeId,
      v.operationId,
    );
    return JSON.stringify(request) === JSON.stringify(value) ? request : null;
  }
  const d = object(a.draft);
  if (
    a.kind !== "save" ||
    !d ||
    !text(d.id, 128) ||
    !revision(d.revision) ||
    !text(d.name, 80) ||
    !text(d.defaultModelId, 128) ||
    !Array.isArray(d.models) ||
    !d.models.length ||
    d.models.length > 32 ||
    (d.routingPolicy !== undefined && !policy(d.routingPolicy))
  )
    return null;
  for (const row of d.models) {
    const m = object(row);
    if (
      !m ||
      !text(m.id, 128) ||
      !text(m.name, 80) ||
      !text(m.providerId) ||
      !text(m.model) ||
      !capabilities(m.capabilities)
    )
      return null;
  }
  const request = freezeRequest(
    { kind: "save", draft: d as unknown as ModelServiceDraft },
    v.storeId,
    v.operationId,
  );
  // Extra fields are corrupt/untrusted metadata, never silently adopted.
  if (JSON.stringify(request) !== JSON.stringify(value)) return null;
  return request;
}
