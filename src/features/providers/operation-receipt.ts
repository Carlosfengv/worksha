import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 } from "@/shared/lib/uuid";
import type { Provider, ProviderVerification } from "./types";

export const MAX_PROVIDER_REQUEST_BYTES = 2 * 1024 * 1024;
export interface ProviderWriteProtocol {
  version: 1;
  storeId: string;
  sequence: number;
  receiptLimit: number;
  receiptBytes: number;
  pendingLimit: number;
  pendingOperationIds: string[];
}
export interface ProviderWriteRequest {
  storeId: string;
  operationId: string;
  expectedSequence: number;
  kind: "save" | "connect";
  provider: Provider;
}
export interface ProviderOperation {
  apiVersion: 1;
  sequence: number;
  status: "pending" | "committed" | "failed" | "cancelled";
  request: ProviderWriteRequest;
  provider: Provider | null;
  verification: ProviderVerification | null;
  error: { code: string; message: string } | null;
}
type RecordValue = Record<string, unknown>;
const object = (value: unknown): value is RecordValue =>
  !!value && typeof value === "object" && !Array.isArray(value);
const keys = (value: RecordValue, allowed: string[]) =>
  Object.keys(value).every((key) => allowed.includes(key));
const integer = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const encoder = new TextEncoder();
const text = (value: unknown, max: number): value is string =>
  typeof value === "string" &&
  encoder.encode(value).length <= max &&
  !/[\u0000-\u001f\u007f]/u.test(value);
const uuid = (value: unknown): value is string =>
  typeof value === "string" &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu.test(
    value,
  );
export const providerOperationId = isUuidV4;
const endpoint = (value: unknown): value is string => {
  if (!text(value, 2048)) return false;
  try {
    const url = new URL(value.trim());
    return (
      ["http:", "https:"].includes(url.protocol) &&
      !url.username &&
      !url.password &&
      !url.search &&
      !url.hash &&
      !!url.hostname
    );
  } catch {
    return false;
  }
};
const strings = (value: unknown): value is string[] =>
  Array.isArray(value) &&
  value.length <= 2000 &&
  value.every((id) => text(id, 800) && !!id && Array.from(id).length <= 200);
