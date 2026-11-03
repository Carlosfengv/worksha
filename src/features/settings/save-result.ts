import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 as uuid } from "@/shared/lib/uuid";
import type {
  SettingsSaved,
  SettingsUpdate,
  ScopedSettingsSaved,
  SettingsWriteProtocol,
} from "./types";

export function freezeSettingsRequest(value: SettingsUpdate): SettingsUpdate {
  if (
    !value ||
    !uuid(value.storeId) ||
    !uuid(value.operationId) ||
    !Number.isSafeInteger(value.expectedRevision) ||
    value.expectedRevision < 0 ||
    value.expectedRevision >= Number.MAX_SAFE_INTEGER ||
    !value.value ||
    typeof value.value.captureContent !== "boolean" ||
    ![7, 30, 90].includes(value.value.retentionDays)
  ) {
    throw new DesktopError({
      code: "invalid_input",
      outcomeUnknown: false,
      message: "设置操作的数据库身份、编号、版本或偏好无效，未发送保存请求",
    });
  }
  return {
    storeId: value.storeId,
    operationId: value.operationId,
    expectedRevision: value.expectedRevision,
    value: {
      captureContent: value.value.captureContent,
      retentionDays: value.value.retentionDays,
    },
  };
}
export function settingsProtocol(
  value: unknown,
): value is SettingsWriteProtocol {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const v = value as Record<string, unknown>;
  return (
    v.version === 1 &&
    uuid(v.storeId) &&
    typeof v.receiptLimit === "number" &&
    Number.isSafeInteger(v.receiptLimit) &&
    v.receiptLimit > 0
  );
}

export function scopedSettingsReceipt(
  value: unknown,
  request: SettingsUpdate,
): ScopedSettingsSaved {
  request = freezeSettingsRequest(request);
  const saved = settingsReceipt(value, request.operationId);
  const scope =
    value && typeof value === "object"
      ? (value as Record<string, unknown>).storeId
      : undefined;
  if (
    scope !== request.storeId ||
    saved.revision !== request.expectedRevision + 1 ||
    saved.value.captureContent !== request.value.captureContent ||
    saved.value.retentionDays !== request.value.retentionDays
  ) {
    throw new DesktopError({
      code: "transport_invalid_response",
      outcomeUnknown: true,
      message: "未能核对原数据库的设置回执，请保留原编号查询结果",
    });
  }
  return { ...saved, storeId: request.storeId };
}

export function settingsReceipt(
  value: unknown,
  operationId: string,
): SettingsSaved {
  const row =
    value && typeof value === "object"
      ? (value as Record<string, unknown>)
      : null;
  const settings =
    row?.value && typeof row.value === "object"
      ? (row.value as Record<string, unknown>)
      : null;
  if (
    !row ||
    row.operationId !== operationId ||
    typeof row.revision !== "number" ||
    !Number.isSafeInteger(row.revision) ||
    row.revision < 1 ||
    !settings ||
    typeof settings.captureContent !== "boolean" ||
    (settings.retentionDays !== 7 &&
      settings.retentionDays !== 30 &&
      settings.retentionDays !== 90)
  ) {
    throw new DesktopError({
      code: "transport_invalid_response",
      outcomeUnknown: true,
      message: "未能核对设置保存回执，请再次核查结果",
    });
  }
  return {
    operationId,
    revision: row.revision,
    value: {
      captureContent: settings.captureContent,
      retentionDays: settings.retentionDays,
    },
  };
}
