import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "@/components/ui/accordion";
import { Button } from "@/components/ui/button";
import { MetricCard } from "@/components/ui/metric-card";
import { ChartLegend, SegmentedBar } from "@/components/ui/chart-primitives";
import { providerCosts } from "@/features/providers/usage-display";
import { number } from "@/features/sessions/display";
import {
  durationLabel,
  requestStates,
  requestIssues,
} from "../request-display";
import type { RequestSummary } from "../request-types";
import { compactNumber } from "@/shared/lib/format";

export function RequestMetrics({
  summary: s,
  nativeView = false,
  onResult,
  onIssue,
}: {
  summary: RequestSummary;
  nativeView?: boolean;
  onResult: (status: string) => void;
  onIssue: (issue: string) => void;
}) {
  const cards = [
    {
      label: nativeView ? "模型响应记录" : "请求量",
      value: compactNumber(s.requests),
      exact: number(s.requests),
      note: nativeView
        ? "逐条响应，不代表已确认 HTTP 请求"
        : `${s.attempts} 次尝试 · ${s.provisional} 条进行中`,
    },
    {
      label: nativeView ? "HTTP 成功率" : "成功率",
      value:
        s.successRate === null ? "—" : `${(s.successRate * 100).toFixed(2)}%`,
      note: nativeView
        ? "原生结果证据不完整，不推断 HTTP 成功率"
        : `${s.statuses.failed ?? 0} 条最终失败 · ${s.statuses.recovered ?? 0} 条已恢复`,
    },
    {
      label: "Token",
      value: compactNumber(s.usage.totalTokens),
      exact: number(s.usage.totalTokens),
      note: `输入 ${compactNumber(s.usage.inputTokens)} · 输出 ${compactNumber(s.usage.outputTokens)}${s.usageMissing ? ` · ${s.usageMissing} 条缺失` : ""}`,
    },
    {
      label: "费用",
      value: nativeView ? "未知" : providerCosts(s.cost, true),
      exact: providerCosts(s.cost),
      note: nativeView
        ? `${s.cost.unknownCalls} 次计价未知，缺少历史账单证据`
        : "实际与估算分列 · 未知次数按尝试统计",
    },
    {
      label: "首次输出 P50",
      value: durationLabel(s.ttft.p50),
      note: `P95 ${durationLabel(s.ttft.p95)} · ${s.ttft.samples} 个样本${s.ttft.samples < 30 ? " · 样本不足" : ""}`,
    },
    {
      label: "端到端输出吞吐 P50",
      value:
        s.throughput.p50 === null
          ? "—"
          : `${s.throughput.p50.toFixed(1)} Token/s`,
      note: `${s.throughput.samples} 个有效样本${s.throughput.samples < 30 ? " · 样本不足" : ""}`,
    },
  ];
  const primary = [
    cards[0],
    cards[2],
    nativeView
      ? {
          label: "缓存读取占比",
          value:
            s.cacheRatio === null
              ? "不可计算"
              : `${(s.cacheRatio * 100).toFixed(1)}%`,
          note: `${s.cacheSamples} 条有效响应 · 未知字段不补零`,
        }
      : cards[1],
    cards[3],
  ];
  const results = Object.entries(requestStates).map(([id, label]) => ({
    id,
    label,
    value: s.statuses[id] ?? 0,
    color:
      id === "failed"
        ? "var(--chart-7)"
        : ["success", "recovered"].includes(id)
          ? "var(--chart-4)"
          : id === "unknown" || id === "cancelled"
            ? "var(--chart-6)"
            : "var(--chart-2)",
  }));
  return (
    <section aria-label="请求指标概览" className="space-y-3">
      <div className="grid min-w-0 gap-3 sm:grid-cols-2 xl:grid-cols-4">
        {primary.map((c) => (
          <MetricCard
            key={c.label}
            label={c.label}
            value={c.value}
            title={"exact" in c ? c.exact : undefined}
            footer={c.note}
            valueClassName="whitespace-normal break-words"
          />
        ))}
      </div>
      <SegmentedBar
        mode="distribution"
        total={s.requests}
        segments={results}
        valueText={`当前筛选结果分布：${Object.entries(s.statuses)
          .map(([id, value]) => `${requestStates[id] ?? id} ${value}`)
          .join("，")}；共 ${s.requests} 条${nativeView ? "响应记录" : "请求"}`}
      />
      <ChartLegend
        aria-label="请求结果筛选"
        className="flex flex-wrap gap-x-5 gap-y-3"
        items={results.map((result) => ({
          id: result.id,
          label: `${result.label} ${result.value}`,
          color: result.color,
        }))}
        onSelect={onResult}
      />
      <div className="flex flex-wrap gap-2" aria-label="异常与结果下钻">
        {Object.entries(s.issues).map(([issue, n]) => (
          <Button
            key={issue}
            size="sm"
            variant="tertiary"
            onClick={() => onIssue(issue)}
          >
            {requestIssues[issue] ?? issue} {n} 次尝试
          </Button>
        ))}
        <Button
          size="sm"
          variant="ghost"
          onClick={() => onIssue("unmeasurable")}
        >
          查看不可测请求
        </Button>
      </div>
      <Accordion className="w-full" type="multiple">
        <AccordionItem value="definitions">
          <AccordionTrigger>指标定义与统计范围</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-2 text-label text-fg-muted">
              <dl className="grid gap-4 pb-2 sm:grid-cols-3">
                {[cards[4], cards[5], cards[1]].map((c) => (
                  <div key={c.label} className="space-y-1">
                    <dt>{c.label}</dt>
                    <dd className="text-body font-medium tabular-nums text-fg-default">
                      {c.value}
                    </dd>
                    <dd>{c.note}</dd>
                  </div>
                ))}
              </dl>
              <p>
                缓存读取 {number(s.usage.cachedInputTokens)} / 写入{" "}
                {number(s.usage.cacheWriteInputTokens)} · 总耗时 P95{" "}
                {durationLabel(s.duration.p95)} · {s.duration.samples} 个样本
                {s.duration.samples < 30 ? " · 样本不足" : ""}
              </p>
              <p>
                以上均为当前筛选。网关按逻辑请求计数，尝试数包含重试；原生按逐条模型响应记录计数，不含普通消息、工具与会话累计量。已关联视图每次消费只使用一份网关证据。成功率
                = 成功与已恢复 /
                成功、已恢复与最终失败；取消、进行中及结果未知另列。
              </p>
              <p>
                TTFT
                从网关收齐请求体到首个有效生成输出，工具参数或公开推理可能先于正文；响应头和心跳不算。吞吐
                = 最终响应输出 Token / 完整请求耗时，包含可确认的重试与退避。
              </p>
              <p>
                P50 / P95 只使用成功、已恢复且可测的终止请求，少于 30
                个样本时提示不足。未知值与真实零值分别显示；Token、费用可能仅包含已知部分。
              </p>
              <p>
                缓存读取占比按缓存字段完整的尝试加权：缓存读取 Token 总量 / 输入
                Token 总量；缓存与推理子集不重复累加。费用沿用 Provider
                的实际扣费或有版本估算规则，订阅不折算费用，不同币种不直接相加。
              </p>
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </section>
  );
}
