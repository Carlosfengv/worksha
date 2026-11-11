import type { UnifiedActivityPage } from "../../../contracts/domain/facts/types";
import type { AgentDiscovery } from "@/shared/types/agent";
import { durationLabel, elapsedLabel } from "./activity-performance";
import { latestSessionActivities } from "./session-activities";

const phaseLabels: Record<string, string> = {
  running: "执行中",
  thinking: "思考中",
  generating: "正在输出",
  tool: "执行工具",
  idle: "等待输入",
  unknown: "状态未知",
};

export function publishedActivityRows(
  page: UnifiedActivityPage | null,
  agents: AgentDiscovery[],
  now: number,
) {
  const snapshotFresh = page != null && Date.parse(page.asOf) >= now - 15000;
  const readable = new Set(
    page?.sourceStates
      .filter(
        ({ kind, state }) =>
          kind === "source-event" &&
          state.authorization === "authorized" &&
          state.coverage !== "unavailable",
      )
      .map(({ state }) => state.sourceId),
  );
  const rows = (page?.items ?? []).flatMap((item) => {
    if (item.kind !== "source-event" || !readable.has(item.sourceId)) return [];
    const agent = agents.find(
      (agent) =>
        agent.sourceId === item.sourceId && item.productIds.includes(agent.id),
    );
    if (
      !agent &&
      (!item.unknownClient ||
        !agents.some((agent) => agent.sourceId === item.sourceId))
    )
      return [];
    const expired =
      item.state === "expired" || (item.state === "current" && !snapshotFresh);
    const active = item.state === "current" && !expired;
    const phase = phaseLabels[item.phase] ?? "状态未知";
    return [
      {
        reported: true as const,
        id: `source:${item.sourceId}:${item.id}`,
        sessionId: item.sessionId ?? null,
        agentName: agent?.name ?? "客户端来源未知",
        title: item.title,
        model: item.model ?? null,
        timing: item.reportedMeasurements ?? null,
        taskElapsed: active
          ? elapsedLabel(item.messageStartedAt, null, now)
          : item.historicalDurationMs != null
            ? durationLabel(Number(item.historicalDurationMs))
            : null,
        source: "Agent 上报",
        href: `/sessions${item.sessionId ? `/${encodeURIComponent(item.sessionId)}` : ""}?scope=unified&source=${encodeURIComponent(item.sourceId)}`,
        phaseLabel: expired
          ? "状态待更新"
          : item.state === "historical" && item.phase !== "idle"
            ? `历史记录 · ${phase}`
            : phase,
        active,
        time: item.occurredAt ?? item.observedAt,
        messageAt: item.messageStartedAt ?? null,
        requestAt: null,
      },
    ];
  });
  return latestSessionActivities(rows);
}

export type PublishedActivityRow = ReturnType<
  typeof publishedActivityRows
>[number];
