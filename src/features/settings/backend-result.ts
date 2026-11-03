import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 as uuid } from "@/shared/lib/uuid";

export const backendStates = {
  starting: "正在启动",
  connected: "最近已确认连接",
  disconnected: "连接待核查",
  startup_failed: "启动未完成",
  reconnecting: "正在核对连接",
  stopping: "正在退出",
  stopped: "已断开桌面连接",
} as const;
export const backendPhases = {
  startup: "启动",
  request: "数据请求",
  recovery: "重新连接",
  process: "后台进程",
  shutdown: "退出",
} as const;
export const backendCategories = {
  starting: "开始启动",
  verified: "身份核对通过",
  startup_failed: "启动失败",
  transport_timeout: "请求超时，结果待核查",
  transport_connection: "连接中断",
  invalid_response: "回复无法确认",
  request_rejected: "请求未成功",
  child_error: "进程监控异常",
  child_exited: "后台进程退出",
  event_stream_closed: "进程监控中断",
  reconnect_started: "开始核对连接",
  reconnect_failed: "连接核对未通过",
  reconnected: "已重新确认连接",
  recovery_cancelled: "连接核对中断",
  stopping: "开始退出",
  stopped: "桌面连接已停止",
} as const;
export interface BackendEvent {
  id: string;
  operationId: string;
  at: number;
  phase: keyof typeof backendPhases;
  category: keyof typeof backendCategories;
  exitCode: number | null;
  signal: number | null;
}
export interface BackendDiagnostics {
  state: keyof typeof backendStates;
  verifiedAt: number | null;
  canReconnect: boolean;
  events: BackendEvent[];
}
const object = (v: unknown): Record<string, unknown> | null =>
  v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
const time = (v: unknown): v is number =>
  typeof v === "number" &&
  Number.isSafeInteger(v) &&
  v >= 0 &&
  v <= 8640000000000000;
const exit = (v: unknown): v is number | null =>
  v === null ||
  (typeof v === "number" &&
    Number.isInteger(v) &&
    v >= -2147483648 &&
    v <= 2147483647);
function invalid(): never {
  throw new DesktopError({
    code: "transport_invalid_response",
    outcomeUnknown: false,
    message: "本地连接诊断回复无法确认，请重新检查状态。",
  });
}
function key<T extends object>(v: unknown, values: T): v is keyof T {
  return typeof v === "string" && Object.hasOwn(values, v);
}
export function backendDiagnostics(value: unknown): BackendDiagnostics {
  const v = object(value);
  if (
    !v ||
    !key(v.state, backendStates) ||
    !(v.verifiedAt === null || time(v.verifiedAt)) ||
    typeof v.canReconnect !== "boolean" ||
    !Array.isArray(v.events) ||
    v.events.length > 32 ||
    (v.state === "connected" && v.verifiedAt === null) ||
    (["starting", "reconnecting", "stopping", "stopped"].includes(v.state) &&
      v.canReconnect)
  )
    invalid();
  const ids = new Set<string>();
  const events = v.events.map((value: unknown): BackendEvent => {
    const row = object(value);
    if (
      !row ||
      !uuid(row.id) ||
      !uuid(row.operationId) ||
      ids.has(row.id) ||
      !time(row.at) ||
      !key(row.phase, backendPhases) ||
      !key(row.category, backendCategories) ||
      !exit(row.exitCode) ||
      !exit(row.signal)
    )
      invalid();
    ids.add(row.id);
    return {
      id: row.id,
      operationId: row.operationId,
      at: row.at,
      phase: row.phase,
      category: row.category,
      exitCode: row.exitCode,
      signal: row.signal,
    };
  });
  return {
    state: v.state,
    verifiedAt: v.verifiedAt,
    canReconnect: v.canReconnect,
    events,
  };
}