export function validatedProviderMetadata(value: unknown): Provider | null {
  if (
    !object(value) ||
    !keys(value, [
      "id",
      "name",
      "vendor",
      "endpoint",
      "protocol",
      "model",
      "configured",
      "credentialRef",
      "credentialLabel",
      "models",
      "disabledModels",
      "modelCatalog",
      "revision",
      "category",
    ])
  )
    return null;
  const models = value.models;
  if (!strings(models)) return null;
  const modelIds = new Set(models);
  if (
    !text(value.id, 100) ||
    !value.id ||
    !text(value.name, 400) ||
    !endpoint(value.endpoint) ||
    !text(value.protocol, 32) ||
    !["responses", "messages", "chat-completions"].includes(value.protocol) ||
    !text(value.model, 800) ||
    Array.from(value.model).length > 200 ||
    typeof value.configured !== "boolean" ||
    !integer(value.revision) ||
    !text(value.category, 16) ||
    !["cloud", "custom", "local"].includes(value.category) ||
    (value.vendor !== undefined && !text(value.vendor, 50)) ||
    (value.credentialRef !== null && !uuid(value.credentialRef)) ||
    (value.credentialLabel !== null && !text(value.credentialLabel, 200)) ||
    (value.disabledModels !== undefined &&
      (!strings(value.disabledModels) ||
        !value.disabledModels.every((id) => modelIds.has(id))))
  )
    return null;
  let catalog: Provider["modelCatalog"] = null;
  if (value.modelCatalog != null) {
    const c = value.modelCatalog;
    if (
      !object(c) ||
      !keys(c, [
        "fetchedAt",
        "sourceUrl",
        "endpoint",
        "protocol",
        "credentialRef",
      ]) ||
      !text(c.fetchedAt, 64) ||
      !Number.isFinite(Date.parse(c.fetchedAt)) ||
      !endpoint(c.sourceUrl) ||
      !endpoint(c.endpoint) ||
      !["responses", "messages", "chat-completions"].includes(
        String(c.protocol),
      ) ||
      (c.credentialRef !== null && !uuid(c.credentialRef))
    )
      return null;
    catalog = {
      fetchedAt: c.fetchedAt,
      sourceUrl: c.sourceUrl,
      endpoint: c.endpoint,
      protocol: String(c.protocol),
      credentialRef: c.credentialRef as string | null,
    };
  }
  return {
    id: value.id,
    name: value.name,
    vendor: (value.vendor as string | undefined) ?? "",
    endpoint: value.endpoint,
    protocol: value.protocol as Provider["protocol"],
    model: value.model,
    configured: value.configured,
    credentialRef: value.credentialRef as string | null,
    credentialLabel: value.credentialLabel as string | null,
    models: [...models],
    disabledModels: [...((value.disabledModels as string[] | undefined) ?? [])],
    modelCatalog: catalog,
    revision: value.revision,
    category: value.category as Provider["category"],
  };
}
export function storedProviderRequest(
  value: unknown,
): ProviderWriteRequest | null {
  if (
    !object(value) ||
    !keys(value, [
      "storeId",
      "operationId",
      "expectedSequence",
      "kind",
      "provider",
    ]) ||
    !providerOperationId(value.storeId) ||
    !providerOperationId(value.operationId) ||
    !integer(value.expectedSequence) ||
    value.expectedSequence >= Number.MAX_SAFE_INTEGER ||
    (value.kind !== "save" && value.kind !== "connect")
  )
    return null;
  const p = validatedProviderMetadata(value.provider);
  if (!p || p.revision >= Number.MAX_SAFE_INTEGER) return null;
  const request: ProviderWriteRequest = {
    storeId: value.storeId,
    operationId: value.operationId,
    expectedSequence: value.expectedSequence,
    kind: value.kind,
    provider: p,
  };
  return new TextEncoder().encode(JSON.stringify(request)).length <=
    MAX_PROVIDER_REQUEST_BYTES
    ? request
    : null;
}
export function providerProtocol(value: unknown): ProviderWriteProtocol {
  if (
    !object(value) ||
    !keys(value, [
      "version",
      "storeId",
      "sequence",
      "receiptLimit",
      "receiptBytes",
      "pendingLimit",
      "pendingOperationIds",
    ]) ||
    value.version !== 1 ||
    !providerOperationId(value.storeId) ||
    !integer(value.sequence) ||
    !integer(value.receiptLimit) ||
    value.receiptLimit < 1 ||
    value.receiptLimit > 256 ||
    !integer(value.receiptBytes) ||
    value.receiptBytes < 1 ||
    value.receiptBytes > 16 * 1024 * 1024 ||
    !integer(value.pendingLimit) ||
    value.pendingLimit < 1 ||
    value.pendingLimit > 4 ||
    !Array.isArray(value.pendingOperationIds) ||
    value.pendingOperationIds.length > value.pendingLimit ||
    !value.pendingOperationIds.every(providerOperationId) ||
    new Set(value.pendingOperationIds).size !== value.pendingOperationIds.length
  )
    throw new DesktopError({
      code: "write_protocol_unavailable",
      outcomeUnknown: false,
      message: "当前后台不支持服务商结果核查，请升级后台后再保存或连接。",
    });
  return value as unknown as ProviderWriteProtocol;
}
export function freezeProviderRequest(
  protocol: ProviderWriteProtocol,
  kind: ProviderWriteRequest["kind"],
  draft: Provider,
): ProviderWriteRequest {
  const result = storedProviderRequest({
    storeId: protocol.storeId,
    operationId: crypto.randomUUID(),
    expectedSequence: protocol.sequence,
    kind,
    provider: draft,
  });
  if (!result) throw new Error("服务商操作参数无法安全保留，本次未提交。");
  return structuredClone(result);
}
// Normalize optional Provider fields before comparing; Rust omits empty/default fields.
export const sameProviderRequest = (
  a: ProviderWriteRequest,
  b: ProviderWriteRequest,
) => {
  const first = storedProviderRequest(a),
    second = storedProviderRequest(b);
  return (
    !!first && !!second && JSON.stringify(first) === JSON.stringify(second)
  );
};
export function providerOperation(
  value: unknown,
  expected: ProviderWriteRequest,
): ProviderOperation {
  const invalid = () =>
    new DesktopError({
      code: "invalid_response",
      outcomeUnknown: true,
      message: "服务商操作回执与原请求不一致，请保留原编号核查。",
    });
  if (
    !object(value) ||
    !keys(value, [
      "apiVersion",
      "sequence",
      "status",
      "request",
      "provider",
      "verification",
      "error",
    ]) ||
    value.apiVersion !== 1 ||
    value.sequence !== expected.expectedSequence + 1 ||
    !["pending", "committed", "failed", "cancelled"].includes(
      String(value.status),
    )
  )
    throw invalid();
  const request = storedProviderRequest(value.request);
  if (!request || !sameProviderRequest(request, expected)) throw invalid();
  const p =
    value.provider === null ? null : validatedProviderMetadata(value.provider);
  if (value.status === "committed") {
    if (
      !p ||
      !p.configured ||
      !p.name ||
      p.id !== request.provider.id ||
      p.revision !== request.provider.revision + 1 ||
      value.error !== null
    )
      throw invalid();
    if (request.kind === "save") {
      if (value.verification !== null) throw invalid();
    } else {
      const v = value.verification;
      if (
        !object(v) ||
        !keys(v, [
          "providerId",
          "model",
          "fingerprint",
          "state",
          "checkedAt",
          "message",
        ]) ||
        v.providerId !== p.id ||
        v.model !== p.model ||
        v.state !== "verified" ||
        typeof v.fingerprint !== "string" ||
        !/^[a-f0-9]{64}$/u.test(v.fingerprint) ||
        !text(v.checkedAt, 64) ||
        !Number.isFinite(Date.parse(v.checkedAt)) ||
        !text(v.message, 512)
      )
        throw invalid();
    }
  } else {
    if (
      value.provider !== null ||
      value.verification !== null ||
      (value.status !== "cancelled" && request.kind !== "connect")
    )
      throw invalid();
    if (value.status === "failed") {
      if (
        !object(value.error) ||
        !keys(value.error, ["code", "message"]) ||
        !text(value.error.code, 80) ||
        !value.error.code ||
        !text(value.error.message, 512)
      )
        throw invalid();
    } else if (value.error !== null) throw invalid();
  }
  return {
    apiVersion: 1,
    sequence: value.sequence as number,
    status: value.status as ProviderOperation["status"],
    request,
    provider: p,
    verification: value.verification as ProviderVerification | null,
    error: value.error as ProviderOperation["error"],
  };
}
