import type {
  AgentConnection,
  AgentConnections,
  ConfigAgentId,
  AgentProductId,
} from "@/shared/types/agent";
import type { SessionSource } from "@/features/sessions/types";

const modes = {
  gateway: { label: "本地网关 · 默认", status: "success" },
  official_login: { label: "官方登录 · 默认", status: "info" },
  official_api: { label: "官方 API · 默认", status: "neutral" },
  direct: { label: "Provider 直连 · 默认", status: "neutral" },
  unknown: { label: "连接方式待确认", status: "warning" },
} as const;
const nativeSourceLabels: Record<string, string> = {
  pending: "等待索引",
  scanning: "正在索引",
  ready: "原生观测已就绪",
  available: "原生观测已就绪",
  partial: "原生会话部分可读取",
  empty: "暂无原生记录",
  not_found: "未发现原生记录",
  permission_denied: "原生会话无权限",
  error: "原生读取失败",
  unsupported: "原生会话版本不支持",
};

export function connectionView({
  agentId,
  connections,
  live,
  source,
  nativeRealtime,
  productId,
  now = Date.now(),
}: {
  agentId: ConfigAgentId;
  connections?: AgentConnections;
  live?: { count: number; stale: boolean };
  source?: Pick<SessionSource, "enabled" | "status" | "runtimeActive">;
  nativeRealtime?: boolean;
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
  const absent =
    fresh && config?.configState === "absent" && !config.configOverride;
  const nativeDefault = absent || mode === "official_login";
  const realtime =
    nativeRealtime ??
    Boolean(
      fresh &&
      productId &&
      source?.enabled &&
      source.runtimeActive !== false &&
      connections?.nativeRealtimeProducts?.includes(productId),
    );
  const nativeStatus = realtime
    ? "原生实时观测已连接"
    : source?.runtimeActive === false
      ? "原生采集已停用"
      : !source
        ? "原生观测待确认"
        : !source.enabled
          ? "会话待授权"
          : (nativeSourceLabels[source.status] ?? "原生观测待确认");
  const showNative =
    nativeDefault || realtime || (!gatewayConfigured && !!source?.enabled);
  const nativeFailed =
    !realtime &&
    source?.enabled &&
    ["partial", "permission_denied", "error", "unsupported"].includes(
      source.status,
    );
  return {
    ...(absent
      ? { label: "默认原生入口 · 实际连接待确认", status: "neutral" as const }
      : modes[mode]),
    mode,
    config,
    fresh,
    active,
    confirmed,
    gatewayConfigured,
    nativeDefault,
    showNative,
    observationStatus:
      confirmed || gatewayConfigured || realtime
        ? ("success" as const)
        : showNative && !nativeFailed
          ? ("neutral" as const)
          : ("warning" as const),
    title: confirmed
      ? `网关正在代理 · ${active} 个请求`
      : realtime
        ? nativeStatus
        : gatewayConfigured
          ? "网关路由已确认"
          : showNative
            ? nativeStatus
            : "采集受限",
    description: confirmed
      ? "当前请求已通过网关；其他实例的连接单独确认。"
      : realtime
        ? "已收到此客户端的新鲜原生实时样本。"
        : gatewayConfigured
          ? requestStateKnown
            ? "默认配置已匹配本应用的网关路由，当前没有进行中的网关请求。项目、会话或启动参数仍可能覆盖默认连接，实际流量以请求记录为准。"
            : "默认配置已匹配本应用的网关路由，实时请求状态待更新；暂不确认当前是否正在代理。"
          : showNative
            ? "原生会话和实时采集各自保留来源与状态。"
            : "未经网关的请求，TTFT、实时 Token/s 和异常可能缺失。",
    reason: !fresh
      ? (connections?.error ??
        (connections
          ? "连接配置快照已过期，等待重新读取"
          : "正在读取本机连接配置"))
      : config?.reason,
    loginNote: mode === "official_login" ? "账户核验状态见额度。" : null,
  };
}
