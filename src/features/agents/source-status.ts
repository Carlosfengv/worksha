import type { AgentDiscovery } from "@/shared/types/agent";
import type { Status } from "../../../contracts/plugins/sources/types";

export function sourceStatus(
  agent: AgentDiscovery,
  status?: Pick<Status, "sourceAuthorized" | "unifiedAuthorized" | "freshness">,
) {
  if (!agent.found) return "待接入";
  if (agent.sourceState?.enabled === false) return "已停用";
  if (agent.sourceState?.runtimeActive === false) return "待启用";
  if (status && !status.sourceAuthorized) return "尚未授权读取";
  const freshness = status?.freshness ?? agent.sourceState?.freshness;
  const reading =
    freshness === "failed"
      ? "读取失败"
      : freshness === "stale"
        ? "数据已过期"
        : freshness === "current"
          ? "数据已读取"
          : status?.sourceAuthorized
            ? "已授权读取"
            : null;
  if (!(status?.unifiedAuthorized ?? agent.connected)) {
    if (!reading) return "读取授权待确认";
    return [reading, "待授权统一展示"].filter(Boolean).join(" · ");
  }
  return ["已加入统一展示", reading ?? "数据待更新"].join(" · ");
}
