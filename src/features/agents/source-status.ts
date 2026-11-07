import type { AgentDiscovery } from "@/shared/types/agent";

export function sourceStatus(agent: AgentDiscovery) {
  if (agent.sourceState?.enabled === false) return "已停用";
  if (agent.sourceState?.runtimeActive === false) return "待启用";
  if (!agent.connected) return "尚未授权读取";
  if (agent.sourceState?.freshness === "failed") return "读取失败";
  if (agent.sourceState?.freshness === "current") return "已授权 · 数据已读取";
  return "已授权 · 数据待更新";
}
