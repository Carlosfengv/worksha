import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { ChartTooltip, Grid, YAxis } from "@/components/ui/chart-core";
import { Bar, BarChart, BarXAxis } from "@/components/ui/bar-chart";
import { SegmentedBar } from "@/components/ui/chart-primitives";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import type { AnalysisValue, CallAnalysis } from "@/features/observation/types";
import { date, number } from "@/features/sessions/display";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { memo, useMemo } from "react";
import { chartColor, compactNumber } from "@/shared/lib/format";
import { summaryMoney } from "@/shared/lib/money";
import { DISPLAY_LOCALE, DISPLAY_TIME_ZONE } from "@/shared/lib/date-time";

export const metricLabels = {
  calls: "调用次数",
  tokens: "Token 用量",
  cost: "费用",
};
export function displayValue(
  value: number | null,
  metric: string,
  compact = false,
) {
  if (value === null) return "—";
  if (metric === "cost") {
    const amount = value / 1_000_000;
    if (compact) return summaryMoney(amount, "USD");
    if (amount > 0 && amount < 1e-9) return "< $0.000000001";
    return `$${amount.toLocaleString("en-US", { maximumFractionDigits: 9 })}`;
  }
  return compact ? compactNumber(value) : number(value);
}
const groupedTailColor = chartColor("其他");

function chartGroups(groups: AnalysisValue[]) {
  const count = groups.length <= 6 ? groups.length : 5;
  const series: { id: string | null; members: string[] }[] = groups
    .slice(0, count)
    .map(({ id }) => ({ id, members: [id] }));
  if (groups.length > count)
    series.push({ id: null, members: groups.slice(count).map(({ id }) => id) });
  return series;
}

function groupHref(analysis: CallAnalysis, id: string) {
  const { agent, dimension, metric, source } = analysis.query;
  const query = new URLSearchParams({
    source,
    metric,
    dimension,
    range: "custom",
    start: analysis.windowStart,
    end: analysis.asOf,
    [dimension]: id,
  });
  if (dimension === "model" && agent) query.set("agent", agent);
  return `#/requests?${query}`;
}

