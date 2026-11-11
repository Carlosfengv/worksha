import type {
  CallKind,
  NativeSession,
  SessionSource,
} from "@/features/sessions/types";
export { formatDateTime as date } from "@/shared/lib/date-time";
import {
  agentFamily,
  createAgentNames,
  createFamilyNames,
} from "@/shared/types/agent";

export function sourceLabel(source?: SessionSource) {
  if (source?.runtimeActive === false)
    return source.enabled ? "已授权 · 插件未运行" : "来源插件已停用";
  if (!source?.enabled) return "未读取会话";
  if (source.error && !(source.status === "partial" && source.skippedFiles > 0))
    return "会话读取异常";
  return source.status === "ready"
    ? "历史会话已接入"
    : source.status === "partial"
      ? "部分会话可读"
      : "正在读取会话";
}

export const agentNames = createAgentNames();
export const familyNames = createFamilyNames();
export function clientAttributionLabel(
  value: string | null | undefined,
  family: string,
) {
  if (value === "desktop_tool_execution")
    return "桌面本机工具执行记录：调用 ID、轮次 ID 与工具名精确关联";
  if (value === "desktop_runtime_execution")
    return "桌面本机执行日志：会话、轮次、时间与桌面直属执行进程一致；仅确认当前活动";
  if (value === "cli_runtime_execution")
    return "CLI 本机执行日志：会话、轮次、时间与终端内 TUI 执行进程一致；仅确认当前活动";
  if (value === "native_exec_turn")
    return "原生 CLI 本轮标记：exec、轮次与根轮次一致；仅确认当前活动";
  if (value === "native_record") return "原生逐条客户端标识";
  if (value === "sdk_client") return "SDK 模式未报告最终客户端，保留未知";
  if (value === "unrecognized_client")
    return "原生客户端标识无法识别，保留未知";
  if (value === "conflicting_evidence") return "客户端证据冲突，保留未知";
  if (value === "ambiguous_evidence")
    return "共享或分支记录无法唯一关联，保留未知";
  return family === "codex"
    ? "缺少逐次客户端证据；会话创建来源不能证明本次执行"
    : "原生记录未提供明确客户端标识";
}
export const kindNames: Record<CallKind, string> = {
  model: "模型",
  tool: "工具",
  mcp: "MCP",
  skill: "Skill",
  diagnostic: "调用异常记录",
};
export function number(value: number | null | undefined) {
  return value == null ? "未知" : value.toLocaleString("zh-CN");
}
export function bytes(value: number | null | undefined) {
  if (value == null) return "未知";
  return value < 1024
    ? `${value} B`
    : value < 1024 ** 2
      ? `${(value / 1024).toFixed(1)} KiB`
      : `${(value / 1024 ** 2).toFixed(1)} MiB`;
}
export function status(value: string) {
  return (
    (
      {
        error: "失败",
        missing_result: "未记录结果",
        returned: "已返回",
        recorded: "已记录",
        cancelled: "已中止",
        timeout: "超时",
        declined: "已拒绝",
        imported: "已导入",
      } as Record<string, string>
    )[value] ?? "未知"
  );
}
export function sessionLink(
  sessionId: string,
  options: {
    agent?: string;
    anchor?: string;
    from?: string;
    tab?: string;
    returnTo?: string;
    view?: "context" | "requests" | "timeline";
  } = {},
) {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(options))
    if (value) search.set(key, value);
  return `/sessions/${encodeURIComponent(sessionId)}${search.size ? `?${search}` : ""}`;
}
export function mcpReturnPath(
  value?: string,
  validAgent = (id: string) =>
    id === "unknown" || agentFamily(id) !== undefined,
) {
  if (!value) return null;
  const [path, search] = value.split("?");
  if (!/^\/mcp(?:\/[a-f0-9]{64})?$/.test(path)) return null;
  const params = new URLSearchParams(search);
  for (const [key, entry] of params) {
    if (key === "agent" && validAgent(entry)) continue;
    if (key === "days" && ["7", "30"].includes(entry)) continue;
    if (
      ["model", "project"].includes(key) &&
      (entry === "unknown" || /^[a-f0-9]{64}$/.test(entry))
    )
      continue;
    return null;
  }
  return `${path}${params.size ? `?${params}` : ""}`;
}
export function requestReturnPath(value?: string) {
  if (!value || value.length > 16384 || value.includes("#")) return null;
  const separator = value.indexOf("?");
  const path = separator < 0 ? value : value.slice(0, separator);
  const match = /^\/requests\/([^/?#]+)$/.exec(path);
  if (!match) return null;
  try {
    const id = decodeURIComponent(match[1]);
    if (
      !id ||
      [...id].some(
        (char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127,
      )
    )
      return null;
    const query = new URLSearchParams(
      separator < 0 ? "" : value.slice(separator + 1),
    );
    return `/requests/${encodeURIComponent(id)}${query.size ? `?${query}` : ""}`;
  } catch {
    return null;
  }
}
export function projectLabel(session: NativeSession) {
  if (session.projectHidden) return "项目已隐藏";
  return session.projectName ?? "项目未知";
}
export function sessionAgentLabel(session: NativeSession, labels = agentNames) {
  const names = (session.agentProducts ?? []).map((id) => labels[id] ?? id);
  if (session.hasUnknownClient || !names.length)
    names.push(labels[session.agentId] ?? session.agentId);
  return names.join(" / ");
}

export function sessionStorage(session: NativeSession) {
  return session.diskScope === "native_database_payload"
    ? `${bytes(session.diskBytes)} 数据库内容估算`
    : `${bytes(session.diskAllocatedBytes)} 会话文件`;
}
