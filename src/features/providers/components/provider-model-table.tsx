import type { ColumnDef } from "@tanstack/react-table";
import { AiBrain03Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DataTable,
  DataTableColumnHeader,
  useDataTable,
} from "@/components/ui/data-table";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import {
  dataTableControls,
  dataTablePagination,
} from "@/shared/ui/data-table-labels";
import { date } from "@/features/sessions/display";
import { modelLogo } from "@/shared/lib/model-logo";
import {
  inventoryCalls,
  modelLimit,
  modelSupport,
  type InventoryRow,
} from "../model-inventory";
import { providerCosts, providerTokens } from "../usage-display";
import { ProviderModelEvidence } from "./provider-model-evidence";
import { ProviderModelObservations } from "./provider-model-observations";

const columns: ColumnDef<InventoryRow>[] = [
  {
    id: "model",
    accessorFn: (row) => row.model.id,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="模型"
        labels={dataTableControls}
      />
    ),
    cell: ({ row: { original: row } }) => {
      const logo = modelLogo(row.model.id);
      return (
        <div className="flex max-w-64 items-start gap-2">
          {logo ? (
            <span
              aria-hidden="true"
              className="mt-1 size-5 shrink-0 bg-current text-fg-muted"
              style={{
                mask: `url("${logo}") center / contain no-repeat`,
                WebkitMask: `url("${logo}") center / contain no-repeat`,
              }}
            />
          ) : (
            <HugeiconsIcon
              icon={AiBrain03Icon}
              size={20}
              aria-hidden="true"
              className="mt-1 size-5 shrink-0 text-fg-muted"
            />
          )}
          <div className="min-w-0 space-y-1 break-all">
            <p>{row.model.name}</p>
            {row.model.name !== row.model.id && (
              <p className="text-label text-fg-muted">{row.model.id}</p>
            )}
            {row.model.id === row.provider.model && <Badge>默认</Badge>}
          </div>
        </div>
      );
    },
  },
  {
    id: "provider",
    accessorFn: (row) => row.provider.name,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="Provider"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => (
      <Button variant="link" size="sm" asChild>
        <a href={`#/providers/${encodeURIComponent(row.original.provider.id)}`}>
          {row.original.provider.name}
        </a>
      </Button>
    ),
  },
  {
    id: "state",
    accessorKey: "state",
    header: "可用状态",
    cell: ({ row }) => <Badge>{row.original.state}</Badge>,
  },
  {
    id: "context",
    accessorFn: (row) => row.model.contextWindow ?? row.model.maxInputTokens,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="上下文大小"
        labels={dataTableControls}
      />
    ),
    cell: ({ row: { original: row } }) => (
      <div className="space-y-1 whitespace-nowrap">
        <p>{modelLimit(row.model.contextWindow ?? row.model.maxInputTokens)}</p>
        {!row.model.contextWindow && row.model.maxInputTokens != null && (
          <p className="text-label text-fg-muted">输入上限</p>
        )}
        {row.contextNote && (
          <p className="max-w-64 whitespace-normal text-label text-fg-muted">
            {row.contextNote}
          </p>
        )}
      </div>
    ),
  },
  {
    id: "output",
    header: "最大输出",
    cell: ({ row }) => (
      <span className="whitespace-nowrap">
        {modelLimit(row.original.model.maxOutputTokens)}
      </span>
    ),
  },
  {
    id: "capabilities",
    header: "目录 / 官方声明",
    cell: ({ row }) => (
      <dl className="grid min-w-48 grid-cols-2 gap-x-4 gap-y-1 text-label">
        {(
          [
            ["tools", "工具调用"],
            ["vision", "图片输入"],
            ["reasoning", "推理思考"],
            ["structuredOutput", "结构化输出"],
            ["jsonOutput", "JSON 模式"],
          ] as const
        ).map(([key, label]) => (
          <div key={key}>
            <dt className="inline text-fg-muted">{label}：</dt>
            <dd className="inline">
              {modelSupport(row.original.model.capabilities?.[key])}
            </dd>
          </div>
        ))}
      </dl>
    ),
  },
  {
    id: "observations",
    header: "实测状态",
    cell: ({ row }) => <ProviderModelObservations row={row.original} />,
  },
  {
    id: "calls",
    accessorFn: (row) =>
      row.totals?.calls ?? (row.usageState === "ready" ? 0 : undefined),
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="调用次数"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => inventoryCalls(row.original),
  },
  ...(
    [
      ["inputTokens", "输入 Token"],
      ["outputTokens", "输出 Token"],
      ["totalTokens", "总 Token"],
    ] as const
  ).map(([field, label]): ColumnDef<InventoryRow> => ({
    id: field,
    header: label,
    cell: ({ row: { original: row } }) =>
      row.totals
        ? providerTokens(row.totals, field)
        : row.usageState === "ready"
          ? "0"
          : "未知",
  })),
  {
    id: "cost",
    header: "累计费用",
    cell: ({ row }) => (
      <span className="block min-w-36 max-w-64 text-label">
        {row.original.totals
          ? providerCosts(row.original.totals.cost)
          : row.original.usageState === "ready"
            ? "无调用"
            : "未知"}
      </span>
    ),
  },
  {
    id: "catalog",
    header: "目录信息",
    cell: ({ row: { original: row } }) => (
      <div className="min-w-36 text-label text-fg-muted">
        <p>
          {row.catalog?.loading
            ? "读取中…"
            : row.catalog?.error
              ? "读取失败"
              : row.catalog?.data
                ? "目录已读取"
                : "尚未读取"}
        </p>
        {row.catalog?.data && (
          <p>
            {row.catalog.error ||
            row.catalog.loading ||
            Date.now() - Date.parse(row.catalog.data.fetchedAt) > 86_400_000
              ? "上次快照 · "
              : ""}
            {date(row.catalog.data.fetchedAt)}
          </p>
        )}
        <ProviderModelEvidence sources={row.model.documentation ?? []} />
      </div>
    ),
  },
];

export function ProviderModelTable({ rows }: { rows: InventoryRow[] }) {
  const pagination = useTablePagination(rows.length, 50);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.key,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  return (
    <DataTable
      table={table}
      aria-label="全部 Provider 模型"
      role="region"
      paginationProps={{
        labels: dataTablePagination,
        pageSizeOptions: [20, 50, 100],
      }}
    />
  );
}
