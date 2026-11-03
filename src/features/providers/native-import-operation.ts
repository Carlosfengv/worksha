import { DesktopError } from "@/shared/lib/desktop-command";
import {
  providerOperationId,
  validatedProviderMetadata,
} from "./operation-receipt";
import type { Provider, ProviderImportCandidate } from "./types";

export interface NativeImportIdentity {
  storeId: string;
  namespaceId: string;
  operationId: string;
  expectedSequence: number;
}
export interface NativeImportRequest {
  candidateId: string;
  expectedRevision: string;
  model: string | null;
  operation: NativeImportIdentity;
}
export interface NativeImportProtocol {
  version: 1;
  storeId: string;
  namespaceId: string;
  namespaceState: "active";
  sequence: number;
  operationLimit: 4096;
  pendingLimit: 4;
  pendingRequests: NativeImportRequest[];
}
export interface NativeImportOperation {
  version: 1;
  sequence: number;
  status: "pending" | "committed" | "cancelled" | "expired";
  request: NativeImportRequest;
  provider: Provider | null;
  reused: boolean;
  createsCredential: boolean;
}
const object = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);
const keys = (value: Record<string, unknown>, allowed: string[]) =>
  Object.keys(value).every((key) => allowed.includes(key));
const text = (value: unknown, max: number): value is string =>
  typeof value === "string" &&
  new TextEncoder().encode(value).length <= max &&
  !/[\u0000-\u001f\u007f]/u.test(value);
const sequence = (value: unknown, max = 4096): value is number =>
  typeof value === "number" &&
  Number.isSafeInteger(value) &&
  value >= 0 &&
  value <= max;
const failure = () =>
  new DesktopError({
    code: "native_import_result_unknown",
    outcomeUnknown: true,
    message: "原导入回复无法核查，请保留原编号查询；不要重新导入。",
  });
const unavailable = () =>
  new DesktopError({
    code: "native_import_protocol_unavailable",
    outcomeUnknown: false,
    message: "当前后台的原生导入能力或来源无法核查，未提交导入。",
  });

export function storedNativeImport(value: unknown): NativeImportRequest | null {
  if (
    !object(value) ||
    !keys(value, ["candidateId", "expectedRevision", "model", "operation"]) ||
    !text(value.candidateId, 256) ||
    !value.candidateId ||
    typeof value.expectedRevision !== "string" ||
    !/^[a-f0-9]{64}$/u.test(value.expectedRevision) ||
    (value.model !== null && !text(value.model, 800)) ||
    !object(value.operation)
  )
    return null;
  const operation = value.operation;
  if (
    !keys(operation, [
      "storeId",
      "namespaceId",
      "operationId",
      "expectedSequence",
    ]) ||
    !providerOperationId(operation.storeId) ||
    !providerOperationId(operation.namespaceId) ||
    !providerOperationId(operation.operationId) ||
    !sequence(operation.expectedSequence, 4095)
  )
    return null;
  return {
    candidateId: value.candidateId,
    expectedRevision: value.expectedRevision,
    model: value.model as string | null,
    operation: {
      storeId: operation.storeId,
      namespaceId: operation.namespaceId,
      operationId: operation.operationId,
      expectedSequence: operation.expectedSequence,
    },
  };
}
export const sameNativeImport = (
  left: NativeImportRequest,
  right: NativeImportRequest,
) =>
  left.candidateId === right.candidateId &&
  left.expectedRevision === right.expectedRevision &&
  left.model === right.model &&
  left.operation.storeId === right.operation.storeId &&
  left.operation.namespaceId === right.operation.namespaceId &&
  left.operation.operationId === right.operation.operationId &&
  left.operation.expectedSequence === right.operation.expectedSequence;
