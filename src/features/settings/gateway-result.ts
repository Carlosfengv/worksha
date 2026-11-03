import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 as uuid } from "@/shared/lib/uuid";
import type { GatewayClientFileResult } from "./types";

export function gatewayFileResult(value: unknown): GatewayClientFileResult {
  const row =
    value && typeof value === "object" && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null;
  if (
    row?.status === "cancelled" &&
    row.id === undefined &&
    row.key === undefined &&
    row.apiKey === undefined
  )
    return { status: "cancelled" };
  if (
    row?.status === "saved" &&
    row.key === undefined &&
    row.apiKey === undefined &&
    uuid(row.id) &&
    typeof row.fileName === "string" &&
    row.fileName.length > 0 &&
    row.fileName.length <= 1024 &&
    !row.fileName.includes("\0")
  ) {
    if (row.sourceInstanceId === undefined && row.sourceCurrent === undefined)
      return {
        status: "saved",
        id: row.id,
        fileName: row.fileName,
      };
    if (uuid(row.sourceInstanceId) && typeof row.sourceCurrent === "boolean")
      return {
        status: "saved",
        id: row.id,
        fileName: row.fileName,
        sourceInstanceId: row.sourceInstanceId,
        sourceCurrent: row.sourceCurrent,
      };
  }
  throw new DesktopError({
    code: "transport_invalid_response",
    outcomeUnknown: true,
    message:
      "连接文件结果不可核对，请核查目标文件与原后台客户端；没有自动再次签发密钥",
  });
}
