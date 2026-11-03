import { useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { curveLinear } from "@visx/curve";
import { Badge } from "@/components/ui/badge";
import {
  ChartLegend,
  ChartTooltip,
  Grid,
  XAxis,
  YAxis,
} from "@/components/ui/chart-core";
import {
  DataTable,
  DataTableColumnHeader,
  useDataTable,
} from "@/components/ui/data-table";
import { Line, LineChart } from "@/components/ui/line-chart";
import { date } from "@/features/sessions/display";
import { dataTableControls } from "@/shared/ui/data-table-labels";
import {
  accountQuotaForecasts,
  providerQuotaForecasts,
  quotaOutcome,
  quotaPercent,
  type QuotaForecast,
} from "../forecast";
import type { BillingAccount } from "../types";
import { resetCountdown } from "../display";

const quotaDate = (value: number | null) =>
  value === null ? "未知" : date(new Date(value).toISOString());

const columns: ColumnDef<QuotaForecast>[] = [
  {
    accessorKey: "label",
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="额度窗口"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => (
      <span className="block max-w-56 break-words">{row.original.label}</span>
    ),
  },
  {
    accessorKey: "usedPercent",
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="已用 / 剩余"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => (
      <span className="whitespace-nowrap tabular-nums">
        {quotaPercent(row.original.usedPercent)} /{" "}
        {quotaPercent(row.original.remainingPercent)}
      </span>
    ),
  },
  {
    accessorKey: "resetsAt",
    header: "重置时间",
    cell: ({ row }) => (
      <span className="whitespace-nowrap text-label">
        {quotaDate(row.original.resetsAt)}
      </span>
    ),
  },
  {
    id: "outcome",
    header: "额度预测",
    cell: ({ row }) => (
      <div className="min-w-40 max-w-64 space-y-1 whitespace-normal text-label">
        <p
          className={
            row.original.runsOutAt !== null ? "text-fg-warning" : undefined
          }
        >
          {quotaOutcome(row.original)}
        </p>
        <p className="text-fg-subtle">{row.original.reason}</p>
      </div>
    ),
  },
];

function ForecastChart({ row }: { row: QuotaForecast }) {
  const { observedAt, resetsAt, usedPercent, projectedPercent, runsOutAt } =
    row;
  const data = useMemo(() => {
    if (
      observedAt === null ||
      resetsAt === null ||
      usedPercent === null ||
      projectedPercent === null
    )
      return [];
    const end = runsOutAt ?? resetsAt;
    const points: Record<string, unknown>[] = [
      {
        date: new Date(observedAt),
        observed: usedPercent,
        projected: usedPercent,
        limit: 100,
      },
      {
        date: new Date(end),
        observed: null,
        projected: Math.min(100, projectedPercent),
        limit: 100,
      },
    ];
    if (end < resetsAt)
      points.push({
        date: new Date(resetsAt),
        observed: null,
        projected: null,
        limit: 100,
      });
    return points;
  }, [observedAt, resetsAt, usedPercent, projectedPercent, runsOutAt]);
  if (observedAt === null || resetsAt === null) return null;
  return (
    <figure className="min-w-0 space-y-3" aria-label={`${row.label}额度预测图`}>
      <LineChart
        data={data}
        className="h-56 w-full"
        aspectRatio="auto"
        xDomain={[new Date(observedAt), new Date(resetsAt)]}
        yDomain={[0, 100]}
        yDomainTween={false}
        animationDuration={0}
        margin={{ top: 16, right: 28, bottom: 36, left: 48 }}
      >
        <Grid />
        <XAxis
          numTicks={2}
          formatDate={(value) =>
            value.toLocaleString(DISPLAY_LOCALE, {
              timeZone: DISPLAY_TIME_ZONE,
              month: "numeric",
              day: "numeric",
              hour: "2-digit",
              minute: "2-digit",
            })
          }
        />
        <YAxis numTicks={3} formatValue={(value) => `${value}%`} />
        <Line
          dataKey="limit"
          stroke="var(--chart-6)"
          dashFromIndex={0}
          dashArray="2,4"
          curve={curveLinear}
          animate={false}
          fadeEdges={false}
        />
        <Line
          dataKey="projected"
          stroke="var(--chart-2)"
          dashFromIndex={0}
          dashArray="5,4"
          curve={curveLinear}
          animate={false}
          fadeEdges={false}
          showMarkers
        />
        <Line
          dataKey="observed"
          stroke="var(--chart-1)"
          curve={curveLinear}
          animate={false}
          fadeEdges={false}
          showMarkers
        />
        <ChartTooltip
          showDatePill={false}
          content={({ point }) => (
            <div className="space-y-1 p-2 text-label">
              <p>{quotaDate((point.date as Date).getTime())}</p>
              {typeof point.observed === "number" && (
                <p>快照已用：{quotaPercent(point.observed)}</p>
              )}
              {typeof point.projected === "number" && (
                <p>线性预测：{quotaPercent(point.projected)}</p>
              )}
            </div>
          )}
        />
      </LineChart>
      <ChartLegend
        layout="inline"
        showValue={false}
        items={[
          {
            label: "快照已用",
            value: usedPercent ?? 0,
            color: "var(--chart-1)",
          },
          {
            label: "线性预测（虚线）",
            value: projectedPercent ?? 0,
            color: "var(--chart-2)",
          },
          { label: "额度上限", value: 100, color: "var(--chart-6)" },
        ]}
      />
      <figcaption className="text-label text-fg-subtle">
        暂无历史采样，仅展示已读取的用量与后续估算。
      </figcaption>
    </figure>
  );
}

export function ProviderQuotaForecast({
  account,
  providerId,
  now,
  directoryError,
}: {
  account: BillingAccount;
  providerId?: string;
  now: number;
  directoryError: boolean;
}) {
  const rows = useMemo(
    () =>
      providerId === undefined
        ? accountQuotaForecasts(account, now, directoryError)
        : providerQuotaForecasts(account, providerId, now, directoryError),
    [account, providerId, now, directoryError],
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selected =
    rows.find((row) => row.id === selectedId) ??
    rows.find((row) => row.runsOutAt !== null) ??
    rows[0];
  const atRisk = rows.filter(
    (row) => row.runsOutAt !== null && row.usedPercent !== 100,
  ).length;
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.id,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: { pageIndex: 0, pageSize: Math.max(1, rows.length) } },
  });
  if (
    providerId !== undefined &&
    (!providerId || account.providerId !== providerId)
  )
    return null;
  return (
    <section
      className="@container/quota min-w-0 space-y-4"
      aria-label="账户额度预测"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-body font-medium">额度预测</h4>
        {atRisk > 0 && <Badge>{atRisk} 个窗口可能提前用完</Badge>}
      </div>
      {!rows.length ? (
        <p className="text-body text-fg-muted" role="status">
          {!account.snapshot
            ? "尚未读取账户快照，额度预测保持未知。"
            : account.accountType === "api"
              ? "此账户未提供百分比额度窗口；余额与消费金额按来源展示，暂无法预测。"
              : "来源未提供额度窗口，暂无法预测。"}
        </p>
      ) : (
        <>
          <div className="grid min-w-0 gap-5 @5xl/quota:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)]">
            <div className="min-w-0 space-y-2">
              <DataTable
                table={table}
                aria-label="账户额度窗口"
                activeRowId={selected?.id}
                onRowActivate={(row) => setSelectedId(row.original.id)}
                showPagination={false}
              />
              <p className="text-label text-fg-subtle">
                选择额度窗口查看预测详情，支持 Enter 或空格键。
              </p>
            </div>
            {selected && (
              <section
                className="min-w-0 space-y-3"
                aria-label="选中额度窗口预测"
              >
                <h5 className="break-words text-body font-medium">
                  {selected.label}
                </h5>
                <p className="text-body">{quotaOutcome(selected)}</p>
                <p className="text-label text-fg-muted">{selected.reason}</p>
                {selected.state === "ready" && selected.usedPercent !== 100 ? (
                  <ForecastChart row={selected} />
                ) : null}
                {selected.runsOutAt !== null &&
                  selected.usedPercent !== 100 && (
                    <p className="text-label text-fg-warning">
                      预计用完：{quotaDate(selected.runsOutAt)}
                    </p>
                  )}
                <dl className="grid gap-3 text-label sm:grid-cols-2">
                  <div>
                    <dt className="text-fg-subtle">采集时间</dt>
                    <dd>{quotaDate(selected.observedAt)}</dd>
                  </div>
                  <div>
                    <dt className="text-fg-subtle">重置时间</dt>
                    <dd>{quotaDate(selected.resetsAt)}</dd>
                  </div>
                  <div>
                    <dt className="text-fg-subtle">距离重置</dt>
                    <dd className="tabular-nums">
                      {resetCountdown(
                        selected.resetsAt === null
                          ? null
                          : selected.resetsAt / 1000,
                        now,
                      )}
                    </dd>
                  </div>
                </dl>
              </section>
            )}
          </div>
          <p className="text-label text-fg-subtle">
            按来源给出的窗口时长与重置时间推算周期平均速度，假设后续速度不变。预测不代表服务商承诺，也不会自动停用连接。
          </p>
        </>
      )}
    </section>
  );
}
import { DISPLAY_LOCALE, DISPLAY_TIME_ZONE } from "@/shared/lib/date-time";
