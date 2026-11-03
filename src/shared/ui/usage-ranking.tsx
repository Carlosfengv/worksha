import { Bar, BarChart, BarYAxis } from "@/components/ui/bar-chart";
import { ChartTooltip, Grid } from "@/components/ui/chart-core";
import { ChartLegend } from "@/components/ui/chart-primitives";

export interface UsageRankingItem {
  id: string;
  label: string;
  value: number | null;
  color: string;
  detail?: string;
  ratio?: string;
}

/** Aggregated usage only; the host supplies scope, units and real drilldowns. */
export function UsageRanking({
  title,
  description,
  items,
  formatValue,
  onSelect,
}: {
  title: string;
  description: string;
  items: UsageRankingItem[];
  formatValue: (value: number | null) => string;
  onSelect?: (id: string) => void;
}) {
  const sorted = [...items].sort(
    (a, b) => (b.value ?? -1) - (a.value ?? -1) || a.id.localeCompare(b.id),
  );
  const plotted = sorted
    .filter(
      (item) =>
        item.value !== null && Number.isFinite(item.value) && item.value >= 0,
    )
    .slice(0, 8)
    .map((item, index) => ({
      ...item,
      category: `${index + 1}. ${item.label}`,
    }));
  const chartData = plotted.map((item) => ({
    category: item.category,
    value: item.value,
  }));
  return (
    <section
      aria-label={title}
      className="min-w-0 space-y-4 rounded-xl border-hairline border-border p-4"
    >
      <div className="space-y-1">
        <h3 className="text-body font-medium">{title}</h3>
        <p className="text-label text-fg-subtle">{description}</p>
      </div>
      {plotted.length > 0 && (
        <div
          role="group"
          aria-label={`${title}，条形长度表示实际数值，完整数值见下方列表`}
          style={{ height: Math.max(120, plotted.length * 36 + 16) }}
        >
          <BarChart
            data={chartData}
            xDataKey="category"
            orientation="horizontal"
            barWidth={16}
            aspectRatio="auto"
            className="h-full"
            margin={{ top: 8, bottom: 8, left: 96, right: 16 }}
            animationDuration={0}
          >
            <Grid horizontal={false} vertical numTicksColumns={4} />
            <BarYAxis />
            <Bar
              dataKey="value"
              fill="var(--chart-1)"
              fillForDatum={(_, index) => plotted[index].color}
              animate={false}
            />
            <ChartTooltip
              showDatePill={false}
              showDots={false}
              content={({ point, index }) => (
                <div className="space-y-1 p-3 text-label">
                  <p className="font-medium">{plotted[index].label}</p>
                  <p>{formatValue(point.value as number | null)}</p>
                  {plotted[index].detail ? (
                    <p>{plotted[index].detail}</p>
                  ) : null}
                </div>
              )}
            />
          </BarChart>
        </div>
      )}
      {items.length ? (
        <div className="max-h-72 overflow-y-auto overscroll-contain p-1">
          <ChartLegend
            aria-label={`${title}完整数值`}
            items={sorted.map((item) => ({
              id: item.id,
              color: item.color,
              label: (
                <span>
                  {item.label}
                  {item.detail && (
                    <span className="mt-0.5 block text-fg-subtle">
                      {item.detail}
                    </span>
                  )}
                </span>
              ),
              value: formatValue(item.value),
              ratio: item.ratio,
            }))}
            onSelect={onSelect}
          />
        </div>
      ) : (
        <p className="py-6 text-center text-body text-fg-muted">暂无分组数据</p>
      )}
      {sorted.length > 8 && (
        <p className="text-label text-fg-subtle">
          图示前 8 个已知分组；列表保留全部分组，未知值不绘制为零。
        </p>
      )}
    </section>
  );
}
