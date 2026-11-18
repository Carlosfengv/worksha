import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { SegmentedBar } from "@/components/ui/chart-primitives";
import { Badge } from "@/components/ui/badge";
import { compactNumber } from "@/shared/lib/format";
import {
  DataTable,
  useDataTable,
  DataTableColumnHeader,
  DataTableViewOptions,
} from "@/components/ui/data-table";
import { dataTableControls } from "@/shared/ui/data-table-labels";
import { providerCosts } from "@/features/providers/usage-display";
import { date, number } from "@/features/sessions/display";
import type { ColumnDef, SortingState } from "@tanstack/react-table";
import { useMemo } from "react";
import {
  durationLabel,
  requestAgentName,
  requestStates,
} from "../request-display";
import type { RequestRecord } from "../request-types";

const defaultHref = (id: string) => `#/requests/${encodeURIComponent(id)}`;

export function RequestsTable({
  rows,
  nativeView = false,
  onOpen,
  requestHref = defaultHref,
  onSort,
  sort = "time",
  descending = true,
}: {
  rows: RequestRecord[];
  nativeView?: boolean;
  onOpen?: (id: string) => void;
  requestHref?: (id: string) => string;
  onSort?: (sort: string, descending: boolean) => void;
  sort?: string;
  descending?: boolean;
}) {
  const { agentNames } = useAgentDirectory();
  const columns = useMemo<ColumnDef<RequestRecord>[]>(
    () => [
      {
        id: "time",
        accessorFn: (r) => r.startedAt,
        header: ({ column }) => (
          <DataTableColumnHeader
            column={column}
            label={nativeView ? "记录时间" : "时间"}
            labels={dataTableControls}
          />
        ),
        enableHiding: false,
        cell: ({ row }) => (
          <a
            href={requestHref(row.original.id)}
            data-workspace-resource
            data-request-id={row.original.id}
            aria-label={`查看请求 ${row.original.id} · ${date(row.original.startedAt)}`}
            className="whitespace-nowrap text-label font-medium hover:underline"
            onClick={
              onOpen
                ? (event) => {
                    event.preventDefault();
                    onOpen(row.original.id);
                  }
                : undefined
            }
          >
            {date(row.original.startedAt)}
          </a>
        ),
      },
      {
        id: "agent",
        accessorFn: (r) => r.model,
        header: "Agent / 模型",
        enableSorting: false,
        meta: { label: "Agent / 模型" },
        cell: ({ row }) => (
          <>
            <p>{requestAgentName(row.original, agentNames)}</p>
            {row.original.client && (
              <p className="text-label text-fg-muted">
                局域网 · {row.original.client.name}
              </p>
            )}
            <p
              className="max-w-48 truncate text-label text-fg-muted"
              title={row.original.model ?? "模型未知"}
            >
              {row.original.model ?? "模型未知"}
            </p>
          </>
        ),
      },
      {
        id: "source",
        accessorKey: "source",
        header: "来源",
        enableSorting: false,
        enableHiding: false,
        cell: ({ row: { original } }) =>
          original.source === "native"
            ? original.native?.association === "response_id"
              ? "原生 · 已关联"
              : "原生观测"
            : original.nativeLinks?.length
              ? "网关 · 已关联"
              : "本地网关",
      },
      {
        id: "provider",
        accessorKey: "provider",
        header: "Provider",
        enableSorting: false,
        meta: { label: "Provider" },
        cell: ({ row }) => (
          <span
            className="inline-block max-w-40 truncate"
            title={row.original.provider}
          >
            {row.original.provider}
          </span>
        ),
      },
      {
        id: "status",
        accessorKey: "status",
        header: "结果",
        enableSorting: false,
        meta: { label: "结果" },
        cell: ({ row }) => (
          <>
            <Badge
              size="sm"
              variant="plain"
              status={
                row.original.status === "failed"
                  ? "danger"
                  : row.original.status === "recovered"
                    ? "warning"
                    : "neutral"
              }
            >
              {requestStates[row.original.status] ?? "结果未知"}
            </Badge>
            {!row.original.native && (
              <p className="text-label text-fg-subtle">
                {row.original.attempts.length} 次尝试
              </p>
            )}
          </>
        ),
      },
      {
        id: "ttft",
        accessorKey: "ttftMs",
        header: ({ column }) => (
          <DataTableColumnHeader
            column={column}
            label="TTFT"
            labels={dataTableControls}
          />
        ),
        cell: ({ row }) => durationLabel(row.original.ttftMs),
      },
      {
        id: "duration",
        accessorKey: "durationMs",
        header: ({ column }) => (
          <DataTableColumnHeader
            column={column}
            label="总耗时"
            labels={dataTableControls}
          />
        ),
        cell: ({ row }) => {
          const { durationMs, ttftMs, status } = row.original;
          return (
            <div className="min-w-16 space-y-1 text-label">
              <span>
                {["waiting", "streaming"].includes(status)
                  ? "计时中"
                  : durationLabel(durationMs)}
              </span>
              {durationMs != null &&
                ttftMs != null &&
                durationMs > 0 &&
                ttftMs <= durationMs && (
                  <SegmentedBar
                    mode="distribution"
                    total={durationMs}
                    valueText="首字等待与传输耗时"
                    className="h-1.5"
                    segments={[
                      { id: "waiting", label: "等待", value: ttftMs },
                      {
                        id: "stream",
                        label: "传输",
                        value: durationMs - ttftMs,
                      },
                    ]}
                  />
                )}
            </div>
          );
        },
      },
      {
        id: "tokens",
        accessorFn: (r) => r.usage.totalTokens,
        header: ({ column }) => (
          <DataTableColumnHeader
            column={column}
            label="Token"
            labels={dataTableControls}
          />
        ),
        cell: ({ row }) => (
          <>
            <span
              className="tabular-nums"
              title={number(row.original.usage.totalTokens)}
            >
              {compactNumber(row.original.usage.totalTokens)}
            </span>
            {row.original.usageMissing > 0 && (
              <p className="text-label text-fg-subtle">部分缺失</p>
            )}
          </>
        ),
      },
      {
        id: "cost",
        accessorFn: (r) => r.cost,
        header: ({ column }) => (
          <DataTableColumnHeader
            column={column}
            label="费用"
            labels={dataTableControls}
          />
        ),
        cell: ({ row }) => (
          <span
            className="inline-block max-w-52 whitespace-normal text-label"
            title={providerCosts(row.original.cost)}
          >
            {providerCosts(row.original.cost, true)}
          </span>
        ),
      },
      {
        id: "throughput",
        accessorKey: "throughput",
        header: "端到端吞吐",
        enableSorting: false,
        meta: { label: "端到端吞吐" },
        cell: ({ row }) =>
          row.original.throughput === null
            ? "不可测"
            : `${row.original.throughput.toFixed(1)} Token/s`,
      },
      {
        id: "cache",
        accessorFn: (r) => r.usage.cachedInputTokens,
        header: "缓存读取",
        enableSorting: false,
        meta: { label: "缓存读取" },
        cell: ({ row }) => number(row.original.usage.cachedInputTokens),
      },
      {
        id: "session",
        accessorKey: "sessionTitle",
        header: "Session",
        enableSorting: false,
        meta: { label: "Session" },
        cell: ({ row }) => (
          <span className="inline-block max-w-48 truncate">
            {row.original.sessionTitle ?? row.original.sessionId ?? "未关联"}
          </span>
        ),
      },
    ],
    [onOpen, requestHref, nativeView, agentNames],
  );
  const sorting: SortingState = useMemo(
    () => [{ id: sort, desc: descending }],
    [sort, descending],
  );
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (r) => `${r.source}:${r.id}`,
    manualPagination: true,
    manualSorting: true,
    autoResetPageIndex: false,
    enableRowSelection: false,
    enableSorting: !!onSort,
    state: { sorting },
    initialState: {
      columnVisibility: {
        throughput: false,
        cache: nativeView,
        session: false,
        ttft: !nativeView,
        duration: !nativeView,
        cost: !nativeView,
      },
    },
    onSortingChange: (updater) => {
      const next = typeof updater === "function" ? updater(sorting) : updater;
      onSort?.(next[0]?.id ?? "time", next[0]?.desc ?? true);
    },
  });
  return (
    <section
      className="min-w-0 space-y-2"
      aria-label={nativeView ? "原生响应明细" : "请求明细"}
    >
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-title font-medium">
          {nativeView ? "原生响应列表" : "请求列表"}
        </h2>
        <DataTableViewOptions table={table} labels={dataTableControls} />
      </div>
      <DataTable
        table={table}
        role="region"
        aria-label={nativeView ? "原生响应" : "请求"}
        showPagination={false}
        onRowActivate={(row) =>
          onOpen
            ? onOpen(row.original.id)
            : (window.location.hash = requestHref(row.original.id))
        }
        emptyMessage={nativeView ? "暂无匹配原生响应" : "暂无匹配请求"}
      />
    </section>
  );
}