export const CallAnalysisCharts = memo(function CallAnalysisCharts({
  analysis,
}: {
  analysis: CallAnalysis;
}) {
  const { agentNames } = useAgentDirectory();
  const { dimension, metric, source, range } = analysis.query;
  const values = dimension === "agent" ? analysis.agents : analysis.models;
  const ranked = useMemo(
    () =>
      [...values].sort(
        (a, b) => (b.value ?? -1) - (a.value ?? -1) || a.id.localeCompare(b.id),
      ),
    [values],
  );
  const groups = useMemo(() => chartGroups(ranked), [ranked]);
  const chart = useMemo(
    () =>
      analysis.buckets.map((bucket) => {
        const values: Record<string, unknown> = {
          start: bucket.start,
          total: bucket.value,
        };
        groups.forEach((g, i) => {
          const items = bucket.groups.filter((v) => g.members.includes(v.id));
          const known = items.filter((v) => v.value !== null);
          values[`g${i}`] = known.length
            ? known.reduce((n, v) => n + v.value!, 0)
            : items.length
              ? null
              : 0;
        });
        return values;
      }),
    [analysis, groups],
  );
  const label = metricLabels[metric];
  const groupLabel = dimension === "agent" ? "Agent" : "模型";
  const name = (id: string | null) =>
    id === null
      ? "其余分组"
      : dimension === "agent" && Object.hasOwn(agentNames, id)
        ? agentNames[id]
        : id;
  const formatBucket = (start: string) =>
    new Date(start).toLocaleString(DISPLAY_LOCALE, {
      timeZone: DISPLAY_TIME_ZONE,
      ...(range === 168
        ? { month: "numeric", day: "numeric" }
        : { hour: "2-digit", minute: "2-digit" }),
    });
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 text-label text-fg-muted">
        <p>
          {source === "native" && metric === "calls"
            ? "已识别模型响应"
            : `已知${label}`}{" "}
          <strong
            className="font-medium text-fg-default tabular-nums"
            title={displayValue(analysis.total, metric)}
          >
            {displayValue(analysis.total, metric, true)}
          </strong>
        </p>
        <p>
          {analysis.samples} 条已记录
          {analysis.provisional > 0 && ` · ${analysis.provisional} 条进行中`}
          {analysis.missing > 0 &&
            ` · ${analysis.missing} 条${metric === "cost" ? "费用未知 / 部分" : "用量缺失"}`}
        </p>
        <p className="text-fg-subtle">更新于 {date(analysis.asOf)}</p>
      </div>
      {analysis.query.metric === "cost" && analysis.cost && (
        <p className="text-label text-fg-muted" aria-label="费用构成">
          实际{" "}
          {displayValue(
            analysis.cost.actualUsd === null
              ? null
              : analysis.cost.actualUsd * 1e6,
            "cost",
          )}
          {" · "}估算{" "}
          {displayValue(
            analysis.cost.estimatedUsd === null
              ? null
              : analysis.cost.estimatedUsd * 1e6,
            "cost",
          )}
          {analysis.cost.unknownAttempts > 0
            ? ` · ${analysis.cost.unknownAttempts} 次费用未知`
            : ""}
        </p>
      )}
      <div className="grid min-w-0 gap-4 @lg:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
        <div
          role="group"
          aria-label={`${label}趋势，按${groupLabel}分组；完整数值可在下方展开明细`}
          className="min-w-0"
        >
          {analysis.total !== null &&
          chart.some((bucket) => bucket.total !== null) &&
          groups.length > 0 ? (
            <div className="h-48">
              <BarChart
                data={chart}
                xDataKey="start"
                stacked
                aspectRatio="auto"
                className="h-full"
                margin={{ top: 8, right: 20, left: 48, bottom: 36 }}
                animationDuration={0}
                yDomain={[0, "dataMax"]}
              >
                <Grid numTicksRows={3} />
                <BarXAxis maxLabels={4} formatLabel={formatBucket} />
                <YAxis
                  numTicks={3}
                  formatValue={(value) => displayValue(value, metric, true)}
                />
                {groups.map((group, index) => (
                  <Bar
                    key={group.id === null ? "tail" : `group:${group.id}`}
                    dataKey={`g${index}`}
                    fill={
                      group.id === null
                        ? groupedTailColor
                        : chartColor(group.id)
                    }
                    animate={false}
                  />
                ))}
                <ChartTooltip
                  showDatePill={false}
                  showDots={false}
                  content={({ point }) => (
                    <div className="space-y-1 p-2 text-label">
                      <p>{date(point.start as string)}</p>
                      <p>
                        已知总量{" "}
                        {displayValue(point.total as number | null, metric)}
                      </p>
                      {groups.map((group, index) => (
                        <p
                          key={group.id === null ? "tail" : `group:${group.id}`}
                        >
                          {name(group.id)}{" "}
                          {displayValue(
                            point[`g${index}`] as number | null,
                            metric,
                          )}
                        </p>
                      ))}
                    </div>
                  )}
                />
              </BarChart>
            </div>
          ) : (
            <ConnectionEmpty
              title={
                !analysis.samples
                  ? "暂无已记录调用"
                  : analysis.total === null
                    ? "此指标尚不可测"
                    : "暂无可按时段绘制的记录"
              }
              description={
                analysis.samples && analysis.total === null
                  ? metric === "cost"
                    ? "缺少可核实的价格或用量。"
                    : "缺少用量数据。"
                  : undefined
              }
            />
          )}
        </div>
        <section
          aria-label={`${groupLabel}占比 · ${label}`}
          className="min-w-0 space-y-2 border-t border-border pt-3 @lg:border-t-0 @lg:border-l @lg:pt-0 @lg:pl-4"
        >
          <h3 className="text-body font-medium">{groupLabel}占比</h3>
          <p className="text-label text-fg-subtle">
            按已知总量计算{ranked.length > 6 ? " · 趋势合并尾部分组" : ""}
          </p>
          {ranked.length > 0 ? (
            <ul className="max-h-44 space-y-3 overflow-y-auto overscroll-contain p-1">
              {ranked.map((group, index) => {
                const ratio =
                  group.value !== null &&
                  analysis.total !== null &&
                  analysis.total > 0
                    ? `${((group.value / analysis.total) * 100).toFixed(1)}%`
                    : "—";
                const label = name(group.id);
                return (
                  <li key={group.id} className="min-w-0 space-y-1">
                    <div className="flex items-baseline justify-between gap-2 text-label">
                      <a
                        href={groupHref(analysis, group.id)}
                        className="min-w-0 truncate rounded-sm font-medium underline-offset-4 hover:underline focus-visible:ring-1 focus-visible:ring-focus-ring"
                        title={label}
                      >
                        {label}
                      </a>
                      <span className="shrink-0 text-fg-muted tabular-nums">
                        {displayValue(group.value, metric, true)} · {ratio}
                      </span>
                    </div>
                    <SegmentedBar
                      mode="distribution"
                      total={analysis.total}
                      valueText={`${label} ${ratio}`}
                      className="h-1.5"
                      segments={[
                        {
                          id: group.id,
                          label,
                          value: group.value,
                          color:
                            ranked.length > 6 && index >= 5
                              ? groupedTailColor
                              : chartColor(group.id),
                        },
                      ]}
                    />
                    {group.missing > 0 && (
                      <p className="text-label text-fg-subtle">
                        {group.missing} 条指标缺失
                      </p>
                    )}
                  </li>
                );
              })}
            </ul>
          ) : (
            <p className="text-label text-fg-muted">暂无分组数据</p>
          )}
        </section>
      </div>
      <Accordion className="w-full" type="multiple">
        <AccordionItem value="scope">
          <AccordionTrigger>统计说明</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-2 text-label text-fg-muted">
              <p>
                {analysis.query.source === "native"
                  ? "原生记录按响应时间统计，不代表完整请求量。"
                  : "仅统计经网关的请求，按开始时间统计；重试按已有幂等标识关联。"}
              </p>
              <p>
                来源未知的记录单独列出，共享会话与用量不重复相加。占比按已知总量计算。
              </p>
              {analysis.missingTime > 0 && (
                <p>{analysis.missingTime} 条记录时间未知，未计入趋势。</p>
              )}
              <p>
                费用仅汇总已知 USD，实际与估算分别标明；计价依据与 Providers
                一致。
                未知费用不视为零，订阅额度不折算；原生与网关来源分别统计。
              </p>
            </div>
          </AccordionContent>
        </AccordionItem>
        <AccordionItem value="details">
          <AccordionTrigger>查看时段与分组明细</AccordionTrigger>
          <AccordionContent>
            <div className="max-h-96 overflow-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>时段 / 分组</TableHead>
                    <TableHead>{label}</TableHead>
                    <TableHead>缺失</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {analysis.buckets.flatMap((b) =>
                    b.groups.map((g) => (
                      <TableRow key={`${b.start}:${g.id}`}>
                        <TableCell>
                          {date(b.start)} · {name(g.id)}
                        </TableCell>
                        <TableCell>
                          {displayValue(g.value, analysis.query.metric)}
                        </TableCell>
                        <TableCell>{g.missing}</TableCell>
                      </TableRow>
                    )),
                  )}
                </TableBody>
              </Table>
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </div>
  );
});
