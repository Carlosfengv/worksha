import { useState } from "react";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "@/components/ui/accordion";
import {
  Select,
  SelectTrigger,
  SelectContent,
  SelectItem,
} from "@/components/ui/select";
import {
  Table,
  TableHeader,
  TableBody,
  TableHead,
  TableRow,
  TableCell,
} from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import {
  HeatmapChart,
  HeatmapCells,
  HeatmapXAxis,
  HeatmapYAxis,
  HeatmapTooltip,
  HeatmapLegend,
  HeatmapInteractionProvider,
  HeatmapInteractionBoundary,
  HEATMAP_DEFAULT_LEVEL_COLORS,
} from "@/components/ui/heatmap-chart";
import type { HeatmapDay, RequestDashboard } from "../request-types";
import { requestValue } from "../request-display";
import {
  heatmapDateKey,
  requestHeatmapColumns,
  requestHeatmapValue,
} from "../request-visualization";
export function RequestHeatmap({
  data,
  onDay,
  onAllDates,
}: {
  data: RequestDashboard;
  onDay: (start: string, end: string) => void;
  onAllDates?: () => void;
}) {
  const [metric, setMetric] = useState("tokens");
  const heatmap = data.heatmap;
  if (!heatmap) return null;
  const currency = data.query.currency ?? "USD";
  const value = (day: HeatmapDay) => requestHeatmapValue(day, metric, currency);
  const max = Math.max(0, ...heatmap.days.map((d) => value(d) ?? 0));
  const columns = requestHeatmapColumns(heatmap.days, metric, currency);
  const byDate = new Map(heatmap.days.map((day) => [day.date, day]));
  const zeroColor = "color-mix(in oklch, var(--chart-1) 12%, var(--muted))";
  const native = data.query.source === "native";
  const unit = native ? "模型响应记录" : "请求";
  const rangeDays = heatmap.days.filter(
    (d) => !["future", "out_of_range"].includes(d.state),
  ).length;
  const caption = (d: HeatmapDay) =>
    d.state === "future"
      ? `${d.date} · 未来日期`
      : d.state === "out_of_range"
        ? `${d.date} · 当前日期范围之外`
        : d.state !== "recorded"
          ? `${d.date} · 未记录，无法区分无用量或采集缺口`
          : `${d.date} · ${d.records} 条${unit} · ${requestValue(value(d), metric)}${metric === "cost" ? ` ${currency}` : metric === "tokens" ? " Token" : ""}${d.usageMissing ? ` · ${d.usageMissing} 条用量缺失` : ""}${d.cost.unknownCalls && metric === "cost" ? ` · ${d.cost.unknownCalls} 次费用未知` : ""}`;
  return (
    <section className="min-w-0" aria-label="用量热力图">
      <Accordion className="w-full" type="multiple">
        <AccordionItem value="heatmap">
          <AccordionTrigger>用量热力图 · 最近 53 周</AccordionTrigger>
          <AccordionContent>
            <div className="min-w-0 space-y-3">
              {rangeDays < 32 && (
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <p className="text-label text-fg-muted">
                    当前日期筛选覆盖 {rangeDays} 天，范围外的格子不表示零用量。
                  </p>
                  {onAllDates && (
                    <Button size="sm" variant="tertiary" onClick={onAllDates}>
                      查看全部日期的年度分布
                    </Button>
                  )}
                </div>
              )}
              <div className="flex flex-wrap items-center justify-between gap-3">
                <p className="text-label text-fg-muted">
                  按当前来源与全部筛选展示，日期为
                  Asia/Shanghai。点击有记录的日期查看明细。
                </p>
                <Select
                  value={metric}
                  onValueChange={(v) => setMetric(v ?? "tokens")}
                >
                  <SelectTrigger aria-label="热力图指标" />
                  <SelectContent>
                    <SelectItem value="tokens">Token</SelectItem>
                    <SelectItem value="calls">{unit}量</SelectItem>
                    <SelectItem value="cost">费用（{currency}）</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <HeatmapInteractionProvider>
                <HeatmapInteractionBoundary>
                  <div className="min-w-0 overflow-x-auto">
                    <HeatmapChart
                      data={columns}
                      className="min-w-160"
                      animate={false}
                      colorScale={(count) =>
                        count == null || !Number.isFinite(count)
                          ? "var(--surface-raised)"
                          : count === 0
                            ? zeroColor
                            : HEATMAP_DEFAULT_LEVEL_COLORS[
                                Math.max(
                                  1,
                                  Math.min(
                                    4,
                                    Math.ceil((count / (max || 1)) * 4),
                                  ),
                                )
                              ]
                      }
                      onCellSelect={(bin) => {
                        const day = byDate.get(heatmapDateKey(bin.date));
                        if (day?.state === "recorded")
                          onDay(day.start, day.end);
                      }}
                    >
                      <HeatmapCells hideGhostCells={false} />
                      <HeatmapXAxis />
                      <HeatmapYAxis />
                      <HeatmapTooltip
                        formatHeader={(_, date) => heatmapDateKey(date)}
                        formatLabel={(_, date) => {
                          const day = byDate.get(heatmapDateKey(date));
                          return day ? caption(day) : "当前年度范围之外";
                        }}
                      />
                    </HeatmapChart>
                  </div>
                  <HeatmapLegend
                    lessLabel="已知用量较少"
                    moreLabel="较多"
                    interactive={false}
                    colorScale={(level) =>
                      level === 0
                        ? zeroColor
                        : HEATMAP_DEFAULT_LEVEL_COLORS[
                            Math.max(0, Math.min(4, level ?? 0))
                          ]
                    }
                  />
                </HeatmapInteractionBoundary>
              </HeatmapInteractionProvider>
              <p className="text-label text-fg-muted">
                颜色越深，已知用量越多；浅蓝表示已知零值。灰色表示指标未知、未记录、范围外或未来，具体状态见日期明细。
                {heatmap.missingTime} 条无日期，{heatmap.outsideWindow}{" "}
                条早于此窗口。仅有明细留存期间的可用数据；未建立 53
                周永久聚合，缺口不补零，也不生成到期明细。
              </p>
              <Accordion className="w-full" type="multiple">
                <AccordionItem value="days">
                  <AccordionTrigger>有记录日期与数值</AccordionTrigger>
                  <AccordionContent>
                    <div className="max-h-72 overflow-auto">
                      <Table>
                        <TableHeader>
                          <TableRow>
                            <TableHead>日期</TableHead>
                            <TableHead>{unit}量</TableHead>
                            <TableHead>当前指标</TableHead>
                            <TableHead>缺失</TableHead>
                            <TableHead>查看</TableHead>
                          </TableRow>
                        </TableHeader>
                        <TableBody>
                          {heatmap.days
                            .filter((d) => d.state === "recorded")
                            .map((d) => (
                              <TableRow key={d.date}>
                                <TableCell>{d.date}</TableCell>
                                <TableCell>{d.records}</TableCell>
                                <TableCell>
                                  {requestValue(value(d), metric)}
                                  {metric === "cost" ? ` ${currency}` : ""}
                                </TableCell>
                                <TableCell>
                                  {metric === "cost"
                                    ? d.cost.unknownCalls
                                    : d.usageMissing}
                                </TableCell>
                                <TableCell>
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    onClick={() => onDay(d.start, d.end)}
                                  >
                                    查看当天
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
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </section>
  );
}
