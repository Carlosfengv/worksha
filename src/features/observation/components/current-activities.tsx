import { useEffect, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type { Snapshot } from "@/app/types";
import { Badge } from "@/components/ui/badge";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyActions,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";
import { date, sessionLink } from "@/features/sessions/display";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { recordAgentKey } from "@/shared/types/agent";
import { activityGroup, nativeSamplingLabel } from "../activity-group";
import { phaseNames, seconds, speed, timingNote } from "../display";
import { requestStates } from "../request-display";
import type { Overview, SessionContextSample } from "../types";
import { activityRows } from "./agent-activity";
import { ActivityContext } from "./activity-context";
import { ActivityTime } from "./activity-time";
import { summarizeActivity } from "../activity-summary";
import { latestSessionActivities } from "../session-activities";
import { ActivitySummary } from "./activity-summary";

type ActivityRow = ReturnType<typeof activityRows>[number] & {
  agentName: string;
  agentLogo: string | undefined;
  context: SessionContextSample | undefined;
};
const agentLogos: Record<string, string> = {
  claude: "/agent-logos/claude.png",
  codex: "/agent-logos/codex.png",
  opencode: "/agent-logos/opencode.png",
};
const activityWindowMs = 3 * 60 * 60 * 1000;
function withinActivityWindow(time: string, now: number) {
  const timestamp = Date.parse(time);
  return !Number.isFinite(timestamp) || now - timestamp <= activityWindowMs;
}
const columns: ColumnDef<ActivityRow>[] = [
  {
    id: "task",
    header: "任务 / Agent",
    cell: ({ row: { original: a } }) => (
      <div className="flex min-w-52 max-w-72 items-center gap-3 whitespace-normal wrap-anywhere">
        {a.agentLogo && (
          <img
            src={a.agentLogo}
            alt=""
            aria-hidden="true"
            width={36}
            height={36}
            draggable={false}
            className="size-9 shrink-0 object-contain"
          />
        )}
        <div className="min-w-0 space-y-1">
          <p className="line-clamp-2 font-medium" title={a.title}>
            {a.title}
          </p>
          {a.userInput?.text.trim() ? (
            a.sessionTitle ? (
              <p
                className="truncate text-label text-fg-muted"
                title={a.sessionTitle}
              >
                会话：{a.sessionTitle}
              </p>
            ) : null
          ) : (
            <p
              className="text-label text-fg-muted"
              title="此来源尚未提供这条消息的用户输入，可进入会话查看。"
            >
              最近输入：来源未提供
            </p>
          )}
          <p className="text-label text-fg-muted">
            {a.agentName} · {a.source}
          </p>
          {a.messageUncertain && a.sessionId && (
            <p className="text-label text-fg-subtle">消息归属待确认</p>
          )}
        </div>
      </div>
    ),
  },
  {
    id: "phase",
    header: "阶段",
    cell: ({ row: { original: a } }) => (
      <div className="min-w-28 max-w-40 space-y-1 whitespace-normal wrap-anywhere">
        <Badge size="sm" status="neutral">
          {a.expired
            ? "状态待更新"
            : a.phase === "waiting_input"
              ? "已结束 · 等待输入"
              : (phaseNames[a.phase] ?? requestStates[a.phase] ?? a.phase)}
        </Badge>
        {(a.toolNames.length > 0 ||
          a.step ||
          ["tool_arguments", "tool_requested"].includes(a.phase)) && (
          <p
            className="line-clamp-2 text-label text-fg-muted"
            title={a.step ?? a.toolNames.join("、")}
          >
            {a.step
              ? `${a.expired ? "最近步骤：" : ""}${a.step}`
              : a.toolNames.length > 0
                ? `${a.expired ? "最近工具" : "工具"}：${a.toolNames.join("、")}`
                : "工具名称未提供"}
          </p>
        )}
        <p className="text-label text-fg-muted">{a.model ?? "模型未知"}</p>
        {a.taskElapsed && (
          <p className="text-label text-fg-muted">本轮耗时 {a.taskElapsed}</p>
        )}
      </div>
    ),
  },
  {
    id: "ttft",
    header: "TTFT",
    cell: ({ row: { original: a } }) => (
      <div
        className="min-w-28 max-w-40 space-y-1 whitespace-normal text-label tabular-nums"
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
        <p>
          {a.ttft !== null
            ? `${a.ttftSource === "native_text" ? "正文首出" : "TTFT"} ${seconds(a.ttft)} 秒`
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
    ),
  },
  {
    id: "tokenRate",
    header: "Token 速率",
    cell: ({ row: { original: a } }) => (
      <div
        className="min-w-36 max-w-48 space-y-1 whitespace-normal text-label tabular-nums"
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
        <p>
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
                "近5秒 · 输出估算"
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
        <p className="text-fg-muted">
          {a.statistics.outputTokens == null
            ? "本轮用量待上报"
            : `${a.statistics.estimated ? "本轮可见正文约" : "本轮已报输出"} ${a.statistics.outputTokens.toLocaleString("zh-CN", { maximumFractionDigits: 1 })} Token${a.statistics.partial ? " · 部分上报" : ""}`}
        </p>
        {a.statistics.reasoningTokens != null && (
          <p className="text-fg-muted">
            其中思考 {a.statistics.reasoningTokens.toLocaleString("zh-CN")}{" "}
            Token
          </p>
        )}
        {a.speedKind !== "average" && a.statistics.averageTextRate != null && (
          <p className="text-fg-muted">
            正文采样均速 {speed(a.statistics.averageTextRate)} Token/s
          </p>
        )}
      </div>
    ),
  },
  {
    id: "context",
    header: "上下文",
    cell: ({ row: { original: a } }) => (
      <div className="w-36 whitespace-normal">
        {a.active || !a.sessionId ? (
          <ActivityContext sessionId={a.sessionId} sample={a.context} />
        ) : (
          <a
            href={`#${sessionLink(a.sessionId, { view: "context" })}`}
            aria-label={`查看会话上下文：${a.sessionId}`}
            className="inline-block rounded-lg p-2 text-label text-fg-muted outline-none hover:bg-hover hover:text-fg-default focus-visible:ring-1 focus-visible:ring-focus-ring"
          >
            查看会话上下文
          </a>
        )}
      </div>
    ),
  },
  {
    id: "time",
    header: "请求 / 活动时间",
    cell: ({ row: { original: a } }) => (
      <span className="whitespace-nowrap text-label text-fg-muted">
        <ActivityTime
          value={a.messageAt ?? a.requestAt ?? a.time}
          kind={a.messageAt || a.requestAt ? "request" : "activity"}
          title={`最近活动 · ${date(a.time)}`}
        />
      </span>
    ),
  },
];

export function CurrentActivities({
  data,
  overview,
  stale,
  filter,
  error,
  latest,
}: {
  data: Snapshot;
  overview: Overview | null;
  stale: boolean;
  filter: string;
  error: string | null;
  latest?: { text: string; time: string; href: string };
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const update = () => {
      if (document.visibilityState === "visible") setNow(Date.now());
    };
    const timer = window.setInterval(update, 1000);
    document.addEventListener("visibilitychange", update);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);
  const recentLatest =
    latest && withinActivityWindow(latest.time, now) ? latest : undefined;
  const { agentNames, agentProducts, agentFamily, isSharedSource } =
    useAgentDirectory();
  const family = agentFamily(filter);
  const nativeConnected = data.sessionSources.some(
    (source) =>
      source.enabled &&
      (filter === "all" ||
        (filter === "unknown"
          ? isSharedSource(source.agentId)
          : source.agentId === family)),
  );
  const sourceHref = data.agents.some((agent) => agent.id === filter)
    ? `/agents/${encodeURIComponent(filter)}?tab=connection`
    : "/agents";
  const rows = useMemo(() => {
    const products = [
      ...data.agents.map((a) => a.id),
      ...[...new Set(agentProducts.map((a) => a.family))]
        .filter(isSharedSource)
        .map((family) => `${family}-unknown`),
    ].filter(
      (id) =>
        filter === "all" ||
        (filter === "unknown" ? id.endsWith("-unknown") : id === filter),
    );
    const contexts = new Map(
      overview?.sessionContexts?.map((sample) => [sample.sessionId, sample]),
    );
    const groups = products.map((id) =>
      activityGroup(
        id,
        overview?.gatewayActivities ?? [],
        overview?.nativeActivities ?? [],
        stale,
        now,
        data.agents,
      ),
    );
    const combined = {
      ...(groups[0] ?? activityGroup("", [], [], stale, now, data.agents)),
      gateway: groups.flatMap((g) => g.gateway),
      native: groups.flatMap((g) => g.native),
      activeGateway: groups.flatMap((g) => g.activeGateway),
      activeNative: groups.flatMap((g) => g.activeNative),
      freshNative: groups.flatMap((g) => g.freshNative),
    };
    const activities = latestSessionActivities(
      activityRows(combined, "recent", now),
    ).map((a) => {
      const id = recordAgentKey(a, data.agents);
      return {
        ...a,
        agentName: agentNames[id] ?? id,
        agentLogo: agentLogos[agentFamily(id) ?? ""],
        context: a.sessionId ? contexts.get(a.sessionId) : undefined,
      };
    });
    const timestamp = (time: string) => {
      const parsed = Date.parse(time);
      return Number.isFinite(parsed) ? parsed : 0;
    };
    return activities
      .filter((a) => withinActivityWindow(a.time, now))
      .sort(
        (a, b) =>
          timestamp(b.messageAt ?? "") - timestamp(a.messageAt ?? "") ||
          a.id.localeCompare(b.id),
      );
  }, [
    data.agents,
    overview,
    stale,
    filter,
    agentProducts,
    agentNames,
    agentFamily,
    isSharedSource,
    now,
  ]);
  const pagination = useTablePagination(rows.length, 5);
  const summary = useMemo(
    () =>
      summarizeActivity(
        overview?.gatewayActivities ?? [],
        overview?.nativeActivities ?? [],
        filter,
        data.agents,
        now,
      ),
    [overview, filter, data.agents, now],
  );
  const snapshotExpired =
    stale ||
    Boolean(
      overview &&
      (!Number.isFinite(Date.parse(overview.asOf)) ||
        now - Date.parse(overview.asOf) > 6000),
    );
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => (row.sessionId ? `session:${row.sessionId}` : row.id),
    enableSorting: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: {
      pagination: pagination.value,
    },
    onPaginationChange: pagination.onChange,
  });
  return (
    <div className="space-y-2">
      <ActivitySummary
        summary={summary}
        state={
          !overview
            ? error
              ? "error"
              : "loading"
            : snapshotExpired
              ? "stale"
              : "ready"
        }
      />
      {overview && stale && (
        <p className="text-label text-fg-muted" role="status">
          实时活动快照已过期，保留最近记录；进行中状态待确认。
        </p>
      )}
      <DataTable
        table={table}
        aria-label="当前活动列表"
        role="region"
        isLoading={!overview && !error}
        loadingMessage="正在读取实时活动…"
        onRowActivate={(row) => {
          window.location.hash = row.original.href;
        }}
        showPagination={rows.length > 5}
        paginationProps={{
          labels: dataTablePagination,
          pageSizeOptions: [5, 10, 20],
        }}
        emptyState={
          !overview || stale ? (
            <div className="space-y-2 p-6 text-center text-body" role="status">
              <p className="font-medium">
                {overview ? "实时活动快照已过期" : "暂时无法读取实时活动"}
              </p>
              <p className="text-label text-fg-muted">
                {overview
                  ? "保留的记录不能确认仍在运行，等待观测服务更新。"
                  : "观测服务读取失败，恢复后会自动更新。"}
              </p>
            </div>
          ) : (
            <Empty reason="no-results" scope="inline">
              <EmptyHeader>
                <EmptyTitle>
                  {nativeConnected ? "暂无活动记录" : "尚未连接原生观测来源"}
                </EmptyTitle>
                <EmptyDescription>
                  {nativeConnected
                    ? "仅显示最近 3 小时的活动，按最后活动时间排序。"
                    : "当前范围未开启原生会话读取，请在 Agent 的“连接与观测”中确认读取范围。网关请求经过本应用时会自动显示。"}
                </EmptyDescription>
              </EmptyHeader>
              {recentLatest && (
                <EmptyContent>
                  <p className="text-label text-fg-muted">
                    最近观测：{recentLatest.text} · {date(recentLatest.time)}
                  </p>
                </EmptyContent>
              )}
              <EmptyActions>
                {!nativeConnected && (
                  <Button
                    size="sm"
                    variant="tertiary"
                    onClick={() => {
                      window.location.hash = sourceHref;
                    }}
                  >
                    连接观测来源
                  </Button>
                )}
                {(nativeConnected || recentLatest) && (
                  <Button
                    size="sm"
                    variant="tertiary"
                    onClick={() => {
                      window.location.hash = recentLatest?.href ?? sourceHref;
                    }}
                  >
                    {recentLatest ? "查看最近记录" : "管理观测来源"}
                  </Button>
                )}
              </EmptyActions>
            </Empty>
          )
        }
      />
      {rows.some((row) => row.source.startsWith("原生记录")) && (
        <p className="text-label text-fg-subtle">
          原生直连缺少流式时序时无法测量实时
          Token/s；经本地网关的请求可采集，Agent 上报首出单独标记。
          <a
            href={`#${sourceHref}`}
            className="ml-2 rounded-lg text-fg-muted outline-none hover:text-fg-default focus-visible:ring-1 focus-visible:ring-focus-ring"
          >
            查看连接与观测
          </a>
        </p>
      )}
    </div>
  );
}
