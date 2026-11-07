import type { CompositionChange } from "../../../contracts/plugins/builtin/types";
import {
  fitsRecoveryBudget,
  notifyRecoveryChanged,
  recoveryKeys,
  removeRecoveryRecord,
  writeRecoveryRecord,
} from "@/shared/lib/recovery-storage";
import { builtinChange } from "./builtin-contract";

const PREFIX = "local-agent.builtin-composition.v1.";
export const BUILTIN_OPERATIONS_CHANGED =
  "builtin-composition-operations-changed";
const MAX_BYTES = 65536;
export function builtinPending() {
  try {
    const requests: CompositionChange[] = [];
    let invalid = 0;
    for (const key of recoveryKeys(PREFIX)) {
      try {
        const raw = localStorage.getItem(key) ?? "";
        if (!fitsRecoveryBudget(raw, MAX_BYTES)) throw Error("budget");
        const request = builtinChange(JSON.parse(raw));
        if (key !== PREFIX + request.operationId || requests.length >= 16)
          throw Error("identity");
        requests.push(request);
      } catch {
        invalid++;
      }
    }
    return { requests, invalid, available: true };
  } catch {
    return { requests: [], invalid: 0, available: false };
  }
}
export function rememberBuiltin(request: CompositionChange) {
  request = builtinChange(request);
  const pending = builtinPending();
  if (
    !pending.available ||
    pending.invalid ||
    pending.requests.length >= 16 ||
    pending.requests.some((previous) => previous.storeId === request.storeId)
  )
    throw Error("无法保留新的插件操作，请先核查已有记录；本次未提交。");
  writeRecoveryRecord(
    PREFIX + request.operationId,
    JSON.stringify(request),
    MAX_BYTES,
  );
  notifyRecoveryChanged(BUILTIN_OPERATIONS_CHANGED);
}
export function forgetBuiltin(request: CompositionChange) {
  removeRecoveryRecord(
    PREFIX + request.operationId,
    (raw) => raw === JSON.stringify(request),
  );
  notifyRecoveryChanged(BUILTIN_OPERATIONS_CHANGED);
}
export function requireBuiltinPending(request: CompositionChange) {
  if (
    localStorage.getItem(PREFIX + request.operationId) !==
    JSON.stringify(request)
  )
    throw Error("保留的原操作已变化，未发送新的保存，请先核查原记录。");
}
