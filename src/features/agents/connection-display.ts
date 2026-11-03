import type {
  AgentConnection,
  AgentConnections,
  ConfigAgentId,
  AgentProductId,
} from "@/shared/types/agent";

const modes = {
  gateway: { label: "本地网关 · 默认", status: "success" },
  official_login: { label: "官方登录 · 默认", status: "info" },
  official_api: { label: "官方 API · 默认", status: "neutral" },
  direct: { label: "Provider 直连 · 默认", status: "neutral" },
  unknown: { label: "连接方式待确认", status: "warning" },
} as const;

export function connectionView({
  agentId,
  connections,
  live,
  productId,
  now = Date.now(),
}: {
  agentId: ConfigAgentId;
  connections?: AgentConnections;
  live?: { count: number; stale: boolean };
  productId?: AgentProductId;
  now?: number;
}) {
  const age = connections ? now - Date.parse(connections.checkedAt) : NaN;
  const fresh =
    Number.isFinite(age) && age >= -1000 && age < 15000 && !connections?.error;
  const config = connections?.items.find((item) => item.agentId === agentId);
  const mode: AgentConnection["mode"] =
    fresh && config ? config.mode : "unknown";
  const active = live
    ? live.stale
      ? 0
      : live.count
    : fresh
      ? productId
        ? (config?.activeByProduct?.[productId] ?? 0)
        : (config?.activeRequests ?? 0)
      : 0;
  const confirmed = active > 0;
  // The service only declares gateway mode after matching the configured
  // endpoint to a route owned by this app. Idle traffic does not undo that.
  const gatewayConfigured = mode === "gateway";
  const requestStateKnown = live ? !live.stale : fresh;
  return {
    ...modes[mode],
    mode,
    config,
    fresh,
    active,
    confirmed,
    gatewayConfigured,
    title: confirmed
      ? `网关正在代理 · ${active} 个请求`
      : gatewayConfigured
        ? "网关路由已确认"
        : "采集受限",
    description: confirmed
      ? "当前请求已通过网关；其他实例的连接单独确认。"
      : gatewayConfigured
        ? requestStateKnown
          ? "默认配置已匹配本应用的网关路由，当前没有进行中的网关请求。项目、会话或启动参数仍可能覆盖默认连接，实际流量以请求记录为准。"
          : "默认配置已匹配本应用的网关路由，实时请求状态待更新；暂不确认当前是否正在代理。"
        : "未经网关的请求，TTFT、实时 Token/s 和异常可能缺失。",
    reason: !fresh
      ? (connections?.error ??
        (connections
          ? "连接配置快照已过期，等待重新读取"
          : "正在读取本机连接配置"))
      : config?.reason,
    loginNote: mode === "official_login" ? "订阅与 Plan 尚未核验。" : null,
  };
}
