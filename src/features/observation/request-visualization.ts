import type { HeatmapColumn } from "@/components/ui/heatmap-chart";
import type { HeatmapDay } from "./request-types";
import { requestValue } from "./request-display";
import { compactNumber } from "@/shared/lib/format";

const secondsFormatter = new Intl.NumberFormat("zh-CN", {
  maximumSignificantDigits: 3,
});
export function requestAxisLabel(value: number, metric: string) {
  if (["ttft", "duration"].includes(metric))
    return `${secondsFormatter.format(value / 1000)}s`;
  if (metric === "cost") return requestValue(value, metric);
  return compactNumber(value);
}

export function heatmapDateKey(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function requestHeatmapValue(
  day: HeatmapDay,
  metric: string,
  currency: string,
): number | null {
  if (day.state !== "recorded") return null;
  if (metric === "calls") return day.records;
  if (metric === "tokens") return day.tokens;
  const costs = [...day.cost.actual, ...day.cost.estimated].filter(
    (cost) => cost.currency === currency,
  );
  return costs.length
    ? costs.reduce((sum, cost) => sum + cost.amount, 0)
    : null;
}

/** Dates represent Shanghai calendar days; chart dates keep that civil identity in any host timezone. */
export function requestHeatmapColumns(
  days: HeatmapDay[],
  metric: string,
  currency: string,
): HeatmapColumn[] {
  const ordered = days
    .filter((day) =>
      Number.isFinite(new Date(`${day.date}T12:00:00`).getTime()),
    )
    .sort((a, b) => a.date.localeCompare(b.date));
  if (!ordered.length) return [];
  const byDate = new Map(ordered.map((day) => [day.date, day]));
  const date = new Date(`${ordered[0].date}T12:00:00`);
  date.setDate(date.getDate() - date.getDay());
  const end = new Date(`${ordered.at(-1)!.date}T12:00:00`);
  end.setDate(end.getDate() + 6 - end.getDay());
  const columns: HeatmapColumn[] = [];
  while (date <= end) {
    const bins = Array.from({ length: 7 }, (_, bin) => {
      const calendarDate = new Date(date);
      const day = byDate.get(heatmapDateKey(calendarDate));
      const value = day ? requestHeatmapValue(day, metric, currency) : null;
      date.setDate(date.getDate() + 1);
      return { bin, date: calendarDate, count: value ?? Number.NaN };
    });
    columns.push({ bin: columns.length, bins });
  }
  return columns;
}
