import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 } from "@/shared/lib/uuid";

export interface CredentialReceipt {
  operationId: string;
  credentialRef: string;
  credentialLabel: string;
}
export interface CredentialOperation {
  operationId: string;
  status: "pending" | "ready" | "cancelled" | "cancelled_pending" | "removed";
  receipt: CredentialReceipt | null;
}
export interface ScopedCredentialReceipt extends CredentialReceipt {
  version: 2;
  storeId: string;
}
export interface ScopedCredentialOperation extends CredentialOperation {
  version: 2;
  storeId: string;
  receipt: ScopedCredentialReceipt | null;
}
export interface CredentialReceiptV3 extends CredentialReceipt {
  version: 3;
  storeId: string;
  namespaceId: string;
}
export interface CredentialOperationV3 extends CredentialOperation {
  version: 3;
  storeId: string;
  namespaceId: string;
  receipt: CredentialReceiptV3 | null;
}
export interface CredentialNamespace {
  version: 3;
  storeId: string;
  namespaceId: string;
  namespaceState: "active" | "unverified";
  legacyState: "fresh" | "imported" | "unverified";
  operationLimit: 4096;
}
export interface CredentialIdentityV3 {
  storeId: string;
  namespaceId: string;
  operationId: string;
}
export interface CredentialInitialization {
  storeId: string;
  namespaceId: string;
  choice: "restore" | "new";
}
export type RecoverableCredentialReceipt =
  ScopedCredentialReceipt | CredentialReceiptV3;

export function credentialNamespace(
  value: unknown,
  storeId: string,
): CredentialNamespace {
  const row = object(value);
  if (
    !isCredentialOperationId(storeId) ||
    !fields(row, [
      "version",
      "storeId",
      "namespaceId",
      "namespaceState",
      "legacyState",
      "operationLimit",
    ]) ||
    row!.version !== 3 ||
    row!.storeId !== storeId ||
    !isCredentialOperationId(row!.namespaceId) ||
    !["active", "unverified"].includes(String(row!.namespaceState)) ||
    !["fresh", "imported", "unverified"].includes(String(row!.legacyState)) ||
    row!.operationLimit !== 4096
  ) {
    throw new DesktopError({
      code: "credential_protocol_unavailable",
      outcomeUnknown: false,
      message: "当前后台的凭据恢复协议无法核查，未发送密钥。",
    });
  }
  return row as unknown as CredentialNamespace;
}
export function credentialResultV3(
  value: unknown,
  request: CredentialIdentityV3,
): CredentialOperationV3 {
  const row = object(value),
    operation = object(row?.operation),
    receipt = object(operation?.receipt);
  if (
    !isCredentialOperationId(request.storeId) ||
    !isCredentialOperationId(request.namespaceId) ||
    !isCredentialOperationId(request.operationId) ||
    !fields(row, ["version", "storeId", "namespaceId", "operation"]) ||
    row!.version !== 3 ||
    row!.storeId !== request.storeId ||
    row!.namespaceId !== request.namespaceId ||
    !fields(operation, ["operationId", "status", "receipt"]) ||
    (receipt &&
      !fields(receipt, ["operationId", "credentialRef", "credentialLabel"]))
  )
    invalid();
  const parsed = credentialOperation(operation, request.operationId);
  return {
    ...parsed,
    version: 3,
    storeId: request.storeId,
    namespaceId: request.namespaceId,
    receipt: parsed.receipt
      ? {
          ...parsed.receipt,
          version: 3,
          storeId: request.storeId,
          namespaceId: request.namespaceId,
        }
      : null,
  };
}
export const isCredentialOperationId = isUuidV4;
const object = (value: unknown) =>
  value && typeof value === "object"
    ? (value as Record<string, unknown>)
    : null;
function invalid(): never {
  throw new DesktopError({
    code: "transport_invalid_response",
    outcomeUnknown: true,
    message: "未能核对凭据操作回执，请按原编号核查结果",
  });
}
function fields(row: Record<string, unknown> | null, names: string[]) {
  return (
    !!row &&
    Object.keys(row).length === names.length &&
    names.every((key) => Object.hasOwn(row, key))
  );
}
export function scopedCredentialProtocol(value: unknown, storeId: string) {
  const row = object(value);
  if (
    !isCredentialOperationId(storeId) ||
    !fields(row, ["version", "storeId", "operationLimit"]) ||
    row!.version !== 2 ||
    row!.storeId !== storeId ||
    row!.operationLimit !== 4096
  )
    throw new DesktopError({
      code: "credential_protocol_unavailable",
      outcomeUnknown: false,
      message: "凭据协议或原数据库不匹配，未发送密钥。",
    });
  return { version: 2 as const, storeId, operationLimit: 4096 };
}
export function scopedCredentialOperation(
  value: unknown,
  storeId: string,
  operationId: string,
): ScopedCredentialOperation {
  const row = object(value),
    operation = object(row?.operation),
    receipt = object(operation?.receipt);
  if (
    !isCredentialOperationId(storeId) ||
    !isCredentialOperationId(operationId) ||
    !fields(row, ["version", "storeId", "operation"]) ||
    row!.version !== 2 ||
    row!.storeId !== storeId ||
    !fields(operation, ["operationId", "status", "receipt"]) ||
    (receipt &&
      !fields(receipt, ["operationId", "credentialRef", "credentialLabel"]))
  )
    invalid();
  const parsed = credentialOperation(operation, operationId);
  return {
    ...parsed,
    version: 2,
    storeId,
    receipt: parsed.receipt ? { ...parsed.receipt, version: 2, storeId } : null,
  };
}
export function credentialReceipt(
  value: unknown,
  operationId: string,
): CredentialReceipt {
  const row = object(value);
  if (
    !isCredentialOperationId(operationId) ||
    !row ||
    row.operationId !== operationId ||
    !isCredentialOperationId(row.credentialRef) ||
    typeof row.credentialLabel !== "string" ||
    !row.credentialLabel.trim() ||
    new TextEncoder().encode(row.credentialLabel).length > 200 ||
    /\p{Cc}/u.test(row.credentialLabel)
  )
    invalid();
  return {
    operationId,
    credentialRef: row.credentialRef as string,
    credentialLabel: row.credentialLabel as string,
  };
}
export function credentialOperation(
  value: unknown,
  operationId: string,
): CredentialOperation {
  const row = object(value);
  if (
    !row ||
    row.operationId !== operationId ||
    !["pending", "ready", "cancelled", "cancelled_pending", "removed"].includes(
      String(row.status),
    )
  )
    invalid();
  if (row.status === "ready")
    return {
      operationId,
      status: "ready",
      receipt: credentialReceipt(row.receipt, operationId),
    };
  if (row.receipt !== null) invalid();
  return {
    operationId,
    status: row.status as
      "pending" | "cancelled" | "cancelled_pending" | "removed",
    receipt: null,
  };
}
