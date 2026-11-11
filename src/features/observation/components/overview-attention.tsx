import { Badge } from "@/components/ui/badge";
import {
  InlineNotice,
  InlineNoticeContent,
} from "@/components/ui/inline-notice";
import { date } from "@/features/sessions/display";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { recordAgentKey, type AgentDiscovery } from "@/shared/types/agent";
import type { UnifiedActivityPage } from "../../../../contracts/domain/facts/types";
import { activityWindowSeconds } from "../session-activities";
import type { ModelObservation, Overview } from "../types";

interface AttentionItem {
  id: string;
  title: string;
  detail: string;
  status: "warning" | "danger";
  href?: string;
}

function recentGatewayFailures(overview: Overview, now: number) {
  const requests = new Map<
    string,
    { latest: ModelObservation; active: boolean }
  >();
  for (const { request } of overview.gatewayActivities) {
    const route = request.routing;
    if (route && "trace" in route && route.trace) continue;
    const client = request.agentProductId ?? request.agentId;
    const routeScope =
      request.logicalId?.startsWith("model-service-") && route?.service
        ? [route.bindingId]
        : [
            request.bindingRevision,
            request.providerId ?? [request.provider, request.endpoint],
          ];
    // The same idempotency key can be used by another client, session or route.
    const id = JSON.stringify(
      request.logicalId
        ? [client, request.sessionId, routeScope, request.logicalId]
        : [request.id],
    );
    const previous = requests.get(id);
    const previousTime = Date.parse(previous?.latest.startedAt ?? "") || 0;
    const startedAt = Date.parse(request.startedAt) || 0;
    const latest =
      !previous ||
      startedAt > previousTime ||
      (startedAt === previousTime && request.id > previous.latest.id)
        ? request
        : previous.latest;
    requests.set(id, {
      latest,
      active: Boolean(previous?.active) || request.status === "in_progress",
    });
  }
  const cutoff = now - activityWindowSeconds * 1000;
  return [...requests.values()]
    .flatMap(({ latest, active }) => {
      const endedAt = Date.parse(latest.endedAt ?? "");
      return !active &&
        ["error", "failed", "incomplete"].includes(latest.status) &&
        endedAt >= cutoff
        ? [{ request: latest, endedAt }]
        : [];
    })
    .sort((a, b) => b.endedAt - a.endedAt);
}

export function OverviewAttention({
  overview,
  now,
  error,
  expired,
  nativeObservationPaused,
  publishedPage,
  sourceAgents,
  sourceError,
}: {
  overview: Overview | null;
  now: number;
  error: string | null;
  expired: boolean;
  nativeObservationPaused: boolean;
  publishedPage: UnifiedActivityPage | null;
  sourceAgents: AgentDiscovery[];
  sourceError: string | null;
}) {
  const { agentNames, agentProducts } = useAgentDirectory();
  const items: AttentionItem[] = [];
  const collector = overview?.collector;
  const collectorError = collector?.persistenceError ?? collector?.error;
  if (error || expired) {
    items.push({
      id: "overview",
      title: error ? "观测服务读取失败" : "观测快照已过期",
      detail: error ?? "当前执行状态待确认，活动保留上次记录。",
      status: "warning",
    });
  } else if (nativeObservationPaused || collectorError) {
    items.push({
      id: "collector",
      title: collector?.persistenceError
        ? "网关记录保存异常"
        : nativeObservationPaused
          ? "原生活动采集已暂停"
          : "原生活动采集异常",
      detail: collectorError ?? "可在插件中心查看活动采集状态。",
      status: "warning",
      href: "/plugins?scene=activity",
    });
  }
  if (sourceError) {
    items.push({
      id: "published",
      title: "Agent 上报活动暂不可读取",
      detail: sourceError,
      status: "warning",
      href: "/agents",
    });
  }
  const sourceStates = new Map(
    publishedPage?.sourceStates
      .filter(({ kind }) => kind === "source-event")
      .map(({ state }) => [state.sourceId, state]),
  );
  const checkedSources = new Set<string>();
  for (const agent of sourceAgents) {
    const sourceId = agent.sourceId;
    if (!sourceId || checkedSources.has(sourceId)) continue;
    checkedSources.add(sourceId);
    const state = sourceStates.get(sourceId) ?? agent.sourceState;
    if (!state) continue;
    const needsAuthorization = state.authorization !== "authorized";
    const title = needsAuthorization
      ? state.authorization === "revoked"
        ? "读取授权已撤销"
        : "需要读取授权"
      : state.freshness === "failed"
        ? "来源读取失败"
        : state.freshness === "stale"
          ? "来源数据已过期"
          : null;
    if (title)
      items.push({
        id: `source:${sourceId}`,
        title,
        detail: agent.name,
        status: "warning",
        href: `/agents/${encodeURIComponent(agent.id)}?tab=connection`,
      });
  }

  const failures =
    overview && !expired && !error ? recentGatewayFailures(overview, now) : [];
  for (const { request } of failures.slice(0, 3)) {
    const title = overview?.gatewayActivities
      .find((activity) => activity.request.id === request.id)
      ?.userInput?.text.trim();
    items.push({
      id: `request:${request.id}`,
      title:
        title ||
        (request.model ? `请求失败 · ${request.model}` : "网关请求失败"),
      detail: [
        agentNames[recordAgentKey(request, agentProducts)] ?? request.provider,
        request.status === "incomplete" ? "响应未完整完成" : null,
        request.httpStatus != null ? `HTTP ${request.httpStatus}` : null,
        date(request.endedAt),
      ]
        .filter(Boolean)
        .join(" · "),
      status: "danger",
      href: `/requests/${encodeURIComponent(request.id)}`,
    });
  }

  if (items.length === 0) return null;
  return (
    <aside
      aria-labelledby="overview-attention-title"
      className="flex min-w-0 flex-wrap items-start gap-x-3 gap-y-2"
    >
      <h3
        id="overview-attention-title"
        className="shrink-0 py-1 text-body font-medium"
      >
        需要关注
      </h3>
      <ul className="min-w-0 flex-1 basis-60 space-y-2">
        {items.map(({ id, title, detail, status, href }) => (
          <li key={id} className="min-w-0">
            <InlineNotice className="w-full gap-x-2 px-2 text-label">
              <Badge variant="plain" size="sm" status={status}>
                {status === "danger" ? "失败" : "注意"}
              </Badge>
              <InlineNoticeContent className="flex flex-wrap items-baseline gap-x-2 gap-y-1 wrap-anywhere">
                <span className="line-clamp-1 min-w-0 font-medium">
                  {href ? (
                    <a
                      href={`#${href}`}
                      className="rounded-sm underline decoration-border underline-offset-2 hover:decoration-current focus-visible:ring-1 focus-visible:ring-focus-ring"
                      title={title}
                    >
                      {title}
                    </a>
                  ) : (
                    title
                  )}
                </span>
                <span className="min-w-0 text-fg-muted">{detail}</span>
              </InlineNoticeContent>
            </InlineNotice>
          </li>
        ))}
        {failures.length > 3 && (
          <li className="text-label text-fg-muted">
            另有 {failures.length - 3} 条失败请求，可在请求页查看。
          </li>
        )}
      </ul>
    </aside>
  );
}
