import {
  actionIdentity,
  storedRequest,
  type ServiceRequest,
} from "./operation-receipt";
const PREFIX = "local-agent.model-service-operation.v1.";
export const OPERATIONS_CHANGED = "model-service-operations-changed";
const LIMIT = 16;
const MAX_BYTES = 65536;
export function pendingOperations() {
  const requests: ServiceRequest[] = [];
  let invalid = 0;
  for (const key of recoveryKeys(PREFIX)) {
    try {
      const raw = localStorage.getItem(key) ?? "";
      if (!fitsRecoveryBudget(raw, MAX_BYTES)) {
        invalid++;
        continue;
      }
      const request = storedRequest(JSON.parse(raw));
      if (
        !request ||
        PREFIX + request.operationId !== key ||
        requests.length >= LIMIT
      ) {
        invalid++;
        continue;
      }
      requests.push(request);
    } catch {
      invalid++;
    }
  }
  return { requests, invalid };
}
export function canStartOperation(serviceId: string, storeId: string) {
  const pending = pendingOperations();
  if (pending.invalid)
    throw new Error("恢复记录不可读取，请先核查；没有自动丢弃原操作编号。");
  if (pending.requests.length >= LIMIT)
    throw new Error("待核查操作已达上限，请先处理已有操作。");
  if (
    pending.requests.some(
      (r) =>
        r.storeId === storeId &&
        actionIdentity(r.action).serviceId === serviceId,
    )
  )
    throw new Error("此服务已有待核查操作，请关闭弹层并先查询原结果。");
}
export function rememberOperation(request: ServiceRequest) {
  const encoded = JSON.stringify(request);
  if (
    !fitsRecoveryBudget(encoded, MAX_BYTES) ||
    !storedRequest(JSON.parse(encoded))
  )
    throw new Error("发布参数无法安全保留，未提交模型服务。");
  // Verification may have yielded since the editor's initial admission check.
  canStartOperation(actionIdentity(request.action).serviceId, request.storeId);
  const key = PREFIX + request.operationId;
  writeRecoveryRecord(key, encoded, MAX_BYTES);
  notifyRecoveryChanged(OPERATIONS_CHANGED);
}
export function forgetOperation(request: ServiceRequest) {
  const key = PREFIX + request.operationId;
  removeRecoveryRecord(key, (raw) => raw === JSON.stringify(request));
  notifyRecoveryChanged(OPERATIONS_CHANGED);
}
import {
  recoveryKeys,
  fitsRecoveryBudget,
  writeRecoveryRecord,
  removeRecoveryRecord,
  notifyRecoveryChanged,
} from "@/shared/lib/recovery-storage";
