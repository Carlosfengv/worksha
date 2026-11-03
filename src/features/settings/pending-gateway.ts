import { freezeGatewayPending } from "./gateway-operation";
import type { PendingGatewayOperation } from "./types";
const PREFIX = "local-agent.gateway-operation.v1.";
export const GATEWAY_OPERATIONS_CHANGED = "gateway-operations-changed";
const LIMIT = 16,
  MAX_BYTES = 65536;
export type GatewayRecovery = {
  requests: PendingGatewayOperation[];
  invalid: number;
  available: boolean;
  atCapacity: boolean;
};
const encode = (r: PendingGatewayOperation) =>
  JSON.stringify({ version: 1, ...freezeGatewayPending(r) });
function decode(raw: string, key: string) {
  if (!fitsRecoveryBudget(raw, MAX_BYTES)) throw Error("size");
  const row: unknown = JSON.parse(raw);
  if (!row || typeof row !== "object" || Array.isArray(row))
    throw Error("shape");
  const { version, ...pending } = row as Record<string, unknown>;
  if (version !== 1) throw Error("version");
  const result = freezeGatewayPending(pending);
  if (key !== PREFIX + result.request.operationId) throw Error("identity");
  return result;
}
export function readGatewayRecovery(): GatewayRecovery {
  try {
    const requests: PendingGatewayOperation[] = [];
    let invalid = 0;
    for (const key of recoveryKeys(PREFIX)) {
      try {
        const request = decode(localStorage.getItem(key) ?? "", key);
        if (requests.length >= LIMIT) invalid++;
        else requests.push(request);
      } catch {
        invalid++;
      }
    }
    return {
      requests,
      invalid,
      available: true,
      atCapacity: requests.length >= LIMIT,
    };
  } catch {
    return { requests: [], invalid: 0, available: false, atCapacity: false };
  }
}
export function requireGatewayRecovery(pending: PendingGatewayOperation) {
  try {
    const key = PREFIX + pending.request.operationId;
    const stored = decode(localStorage.getItem(key) ?? "", key);
    if (encode(stored) !== encode(pending)) throw Error("changed");
  } catch {
    throw Error("无法核对保留的客户端原请求，未发送新操作；请保留原编号核查。");
  }
}
export function rememberGatewayOperation(pending: PendingGatewayOperation) {
  pending = freezeGatewayPending(pending);
  const existing = readGatewayRecovery();
  if (!existing.available || existing.invalid || existing.atCapacity)
    throw Error(
      "客户端恢复存储不可用、存在坏记录或已满，未发送新操作；原记录已保留。",
    );
  if (
    existing.requests.some((r) => r.request.storeId === pending.request.storeId)
  )
    throw Error("原后台已有待核查的客户端操作，请先处理原结果。");
  const key = PREFIX + pending.request.operationId,
    raw = encode(pending);
  if (!fitsRecoveryBudget(raw, MAX_BYTES))
    throw Error("客户端恢复请求超过容量，未发送。");
  try {
    writeRecoveryRecord(key, raw, MAX_BYTES);
  } catch {
    throw Error("无法可靠保留客户端原请求，未发送操作；没有主动删除已有编号。");
  } finally {
    notifyRecoveryChanged(GATEWAY_OPERATIONS_CHANGED);
  }
}
export function forgetGatewayOperation(pending: PendingGatewayOperation) {
  const key = PREFIX + pending.request.operationId;
  try {
    removeRecoveryRecord(
      key,
      (raw) => encode(decode(raw, key)) === encode(pending),
    );
  } catch {
    throw Error(
      "原客户端结果已确认，但本机恢复记录未能移除；请恢复存储后再次核查。",
    );
  } finally {
    notifyRecoveryChanged(GATEWAY_OPERATIONS_CHANGED);
  }
}
import {
  recoveryKeys,
  fitsRecoveryBudget,
  writeRecoveryRecord,
  removeRecoveryRecord,
  notifyRecoveryChanged,
} from "@/shared/lib/recovery-storage";
