import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 } from "@/shared/lib/uuid";
import type { ModelService, ModelServiceDraft, RoutingPolicy } from "./types";
import { accountStrategies, routingAgents } from "./types";
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
function accountPools(value: unknown) {
  if (value === undefined) return true;
  if (!Array.isArray(value) || value.length > 32) return false;
  const models = new Set<string>();
  let total = 0;
  return value.every((entry: unknown) => {
    const pool = object(entry);
    if (
      !pool ||
      !text(pool.modelId, 128) ||
      models.has(pool.modelId) ||
      !Array.isArray(pool.accounts) ||
      pool.accounts.length < 1 ||
      pool.accounts.length > 8
    )
      return false;
    models.add(pool.modelId);
    total += pool.accounts.length;
    const accounts = new Set<string>();
    const weights = object(pool.weights);
    return (
      total <= 64 &&
      (pool.strategy === undefined ||
        accountStrategies.some((strategy) => strategy === pool.strategy)) &&
      (pool.affinity === undefined ||
        pool.affinity === "off" ||
        pool.affinity === "prefer" ||
        pool.affinity === "strict") &&
      (pool.weights === undefined || weights !== null) &&
      pool.accounts.every((entry: unknown) => {
        const account = object(entry);
        if (
          !account ||
          !operationId(account.id) ||
          accounts.has(account.id) ||
          !revision(account.revision) ||
          account.revision < 1 ||
          account.revision > 4294967295
        )
          return false;
        accounts.add(account.id);
        return true;
      }) &&
      Object.entries(weights ?? {}).every(
        ([id, weight]) =>
          pool.strategy === "weighted" &&
          accounts.has(id) &&
          revision(weight) &&
          weight >= 1 &&
          weight <= 1000,
      )
    );
  });
}
function frozenPools(policy?: RoutingPolicy) {
  return (policy?.accountPools ?? []).map((pool) => ({
    modelId: pool.modelId,
    accounts: pool.accounts.map((a) => ({ id: a.id, revision: a.revision })),
    ...(pool.strategy && pool.strategy !== "priority"
      ? { strategy: pool.strategy }
      : {}),
    ...(pool.affinity && pool.affinity !== "off"
      ? { affinity: pool.affinity }
      : {}),
    ...(pool.weights && Object.keys(pool.weights).length
      ? {
          weights: Object.fromEntries(
            Object.entries(pool.weights).sort(([a], [b]) => a.localeCompare(b)),
          ),
        }
      : {}),
  }));
}
function validRules(value: unknown) {
  if (value === undefined) return true;
  if (!Array.isArray(value) || value.length > 16) return false;
  const ids = new Set<string>();
  return value.every((entry: unknown) => {
    const rule = object(entry);
    if (
      !rule ||
      !operationId(rule.id) ||
      ids.has(rule.id) ||
      !text(rule.name, 160) ||
      [...rule.name].length > 80 ||
      !text(rule.modelId, 128)
    )
      return false;
    ids.add(rule.id);
    return (
      [
        rule.agent,
        rule.projectId,
        rule.tools,
        rule.stream,
        rule.minInputTokens,
        rule.intent,
      ].some((condition) => condition !== undefined) &&
      (rule.agent === undefined ||
        routingAgents.some((agent) => agent === rule.agent)) &&
      (rule.projectId === undefined ||
        (typeof rule.projectId === "string" &&
          /^project:[0-9a-f]{64}$/i.test(rule.projectId))) &&
      [rule.tools, rule.stream].every(
        (flag) => flag === undefined || typeof flag === "boolean",
      ) &&
      (rule.intent === undefined ||
        (text(rule.intent, 96) &&
          [...rule.intent].length <= 48 &&
          rule.intent.trim() === rule.intent)) &&
      (rule.minInputTokens === undefined ||
        (revision(rule.minInputTokens) &&
          rule.minInputTokens >= 1 &&
          rule.minInputTokens <= 2000000))
    );
  });
}
function frozenRules(policy?: RoutingPolicy) {
  return (policy?.rules ?? []).map((rule) => ({
    id: rule.id,
    name: rule.name,
    modelId: rule.modelId,
    agent: rule.agent,
    projectId: rule.projectId,
    tools: rule.tools,
    stream: rule.stream,
    minInputTokens: rule.minInputTokens,
    intent: rule.intent,
  }));
}
function validClassifier(value: unknown) {
  if (value === undefined) return true;
  const classifier = object(value);
  if (!classifier || !text(classifier.modelId, 128)) return false;
  if (classifier.account === undefined) return true;
  const account = object(classifier.account);
  return (
    !!account &&
    operationId(account.id) &&
    revision(account.revision) &&
    account.revision >= 1 &&
    account.revision <= 4294967295
  );
}
function frozenClassifier(policy?: RoutingPolicy) {
  const classifier = policy?.intentClassifier;
  if (!classifier) return undefined;
  return {
    modelId: classifier.modelId,
    ...(classifier.account
      ? {
          account: {
            id: classifier.account.id,
            revision: classifier.account.revision,
          },
        }
      : {}),
  };
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
    (v.fallbackOnAccessDenied === undefined ||
      typeof v.fallbackOnAccessDenied === "boolean") &&
    (v.fallbackAcrossModels === undefined ||
      typeof v.fallbackAcrossModels === "boolean") &&
    accountPools(v.accountPools) &&
    validRules(v.rules) &&
    validClassifier(v.intentClassifier) &&
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
        p = object(m?.provider),
        child = object(m?.child),
        expectedChild = request.action.draft.models.find(
          (member) => member.id === m?.id,
        )?.child;
      if (
        !m ||
        !text(m.id, 128) ||
        ids.has(m.id) ||
        !text(m.name, 80) ||
        !capabilities(m.capabilities) ||
        (expectedChild
          ? child?.serviceId !== expectedChild.serviceId ||
            child?.versionId !== expectedChild.versionId
          : m.child !== undefined) ||
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
    if (request.action.draft.models.some((m) => m.child && !ids.has(m.id)))
      invalid();
    if (typeof s.defaultModelId !== "string" || !ids.has(s.defaultModelId))
      invalid();
    const expected = request.action.draft.routingPolicy;
    if (
      JSON.stringify(frozenPools(s.routingPolicy)) !==
        JSON.stringify(frozenPools(expected)) ||
      JSON.stringify(frozenRules(s.routingPolicy)) !==
        JSON.stringify(frozenRules(expected)) ||
      JSON.stringify(frozenClassifier(s.routingPolicy)) !==
        JSON.stringify(frozenClassifier(expected)) ||
      !!s.routingPolicy.fallbackOnAccessDenied !==
        !!expected?.fallbackOnAccessDenied ||
      !!s.routingPolicy.fallbackAcrossModels !==
        !!expected?.fallbackAcrossModels
    )
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
              ...(d.routingPolicy.fallbackOnAccessDenied
                ? { fallbackOnAccessDenied: true }
                : {}),
              maxAttempts: d.routingPolicy.maxAttempts,
              deadlineSeconds: d.routingPolicy.deadlineSeconds,
              cooldownSeconds: d.routingPolicy.cooldownSeconds,
              limits: d.routingPolicy.limits.map((l) => ({
                modelId: l.modelId,
                maxConcurrent: l.maxConcurrent,
                requestsPerMinute: l.requestsPerMinute,
              })),
              ...(d.routingPolicy.accountPools?.length
                ? { accountPools: frozenPools(d.routingPolicy) }
                : {}),
              ...(d.routingPolicy.rules?.length
                ? { rules: frozenRules(d.routingPolicy) }
                : {}),
              ...(d.routingPolicy.intentClassifier
                ? { intentClassifier: frozenClassifier(d.routingPolicy) }
                : {}),
              ...(d.routingPolicy.accountPools?.length &&
              d.routingPolicy.fallbackAcrossModels
                ? { fallbackAcrossModels: true }
                : {}),
            }
          : undefined,
        models: d.models.map((m) => ({
          id: m.id,
          name: m.name,
          providerId: m.providerId,
          model: m.model,
          ...(m.child
            ? {
                child: {
                  serviceId: m.child.serviceId,
                  versionId: m.child.versionId,
                },
              }
            : {}),
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
    const child = object(m?.child);
    if (
      !m ||
      !text(m.id, 128) ||
      !text(m.name, 80) ||
      (child
        ? !text(child.serviceId, 128) ||
          !text(child.versionId, 128) ||
          m.providerId !== "" ||
          m.model !== ""
        : m.child !== undefined || !text(m.providerId) || !text(m.model)) ||
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
