import { bundledDshAgent } from "@/features/agents/bundled-source";
import { sourceProvides } from "@/features/domain/published-sources";
import { overviewActivityResource } from "@/features/domain/queries";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import type { Page, Snapshot } from "@/app/types";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { TabItem, Tabs, TabsList } from "@/components/ui/tabs";
import {
  CallAnalysisCharts,
  metricLabels,
} from "@/features/observation/components/call-analysis";
import { seconds, speed } from "@/features/observation/display";
import { useOverview } from "@/features/observation/hooks/use-overview";
import type { Overview, OverviewQuery } from "@/features/observation/types";
import { date } from "@/features/sessions/display";
import { recordAgentKey } from "@/shared/types/agent";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useMemo, useState } from "react";
import {
  AgentActivity,
  UnattributedActivities,
} from "./components/agent-activity";
import { ActivitySummary } from "./components/activity-summary";
import {
  CurrentActivities,
  useCurrentActivities,
} from "./components/current-activities";
import { OverviewAttention } from "./components/overview-attention";
import { activityWindowSeconds } from "./session-activities";

export function OverviewPage({
  data,
  navigate,
}: {
  data: Snapshot | null;
  navigate: (page: Page) => void;
}) {
  const [agent, setAgent] = useState("all");
  const [range, setRange] = useState("24");
  const [metric, setMetric] = useState<OverviewQuery["metric"]>("calls");
  const [dimension, setDimension] =
    useState<OverviewQuery["dimension"]>("agent");
  const [source, setSource] = useState<OverviewQuery["source"]>("gateway");
  const sourceAgentSelected =
    agent === bundledDshAgent.id ||
    Boolean(data?.agents.some((item) => item.id === agent && item.sourceId));
  const observation = useOverview({
    // The host overview only accepts built-in Agent filters. Use its
    // unfiltered activity snapshot for any attributable gateway requests.
    agent: agent === "all" || sourceAgentSelected ? undefined : agent,
    range: Number(range),
    metric,
    dimension,
    source,
  });
  const agents = data?.agents ?? [];
  const eligibleSourceAgents = useMemo(
    () =>
      (data?.agents ?? []).filter(
        (item) =>
          sourceProvides(item, "activity") &&
          (agent === "all" || item.id === agent),
      ),
    [data?.agents, agent],
  );
  const sourceAgents = useMemo(
    () => eligibleSourceAgents.filter((item) => item.connected),
    [eligibleSourceAgents],
  );
  const sourceIds = [
    ...new Set(sourceAgents.flatMap((item) => item.sourceId ?? [])),
  ]
    .sort()
    .slice(0, 16);
  const published = useSharedQuery(
    overviewActivityResource,
    {
      scope: { kind: "external", sourceIds },
      rangeSeconds: activityWindowSeconds,
      productId: agent === "all" ? null : agent,
      sessionId: null,
      includeGateway: false,
      includeHistorical: true,
    },
    sourceIds.length > 0,
  );
  const publishedPage = published.error ? null : published.data;
  // Source consent can change while the last overview is cached after a read
  // failure. Remove native excerpts before any card, table or popover sees it.
  const overview = useMemo(
    () =>
      observation.data
        ? {
            ...observation.data,
            gatewayActivities: sourceAgentSelected
              ? observation.data.gatewayActivities.filter(
                  ({ request }) =>
                    recordAgentKey(request, data?.agents) === agent,
                )
              : observation.data.gatewayActivities,
            nativeActivities: observation.data.nativeActivities
              .filter(
                (activity) =>
                  !sourceAgentSelected &&
                  data?.sessionSources.some(
                    (source) =>
                      source.enabled &&
                      source.runtimeActive !== false &&
                      source.agentId === activity.agentId,
                  ),
              )
              .map((activity) =>
                data?.builtinComposition?.nativeActivityActive === false
                  ? { ...activity, stale: true }
                  : activity,
              ),
          }
        : null,
    [
      observation.data,
      agent,
      sourceAgentSelected,
      data?.agents,
      data?.sessionSources,
      data?.builtinComposition?.nativeActivityActive,
    ],
  );
  const activity = useCurrentActivities({
    data,
    overview,
    filter: agent,
    error: observation.error,
    publishedPage,
    sourceAgents,
    sourceLoading: published.loading,
    sourceError: published.error,
  });
  const activityUpdatedAt = [overview?.asOf, publishedPage?.asOf]
    .filter((value): value is string => value != null)
    .sort()
    .at(-1);
  return (
    <div className="flex min-w-0 flex-col gap-6">
      <section
        className="@container space-y-4"
        aria-labelledby="activity-title"
      >
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 id="activity-title" className="text-title font-medium">
              Agent 实时活动
            </h2>
            <p className="mt-1 text-label text-fg-subtle">
              {activityUpdatedAt
                ? `快照更新于 ${date(activityUpdatedAt)}`
                : "正在连接观测服务"}
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Select value={agent} onValueChange={setAgent}>
              <SelectTrigger aria-label="首页筛选 Agent" />
              <SelectContent>
                <SelectItem value="all">全部 Agents</SelectItem>
                {agents.map((a) => (
                  <SelectItem key={a.id} value={a.id}>
                    {a.name}
                  </SelectItem>
                ))}
                {data && !agents.some((a) => a.id === bundledDshAgent.id) && (
                  <SelectItem value={bundledDshAgent.id}>
                    {bundledDshAgent.name}
                  </SelectItem>
                )}
                <SelectItem value="unknown">客户端来源未知</SelectItem>
              </SelectContent>
            </Select>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => navigate("agents")}
            >
              管理 Agents
            </Button>
          </div>
        </div>
        <OverviewAttention
          overview={overview}
          now={activity.now}
          error={observation.error}
          expired={activity.overviewExpired}
          nativeObservationPaused={activity.performance.nativeObservationPaused}
          publishedPage={publishedPage}
          sourceAgents={eligibleSourceAgents.filter(
            (item) =>
              item.found &&
              item.sourceState?.enabled !== false &&
              item.sourceState?.runtimeActive !== false,
          )}
          sourceError={published.error}
        />
        {data ? (
          <>
            <ActivitySummary {...activity.performance} />
            <CurrentActivities activity={activity} />
          </>
        ) : (
          <ConnectionEmpty
            title="连接本机数据后查看活动"
            description="请在桌面应用中连接本地观测服务。"
          />
        )}
      </section>
      <div className="space-y-4">
        {!sourceAgentSelected && (
          <section
            className="@container min-w-0 space-y-3 rounded-xl border border-border bg-surface-floating p-4"
            aria-labelledby="analysis-title"
            aria-busy={observation.updating}
          >
            <div className="flex flex-wrap items-start justify-between gap-3">
              <h2 id="analysis-title" className="text-title font-medium">
                调用趋势与占比
              </h2>
              <div className="flex flex-wrap gap-2">
                <Select
                  value={source}
                  onValueChange={(v) => setSource(v as OverviewQuery["source"])}
                >
                  <SelectTrigger aria-label="调用统计来源" />
                  <SelectContent>
                    <SelectItem value="native">原生响应记录</SelectItem>
                    <SelectItem value="gateway">网关请求</SelectItem>
                  </SelectContent>
                </Select>
                <Select value={range} onValueChange={setRange}>
                  <SelectTrigger aria-label="时间范围" />
                  <SelectContent>
                    {[
                      ["6", "最近 6 小时"],
                      ["12", "最近 12 小时"],
                      ["24", "最近 24 小时"],
                      ["168", "最近 7 天"],
                    ].map(([value, label]) => (
                      <SelectItem key={value} value={value}>
                        {label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div className="flex flex-wrap items-center justify-between gap-3">
              <Tabs
                value={metric}
                onValueChange={(v) => setMetric(v as OverviewQuery["metric"])}
                color="default"
                variant="segment"
              >
                <TabsList aria-label="分析指标">
                  {Object.entries(metricLabels).map(([value, label]) => (
                    <TabItem key={value} value={value} label={label} />
                  ))}
                </TabsList>
              </Tabs>
              <Select
                value={dimension}
                onValueChange={(v) =>
                  setDimension(v as OverviewQuery["dimension"])
                }
              >
                <SelectTrigger aria-label="趋势分组" />
                <SelectContent>
                  <SelectItem value="agent">按 Agent</SelectItem>
                  <SelectItem value="model">按模型</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {observation.updating && (
              <p className="text-label text-fg-subtle" role="status">
                {observation.error
                  ? "统计更新失败，保留上次结果"
                  : "正在更新统计，保留上次结果"}
              </p>
            )}
            {overview ? (
              <CallAnalysisCharts analysis={overview.analysis} />
            ) : (
              <ConnectionEmpty
                title={
                  observation.error ? "暂时无法读取分析" : "正在读取调用分析"
                }
              />
            )}
          </section>
        )}
        {!sourceAgentSelected && (
          <Accordion className="w-full" type="multiple">
            <AccordionItem value="statistics">
              <AccordionTrigger>会话与请求统计</AccordionTrigger>
              <AccordionContent>
                <TimingSummary overview={overview} range={range} />
                <Button
                  variant="tertiary"
                  size="sm"
                  onClick={() => navigate("requests")}
                >
                  查看计时样本与请求
                </Button>
              </AccordionContent>
            </AccordionItem>
          </Accordion>
        )}
      </div>
      <Accordion className="w-full" type="multiple">
        <AccordionItem value="agents">
          <AccordionTrigger>Agent 连接与历史指标</AccordionTrigger>
          <AccordionContent>
            {data && (
              <div
                className={`grid items-start gap-4 ${agent === "all" ? "xl:grid-cols-3" : ""}`}
              >
                {data.agents
                  .filter((a) => agent === "all" || a.id === agent)
                  .map((a) => (
                    <AgentActivity
                      key={a.id}
                      agent={a}
                      data={data}
                      overview={overview}
                      stale={
                        activity.overviewExpired || Boolean(observation.error)
                      }
                    />
                  ))}
                <UnattributedActivities
                  overview={overview}
                  stale={activity.overviewExpired || Boolean(observation.error)}
                  filter={agent}
                />
              </div>
            )}
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </div>
  );
}

function Metric({
  label,
  value,
  unit,
}: {
  label: string;
  value: string;
  unit: string;
}) {
  return (
    <div>
      <dt className="text-label text-fg-subtle">{label}</dt>
      <dd className="mt-1 text-title">
        {value} <span className="text-label text-fg-subtle">{unit}</span>
      </dd>
    </div>
  );
}
function TimingSummary({
  overview,
  range,
}: {
  overview: Overview | null;
  range: string;
}) {
  const stats = overview?.timings;
  return (
    <div className="mb-4 space-y-3">
      <p className="text-label text-fg-muted">
        最近 {range === "168" ? "7 天" : `${range} 小时`} · 当前 Agent 筛选 ·
        仅成功且可测的网关请求。原生轮次首出与请求计时分开保留。
      </p>
      <dl className="grid gap-4 sm:grid-cols-3">
        <Metric
          label={`成功 TTFT P50 · ${stats?.ttftSamples ?? 0} 样本`}
          value={seconds(stats?.ttftP50Ms)}
          unit="秒"
        />
        <Metric
          label="成功 TTFT P95"
          value={seconds(stats?.ttftP95Ms)}
          unit="秒"
        />
        <Metric
          label={`端到端吞吐 P50 · ${stats?.throughputSamples ?? 0} 样本`}
          value={speed(stats?.throughputP50)}
          unit="Token/s"
        />
      </dl>
      <p className="text-label text-fg-subtle">
        {(stats?.ttftSamples ?? 0) < 30
          ? "计时样本不足 30 条，不足以代表稳定分布。"
          : ""}{" "}
        端到端吞吐使用报告的输出 Token /
        请求总时延，包含首出等待；不等同于当前文本速度。
        {stats?.retryGroupsUnmeasurable
          ? ` ${stats.retryGroupsUnmeasurable} 组跨请求重试未提供统一单调时钟，已排除计时分布。`
          : ""}
      </p>
    </div>
  );
}
