import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { Button } from "@/components/ui/button";
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

export function RequestsTable({
  rows,
  nativeView = false,
  onOpen,
  onSort,
  sort = "time",
  descending = true,
}: {
  rows: RequestRecord[];
  nativeView?: boolean;
  onOpen?: (id: string) => void;
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
          <span className="whitespace-nowrap text-label">
            {date(row.original.startedAt)}
          </span>
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
            <p className="text-label text-fg-subtle">
              {row.original.native
                ? "原生响应 · " +
                  (row.original.native.association === "response_id"
                    ? "已关联网关"
                    : "路径未确认")
                : `${row.original.attempts.length} 次尝试`}
            </p>
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
        cell: ({ row }) => (
          <>
            {["waiting", "streaming"].includes(row.original.status)
              ? "计时中"
              : durationLabel(row.original.durationMs)}
          </>
        ),
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
      {
        id: "details",
        header: "详情",
        enableSorting: false,
        enableHiding: false,
        cell: ({ row }) => (
          <Button
            size="sm"
            variant="ghost"
            data-request-id={row.original.id}
            onClick={() =>
              onOpen
                ? onOpen(row.original.id)
                : (window.location.hash = `/requests/${encodeURIComponent(row.original.id)}`)
            }
          >
            查看详情
          </Button>
        ),
      },
    ],
    [onOpen, nativeView, agentNames],
  );
  const sorting: SortingState = useMemo(
    () => [{ id: sort, desc: descending }],
    [sort, descending],
  );
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (r) => r.id,
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
        emptyMessage={nativeView ? "暂无匹配原生响应" : "暂无匹配请求"}
      />
    </section>
  );
}
