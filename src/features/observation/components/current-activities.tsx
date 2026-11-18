import { useEffect, useMemo, useState, type ReactNode } from "react";
import type { CellContext, ColumnDef } from "@tanstack/react-table";
import type { Snapshot } from "@/app/types";
import { SegmentedBar } from "@/components/ui/chart-primitives";
import { compactNumber } from "@/shared/lib/format";
import { providerCosts } from "@/features/providers/usage-display";
import { Badge } from "@/components/ui/badge";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { Button } from "@/components/ui/button";
import { TabItem, TabPanel, Tabs, TabsList } from "@/components/ui/tabs";
import type { MetricCardState } from "@/components/ui/metric-card";
import {
  Empty,
  EmptyActions,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";
import { date } from "@/features/sessions/display";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { recordAgentKey, type AgentDiscovery } from "@/shared/types/agent";
import type { UnifiedActivityPage } from "../../../../contracts/domain/facts/types";
import { activityGroup, nativeSamplingLabel } from "../activity-group";
import { phaseNames, seconds, speed, timingNote } from "../display";
import { requestStates } from "../request-display";
import type { Overview } from "../types";
import { activityRows } from "./agent-activity";
import { ActivityTime } from "./activity-time";
import { summarizeActivity } from "../activity-summary";
import {
  activityWindowSeconds,
  latestSessionActivities,
} from "../session-activities";
import {
  publishedActivityRows,
  type PublishedActivityRow,
} from "../published-activities";

type MeasuredActivityRow = ReturnType<typeof activityRows>[number] & {
  reported: false;
  agentName: string;
};
type ActivityRow = PublishedActivityRow | MeasuredActivityRow;

function measurementCell(
  kind: "ttft" | "tokenRate",
  render: (row: MeasuredActivityRow) => ReactNode,
) {
  return ({ row }: CellContext<ActivityRow, unknown>) => {
    const activity = row.original;
    if (!activity.reported) return render(activity);
    const timing = activity.timing;
    if (kind === "ttft" && timing?.firstTokenMs != null)
      return (
        <div className="text-label tabular-nums">
          <p className="font-mono text-body">
            {seconds(timing.firstTokenMs)} 秒
          </p>
          <p className="text-fg-muted">{activity.agentName} · 上报首出</p>
        </div>
      );
    if (kind === "tokenRate" && timing) {
      const running = activity.active ? timing.runningTokensPerSecond : null;
      const rate = running ?? timing.completedTokensPerSecond;
      if (rate != null || timing.outputTokens != null)
        return (
          <div className="space-y-1 text-label tabular-nums">
            {rate != null && (
              <>
                <p>均速 {speed(rate)} Token/s</p>
                <p className="text-fg-muted">
                  {running != null
                    ? `${activity.agentName} · 上报当前步骤`
                    : `${activity.agentName} · 上报已完成步骤`}
                </p>
              </>
            )}
            {timing.outputTokens != null && (
              <p className="text-fg-muted">
                已报输出 {BigInt(timing.outputTokens).toLocaleString("zh-CN")}{" "}
                Token
              </p>
            )}
          </div>
        );
    }
    return <span className="text-label text-fg-muted">来源未提供</span>;
  };
}
function withinActivityWindow(time: string, now: number) {
  const timestamp = Date.parse(time);
  return (
    !Number.isFinite(timestamp) ||
    now - timestamp <= activityWindowSeconds * 1000
  );
}
const columns: ColumnDef<ActivityRow>[] = [
  {
    id: "task",
    header: "当前会话任务",
    cell: ({ row: { original: a } }) => (
      <div className="min-w-24 max-w-96 space-y-1 whitespace-normal">
        <a
          href={`#${a.href}`}
          data-workspace-resource
          data-workspace-title={a.title}
          title={a.title}
          className="line-clamp-2 font-medium wrap-anywhere hover:underline"
        >
          {a.title}
        </a>
        <div className="flex flex-wrap items-center gap-1">
          <Badge size="sm" status="neutral" title={a.agentName}>
            {a.model ?? "模型未知"}
          </Badge>
          <Badge size="sm" status={a.active ? "info" : "neutral"}>
            {a.reported
              ? a.phaseLabel
              : a.expired
                ? "状态待更新"
                : a.phase === "waiting_input"
                  ? "已结束 · 等待输入"
                  : (phaseNames[a.phase] ?? requestStates[a.phase] ?? a.phase)}
          </Badge>
        </div>
        {!a.reported && a.userInput?.text.trim() && a.sessionTitle && (
          <p
            className="line-clamp-1 text-label text-fg-muted"
            title={a.sessionTitle}
          >
            会话：{a.sessionTitle}
          </p>
        )}
        {!a.reported && !a.userInput?.text.trim() && a.sessionId && (
          <p className="text-label text-fg-muted">最近输入：来源未提供</p>
        )}
        {!a.reported && a.messageUncertain && a.sessionId && (
          <p className="text-label text-fg-muted">消息归属待确认</p>
        )}
        {!a.reported && (a.step || a.toolNames.length > 0) && (
          <p
            className="line-clamp-2 text-label text-fg-muted"
            title={a.step ?? a.toolNames.join("、")}
          >
            {a.step
              ? `${a.expired ? "最近步骤：" : ""}${a.step}`
              : `${a.expired ? "最近工具" : "工具"}：${a.toolNames.join("、")}`}
          </p>
        )}
        <p className="flex flex-wrap items-baseline gap-x-1.5 text-label text-fg-muted">
          <span>
            {a.agentName} · {a.source}
          </span>
          <span
            data-activity-elapsed
            title={
              !a.taskElapsed
                ? "来源未提供本轮耗时"
                : a.messageAt || a.requestAt
                  ? `本轮开始 · ${date(a.messageAt ?? a.requestAt)}`
                  : undefined
            }
          >
            {a.taskElapsed ?? "—"}
          </span>
          <ActivityTime value={a.time} kind="activity" />
        </p>
      </div>
    ),
  },
  {
    id: "ttft",
    header: "首字延迟 (TTFT)",
    cell: measurementCell("ttft", (a) => (
      <div
        className="max-w-40 space-y-1 whitespace-normal text-label tabular-nums"
        title={[
          a.ttftSource === "agent"
            ? "Agent 上报的本轮首出耗时，与网关请求 TTFT 分开记录。"
            : a.messageTtftRequest
              ? `本条消息首次网关请求的 TTFT，后续工具续调不重置 · ${a.messageTtftRequest.provider} · ${date(a.messageTtftRequest.startedAt)}`
              : a.ttftSource === "native_token"
                ? "从原生消息开始到桌面事件返回首个推理或正文增量的耗时，包含桌面事件传递延迟；完成后以 Agent 上报值为准。空事件和工具事件不计入。"
                : a.ttftSource === "native_text"
                  ? "从用户消息开始到观察到首段正文的耗时；不包含不可见推理首 Token 的测量。"
                  : a.metricRequest
                    ? `最近网关请求 · ${a.metricRequest.provider} · ${a.metricRequest.model ?? "模型未知"} · ${date(a.metricRequest.startedAt)}`
                    : "从请求开始到首个有效生成内容；心跳和响应头不计入首出。",
          a.issue,
        ]
          .filter(Boolean)
          .join(" · ")}
      >
        <p className="font-mono text-body font-medium">
          {a.ttft !== null
            ? `${seconds(a.ttft)} 秒`
            : a.nativeRealtime
              ? a.nativeRealtime.firstTokenMissed
                ? "TTFT 待补报"
                : a.active
                  ? "等待首 Token"
                  : "TTFT 未记录"
              : a.issue
                ? "首出未记录"
                : a.source === "网关请求" && a.active
                  ? "等待首出"
                  : a.source === "网关请求" || a.metricRequest
                    ? "TTFT 未记录"
                    : "来源未提供"}
        </p>
        <p className="text-fg-muted">
          {a.ttftSource === "agent"
            ? "Agent 上报 · 本轮"
            : a.messageTtftRequest
              ? "消息首出 · 网关实测"
              : a.ttftSource === "native_token"
                ? "原生首 Token · 实时观测"
                : a.ttftSource === "native_text"
                  ? "原生正文 · 实测"
                  : a.nativeRealtime && a.ttft === null
                    ? a.nativeRealtime.firstTokenMissed
                      ? "中途接入 · 待原生补报"
                      : "首个推理或正文返回后显示"
                    : a.ttft === null && a.issue
                      ? timingNote(a.issue)
                      : a.metricRequest
                        ? a.metricRequest.status === "in_progress"
                          ? a.metricRequestActive
                            ? "当前请求 · 网关实测"
                            : "最近请求 · 状态待更新"
                          : "最近请求 · 已记录"
                        : a.source === "网关请求"
                          ? a.active
                            ? "网关实测 · 当前请求"
                            : "网关已记录"
                          : "原生未提供首出时序"}
        </p>
      </div>
    )),
  },
  {
    id: "tokenRate",
    header: "Token/s",
    cell: measurementCell("tokenRate", (a) => (
      <div
        className="max-w-48 space-y-1 whitespace-normal text-label tabular-nums"
        title={
          [
            a.metricRequest
              ? `最近网关请求 · ${a.metricRequest.provider} · ${a.metricRequest.model ?? "模型未知"} · ${date(a.metricRequest.startedAt)}`
              : undefined,
            a.issue,
          ]
            .filter(Boolean)
            .join(" · ") || undefined
        }
      >
        <p className="font-medium text-fg-default">
          {a.speed !== null
            ? `${a.speedKind === "live" ? (a.rateWindowMs === 5000 ? "近5秒" : "实时") : a.speedKind === "average" ? "均速" : "吞吐"} ${speed(a.speed)} Token/s`
            : a.recentSample
              ? `最近 ${speed(a.recentSample.tokensPerSecond)} Token/s`
              : a.nativeRealtime
                ? a.active
                  ? nativeSamplingLabel({
                      phase: a.phase,
                      realtime: a.nativeRealtime,
                    })
                  : "速率未记录"
                : a.issue
                  ? a.active
                    ? "采样恢复中"
                    : "速率未记录"
                  : a.source === "网关请求"
                    ? a.active
                      ? a.phase === "tool_arguments"
                        ? "生成工具参数"
                        : a.phase === "public_reasoning"
                          ? "推理中"
                          : "等待模型响应"
                      : a.expired
                        ? "速度待更新"
                        : "速率未记录"
                    : a.metricRequest
                      ? a.active && a.phase === "thinking"
                        ? "思考中"
                        : a.active && a.phase === "tool_requested"
                          ? "工具处理中"
                          : a.active && a.phase === "tool_returned"
                            ? "工具结果已返回"
                            : a.metricRequest.status === "in_progress"
                              ? "速度待更新"
                              : "速率未记录"
                      : "来源未提供"}
        </p>
        <p className="text-fg-muted">
          {a.speed !== null ? (
            a.speedKind === "live" ? (
              a.rateWindowMs === 5000 ? (
                "输出估算"
              ) : a.outputRate ? (
                "最近 1 秒 · 输出估算"
              ) : (
                "最近 1 秒 · 文本估算"
              )
            ) : a.speedKind === "average" ? (
              "完成正文均速 · 估算"
            ) : (
              "输出 Token ÷ 完整请求时长"
            )
          ) : a.recentSample ? (
            <>
              <ActivityTime
                value={new Date(a.recentSample.at).toISOString()}
                kind="sample"
              />{" "}
              · 历史采样
            </>
          ) : a.nativeRealtime ? (
            !a.active ? (
              "本条消息未记录正文采样"
            ) : a.nativeRealtime.state === "connected" ? (
              "原生未上报生成增量"
            ) : (
              "自动尝试恢复原生连接"
            )
          ) : a.source !== "网关请求" && !a.metricRequest ? (
            "原生未提供流式时序"
          ) : a.issue ? (
            timingNote(a.issue)
          ) : a.active ? (
            "收到生成增量后更新"
          ) : a.expired || a.metricRequest?.status === "in_progress" ? (
            "实时采集待恢复"
          ) : (
            "请求未记录可测速率"
          )}
        </p>
      </div>
    )),
  },
  {
    id: "spend",
    header: "本轮 Token 花费",
    cell: ({ row: { original: activity } }) => {
      if (activity.reported)
        return (
          <span className="text-label">
            {activity.timing?.outputTokens != null
              ? `已报输出 ${BigInt(activity.timing.outputTokens).toLocaleString("zh-CN")} tok`
              : "来源未提供"}
          </span>
        );
      const stats = activity.statistics;
      const uncached =
        stats.cachedInputTokens != null && stats.inputTokens != null
          ? Math.max(0, stats.inputTokens - stats.cachedInputTokens)
          : stats.inputTokens;
      const segments = [
        { id: "input", label: "输入", value: uncached },
        { id: "cache", label: "缓存", value: stats.cachedInputTokens },
        { id: "output", label: "输出", value: stats.outputTokens },
      ];
      return (
        <div
          className="min-w-44 space-y-1 text-label"
          title={`思考 ${compactNumber(stats.reasoningTokens)} tok · ${stats.requestCount} 步网关请求`}
        >
          <div className="flex justify-between gap-3">
            <span className="tabular-nums">
              {stats.streamingEstimated ? "≈ " : ""}
              {compactNumber(stats.totalTokens)} tok
            </span>
            <span className="text-fg-muted">
              {stats.requestCount
                ? providerCosts(stats.cost, true)
                : "直连无计费"}
              {stats.streamingEstimated
                ? " · 估算中"
                : stats.partial
                  ? " · 部分上报"
                  : ""}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <SegmentedBar
              mode="distribution"
              segments={segments}
              total={stats.totalTokens}
              valueText="本轮 Token 构成"
              className="h-1.5 w-12 shrink-0"
            />
            <span className="text-fg-muted">
              入 {compactNumber(uncached)} · 缓{" "}
              {compactNumber(stats.cachedInputTokens)} · 出{" "}
              {compactNumber(stats.outputTokens)}
            </span>
          </div>
        </div>
      );
    },
  },
];

export function useCurrentActivities({
  data,
  overview,
  filter,
  error,
  publishedPage,
  sourceAgents,
  sourceLoading,
  sourceError,
}: {
  data: Snapshot | null;
  overview: Overview | null;
  filter: string;
  error: string | null;
  publishedPage: UnifiedActivityPage | null;
  sourceAgents: AgentDiscovery[];
  sourceLoading: boolean;
  sourceError: string | null;
}) {
  const [clock, setClock] = useState(Date.now);
  // Fresh snapshots can arrive while the visible-window clock is paused.
  const now = Math.max(clock, Date.parse(overview?.asOf ?? "") || 0);
  useEffect(() => {
    const update = () => {
      if (document.visibilityState === "visible") setClock(Date.now());
    };
    const timer = window.setInterval(update, 1000);
    document.addEventListener("visibilitychange", update);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);
  const { agentNames, agentProducts, agentFamily, isSharedSource } =
    useAgentDirectory();
  const family = agentFamily(filter);
  const reportedOnly =
    filter !== "all" &&
    !!data?.agents.some((agent) => agent.id === filter && agent.sourceId);
  const nativeObservationPaused =
    !reportedOnly &&
    (data?.builtinComposition?.nativeActivityActive === false ||
      data?.builtinComposition?.profile === "recovery" ||
      !!data?.builtinComposition?.problem);
  const nativeSourceEnabled =
    !reportedOnly &&
    !!data?.sessionSources.some(
      (source) =>
        source.enabled &&
        source.runtimeActive !== false &&
        (filter === "all" ||
          (filter === "unknown"
            ? isSharedSource(source.agentId)
            : source.agentId === family)),
    );
  const sourceHref =
    reportedOnly ||
    (!nativeObservationPaused &&
      data?.agents.some((agent) => agent.id === filter))
      ? `/agents/${encodeURIComponent(filter)}?tab=connection`
      : nativeObservationPaused
        ? "/plugins?scene=activity"
        : "/agents";
  const reportedSourceEnabled =
    publishedPage?.sourceStates.some(
      ({ state }) =>
        state.authorization === "authorized" &&
        state.coverage !== "unavailable",
    ) ?? false;
  const observationEnabled =
    (nativeSourceEnabled && !nativeObservationPaused) || reportedSourceEnabled;
  const reportedRows = useMemo(
    () => publishedActivityRows(publishedPage, sourceAgents, now),
    [publishedPage, sourceAgents, now],
  );
  const snapshotExpired =
    Boolean(error) ||
    Boolean(
      overview &&
      (!Number.isFinite(Date.parse(overview.asOf)) ||
        now - Date.parse(overview.asOf) > 6000),
    );
  const agents = data?.agents;
  const rows = useMemo(() => {
    const products = [
      ...(agents ?? []).map((a) => a.id),
      ...[...new Set(agentProducts.map((a) => a.family))]
        .filter(isSharedSource)
        .map((family) => `${family}-unknown`),
    ].filter(
      (id) =>
        filter === "all" ||
        (filter === "unknown" ? id.endsWith("-unknown") : id === filter),
    );
    const groups = products.map((id) =>
      activityGroup(
        id,
        overview?.gatewayActivities ?? [],
        overview?.nativeActivities ?? [],
        snapshotExpired,
        now,
        agents,
      ),
    );
    const combined = {
      ...(groups[0] ?? activityGroup("", [], [], snapshotExpired, now, agents)),
      gateway: groups.flatMap((g) => g.gateway),
      native: groups.flatMap((g) => g.native),
      activeGateway: groups.flatMap((g) => g.activeGateway),
      activeNative: groups.flatMap((g) => g.activeNative),
      freshNative: groups.flatMap((g) => g.freshNative),
    };
    const activities = activityRows(combined, "recent", now).map((a) => {
      const id = recordAgentKey(a, agents);
      return {
        ...a,
        reported: false as const,
        agentName: agentNames[id] ?? id,
      };
    });
    const timestamp = (time: string) => {
      const parsed = Date.parse(time);
      return Number.isFinite(parsed) ? parsed : 0;
    };
    return latestSessionActivities<ActivityRow>([
      ...activities,
      ...reportedRows,
    ])
      .filter((a) => withinActivityWindow(a.time, now))
      .sort(
        (a, b) =>
          timestamp(b.messageAt ?? (b.reported ? b.time : "")) -
            timestamp(a.messageAt ?? (a.reported ? a.time : "")) ||
          a.id.localeCompare(b.id),
      );
  }, [
    agents,
    overview,
    snapshotExpired,
    filter,
    agentProducts,
    agentNames,
    isSharedSource,
    now,
    reportedRows,
  ]);
  const summary = useMemo(
    () =>
      summarizeActivity(
        overview?.gatewayActivities ?? [],
        overview?.nativeActivities ?? [],
        filter,
        agents ?? [],
        now,
        reportedRows.flatMap((row) =>
          row.active && row.sessionId ? [row.sessionId] : [],
        ),
      ),
    [overview, filter, agents, now, reportedRows],
  );
  const overviewState = !overview
    ? error
      ? "error"
      : "loading"
    : snapshotExpired
      ? "stale"
      : "ready";
  const sourceState = sourceError
    ? "error"
    : sourceLoading && !publishedPage
      ? "loading"
      : publishedPage &&
          (now - Date.parse(publishedPage.asOf) > 15000 ||
            publishedPage.sourceStates.some(
              ({ state }) => state.problem === "source_stalled",
            ))
        ? "stale"
        : "ready";
  const state: MetricCardState =
    reportedOnly && summary.sources === 0 ? sourceState : overviewState;
  const activeReported = reportedRows.filter((row) => row.active);
  const reportedTtfts = activeReported.flatMap((row) =>
    row.timing?.firstTokenMs == null ? [] : [row.timing.firstTokenMs],
  );
  const reportedRates = activeReported.flatMap((row) =>
    row.timing?.runningTokensPerSecond == null
      ? []
      : [row.timing.runningTokensPerSecond],
  );
  return {
    rows,
    now,
    sourceName: reportedOnly ? agentNames[filter] : undefined,
    overviewExpired: Boolean(overview) && snapshotExpired,
    performance: {
      summary,
      reportedTtfts,
      reportedRates,
      reportedSourceNames: [
        ...new Set(activeReported.map((row) => row.agentName)),
      ],
      nativeSourceEnabled: observationEnabled,
      nativeObservationPaused,
      reportedOnly,
      sourceHref,
      state,
    },
  };
}

export function CurrentActivities({
  activity,
}: {
  activity: ReturnType<typeof useCurrentActivities>;
}) {
  const { rows, sourceName, overviewExpired } = activity;
  const {
    state,
    nativeSourceEnabled: observationEnabled,
    nativeObservationPaused,
    reportedOnly,
    sourceHref,
  } = activity.performance;
  const activeRows = rows.filter((row) => row.active);
  const completedRows = rows.filter((row) => !row.active);
  const running = activeRows.length > 0;
  const [choice, setChoice] = useState<{ running: boolean; mode: string }>();
  const mode =
    choice?.mode === "all" || choice?.running === running
      ? choice.mode
      : running
        ? "running"
        : "recent";
  const visibleRows =
    mode === "all" ? rows : mode === "running" ? activeRows : completedRows;
  const pagination = useTablePagination(visibleRows.length, 5);
  const { table } = useDataTable({
    data: visibleRows,
    columns,
    getRowId: (row) => row.sessionId ?? row.id,
    enableSorting: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: {
      pagination: pagination.value,
    },
    onPaginationChange: pagination.onChange,
  });
  return (
    <Tabs
      value={mode}
      onValueChange={(value) => {
        setChoice({ running, mode: value });
        pagination.onChange((page) => ({ ...page, pageIndex: 0 }));
      }}
      color="default"
      variant="segment"
      className="@container min-w-0 space-y-3"
    >
      <TabsList aria-label="活动范围">
        <TabItem value="running" label="执行中" badge={activeRows.length} />
        <TabItem value="recent" label="最近完成" badge={completedRows.length} />
        <TabItem value="all" label="最近 3 小时" badge={rows.length} />
      </TabsList>
      <TabPanel value={mode} className="min-w-0 space-y-3">
        {overviewExpired && !reportedOnly && rows.length > 0 && (
          <p className="text-label text-fg-muted" role="status">
            实时活动快照已过期，保留最近记录；进行中状态待确认。
          </p>
        )}
        <DataTable
          id="overview-activity-table"
          table={table}
          aria-label="当前活动列表"
          role="region"
          className="[&_td]:align-top"
          isLoading={visibleRows.length === 0 && state === "loading"}
          loadingMessage="正在读取实时活动…"
          showPagination={visibleRows.length > 5}
          paginationProps={{
            labels: dataTablePagination,
            pageSizeOptions: [5, 10, 20],
          }}
          emptyState={
            mode === "running" && rows.length > 0 && state !== "loading" ? (
              <Empty reason="no-results" scope="inline" density="compact">
                <EmptyHeader>
                  <EmptyTitle>当前没有确认在执行的任务</EmptyTitle>
                  <EmptyDescription>
                    可在「最近完成」查看已记录的任务。
                  </EmptyDescription>
                </EmptyHeader>
              </Empty>
            ) : state === "error" || state === "stale" ? (
              <div
                className="space-y-2 p-6 text-center text-body"
                role="status"
              >
                <p className="font-medium">
                  {state === "stale"
                    ? "实时活动快照已过期"
                    : "暂时无法读取实时活动"}
                </p>
                <p className="text-label text-fg-muted">
                  {state === "stale"
                    ? "保留的记录不能确认仍在运行，等待观测服务更新。"
                    : "观测服务读取失败，恢复后会自动更新。"}
                </p>
              </div>
            ) : (
              <Empty reason="no-results" scope="inline" density="compact">
                <EmptyHeader>
                  <EmptyTitle>
                    {observationEnabled
                      ? "暂无活动记录"
                      : nativeObservationPaused
                        ? "原生活动采集已暂停"
                        : reportedOnly
                          ? `尚未收到${sourceName ?? "所选来源"}的活动`
                          : "尚未连接原生观测来源"}
                  </EmptyTitle>
                  <EmptyDescription>
                    {observationEnabled
                      ? "仅显示最近 3 小时的活动，按最后活动时间排序。"
                      : reportedOnly
                        ? "在该 Agent 的连接与观测中授权读取来源文件，以显示上报任务。"
                        : nativeObservationPaused
                          ? "可在官方组合中恢复活动采集；本机来源的读取授权仍需单独确认。"
                          : "当前范围未开启原生会话读取，请在 Agent 的“连接与观测”中确认读取范围。网关请求经过本应用时会自动显示。"}
                  </EmptyDescription>
                </EmptyHeader>
                {!observationEnabled && (
                  <EmptyActions>
                    <Button
                      size="sm"
                      variant="tertiary"
                      onClick={() => {
                        window.location.hash = sourceHref;
                      }}
                    >
                      {nativeObservationPaused
                        ? "管理活动采集"
                        : "连接观测来源"}
                    </Button>
                  </EmptyActions>
                )}
              </Empty>
            )
          }
        />
      </TabPanel>
    </Tabs>
  );
}
