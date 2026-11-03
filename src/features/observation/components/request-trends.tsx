import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { LineChart, Line } from "@/components/ui/line-chart";
import { ChartLegend } from "@/components/ui/chart-primitives";
import { MetricCard } from "@/components/ui/metric-card";
import { UsageRanking } from "@/shared/ui/usage-ranking";
import { Grid, XAxis, YAxis, ChartTooltip } from "@/components/ui/chart-core";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableHeader,
  TableBody,
  TableHead,
  TableRow,
  TableCell,
} from "@/components/ui/table";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "@/components/ui/accordion";
import { curveLinear } from "@visx/curve";
import { useMemo } from "react";
import { date } from "@/features/sessions/display";
import { chartColor, chartDash } from "@/shared/lib/format";
import type { RequestDashboard } from "../request-types";
import { requestMetrics, requestValue } from "../request-display";
import { requestAxisLabel } from "../request-visualization";

export function RequestTrends({
  data,
  onChange,
  onBucket,
  onGroup,
}: {
  data: RequestDashboard;
  onChange: (patch: Record<string, string>) => void;
  onBucket: (start: string, end: string) => void;
  onGroup: (id: string) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const metric = data.query.metric ?? "calls",
    dimension = data.query.dimension ?? "agent";
  const metricLabel =
    metric === "calls" && data.query.source === "native"
      ? "模型响应记录"
      : requestMetrics[metric];
  const performance = ["ttft", "duration", "throughput"].includes(metric);
  const series = useMemo(
    () => (performance ? [] : data.groups.slice(0, 5)),
    [data.groups, performance],
  );
  const chart = useMemo(
    () =>
      data.buckets.map((b) => {
        const point: Record<string, unknown> = {
          date: new Date(b.start),
          value: b.samples > 0 ? b.value : null,
          p95: b.samples > 0 ? b.p95 : null,
        };
        series.forEach((g, index) => {
          const group = b.groups.find((v) => v.id === g.id);
          point[`group${index}`] = group
            ? group.value
            : b.samples === 0 ||
                ["ttft", "duration", "throughput"].includes(metric)
              ? null
              : 0;
        });
        return point;
      }),
    [data.buckets, metric, series],
  );
  const hasValue = data.buckets.some((b) => b.samples > 0 && b.value !== null);
  const showMarkers =
    data.buckets.filter((bucket) => bucket.samples > 0 && bucket.value !== null)
      .length <= 8;
  const distribution =
    metric === "ttft"
      ? data.summary.ttft
      : metric === "duration"
        ? data.summary.duration
        : data.summary.throughput;
  return (
    <section
      className="min-w-0 space-y-4 rounded-xl border border-border p-4"
      aria-label="请求趋势分析"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-title font-medium">趋势分析</h2>
        <div className="flex flex-wrap gap-2">
          <Select
            value={metric}
            onValueChange={(v) => onChange({ metric: v ?? "calls" })}
          >
            <SelectTrigger aria-label="趋势指标" />
            <SelectContent>
              {Object.entries(requestMetrics).map(([id, label]) => (
                <SelectItem key={id} value={id}>
                  {id === "calls" && data.query.source === "native"
                    ? "模型响应记录"
                    : label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select
            value={dimension}
            onValueChange={(v) => onChange({ dimension: v ?? "agent" })}
          >
            <SelectTrigger aria-label="拆分维度" />
            <SelectContent>
              <SelectItem value="agent">按 Agent</SelectItem>
              <SelectItem value="model">按模型</SelectItem>
              <SelectItem value="provider">按 Provider</SelectItem>
            </SelectContent>
          </Select>
          {metric === "cost" && (
            <Select
              value={data.query.currency ?? "USD"}
              onValueChange={(v) => onChange({ currency: v ?? "USD" })}
            >
              <SelectTrigger aria-label="费用币种" />
              <SelectContent>
                {[
                  ...new Set([
                    "USD",
                    ...data.summary.cost.actual.map((c) => c.currency),
                    ...data.summary.cost.estimated.map((c) => c.currency),
                  ]),
                ].map((c) => (
                  <SelectItem key={c} value={c}>
                    {c}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        </div>
      </div>
      {performance && (
        <div className="grid gap-3 sm:grid-cols-2">
          <MetricCard
            label={`${metricLabel} P50`}
            value={requestValue(distribution.p50, metric)}
            footer={`${distribution.samples} 个有效样本 · 当前筛选`}
          />
          <MetricCard
            label={`${metricLabel} P95`}
            value={requestValue(distribution.p95, metric)}
            footer={
              distribution.samples < 30
                ? "样本不足 30 条，请结合明细判断"
                : "95% 的有效样本不超过此值"
            }
          />
        </div>
      )}
      <div className="grid min-w-0 gap-5 xl:grid-cols-3">
        <div
          className="min-w-0 xl:col-span-2"
          role="group"
          aria-label={`${metricLabel}趋势；可在时段明细中下钻`}
        >
          {hasValue ? (
            <LineChart
              data={chart}
              style={{ height: 260 }}
              margin={{ top: 16, right: 16, left: 64, bottom: 40 }}
              animationDuration={0}
              yDomainTween={false}
              yDomain={[0, "dataMax"]}
            >
              <Grid />
              <XAxis
                numTicks={4}
                formatDate={(d) =>
                  d.toLocaleString(DISPLAY_LOCALE, {
                    timeZone: DISPLAY_TIME_ZONE,
                    month: "numeric",
                    day: "numeric",
                    hour: "2-digit",
                  })
                }
              />
              <YAxis
                numTicks={4}
                formatValue={(v) => requestAxisLabel(v, metric)}
              />
              <Line
                dataKey="value"
                stroke="var(--chart-1)"
                curve={curveLinear}
                animate={false}
                fadeEdges={false}
                showMarkers={showMarkers}
              />
              {["ttft", "duration"].includes(metric) && (
                <Line
                  dataKey="p95"
                  stroke="var(--chart-3)"
                  curve={curveLinear}
                  dashFromIndex={0}
                  dashArray="5,4"
                  animate={false}
                  fadeEdges={false}
                  showMarkers={showMarkers}
                />
              )}
              {series.map((g, index) => (
                <Line
                  key={g.id}
                  dataKey={`group${index}`}
                  stroke={chartColor(g.id)}
                  dashFromIndex={index > 0 ? 0 : undefined}
                  dashArray={chartDash(index)}
                  curve={curveLinear}
                  animate={false}
                  fadeEdges={false}
                />
              ))}
              <ChartTooltip
                showDatePill={false}
                content={({ point }) => (
                  <div className="space-y-1 p-2 text-label">
                    <p>{date((point.date as Date).toISOString())}</p>
                    <p>
                      {metricLabel}{" "}
                      {requestValue(point.value as number | null, metric)}{" "}
                      {metric === "cost" ? (data.query.currency ?? "USD") : ""}
                    </p>
                    {series.map((g, index) => (
                      <p key={g.id}>
                        {agentNames[g.label] ?? g.label}：
                        {requestValue(
                          point[`group${index}`] as number | null,
                          metric,
                        )}
                      </p>
                    ))}
                    {["ttft", "duration"].includes(metric) && (
                      <p>
                        P95 {requestValue(point.p95 as number | null, metric)}
                      </p>
                    )}
                  </div>
                )}
              />
            </LineChart>
          ) : (
            <p className="py-16 text-center text-body text-fg-muted">
              当前筛选没有可测的{metricLabel}样本
            </p>
          )}
          <ChartLegend
            className="mb-3 flex flex-wrap gap-3"
            items={[
              {
                id: "total",
                label: performance ? "P50 · 全部有效样本" : "总量 / 全部样本",
                color: "var(--chart-1)",
              },
              ...(["ttft", "duration"].includes(metric)
                ? [{ id: "p95", label: "P95 · 虚线", color: "var(--chart-3)" }]
                : []),
              ...series.map((g) => ({
                id: g.id,
                label: agentNames[g.label] ?? g.label,
                color: chartColor(g.id),
              })),
            ]}
          />
          <p className="text-label text-fg-subtle">
            {["ttft", "duration"].includes(metric)
              ? "实线 P50 · 虚线 P95；不足 30 个样本请结合明细判断。"
              : metric === "throughput"
                ? "各时段成功请求的端到端吞吐 P50，非纯模型解码速度。"
                : metric === "cost"
                  ? `已知 ${data.query.currency ?? "USD"} 费用，包含实际与估算；未知费用不补零。`
                  : "按网关请求开始 / 原生记录时间归桶；空桶表示当前已记录集合没有样本，不能据此判定无请求。"}
          </p>
        </div>
        <UsageRanking
          title={`${dimension === "agent" ? "Agent" : dimension === "model" ? "模型" : "Provider"}分组`}
          description="当前筛选的聚合值；点击名称下钻"
          items={data.groups.map((g) => ({
            id: g.id,
            label: agentNames[g.label] ?? g.label,
            value: g.value,
            color: chartColor(g.id),
            detail: `${g.samples} ${performance ? "个有效样本" : data.query.source === "native" ? "条响应记录" : "条请求"}${g.missing ? ` · ${g.missing} 次缺失` : ""}`,
          }))}
          formatValue={(value) =>
            `${requestValue(value, metric)}${metric === "cost" ? ` ${data.query.currency ?? "USD"}` : ""}`
          }
          onSelect={onGroup}
        />
      </div>
      <Accordion className="w-full" type="multiple">
        <AccordionItem value="buckets">
          <AccordionTrigger>时段明细与下钻</AccordionTrigger>
          <AccordionContent>
            <div className="max-h-80 overflow-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>时段</TableHead>
                    <TableHead>{metricLabel}</TableHead>
                    <TableHead>样本 / 缺失</TableHead>
                    <TableHead>查看</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {data.buckets.map((b) => (
                    <TableRow key={b.start}>
                      <TableCell>{date(b.start)}</TableCell>
                      <TableCell>{requestValue(b.value, metric)}</TableCell>
                      <TableCell>
                        {b.samples} / {b.missing}
                      </TableCell>
                      <TableCell>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => onBucket(b.start, b.end)}
                        >
                          查看此时段
                        </Button>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
      <Accordion className="w-full" type="multiple">
        <AccordionItem value="grouping">
          <AccordionTrigger>分组与费用口径</AccordionTrigger>
          <AccordionContent>
            <p className="text-label text-fg-subtle">
              原生按记录模型 / Provider
              字段分组，不据当前配置推断历史路由。网关请求数按首次路由分组，旧记录缺少独立路由字段时使用记录模型；跨模型
              / Provider 的 Token 和费用按实际尝试分别归属。Provider
              或模型筛选选择的是相关逻辑请求，概览保留这些请求的完整尝试。
            </p>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </section>
  );
}
import { DISPLAY_LOCALE, DISPLAY_TIME_ZONE } from "@/shared/lib/date-time";
