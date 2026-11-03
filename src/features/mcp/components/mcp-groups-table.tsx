import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  DataTable,
  DataTableColumnHeader,
  useDataTable,
} from "@/components/ui/data-table";
import { mcpElapsed } from "@/features/mcp/components/mcp-call-detail";
import type { McpGroup } from "@/features/mcp/types";
import { date, number } from "@/features/sessions/display";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import {
  dataTableControls,
  dataTablePagination,
} from "@/shared/ui/data-table-labels";
import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";
import { Tokens } from "./mcp-token-summary";

const rate = (v: number | null) =>
  v == null ? "未知" : `${(v * 100).toFixed(1)}%`;

type McpGroupRow = { group: McpGroup; onOpen: (id: string) => void };
const groupColumns = (
  agentNames: Record<string, string>,
): ColumnDef<McpGroupRow>[] => [
  {
    id: "service",
    accessorFn: (row) => row.group.name,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="MCP"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => {
      const { group: g } = row.original;
      return (
        <>
          <p className="max-w-64 break-all font-medium">{g.name}</p>
          <p className="text-label text-fg-muted">
            {g.identity === "unattributed"
              ? "未确认的服务来源"
              : `${g.configurationCount} 份当前配置 · ${g.projectPaths.length} 个关联项目`}
          </p>
        </>
      );
    },
  },
  {
    id: "agents",
    header: () => <>关联 Agents</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { group: g } = row.original;
      return (
        <div className="w-56 max-w-64 space-y-1 whitespace-normal wrap-anywhere">
          <p>
            调用：
            {g.agentIds.map((id) => agentNames[id] ?? id).join(" / ") ||
              "尚无记录"}
          </p>
          <p className="mt-1 text-label text-fg-muted">
            配置适用：
            {g.configuredAgentIds.map((id) => agentNames[id]).join(" / ") ||
              "未知"}
          </p>
        </div>
      );
    },
  },
  {
    id: "calls",
    header: () => <>调用次数</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { group: g } = row.original;
      return <>{number(g.callCount)}</>;
    },
  },
  {
    id: "related-tokens",
    header: () => <>关联模型 Token</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { group: g } = row.original;
      return (
        <>
          <Tokens group={g} />
        </>
      );
    },
  },
  {
    id: "latest",
    header: () => <>最近一次调用</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { group: g } = row.original;
      return <>{date(g.lastCallAt)}</>;
    },
  },
  {
    id: "outcomes",
    header: () => <>P95 / 错误</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { group: g } = row.original;
      return (
        <>
          <p>
            {mcpElapsed(g.p95Ms)} · {g.durationSamples} 样本
          </p>
          <p className="text-label text-fg-muted">
            {g.callCount == null ? (
              "错误未知"
            ) : (
              <Badge
                size="sm"
                status={g.errorCount > 0 ? "warning" : "neutral"}
                variant="plain"
              >
                {g.errorCount} / {g.callCount} 次错误 · {rate(g.errorRate)}
              </Badge>
            )}
          </p>
        </>
      );
    },
  },
  {
    id: "details",
    header: () => <>详情</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { group: g, onOpen } = row.original;
      return (
        <>
          <Button size="sm" variant="ghost" onClick={() => onOpen(g.id)}>
            查看 {g.name}
          </Button>
        </>
      );
    },
  },
];
export function McpGroupsTable({
  rows,
  onOpen,
}: {
  rows: McpGroup[];
  onOpen: (id: string) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const columns = useMemo(() => groupColumns(agentNames), [agentNames]);
  const records = useMemo(
    () => rows.map((group) => ({ group, onOpen })),
    [rows, onOpen],
  );

  const pagination = useTablePagination(rows.length, 20);
  const { table } = useDataTable({
    data: records,
    columns,
    getRowId: (row) => row.group.id,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  return (
    <DataTable
      table={table}
      role="region"
      aria-label="MCP 聚合列表"
      paginationProps={{ labels: dataTablePagination }}
      emptyMessage="暂无匹配记录"
    />
  );
}
