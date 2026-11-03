import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { ChartTooltip, Grid, XAxis, YAxis } from "@/components/ui/chart-core";
import { Area, AreaChart } from "@/components/ui/area-chart";
import { Line } from "@/components/ui/line-chart";
import { ChartLegend } from "@/components/ui/chart-primitives";
import { MetricCard } from "@/components/ui/metric-card";
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
import { curveLinear } from "@visx/curve";
import { memo, useMemo } from "react";
import { chartColor, chartDash, compactNumber } from "@/shared/lib/format";
import { summaryMoney } from "@/shared/lib/money";
import { UsageRanking } from "@/shared/ui/usage-ranking";

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
function name(id: string, agentNames: Record<string, string>) {
  return id in agentNames ? agentNames[id as keyof typeof agentNames] : id;
}
function chartGroups(groups: AnalysisValue[]) {
  const sorted = [...groups].sort(
    (a, b) => (b.value ?? -1) - (a.value ?? -1) || a.id.localeCompare(b.id),
  );
  if (sorted.length <= 6) return sorted.map((g) => ({ ...g, members: [g.id] }));
  const rest = sorted.slice(5);
  const known = rest.filter((g) => g.value !== null);
  return [
    ...sorted.slice(0, 5).map((g) => ({ ...g, members: [g.id] })),
    {
      id: "其他",
      value: known.length ? known.reduce((n, g) => n + g.value!, 0) : null,
      missing: rest.reduce((n, g) => n + g.missing, 0),
      samples: rest.reduce((n, g) => n + g.samples, 0),
      members: rest.map((g) => g.id),
    },
  ];
}
export const CallAnalysisCharts = memo(function CallAnalysisCharts({
  analysis,
  onGroup,
}: {
  analysis: CallAnalysis;
  onGroup?: (dimension: "agent" | "model", id: string) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const groups = useMemo(
    () =>
      chartGroups(
        analysis.query.dimension === "agent"
          ? analysis.agents
          : analysis.models,
      ),
    [analysis],
  );
  const chart = useMemo(
    () =>
      analysis.buckets.map((bucket) => {
        const values: Record<string, unknown> = {
          date: new Date(bucket.start),
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
  const label = metricLabels[analysis.query.metric];
  return (
    <div className="space-y-4">
      <div className="grid min-w-0 gap-3 sm:grid-cols-3">
        <MetricCard
          label={
            analysis.query.source === "native" &&
            analysis.query.metric === "calls"
              ? "已识别模型响应"
              : `已知${label}`
          }
          value={displayValue(analysis.total, analysis.query.metric, true)}
          title={displayValue(analysis.total, analysis.query.metric)}
          footer={
            analysis.query.metric === "cost"
              ? "已知 USD · 实际与估算分别保留"
              : "当前来源与筛选范围"
          }
        />
        <MetricCard
          label="已记录样本"
          value={compactNumber(analysis.samples)}
          footer={`${analysis.provisional} 条进行中 · 不代表完整采集量`}
        />
        <MetricCard
          label="指标缺失记录"
          value={compactNumber(analysis.missing)}
          footer="缺失值保留未知，未计入已知总量"
          tone={analysis.missing > 0 ? "warning" : "default"}
        />
      </div>
      <p className="text-label text-fg-subtle">
        更新于 {date(analysis.asOf)} · {analysis.samples} 条记录
        {analysis.missing > 0
          ? ` · ${analysis.missing} 条${analysis.query.metric === "cost" ? "费用未知 / 部分" : "用量缺失"}`
          : ""}
        {analysis.provisional > 0 ? ` · ${analysis.provisional} 条进行中` : ""}
      </p>
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
      {analysis.samples > 0 && analysis.total !== null ? (
        <>
          <div
            role="group"
            aria-label={`${label}趋势，按${analysis.query.dimension === "agent" ? "Agent" : "模型"}分组；完整数值可在下方展开明细`}
            className="min-w-0 rounded-xl border border-border p-2 sm:p-4"
          >
            <AreaChart
              data={chart}
              style={{ height: 280 }}
              margin={{ top: 16, right: 20, left: 48, bottom: 40 }}
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
                    ...(analysis.query.range === 168
                      ? { month: "numeric", day: "numeric" }
                      : { hour: "2-digit", minute: "2-digit" }),
                  })
                }
              />
              <YAxis
                numTicks={4}
                formatValue={(v) =>
                  displayValue(v, analysis.query.metric, true)
                }
              />
              <Area
                dataKey="total"
                stroke="var(--chart-1)"
                fill="var(--chart-1)"
                fillOpacity={0.12}
                curve={curveLinear}
                animate={false}
                fadeEdges={false}
              />
              {groups.map((g, i) => (
                <Line
                  key={g.id}
                  dataKey={`g${i}`}
                  stroke={chartColor(g.id)}
                  curve={curveLinear}
                  animate={false}
                  fadeEdges={false}
                  dashFromIndex={i > 0 ? 0 : undefined}
                  dashArray={chartDash(i)}
                />
              ))}
              <ChartTooltip
                showDatePill={false}
                content={({ point }) => (
                  <div className="space-y-1 p-2 text-label">
                    <p>{date((point.date as Date).toISOString())}</p>
                    <p>
                      已知总量：
                      {displayValue(
                        point.total as number | null,
                        analysis.query.metric,
                      )}
                    </p>
                    {groups.map((g, i) => (
                      <p key={g.id}>
                        {name(g.id, agentNames)}：
                        {displayValue(
                          point[`g${i}`] as number | null,
                          analysis.query.metric,
                        )}
                      </p>
                    ))}
                  </div>
                )}
              />
            </AreaChart>
            <ChartLegend
              className="flex flex-wrap gap-x-4 gap-y-2"
              items={[
                { id: "total", label: "已知总量", color: "var(--chart-1)" },
                ...groups.map((g) => ({
                  id: g.id,
                  label: name(g.id, agentNames),
                  color: chartColor(g.id),
                })),
              ]}
            />
          </div>
        </>
      ) : (
        <div className="rounded-xl border border-border">
          <ConnectionEmpty
            title={analysis.samples ? "此指标尚不可测" : "暂无已记录调用"}
            description={
              analysis.samples
                ? analysis.query.metric === "cost"
                  ? "缺少可核实的价格或用量。"
                  : "缺少用量数据。"
                : undefined
            }
          />
        </div>
      )}
      <div className="grid min-w-0 gap-4 lg:grid-cols-2">
        <Share
          title="Agent"
          dimension="agent"
          values={analysis.agents}
          analysis={analysis}
          onGroup={onGroup}
        />
        <Share
          title="模型"
          dimension="model"
          values={analysis.models}
          analysis={analysis}
          onGroup={onGroup}
        />
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
                          {date(b.start)} · {name(g.id, agentNames)}
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
function Share({
  title,
  dimension,
  values,
  analysis,
  onGroup,
}: {
  title: string;
  dimension: "agent" | "model";
  values: AnalysisValue[];
  analysis: CallAnalysis;
  onGroup?: (dimension: "agent" | "model", id: string) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const total = analysis.total;
  return (
    <UsageRanking
      title={`${title}占比 · ${metricLabels[analysis.query.metric]}`}
      description="占比基于已知总量；点击分组查看同范围明细"
      items={values.map((g) => ({
        id: g.id,
        label: name(g.id, agentNames),
        value: g.value,
        color: chartColor(g.id),
        detail: g.missing ? `${g.missing} 条指标缺失` : undefined,
        ratio:
          g.value !== null && total !== null && total > 0
            ? `${((g.value / total) * 100).toFixed(1)}%`
            : "—",
      }))}
      formatValue={(v) => displayValue(v, analysis.query.metric, true)}
      onSelect={onGroup ? (id) => onGroup(dimension, id) : undefined}
    />
  );
}
import { DISPLAY_LOCALE, DISPLAY_TIME_ZONE } from "@/shared/lib/date-time";