export function nativeImportProtocol(value: unknown): NativeImportProtocol {
  if (
    !object(value) ||
    !keys(value, [
      "version",
      "storeId",
      "namespaceId",
      "namespaceState",
      "sequence",
      "operationLimit",
      "pendingLimit",
      "pendingRequests",
    ]) ||
    value.version !== 1 ||
    !providerOperationId(value.storeId) ||
    !providerOperationId(value.namespaceId) ||
    value.namespaceState !== "active" ||
    !sequence(value.sequence) ||
    value.operationLimit !== 4096 ||
    value.pendingLimit !== 4 ||
    !Array.isArray(value.pendingRequests) ||
    value.pendingRequests.length > 4
  )
    throw unavailable();
  const pendingRequests = value.pendingRequests.map(storedNativeImport);
  if (
    pendingRequests.some(
      (request) =>
        !request ||
        request.operation.storeId !== value.storeId ||
        request.operation.namespaceId !== value.namespaceId ||
        request.operation.expectedSequence >= (value.sequence as number),
    ) ||
    new Set(pendingRequests.map((request) => request?.operation.operationId))
      .size !== pendingRequests.length
  )
    throw unavailable();
  return {
    version: 1,
    storeId: value.storeId,
    namespaceId: value.namespaceId,
    namespaceState: "active",
    sequence: value.sequence,
    operationLimit: 4096,
    pendingLimit: 4,
    pendingRequests: pendingRequests as NativeImportRequest[],
  };
}
export function nativeImportPreview(value: unknown): {
  protocol: NativeImportProtocol;
  candidates: ProviderImportCandidate[];
} {
  if (!object(value) || !Array.isArray(value.candidates)) throw unavailable();
  const protocol = nativeImportProtocol(value.protocol);
  const candidates = value.candidates.map(
    (candidate): ProviderImportCandidate => {
      if (
        !object(candidate) ||
        !text(candidate.id, 256) ||
        !["claude", "codex"].includes(String(candidate.agentId)) ||
        !text(candidate.sourcePath, 4096) ||
        !text(candidate.sourceLabel, 400) ||
        typeof candidate.revision !== "string" ||
        !/^[a-f0-9]{64}$/u.test(candidate.revision) ||
        !text(candidate.name, 400) ||
        !text(candidate.vendor, 100) ||
        (candidate.endpoint !== null && !text(candidate.endpoint, 2048)) ||
        !text(candidate.protocol, 32) ||
        (candidate.model !== null && !text(candidate.model, 800)) ||
        typeof candidate.active !== "boolean" ||
        typeof candidate.canImport !== "boolean" ||
        !text(candidate.credentialSource, 1000) ||
        (candidate.reason !== null && !text(candidate.reason, 2000)) ||
        (candidate.existingProviderId !== null &&
          !text(candidate.existingProviderId, 100))
      )
        throw unavailable();
      return candidate as unknown as ProviderImportCandidate;
    },
  );
  return { protocol, candidates };
}
export function nativeImportOperation(
  value: unknown,
  expected: NativeImportRequest,
): NativeImportOperation {
  if (
    !object(value) ||
    !keys(value, [
      "version",
      "sequence",
      "status",
      "request",
      "provider",
      "reused",
      "createsCredential",
    ]) ||
    value.version !== 1 ||
    !sequence(value.sequence) ||
    value.sequence !== expected.operation.expectedSequence + 1 ||
    !["pending", "committed", "cancelled", "expired"].includes(
      String(value.status),
    ) ||
    typeof value.reused !== "boolean" ||
    typeof value.createsCredential !== "boolean"
  )
    throw failure();
  const request = storedNativeImport(value.request);
  const provider =
    value.provider === null ? null : validatedProviderMetadata(value.provider);
  if (
    !request ||
    !sameNativeImport(request, expected) ||
    (value.provider !== null && !provider) ||
    (!["cancelled", "expired"].includes(String(value.status)) && !provider) ||
    (value.status === "expired" &&
      (provider !== null || value.createsCredential || value.reused)) ||
    (value.reused && value.status !== "committed") ||
    (value.reused && value.createsCredential) ||
    (value.createsCredential && !provider?.credentialRef) ||
    (provider &&
      (!provider.configured ||
        (value.status === "committed"
          ? provider.revision < 1
          : provider.revision !== 0))) ||
    (provider &&
      !value.reused &&
      provider.id !== "custom-" + expected.operation.operationId)
  )
    throw failure();
  return {
    version: 1,
    sequence: value.sequence,
    status: value.status as NativeImportOperation["status"],
    request,
    provider,
    reused: value.reused,
    createsCredential: value.createsCredential,
  };
}
export function nativeImported(value: unknown): {
  provider: Provider;
  reused: boolean;
} {
  if (
    !object(value) ||
    !keys(value, ["provider", "reused"]) ||
    typeof value.reused !== "boolean"
  )
    throw failure();
  const provider = validatedProviderMetadata(value.provider);
  if (!provider || provider.revision < 1 || !provider.configured)
    throw failure();
  return { provider, reused: value.reused };
}

const PREFIX = "local-agent.native-import.v1.";
export const NATIVE_IMPORT_CHANGED = "native-import-changed";
export function nativeImportRecovery() {
  const requests: NativeImportRequest[] = [];
  let invalid = false;
  let count = 0;
  for (let n = 0; n < localStorage.length; n++) {
    const key = localStorage.key(n);
    if (!key?.startsWith(PREFIX)) continue;
    count++;
    if (count > 4) {
      invalid = true;
      continue;
    }
    const raw = localStorage.getItem(key) ?? "";
    let request: NativeImportRequest | null = null;
    if (raw.length <= 4096) {
      try {
        request = storedNativeImport(JSON.parse(raw));
      } catch {
        /* Preserve original recovery bytes. */
      }
    }
    if (!request || key !== PREFIX + request.operation.operationId)
      invalid = true;
    else requests.push(request);
  }
  return { requests, invalid, count };
}
export function rememberNativeImport(request: NativeImportRequest) {
  const records = nativeImportRecovery();
  if (records.invalid || records.count >= 4)
    throw new Error(
      "待核查导入记录已达上限或存在不可读取记录，请先核查原记录。",
    );
  const value = JSON.stringify(request),
    key = PREFIX + request.operation.operationId;
  if (
    !storedNativeImport(request) ||
    value.length > 4096 ||
    localStorage.getItem(key) !== null
  )
    throw new Error("原导入请求无法完整保留，本次未提交。");
  localStorage.setItem(key, value);
  if (localStorage.getItem(key) !== value)
    throw new Error("原导入请求未能完整保存，本次未提交。");
  window.dispatchEvent(new Event(NATIVE_IMPORT_CHANGED));
}
export function forgetNativeImport(request: NativeImportRequest) {
  const key = PREFIX + request.operation.operationId,
    raw = localStorage.getItem(key);
  if (raw !== null) {
    let stored: NativeImportRequest | null = null;
    try {
      stored = storedNativeImport(JSON.parse(raw));
    } catch {
      /* Preserve unreadable records. */
    }
    if (!stored || !sameNativeImport(stored, request))
      throw new Error("原导入恢复记录已变化，未自动移除。");
    localStorage.removeItem(key);
    if (localStorage.getItem(key) !== null)
      throw new Error("导入结果已确认，但恢复提示未能移除。请保留编号核查。");
  }
  window.dispatchEvent(new Event(NATIVE_IMPORT_CHANGED));
}
