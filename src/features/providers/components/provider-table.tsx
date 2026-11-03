import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DataTable,
  DataTableColumnHeader,
  useDataTable,
} from "@/components/ui/data-table";
import {
  availableProviderModelIds,
  providerCatalog,
  providerVendor,
} from "@/features/providers/catalog";
import type {
  Provider,
  ProviderStatus,
  ProviderUsage,
  ProviderUsageTotals,
} from "@/features/providers/types";
import { providerCosts, providerTokens } from "../usage-display";
import { date } from "@/features/sessions/display";
import {
  dataTableControls,
  dataTablePagination,
} from "@/shared/ui/data-table-labels";
import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";
import { verificationName } from "../display";

type ProviderRow = {
  onEdit: (provider: Provider) => void;
  onVerify: (provider: Provider) => void;
  p: Provider;
  defaultEnabled: boolean;
  v: ProviderStatus["verifications"][number] | undefined;
  usage: ProviderUsageTotals | undefined;
};
const providerColumns: ColumnDef<ProviderRow>[] = [
  {
    id: "provider",
    accessorFn: (row) => row.p.name,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="供应商"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => {
      const { p } = row.original;
      return (
        <>
          <Button variant="link" size="sm" asChild>
            <a href={`#/providers/${encodeURIComponent(p.id)}`}>
              {p.name}
              <span className="sr-only">详情</span>
            </a>
          </Button>
          <p
            className="mt-1 max-w-64 truncate text-label text-fg-subtle"
            title={p.endpoint}
          >
            {p.endpoint}
          </p>
          {providerVendor(p) !== "custom" &&
            p.name !== providerCatalog[providerVendor(p)]?.name && (
              <p className="mt-1 text-label text-fg-subtle">
                {providerCatalog[providerVendor(p)]?.name}
              </p>
            )}
        </>
      );
    },
  },
  {
    id: "verification",
    header: () => <>状态</>,
    enableSorting: false,
    cell: ({ row }) => {
      const { p, v } = row.original;
      return (
        <>
          <Badge
            status={
              v?.state === "verified"
                ? "success"
                : v?.state === "failed"
                  ? "warning"
                  : "neutral"
            }
          >
            {p.configured ? verificationName(v) : "未配置"}
          </Badge>
          {v && (
            <p className="mt-1 text-label text-fg-subtle">
              {date(v.checkedAt)}
            </p>
          )}
        </>
      );
    },
  },
  {
    id: "calls",
    accessorFn: (row) => row.usage?.calls,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="调用次数"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => row.original.usage?.calls.toLocaleString("zh-CN") ?? "—",
  },
  {
    id: "tokens",
    accessorFn: (row) => row.usage?.usage.totalTokens,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="Token 用量"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => (
      <>
        <p>{providerTokens(row.original.usage)}</p>
        <p className="text-label text-fg-subtle">
          输入 {providerTokens(row.original.usage, "inputTokens")} · 输出{" "}
          {providerTokens(row.original.usage, "outputTokens")}
        </p>
      </>
    ),
  },
  {
    id: "cost",
    header: "累计费用",
    cell: ({ row }) => (
      <span className="block max-w-64 text-label">
        {providerCosts(row.original.usage?.cost)}
      </span>
    ),
  },
  {
    id: "last-call",
    accessorFn: (row) => row.usage?.lastSeen,
    header: () => <>最近调用</>,
    cell: ({ row }) =>
      row.original.usage ? date(row.original.usage.lastSeen) : "—",
  },
  {
    id: "actions",
    header: () => (
      <>
        <span className="sr-only">操作</span>
      </>
    ),
    enableSorting: false,
    cell: ({ row }) => {
      const { p, defaultEnabled } = row.original;
      return (
        <>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="tertiary"
              size="sm"
              onClick={() => row.original.onEdit(p)}
            >
              配置<span className="sr-only"> {p.name}</span>
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              disabled={!p.configured || !defaultEnabled}
              onClick={() => row.original.onVerify(p)}
            >
              验证<span className="sr-only"> {p.name}</span>
            </Button>
          </div>
        </>
      );
    },
  },
];
export function ProviderTable({
  providers,
  status,
  onEdit,
  onVerify,
  usage,
}: {
  providers: Provider[];
  status: ProviderStatus | null;
  onEdit: (p: Provider) => void;
  onVerify: (p: Provider) => void;
  usage?: ProviderUsage | null;
}) {
  const rows = useMemo(
    () =>
      providers.map((p) => {
        const defaultEnabled = availableProviderModelIds(p).includes(p.model);
        const v = status?.verifications.find(
          (v) => v.providerId === p.id && (v.model ?? p.model) === p.model,
        );
        return {
          p,
          defaultEnabled,
          v,
          onEdit,
          onVerify,
          usage: usage?.providers.find((row) => row.providerId === p.id)
            ?.totals,
        };
      }),
    [providers, status, onEdit, onVerify, usage],
  );

  const pagination = useTablePagination(rows.length, 20);
  const { table } = useDataTable({
    data: rows,
    columns: providerColumns,
    getRowId: (row) => row.p.id,
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
      aria-label="供应商连接"
      emptyMessage="尚无已配置的供应商连接"
      paginationProps={{ labels: dataTablePagination }}
    />
  );
}
