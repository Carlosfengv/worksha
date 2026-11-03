import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import type { Page, Snapshot } from "@/app/types";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
import { phaseNames, seconds, speed } from "@/features/observation/display";
import { useOverview } from "@/features/observation/hooks/use-overview";
import type { Overview, OverviewQuery } from "@/features/observation/types";
import { date } from "@/features/sessions/display";
import { recordAgentKey } from "@/shared/types/agent";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useMemo, useState } from "react";
import {
  AgentActivity,
  UnattributedActivities,
  activityRows,
} from "./components/agent-activity";
import { CurrentActivities } from "./components/current-activities";
import { requestStates } from "./request-display";
import { activityGroup } from "./activity-group";

export function OverviewPage({
  data,
  navigate,
}: {
  data: Snapshot | null;
  navigate: (page: Page) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const [agent, setAgent] = useState("all");
  const [range, setRange] = useState("24");
  const [metric, setMetric] = useState<OverviewQuery["metric"]>("calls");
  const [dimension, setDimension] =
    useState<OverviewQuery["dimension"]>("agent");
  const [source, setSource] = useState<OverviewQuery["source"]>("native");
  const observation = useOverview({
    agent: agent === "all" ? undefined : agent,
    range: Number(range),
    metric,
    dimension,
    source,
  });
  // Source consent can change while the last overview is cached after a read
  // failure. Remove native excerpts before any card, table or popover sees it.
  const overview = useMemo(
    () =>
      observation.data
        ? {
            ...observation.data,
            nativeActivities: observation.data.nativeActivities.filter(
              (activity) =>
                data?.sessionSources.some(
                  (source) =>
                    source.enabled && source.agentId === activity.agentId,
                ),
            ),
          }
        : null,
    [observation.data, data?.sessionSources],
  );
  const stale =
    Boolean(observation.error) ||
    (overview ? Date.now() - Date.parse(overview.asOf) > 6000 : false);
  const gateways = overview?.gatewayActivities ?? [];
  const natives = overview?.nativeActivities ?? [];
  const products = new Set([
    ...gateways.map((a) => recordAgentKey(a.request, data?.agents)),
    ...natives.map((a) => recordAgentKey(a, data?.agents)),
  ]);
  const groups = [...products].map((id) =>
    activityGroup(id, gateways, natives, stale, Date.now(), data?.agents),
  );
  const events = activityRows(
    {
      ...activityGroup("", [], [], stale),
      gateway: groups.flatMap((g) => g.gateway),
      native: groups.flatMap((g) => g.native),
      activeGateway: groups.flatMap((g) => g.activeGateway),
      activeNative: groups.flatMap((g) => g.activeNative),
      freshNative: groups.flatMap((g) => g.freshNative),
    },
    "recent",
  )
    .filter(
      (a) =>
        !Number.isFinite(Date.parse(a.time)) ||
        Date.now() - Date.parse(a.time) <= 3 * 60 * 60 * 1000,
    )
    .sort(
      (a, b) =>
        (b.messageAt ?? "").localeCompare(a.messageAt ?? "") ||
        a.id.localeCompare(b.id),
    )
    .map((a) => ({
      id: a.id,
      time: a.messageAt ?? a.requestAt ?? a.time,
      agent: recordAgentKey(a, data?.agents),
      text: `${a.title} · ${a.expired ? "状态待更新" : (phaseNames[a.phase] ?? requestStates[a.phase] ?? a.phase)}${a.step ? ` · ${a.step}` : ""}`,
      session: a.sessionId,
      source: a.source,
      href: a.href,
    }))
    .slice(0, 12);
  return (
    <div className="flex min-w-0 flex-col gap-6">
      <section className="space-y-3" aria-labelledby="activity-title">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 id="activity-title" className="text-title font-medium">
              Agent 实时活动
            </h2>
            <p className="mt-1 text-label text-fg-subtle">
              最近 3 小时活动，每个 Session 仅显示最新消息，按消息发起时间排序 ·{" "}
              {overview ? `更新于 ${date(overview.asOf)}` : "正在连接观测服务"}
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Select value={agent} onValueChange={setAgent}>
              <SelectTrigger aria-label="首页筛选 Agent" />
              <SelectContent>
                <SelectItem value="all">全部 Agents</SelectItem>
                {data?.agents.map((a) => (
                  <SelectItem key={a.id} value={a.id}>
                    {a.name}
                  </SelectItem>
                ))}
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
        {data ? (
          <CurrentActivities
            data={data}
            overview={overview}
            stale={stale}
            filter={agent}
            error={observation.error}
            latest={events[0]}
          />
        ) : (
          <ConnectionEmpty
            title="连接本机数据后查看活动"
            description="请在桌面应用中连接本地观测服务。"
          />
        )}
        {(observation.error ||
          overview?.collector.error ||
          overview?.collector.persistenceError) && (
          <Alert>
            <AlertTitle>观测数据需要检查</AlertTitle>
            <AlertDescription>
              {observation.error ??
                overview?.collector.persistenceError ??
                overview?.collector.error}{" "}
              {stale ? "保留最后快照，当前活动已标记过期。" : ""}
            </AlertDescription>
          </Alert>
        )}
      </section>
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
                      stale={stale}
                    />
                  ))}
                <UnattributedActivities
                  overview={overview}
                  stale={stale}
                  filter={agent}
                />
              </div>
            )}
          </AccordionContent>
        </AccordionItem>
      </Accordion>
      <section
        className="space-y-4"
        aria-labelledby="analysis-title"
        aria-busy={observation.updating}
      >
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 id="analysis-title" className="text-title font-medium">
              调用趋势与占比
            </h2>
          </div>
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
            onValueChange={(v) => setDimension(v as OverviewQuery["dimension"])}
          >
            <SelectTrigger aria-label="趋势分组" />
            <SelectContent>
              <SelectItem value="agent">按 Agent</SelectItem>
              <SelectItem value="model">按模型</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <p className="text-label text-fg-subtle" role="status">
          {observation.updating
            ? observation.error
              ? "统计更新失败，保留上次结果"
              : "正在更新统计，保留上次结果"
            : null}
        </p>
        {overview ? (
          <CallAnalysisCharts
            analysis={overview.analysis}
            onGroup={(groupDimension, id) => {
              const query = new URLSearchParams({
                source: overview.analysis.query.source,
                metric: overview.analysis.query.metric,
                dimension: groupDimension,
                range: "custom",
                start: overview.analysis.windowStart,
                end: overview.analysis.asOf,
                [groupDimension]: id,
              });
              if (groupDimension !== "agent" && agent !== "all")
                query.set("agent", agent);
              window.location.hash = `/requests?${query}`;
            }}
          />
        ) : (
          <ConnectionEmpty
            title={observation.error ? "暂时无法读取分析" : "正在读取调用分析"}
          />
        )}
      </section>
      <section className="space-y-3">
        <h2 className="text-title font-medium">最近活动</h2>
        {events.length ? (
          <ul className="divide-y divide-border rounded-xl border border-border">
            {events.map((e) => (
              <li
                key={`${e.source}:${e.id}`}
                className="flex flex-wrap items-center justify-between gap-2 p-3"
              >
                <div className="min-w-0 flex-1">
                  <p className="truncate text-body" title={e.text}>
                    {e.text}
                  </p>
                  <p className="mt-1 text-label text-fg-subtle">
                    {agentNames[e.agent]} · {e.source} · {date(e.time)}
                  </p>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    window.location.hash = e.href;
                  }}
                >
                  {e.session ? "查看会话" : "查看请求"}
                </Button>
              </li>
            ))}
          </ul>
        ) : (
          <ConnectionEmpty compact title="等待新的 Agent 事件" />
        )}
      </section>
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
