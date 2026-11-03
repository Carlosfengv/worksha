import { useMemo, useState } from "react";
import { recordAgentKey } from "@/shared/types/agent";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import type { ColumnDef } from "@tanstack/react-table";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Combobox,
  ComboboxContent,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import {
  DataTable,
  DataTableColumnHeader,
  useDataTable,
} from "@/components/ui/data-table";
import { ProviderCallCost } from "./provider-call-cost";
import type { ProviderUsage, ProviderUsageTotals } from "../types";
import {
  attributionLabel,
  providerLatency,
  providerTokens,
  providerCosts,
} from "../usage-display";
import { date, number, sessionLink } from "@/features/sessions/display";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { providerUsageResource } from "../queries";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import {
  dataTableControls,
  dataTablePagination,
} from "@/shared/ui/data-table-labels";
import {
  ProviderUsageFilters,
  type UsageFilters,
} from "./provider-usage-filters";

type ModelRow = ProviderUsage["models"][number];
const modelColumns: ColumnDef<ModelRow>[] = [
  {
    id: "model",
    accessorFn: (r) => r.model ?? "模型未记录",
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="模型"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => (
      <span className="break-all">{row.original.model ?? "模型未记录"}</span>
    ),
  },
  {
    id: "calls",
    accessorFn: (r) => r.totals.calls,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="调用次数"
        labels={dataTableControls}
      />
    ),
  },
  ...(
    [
      ["inputTokens", "输入 Token"],
      ["outputTokens", "输出 Token"],
      ["totalTokens", "总 Token"],
    ] as const
  ).map(([field, label]): ColumnDef<ModelRow> => ({
    id: field,
    accessorFn: (r) => r.totals.usage[field],
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label={label}
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => providerTokens(row.original.totals, field),
  })),
  {
    id: "cost",
    header: "累计费用",
    cell: ({ row }) => (
      <span className="block max-w-64 text-label">
        {providerCosts(row.original.totals.cost)}
      </span>
    ),
  },
  { id: "errors", accessorFn: (r) => r.totals.errors, header: "错误次数" },
  {
    id: "latency",
    accessorFn: (r) => r.totals.latencyP95Ms,
    header: "延迟 P95",
    cell: ({ row }) => providerLatency(row.original.totals.latencyP95Ms),
  },
  {
    id: "last",
    accessorFn: (r) => r.totals.lastSeen,
    header: "最近调用",
    cell: ({ row }) => date(row.original.totals.lastSeen),
  },
];
type CallRow = ProviderUsage["calls"][number];
const callColumns = (
  agentNames: Record<string, string>,
): ColumnDef<CallRow>[] => [
  {
    id: "model",
    header: "模型 / Agent",
    cell: ({ row }) => (
      <>
        <p className="max-w-72 break-all">
          {row.original.model ?? "模型未记录"}
        </p>
        <p className="text-label text-fg-muted">
          {agentNames[row.original.agentProductId ?? row.original.agentId] ??
            "客户端未知"}
        </p>
      </>
    ),
  },
  {
    id: "time",
    header: "调用时间",
    cell: ({ row }) => date(row.original.time),
  },
  {
    id: "tokens",
    header: "输入 / 输出 Token",
    cell: ({ row }) => (
      <>
        {number(row.original.usage.inputTokens)} /{" "}
        {number(row.original.usage.outputTokens)}
      </>
    ),
  },
  {
    id: "status",
    header: "结果 / 延迟",
    cell: ({ row }) => (
      <>
        <p>
          {{
            success: "已完成",
            returned: "已记录响应",
            error: "失败",
            failed: "失败",
            cancelled: "已取消",
            in_progress: "进行中",
          }[row.original.status] ?? "结果未知"}
        </p>
        <p className="text-label text-fg-muted">
          {providerLatency(row.original.durationMs)}
        </p>
      </>
    ),
  },
  {
    id: "cost",
    header: "费用",
    cell: ({ row }) => <ProviderCallCost call={row.original} />,
  },
  {
    id: "evidence",
    header: "归属依据",
    cell: ({ row }) => (
      <>
        <p>{attributionLabel(row.original.attribution)}</p>
        <p className="max-w-64 break-all text-label text-fg-muted">
          {row.original.provider ?? "原生 Provider 未记录"}
        </p>
      </>
    ),
  },
  {
    id: "detail",
    header: "详情",
    cell: ({ row }) => {
      const c = row.original;
      const href = c.requestId
        ? `/requests/${encodeURIComponent(c.requestId)}?${new URLSearchParams({ range: "all", agent: recordAgentKey(c), ...(c.providerId ? { provider: c.providerId } : {}) })}`
        : c.sessionId
          ? sessionLink(c.sessionId, { anchor: c.id })
          : null;
      return href ? (
        <Button variant="link" size="sm" asChild>
          <a href={`#${href}`}>{c.requestId ? "查看请求" : "查看 Session"}</a>
        </Button>
      ) : (
        "—"
      );
    },
  },
];
export function ProviderUsageSummary({
  totals,
}: {
  totals: ProviderUsageTotals | null | undefined;
}) {
  return (
    <dl
      className="grid gap-4 sm:grid-cols-2 xl:grid-cols-5"
      aria-label="供应商调用汇总"
    >
      {[
        ["调用次数", number(totals?.calls)],
        ["总 Token", providerTokens(totals)],
        ["错误次数", number(totals?.errors)],
        ["延迟 P95", providerLatency(totals?.latencyP95Ms)],
        ["累计费用", providerCosts(totals?.cost)],
      ].map(([label, value]) => (
        <div key={label}>
          <dt className="text-label text-fg-muted">{label}</dt>
          <dd className="mt-1 text-title font-medium">{value}</dd>
        </div>
      ))}
    </dl>
  );
}
export function ProviderUsageStatus({
  data,
  error,
  reload,
}: {
  data: ProviderUsage | null;
  error: string | null;
  reload: () => void;
}) {
  return error ? (
    <Alert status="warning">
      <AlertTitle>调用统计未更新</AlertTitle>
      <AlertDescription>
        {error}
        {data ? " · 保留上次快照。" : ""}
        <Button variant="tertiary" size="sm" onClick={reload}>
          重试调用统计
        </Button>
      </AlertDescription>
    </Alert>
  ) : (
    <p className="text-label text-fg-subtle">
      {!data
        ? "正在读取调用统计…"
        : data.sourceState === "disabled"
          ? "原生会话来源未接入"
          : Date.now() - Date.parse(data.asOf) > 30000
            ? `最后快照 · ${date(data.asOf)}`
            : `${data.sourceState === "ready" ? "更新于" : "部分采集 · 更新于"} ${date(data.asOf)}`}
    </p>
  );
}
export function ProviderUsageNotes() {
  return (
    <Accordion>
      <AccordionItem value="scope">
        <AccordionTrigger>统计说明</AccordionTrigger>
        <AccordionContent>
          <div className="space-y-2 text-label text-fg-muted">
            <p>
              汇总覆盖当前筛选下的全部已保留记录，分页不影响总数。网关请求与原生响应可能重复，分别查看，不相加。
            </p>
            <p>
              按请求连接
              ID、保存的路由快照或明确的原生专用标识归属；不按当前模型、名称或
              Endpoint 猜测历史连接。缺少证据的记录进入“未归属调用”。
            </p>
            <p>
              包含失败和重试尝试；缓存与推理 Token 已包含在输入 /
              输出中。未报告用量保留未知，部分报告标为“部分”。实际费用仅取服务商返回的扣费字段；估算费用按真实
              Token
              和对应价格计算，两者分别汇总，每次调用只计一类。缺少价格或必要用量的费用为未知，账户余额不当作调用费用。
            </p>
            <p>
              估算仅含模型 Token
              费用；不含税、充值手续费及独立搜索等费用。价格有适用期，高峰 /
              非高峰按 UTC
              判断；超出已核实范围保留未知。不同币种分别展示，不自动换算。
            </p>
            <p>
              原生范围取决于会话来源授权和索引状态；网关保留集上限为 100,000
              条，不代表服务商全部账单。
            </p>
          </div>
        </AccordionContent>
      </AccordionItem>
    </Accordion>
  );
}
export function ProviderHistory({
  providerId,
  filters: controlledFilters,
  onFiltersChange,
}: {
  providerId: string;
  filters?: UsageFilters;
  onFiltersChange?: (filters: UsageFilters) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const [localFilters, setLocalFilters] = useState<UsageFilters>({
    source: "gateway",
  });
  const filters = controlledFilters ?? localFilters;
  const [model, setModel] = useState("all");
  const [cursors, setCursors] = useState<string[]>([]);
  const cursor = cursors.at(-1);
  const resource = useSharedQuery(providerUsageResource, {
    ...filters,
    providerId,
    model: model === "all" ? undefined : model.slice(3),
    cursor,
  });
  const data = resource.data;
  const options = useMemo(
    () => [
      { value: "all", label: "全部历史模型" },
      ...(data?.modelOptions ?? []).map((m) => ({
        value: `id:${m ?? ""}`,
        label: m ?? "模型未记录",
      })),
    ],
    [data?.modelOptions],
  );
  const pagination = useTablePagination(data?.models.length ?? 0, 20);
  const models = useDataTable({
    data: data?.models ?? [],
    columns: modelColumns,
    getRowId: (r) => JSON.stringify(r.model),
    enableRowSelection: false,
    enableHiding: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
    autoResetPageIndex: false,
  });
  const calls = useDataTable({
    data: data?.calls ?? [],
    columns: useMemo(() => callColumns(agentNames), [agentNames]),
    getRowId: (r) => r.id,
    enableRowSelection: false,
    enableHiding: false,
    manualPagination: true,
    manualSorting: true,
  });
  const reset = () => {
    setCursors([]);
  };
  return (
    <section className="space-y-4" aria-label="供应商调用历史">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h3 className="text-title font-medium">调用与用量</h3>
        <Button
          variant="tertiary"
          size="sm"
          onClick={resource.reload}
          loading={resource.loading}
        >
          刷新调用统计
        </Button>
      </div>
      <ProviderUsageFilters
        value={filters}
        onChange={(value) => {
          (onFiltersChange ?? setLocalFilters)(value);
          reset();
        }}
      />
      <div className="max-w-md">
        <Combobox
          items={options.map((o) => o.value)}
          value={model}
          itemToStringLabel={(v) =>
            options.find((o) => o.value === v)?.label ??
            (v.startsWith("id:") ? v.slice(3) : v)
          }
          onValueChange={(v) => {
            setModel(v ?? "all");
            reset();
          }}
          limit={200}
        >
          <ComboboxInput
            aria-label="筛选供应商历史模型"
            placeholder="搜索历史模型"
          />
          <ComboboxContent>
            <ComboboxList>
              {(value: string) => (
                <ComboboxItem key={value} value={value}>
                  {options.find((o) => o.value === value)?.label ?? value}
                </ComboboxItem>
              )}
            </ComboboxList>
          </ComboboxContent>
        </Combobox>
      </div>
      <ProviderUsageStatus
        data={data}
        error={resource.error}
        reload={resource.reload}
      />
      <ProviderUsageSummary
        totals={data?.sourceState === "disabled" ? null : data?.selected}
      />
      {data && data.missingTime > 0 && (
        <p className="text-label text-fg-muted">
          {number(data.missingTime)} 条记录时间未知，按日期筛选时不计入。
        </p>
      )}
      <DataTable
        table={models.table}
        role="region"
        aria-label="供应商按模型用量"
        isLoading={!data && resource.loading}
        emptyMessage="没有匹配的模型调用"
        paginationProps={{ labels: dataTablePagination }}
      />
      <h4 className="font-medium">调用记录 · {number(data?.totalCalls)}</h4>
      <DataTable
        table={calls.table}
        role="region"
        aria-label="供应商调用记录"
        isLoading={!data && resource.loading}
        showPagination={false}
        emptyMessage="没有匹配的调用记录"
      />
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-label text-fg-muted">
          第 {cursors.length + 1} 页 · 每页 40 条
        </p>
        <div className="flex gap-2">
          {cursors.length > 0 && (
            <Button variant="tertiary" size="sm" onClick={reset}>
              返回调用第一页
            </Button>
          )}
          {data?.nextCursor && (
            <Button
              variant="tertiary"
              size="sm"
              onClick={() => setCursors([...cursors, data.nextCursor!])}
            >
              下一页调用
            </Button>
          )}
        </div>
      </div>
      <ProviderUsageNotes />
    </section>
  );
}
