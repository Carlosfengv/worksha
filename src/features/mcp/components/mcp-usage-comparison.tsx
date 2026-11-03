import { ChartLegend, SegmentedBar } from "@/components/ui/chart-primitives";
import { MetricCard } from "@/components/ui/metric-card";
import { chartColor, compactNumber } from "@/shared/lib/format";
import { UsageRanking } from "@/shared/ui/usage-ranking";
import type { McpGroup } from "../types";

export function McpUsageComparison({
  rows,
  onOpen,
}: {
  rows: McpGroup[];
  onOpen: (id: string) => void;
}) {
  const known = rows.filter((group) => group.callCount !== null);
  const calls = known.reduce((sum, group) => sum + group.callCount!, 0);
  const results = [
    {
      id: "returned",
      label: "已返回",
      value: known.reduce((sum, group) => sum + group.returnedCount, 0),
      color: "var(--chart-4)",
    },
    {
      id: "error",
      label: "错误",
      value: known.reduce((sum, group) => sum + group.errorCount, 0),
      color: "var(--chart-7)",
    },
    {
      id: "unknown",
      label: "结果未知",
      value: known.reduce((sum, group) => sum + group.unknownCount, 0),
      color: "var(--chart-6)",
    },
  ];
  const notReported = Math.max(
    0,
    calls - results.reduce((sum, result) => sum + result.value, 0),
  );
  if (notReported)
    results.push({
      id: "not-reported",
      label: "结果未报告",
      value: notReported,
      color: "var(--chart-6)",
    });
  const timed = rows.filter(
    (group) => group.durationSamples > 0 && group.p95Ms !== null,
  );
  return (
    <section aria-label="MCP 调用比较" className="min-w-0 space-y-4">
      <div className="grid min-w-0 gap-3 sm:grid-cols-3">
        <MetricCard
          label="已记录调用"
          value={known.length ? compactNumber(calls) : "未知"}
          footer={`${rows.length} 个当前匹配分组 · ${rows.length - known.length} 个调用数未知`}
        />
        <MetricCard
          label="已记录错误"
          value={known.length ? compactNumber(results[1].value) : "未知"}
          footer="仅统计已知调用分组，不推断连接健康"
          tone={results[1].value > 0 ? "warning" : "default"}
        />
        <MetricCard
          label="可比较耗时的 MCP"
          value={compactNumber(timed.length)}
          footer="仅包含有真实耗时样本的分组"
        />
      </div>
      {known.length > 0 && (
        <div className="space-y-3 rounded-xl border-hairline border-border p-4">
          <h3 className="text-body font-medium">调用结果分布</h3>
          <SegmentedBar
            mode="distribution"
            total={calls}
            segments={results}
            valueText={`当前匹配分组的已记录结果：${results.map((result) => `${result.label} ${result.value}`).join("，")}；调用数未知的分组不参与`}
          />
          <ChartLegend
            className="flex flex-wrap gap-x-6 gap-y-2"
            items={results.map((result) => ({
              ...result,
              value: `${compactNumber(result.value)} 次`,
            }))}
          />
          <p className="text-label text-fg-subtle">
            已返回表示工具返回了记录，不等同于任务成功；未采集的调用与结果保留未知。
          </p>
        </div>
      )}
      <div className="grid min-w-0 gap-4 xl:grid-cols-2">
        <UsageRanking
          title="MCP 调用排行"
          description="当前筛选的已记录调用；点击名称查看服务明细"
          items={rows.map((group) => ({
            id: group.id,
            label: group.name,
            value: group.callCount,
            color: chartColor(group.id),
            detail:
              group.callCount === null
                ? "尚无可确认的调用记录"
                : `${group.errorCount} 次已记录错误 · ${group.sessionCount} 个关联 Session`,
          }))}
          formatValue={(value) =>
            value === null ? "未知" : `${compactNumber(value)} 次`
          }
          onSelect={onOpen}
        />
        <UsageRanking
          title="MCP 耗时比较 · P95"
          description="条形越长耗时越高；不同分组的 P95 不相加"
          items={rows.map((group) => ({
            id: group.id,
            label: group.name,
            value: group.durationSamples > 0 ? group.p95Ms : null,
            color: chartColor(group.id),
            detail: `${group.durationSamples} 个耗时样本${group.durationSamples < 30 ? " · 样本不足 30 条" : ""}`,
          }))}
          formatValue={(value) =>
            value === null
              ? "未知"
              : `${(value / 1000).toLocaleString("zh-CN", { maximumFractionDigits: 3 })} 秒`
          }
          onSelect={onOpen}
        />
      </div>
      <p className="text-label text-fg-subtle">
        范围为当前 Agent、会话日期与搜索；各服务的关联模型 Token
        可能重叠，不作为可相加的服务成本。
      </p>
    </section>
  );
}
