import { costText } from "@/shared/lib/money";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import type { AgentUsage, AgentUsageQuery } from "@/features/agents/types";
import { usageNumber } from "@/features/agents/usage-display";
import { date, number } from "@/features/sessions/display";
import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";

type UsageItem = AgentUsage["items"][number];
type UsageRow = {
  item: UsageItem;
  asOf: string;
  onSelect: (value: { row: UsageItem; asOf: string }) => void;
};
export function ModelUsageTable({
  items,
  asOf,
  onSelect,
  source,
}: {
  items: UsageItem[];
  asOf: string;
  source: AgentUsageQuery["source"];
  onSelect: (value: { row: UsageItem; asOf: string }) => void;
}) {
  const columns = useMemo<ColumnDef<UsageRow>[]>(
    () => [
      {
        id: "model-provider",
        header: () => <>模型 / Provider</>,
        enableSorting: false,
        cell: ({ row }) => {
          const { item: rowData } = row.original;
          return (
            <>
              <p className="max-w-72 break-words font-medium">
                {rowData.model ?? "模型未记录"}
              </p>
              <p className="max-w-72 break-words text-label text-fg-muted">
                {rowData.provider ?? "Provider 未记录"}
              </p>
              <p className="text-label text-fg-subtle">
                最近记录 {date(rowData.lastSeen)}
              </p>
            </>
          );
        },
      },
      {
        id: "attempts",
        header: () => <>{source === "native" ? "响应数" : "尝试数"}</>,
        enableSorting: false,
        cell: ({ row }) => {
          const { item: rowData } = row.original;
          return <>{number(rowData.totals.attempts)}</>;
        },
      },
      {
        id: "input",
        header: () => <>输入 Token</>,
        enableSorting: false,
        cell: ({ row }) => {
          const { item: rowData } = row.original;
          return <>{usageNumber(rowData.totals, "inputTokens")}</>;
        },
      },
      {
        id: "output",
        header: () => <>输出 Token</>,
        enableSorting: false,
        cell: ({ row }) => {
          const { item: rowData } = row.original;
          return <>{usageNumber(rowData.totals, "outputTokens")}</>;
        },
      },
      {
        id: "total",
        header: () => <>总 Token</>,
        enableSorting: false,
        cell: ({ row }) => {
          const { item: rowData } = row.original;
          return <>{usageNumber(rowData.totals, "totalTokens")}</>;
        },
      },
      {
        id: "cost",
        header: "费用",
        enableSorting: false,
        cell: ({ row }) => (
          <span className="text-label">
            {costText(row.original.item.totals.cost)}
          </span>
        ),
      },
      {
        id: "details",
        header: () => <>明细</>,
        enableSorting: false,
        cell: ({ row }) => {
          const { item: rowData, asOf, onSelect } = row.original;
          return (
            <>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => onSelect({ row: rowData, asOf })}
              >
                用量详情
                <span className="sr-only"> {rowData.model ?? "未知模型"}</span>
              </Button>
            </>
          );
        },
      },
    ],
    [source],
  );
  const rows = useMemo(
    () => items.map((item) => ({ item, asOf, onSelect })),
    [items, asOf, onSelect],
  );

  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) =>
      JSON.stringify([
        row.item.providerId,
        row.item.routeRevision,
        row.item.model,
      ]),
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    manualPagination: true,
    manualSorting: true,
  });
  return (
    <DataTable
      table={table}
      role="region"
      aria-label="按模型与 Provider 汇总的用量"
      showPagination={false}
      emptyMessage="暂无匹配记录"
    />
  );
}
