import { useCallback, useEffect, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import {
  Combobox,
  ComboboxInput,
  ComboboxContent,
  ComboboxList,
  ComboboxItem,
  ComboboxEmpty,
} from "@/components/ui/combobox";
import { AssociatedCostsView } from "@/shared/ui/associated-costs";
import { CostTotalsView } from "@/shared/ui/associated-costs";
import { costText } from "@/shared/lib/money";
import { date, number } from "@/features/sessions/display";
import { ProviderCallCost } from "@/features/providers/components/provider-call-cost";
import { costsApi } from "./api";
import type { CostCall, CostGroup, CostPage, CostQuery } from "./types";

type Option = { value: string; label: string };
function Filter({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: Option[];
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <Combobox
      items={options}
      value={options.find((o) => o.value === value) ?? options[0]}
      onValueChange={(o) => onChange(o?.value ?? "")}
      itemToStringLabel={(o) => o.label}
      isItemEqualToValue={(a, b) => a.value === b.value}
    >
      <ComboboxInput aria-label={label} placeholder={label} />
      <ComboboxContent>
        <ComboboxEmpty>无匹配项</ComboboxEmpty>
        <ComboboxList>
          {(o: Option) => (
            <ComboboxItem key={o.value} value={o}>
              {o.label}
            </ComboboxItem>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  );
}
const callColumns: ColumnDef<CostCall>[] = [
  {
    id: "call",
    header: "调用",
    cell: ({ row: { original: a } }) => (
      <div className="max-w-64">
        <p className="truncate" title={a.model ?? "模型未知"}>
          {a.model ?? "模型未知"}
        </p>
        <p className="truncate text-label text-fg-muted" title={a.provider}>
          {a.provider}
        </p>
        <p className="text-label text-fg-muted">{date(a.time)}</p>
      </div>
    ),
  },
  {
    id: "usage",
    header: "状态 / Token",
    cell: ({ row: { original: a } }) => (
      <>
        <p>
          {(
            {
              success: "成功",
              error: "失败",
              failed: "失败",
              in_progress: "进行中",
              cancelled: "取消",
              interrupted: "结果未知",
            } as Record<string, string>
          )[a.status] ?? "未知"}
        </p>
        <p className="text-label text-fg-muted">
          {number(a.usage.totalTokens)} Token
        </p>
        <p className="text-label text-fg-muted">
          {a.association === "response_id" ? "响应 ID 关联" : "Session ID 关联"}
        </p>
      </>
    ),
  },
  {
    id: "cost",
    header: "本次费用",
    cell: ({ row }) => <ProviderCallCost call={row.original} />,
  },
  {
    id: "request",
    header: "详情",
    cell: ({ row }) => (
      <Button
        variant="tertiary"
        size="sm"
        onClick={() => {
          window.location.hash = `/requests/${encodeURIComponent(row.original.id)}`;
        }}
      >
        请求详情
      </Button>
    ),
  },
];

export function CostDetails({
  scope,
  id,
  agent,
  days,
}: Pick<CostQuery, "scope" | "id" | "agent" | "days">) {
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [groupBy, setGroupBy] = useState("provider");
  const [cursor, setCursor] = useState<string>();
  const [refresh, setRefresh] = useState(0);
  const query = useMemo(
    () => ({
      scope,
      id,
      agent,
      days,
      provider: provider || undefined,
      model: model || undefined,
    }),
    [scope, id, agent, days, provider, model],
  );
  const key = JSON.stringify(query);
  const [state, setState] = useState<{
    key: string;
    data: CostPage | null;
    loading: boolean;
    error: string | null;
  }>({ key, data: null, loading: true, error: null });
  useEffect(() => {
    let alive = true;
    setState((old) => ({
      key,
      data: old.key === key ? old.data : null,
      loading: true,
      error: null,
    }));
    void costsApi
      .details({ ...query, cursor })
      .then((data) => {
        if (alive)
          setState((old) => ({
            key,
            data: {
              ...data,
              calls:
                cursor && old.key === key && old.data
                  ? [...old.data.calls, ...data.calls]
                  : data.calls,
            },
            loading: false,
            error: null,
          }));
      })
      .catch((reason) => {
        if (alive)
          setState((old) => ({
            key,
            data: old.key === key ? old.data : null,
            loading: false,
            error: String(reason instanceof Error ? reason.message : reason),
          }));
      });
    return () => {
      alive = false;
    };
  }, [query, key, cursor, refresh]);
  const data = state.key === key ? state.data : null;
  const loading = state.key !== key || state.loading;
  const choose = useCallback((p: string, m: string) => {
    setProvider(p);
    setModel(m);
    setCursor(undefined);
  }, []);
  const groupColumns = useMemo<ColumnDef<CostGroup>[]>(
    () => [
      {
        id: "name",
        header: () => (groupBy === "provider" ? "Provider" : "模型 / Provider"),
        cell: ({ row }) => (
          <Button
            variant="link"
            size="sm"
            onClick={() =>
              choose(
                row.original.providerKey,
                groupBy === "model" ? (row.original.model ?? "_unknown") : "",
              )
            }
          >
            <span
              className="max-w-64 truncate"
              title={`${row.original.provider} · ${row.original.model ?? "模型未知"}`}
            >
              {groupBy === "provider"
                ? row.original.provider
                : `${row.original.model ?? "模型未知"} · ${row.original.provider}`}
            </span>
          </Button>
        ),
      },
      { accessorKey: "attempts", header: "调用次数" },
      {
        id: "cost",
        header: "费用",
        cell: ({ row }) => costText(row.original.cost),
      },
    ],
    [groupBy, choose],
  );
  const groups =
    (groupBy === "provider" ? data?.filteredProviders : data?.filteredModels) ??
    [];
  const groupTable = useDataTable({
    data: groups,
    columns: groupColumns,
    getRowId: (g) => JSON.stringify([g.providerKey, g.model]),
    enableRowSelection: false,
    manualPagination: true,
    enableSorting: false,
  });
  const callTable = useDataTable({
    data: data?.calls ?? [],
    columns: callColumns,
    getRowId: (a) => a.id,
    enableRowSelection: false,
    manualPagination: true,
    enableSorting: false,
  });
  const providerOptions = [
    { value: "", label: "全部 Provider" },
    ...(data?.providers ?? []).map((g) => ({
      value: g.providerKey,
      label: g.provider,
    })),
  ];
  const modelOptions = [
    { value: "", label: "全部模型" },
    ...Array.from(
      new Set(
        (data?.models ?? [])
          .filter((g) => !provider || g.providerKey === provider)
          .map((g) => g.model),
      ),
    ).map((m) => ({ value: m ?? "_unknown", label: m ?? "模型未知" })),
  ];
  return (
    <section className="min-w-0 space-y-4" aria-label="费用明细">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h3 className="text-title font-medium">费用明细</h3>
        <Button
          variant="tertiary"
          loading={loading}
          onClick={() => {
            setCursor(undefined);
            setRefresh((n) => n + 1);
          }}
        >
          刷新明细
        </Button>
      </div>
      {state.key === key && state.error && (
        <p role="alert">
          {state.error}。{data ? "显示上次快照，请刷新明细。" : "请重新读取。"}
        </p>
      )}
      {data && <AssociatedCostsView value={data.summary} />}
      <div className="grid gap-3 sm:grid-cols-3">
        <Select value={groupBy} onValueChange={setGroupBy}>
          <SelectTrigger aria-label="费用分组" />
          <SelectContent>
            <SelectItem value="provider">按 Provider</SelectItem>
            <SelectItem value="model">按模型</SelectItem>
          </SelectContent>
        </Select>
        <Filter
          label="费用 Provider"
          options={providerOptions}
          value={provider}
          onChange={(p) => choose(p, "")}
        />
        <Filter
          label="费用模型"
          options={modelOptions}
          value={model}
          onChange={(m) => choose(provider, m)}
        />
      </div>
      {data && (provider || model) && (
        <CostTotalsView
          cost={data.total ? data.filteredCost : undefined}
          label="筛选费用"
        />
      )}
      <DataTable
        table={groupTable.table}
        aria-label="费用分组汇总"
        showPagination={false}
        isLoading={loading && !data}
        emptyState="暂无已关联的费用分组"
      />
      <h4 className="text-body font-medium">
        逐次调用 · {data?.total ?? "未知"}
      </h4>
      <DataTable
        table={callTable.table}
        aria-label="费用调用记录"
        showPagination={false}
        isLoading={loading && !data}
        emptyState={
          state.error ? "费用记录暂不可用" : "暂无匹配的网关调用，费用未知"
        }
      />
      {data?.nextCursor && (
        <Button
          variant="tertiary"
          loading={loading}
          disabled={!!state.error}
          onClick={() => setCursor(data.nextCursor!)}
        >
          加载更多费用调用
        </Button>
      )}
      <p className="text-label text-fg-muted">
        每次网关尝试计量一次，重试分别记录。原生响应不重复计费；未关联和缺少计价依据的保留未知。
      </p>
    </section>
  );
}
