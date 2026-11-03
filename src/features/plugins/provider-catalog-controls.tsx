import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { Field, FieldLabel } from "@/components/ui/field";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import type {
  Provider,
  ProviderModelCatalog,
  ProviderModelInfo,
} from "@/features/providers/types";
import type { Binding } from "../../../contracts/domain/provider-access/types";
import type { PluginStatus } from "./types";
import { providerCatalogApi } from "@/features/providers/api";

type Props = {
  status: PluginStatus;
  providers: Provider[];
  onChanged: () => void;
};
const modelColumns: ColumnDef<ProviderModelInfo>[] = [
  { accessorKey: "id", header: "模型 ID" },
  { accessorKey: "name", header: "名称" },
  {
    id: "window",
    header: "声明上下文",
    cell: ({ row }) =>
      row.original.contextWindow?.toLocaleString("zh-CN") ?? "未知",
  },
  {
    id: "tools",
    header: "声明工具支持",
    cell: ({ row }) =>
      row.original.capabilities?.tools === true
        ? "支持"
        : row.original.capabilities?.tools === false
          ? "不支持"
          : "未知",
  },
];
function Models({ catalog }: { catalog: ProviderModelCatalog }) {
  const { table } = useDataTable({
    data: catalog.models,
    columns: modelColumns,
    getRowId: (r) => r.id,
    enableRowSelection: false,
  });
  return (
    <DataTable
      table={table}
      aria-label="插件读取的模型目录"
      emptyMessage="本次读取确认目录为空"
      paginationProps={{ labels: dataTablePagination }}
    />
  );
}
export function ProviderCatalogControls(props: Props) {
  const epoch = useBackendReadEpoch();
  const [scope, setScope] = useState({ epoch, serial: 0 });
  if (scope.epoch !== epoch) setScope({ epoch, serial: scope.serial + 1 });
  // Retire pending actions across plugin authorization or backend replacement.
  return (
    <CatalogControls
      key={`${props.status.manifest.id}:${props.status.digest}:${props.status.activationEpoch}:${props.status.grantRevision}:${scope.serial}`}
      {...props}
    />
  );
}
function CatalogControls({ status, providers, onChanged }: Props) {
  const query = useMemo(
    () => ({ pluginId: status.manifest.id, digest: status.digest }),
    [status.manifest.id, status.digest],
  );
  const loader = useCallback(() => providerCatalogApi.state(query), [query]);
  const resource = usePollingResource(
    `provider-catalog:${query.pluginId}:${query.digest}:${status.activationEpoch}:${status.grantRevision}`,
    loader,
  );
  const [selected, setSelected] = useState<string | null>(null);
  const [allowPrivate, setAllowPrivate] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [result, setResult] = useState<{
    bindingId: string;
    catalog: ProviderModelCatalog;
  } | null>(null);
  const [pagination, setPagination] = useState({ pageIndex: 0, pageSize: 10 });
  const alive = useRef(true),
    running = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const invalidate = resource.invalidate;
  const run = useCallback(
    async (work: () => Promise<void>) => {
      if (running.current) return;
      running.current = true;
      setBusy(true);
      setError(null);
      setNotice(null);
      setResult(null);
      try {
        await work();
        if (alive.current) {
          await invalidate();
          if (alive.current) onChanged();
        }
      } catch (e) {
        if (alive.current) {
          setError(e instanceof Error ? e.message : String(e));
          void invalidate();
        }
      } finally {
        running.current = false;
        if (alive.current) setBusy(false);
      }
    },
    [invalidate, onChanged],
  );
  const read = useCallback(
    (binding: Binding) => {
      if (!binding.providerId || !binding.expectedRevision) return;
      const target = {
        kind: "saved" as const,
        providerId: binding.providerId,
        expectedRevision: binding.expectedRevision,
      };
      void run(async () => {
        const catalog = await providerCatalogApi.read({
          ...query,
          action: { kind: "read", bindingId: binding.bindingId, target },
        });
        if (alive.current) {
          setResult({ bindingId: binding.bindingId, catalog });
          setNotice("本次目录读取完成；目录声明不代表模型能力或连接已验证。");
        }
      });
    },
    [query, run],
  );
  const unbind = useCallback(
    (binding: Binding) =>
      void run(async () => {
        await providerCatalogApi.change({
          ...query,
          action: { kind: "unbind", bindingId: binding.bindingId },
        });
        if (alive.current)
          setNotice("模型目录绑定已解除，已保存的连接和模型保持。");
      }),
    [query, run],
  );
  const columns = useMemo<ColumnDef<Binding>[]>(
    () => [
      { accessorKey: "label", header: "授权连接" },
      {
        accessorKey: "endpoint",
        header: "服务地址",
        cell: ({ row }) => (
          <span className="break-all">{row.original.endpoint}</span>
        ),
      },
      {
        id: "state",
        header: "目录授权",
        cell: ({ row }) =>
          row.original.authorized
            ? "已授权，需手动读取"
            : "绑定失效，需重新授权",
      },
      {
        id: "actions",
        header: "操作",
        cell: ({ row }) => (
          <div className="flex flex-wrap gap-2">
            <Button
              variant="tertiary"
              aria-label={`读取 ${row.original.label} 的模型目录`}
              disabled={
                busy || !row.original.authorized || !row.original.providerId
              }
              onClick={() => read(row.original)}
            >
              读取目录
            </Button>
            <Button
              variant="tertiary"
              aria-label={`解除 ${row.original.label} 的目录绑定`}
              disabled={busy}
              onClick={() => unbind(row.original)}
            >
              解除绑定
            </Button>
          </div>
        ),
      },
    ],
    [busy, read, unbind],
  );
  const bindings = resource.data?.bindings;
  const rows = useMemo(() => bindings ?? [], [bindings]);
  const visiblePagination = {
    ...pagination,
    pageIndex: Math.min(
      pagination.pageIndex,
      Math.max(0, Math.ceil(rows.length / pagination.pageSize) - 1),
    ),
  };
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (r) => r.bindingId,
    enableSorting: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: visiblePagination },
    onPaginationChange: (update) =>
      setPagination(
        typeof update === "function" ? update(visiblePagination) : update,
      ),
  });
  const policy = resource.data?.policy;
  const choices = providers.filter((p) => p.protocol === policy?.baseProtocol);
  const names = new Map(
    choices.map((p) => [p.id, `${p.name} · ${p.endpoint}`]),
  );
  const provider = choices.find((p) => p.id === selected);
  const resultCurrent =
    result &&
    rows.some(
      (b) =>
        b.bindingId === result.bindingId &&
        b.authorized &&
        (!b.providerId ||
          providers.some(
            (p) =>
              p.id === b.providerId &&
              String(p.revision) === b.expectedRevision,
          )),
    );
  const opted = status.manifest.capabilities.includes("provider.models@1");
  if (!opted && !rows.length) return null;
  return (
    <section className="space-y-4" aria-label="模型目录接入">
      <h3 className="text-title font-medium">模型目录接入</h3>
      <p className="text-label text-fg-muted">
        仅在明确授权后读取指定连接的目录。插件不会获取原始凭据，也不会修改连接、配置或执行模型调用；停用、撤权或升级后需重新授权。
      </p>
      {(error || resource.error) && (
        <Alert status="danger">
          <AlertTitle>模型目录操作未完成</AlertTitle>
          <AlertDescription>{error ?? resource.error}</AlertDescription>
        </Alert>
      )}
      {notice && (
        <p role="status" className="text-label text-fg-muted">
          {notice}
        </p>
      )}
      {resource.loading && !resource.data && (
        <p role="status">正在核对目录能力与绑定…</p>
      )}
      {policy && (
        <div className="space-y-3">
          <p className="text-label text-fg-muted">
            获准目录路径：{policy.path} · 基础协议：{policy.baseProtocol}
          </p>
          <Field>
            <FieldLabel>目录连接</FieldLabel>
            <Combobox<string>
              items={[...names.keys()]}
              value={selected}
              onValueChange={setSelected}
              itemToStringLabel={(id) => names.get(id) ?? id}
              limit={200}
            >
              <ComboboxInput
                aria-label="模型目录连接"
                placeholder="搜索协议匹配的连接"
                triggerAriaLabel="展开目录连接"
              />
              <ComboboxContent>
                <ComboboxEmpty>
                  请先在 Providers 中添加协议匹配的连接。
                </ComboboxEmpty>
                <ComboboxList>
                  {(id: string) => (
                    <ComboboxItem key={id} value={id}>
                      {names.get(id)}
                    </ComboboxItem>
                  )}
                </ComboboxList>
              </ComboboxContent>
            </Combobox>
          </Field>
          <label className="flex items-center gap-2 text-label">
            <Checkbox
              checked={allowPrivate}
              onCheckedChange={(v) => setAllowPrivate(v === true)}
            />
            允许此目录访问本机或局域网地址
          </label>
          {provider && (
            <p className="break-all text-label text-fg-muted">
              将授权：{provider.name} · {provider.endpoint}
              。授权不会自动发送请求。
            </p>
          )}
          <Button
            variant="tertiary"
            disabled={busy || !resource.data?.canBind || !provider}
            onClick={() =>
              provider &&
              void run(async () => {
                await providerCatalogApi.change({
                  ...query,
                  action: {
                    kind: "bind",
                    target: {
                      kind: "saved",
                      providerId: provider.id,
                      expectedRevision: String(provider.revision),
                    },
                    allowPrivateNetwork: allowPrivate,
                  },
                });
                if (alive.current)
                  setNotice(
                    "目录范围已授权。点击对应记录的“读取目录”后才会发送请求。",
                  );
              })
            }
          >
            授权模型目录范围
          </Button>
        </div>
      )}
      {!policy && !resource.loading && (
        <p className="text-label text-fg-muted">
          当前版本不能新增目录授权；已有失效绑定仍可解除。
        </p>
      )}
      <DataTable
        table={table}
        aria-label="模型目录绑定"
        emptyMessage="尚未授权任何目录范围"
        paginationProps={{ labels: dataTablePagination }}
      />
      {resultCurrent && result && (
        <div className="space-y-3">
          <p className="break-all text-label text-fg-muted">
            本次目录来源：{result.catalog.sourceUrl}
            {result.catalog.truncated ? " · 目录范围不完整" : ""}
          </p>
          <Models catalog={result.catalog} />
        </div>
      )}
      {result && !resultCurrent && (
        <p role="status" className="text-label text-fg-muted">
          目录范围已变化，旧结果不再用于本次操作，请重新授权并读取。
        </p>
      )}
    </section>
  );
}
