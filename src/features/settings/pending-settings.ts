import { freezeSettingsRequest } from "./save-result";
import type { SettingsUpdate } from "./types";

const PREFIX = "local-agent.settings-operation.v1.";
export const SETTINGS_OPERATIONS_CHANGED = "settings-operations-changed";
const LIMIT = 16;
const MAX_BYTES = 4096;
export type SettingsRecovery = {
  requests: SettingsUpdate[];
  invalid: number;
  available: boolean;
  atCapacity: boolean;
};
const encoded = (request: SettingsUpdate) =>
  JSON.stringify({ version: 1, request: freezeSettingsRequest(request) });
function decode(raw: string, key: string): SettingsUpdate {
  if (!fitsRecoveryBudget(raw, MAX_BYTES)) throw new Error("size");
  const value: unknown = JSON.parse(raw);
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("shape");
  const row = value as { version?: unknown; request?: SettingsUpdate };
  if (row.version !== 1 || !row.request) throw new Error("version");
  const request = freezeSettingsRequest(row.request);
  if (PREFIX + request.operationId !== key) throw new Error("identity");
  return request;
}
export function readSettingsRecovery(): SettingsRecovery {
  try {
    const requests: SettingsUpdate[] = [];
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
export function requireSettingsRecovery(request: SettingsUpdate) {
  try {
    const key = PREFIX + request.operationId;
    const stored = decode(localStorage.getItem(key) ?? "", key);
    if (encoded(stored) !== encoded(request)) throw new Error("changed");
  } catch {
    throw new Error(
      "无法核对保留的原请求，未发送新的保存。请保留记录并核查原结果。",
    );
  }
}
export function rememberSettingsOperation(request: SettingsUpdate) {
  request = freezeSettingsRequest(request);
  const pending = readSettingsRecovery();
  if (!pending.available)
    throw new Error("无法保留设置操作编号，未发送保存。请恢复本机存储后再试。");
  if (pending.invalid)
    throw new Error("设置恢复记录不可读取，未发送保存；请先核查原操作。");
  if (pending.requests.some((r) => r.storeId === request.storeId))
    throw new Error("此后台已有待核查的设置保存，请先核对原结果。");
  if (pending.requests.length >= LIMIT)
    throw new Error("待核查设置操作已达上限，未发送保存。请先处理已有记录。");
  const key = PREFIX + request.operationId;
  const raw = encoded(request);
  try {
    // Unique operation keys prevent another window's request being overwritten.
    writeRecoveryRecord(key, raw, MAX_BYTES);
  } catch {
    throw new Error(
      "无法可靠保留设置操作，未发送保存。原有恢复记录未主动删除。",
    );
  } finally {
    notifyRecoveryChanged(SETTINGS_OPERATIONS_CHANGED);
  }
}
export function forgetSettingsOperation(request: SettingsUpdate) {
  const key = PREFIX + request.operationId;
  try {
    removeRecoveryRecord(
      key,
      (raw) => encoded(decode(raw, key)) === encoded(request),
    );
  } catch {
    throw new Error(
      "原结果已核对，但恢复记录未能移除；请恢复本机存储后再次核查。",
    );
  } finally {
    notifyRecoveryChanged(SETTINGS_OPERATIONS_CHANGED);
  }
}
import {
  recoveryKeys,
  fitsRecoveryBudget,
  writeRecoveryRecord,
  removeRecoveryRecord,
  notifyRecoveryChanged,
} from "@/shared/lib/recovery-storage";
