import { PluginDetailSlots } from "@/features/plugins/detail-slots";
import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DataTable,
  DataTableColumnHeader,
  useDataTable,
} from "@/components/ui/data-table";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabPanel, TabsList, TabItem } from "@/components/ui/tabs";
import { AccountQuota } from "@/features/accounts/components/account-quota";
import {
  availableProviderModelIds,
  providerCatalog,
  providerModelIds,
  providerVendor,
} from "@/features/providers/catalog";
import type { Provider, ProviderStatus } from "@/features/providers/types";
import { useProviderWrite } from "../hooks/use-provider-write";
import { ProviderWriteStatus } from "./provider-write-status";
import { providerOperationId } from "../operation-receipt";
import { date } from "@/features/sessions/display";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import {
  dataTableControls,
  dataTablePagination,
} from "@/shared/ui/data-table-labels";
import type { ColumnDef } from "@tanstack/react-table";
import { useCallback, useMemo, useState } from "react";

import { message, protocolName, verificationName } from "../display";
import { ProviderHistory } from "./provider-history";
import type { UsageFilters } from "./provider-usage-filters";
import { ProviderOverview } from "./provider-overview";
type ModelRow = {
  id: string;
  provider: Provider;
  available: Set<string>;
  savingModel: string | null;
  writeUnavailable: boolean;
  saveModels: (id: string, changes: Partial<Provider>) => Promise<void>;
};
const modelColumns: ColumnDef<ModelRow>[] = [
  {
    id: "model",
    accessorFn: (row) => row.id,
    header: ({ column }) => (
      <DataTableColumnHeader
        column={column}
        label="模型 ID"
        labels={dataTableControls}
      />
    ),
    cell: ({ row }) => {
      const { id } = row.original;
      return <>{id}</>;
    },
  },
  {
    id: "default",
    header: () => <>默认值</>,
    enableSorting: false,
    cell: ({ row }) => {
      const {
        id,
        provider,
        available,
        savingModel,
        saveModels,
        writeUnavailable,
      } = row.original;
      return (
        <>
          {id === provider.model ? (
            <Badge>默认模型</Badge>
          ) : available.has(id) ? (
            <Button
              variant="tertiary"
              size="sm"
              disabled={!!savingModel || writeUnavailable}
              onClick={() => void saveModels(id, { model: id })}
            >
              设为默认
            </Button>
          ) : (
            <span className="text-fg-subtle">—</span>
          )}
        </>
      );
    },
  },
  {
    id: "enabled",
    header: () => <>可用状态</>,
    enableSorting: false,
    cell: ({ row }) => {
      const {
        id,
        provider,
        available,
        savingModel,
        saveModels,
        writeUnavailable,
      } = row.original;
      return (
        <>
          <div className="flex items-center gap-2">
            <Switch
              label={`启用模型 ${id}`}
              labelVisibility="sr-only"
              checked={available.has(id)}
              disabled={
                !!savingModel || writeUnavailable || !provider.configured
              }
              onCheckedChange={(enabled) =>
                void saveModels(id, {
                  disabledModels: enabled
                    ? (provider.disabledModels ?? []).filter(
                        (model) => model !== id,
                      )
                    : [...(provider.disabledModels ?? []), id],
                })
              }
            />
            <span className="text-label text-fg-muted">
              {savingModel === id
                ? "保存中…"
                : available.has(id)
                  ? "已启用"
                  : "已停用"}
            </span>
          </div>
        </>
      );
    },
  },
];

export function ProviderDetail({
  provider,
  status,
  onEdit,
  onVerify,
  onUpdated,
  historyFilters,
  onHistoryFiltersChange,
  storeId,
}: {
  provider: Provider;
  status: ProviderStatus | null;
  onEdit: (p: Provider) => void;
  onVerify: (p: Provider) => void;
  onUpdated: () => Promise<void>;
  historyFilters?: UsageFilters;
  onHistoryFiltersChange?: (filters: UsageFilters) => void;
  storeId?: string;
}) {
  const [search, setSearch] = useState("");
  const [detailTab, setDetailTab] = useState("overview");
  const [page, setPage] = useState(0);
  const [activeModel, setActiveModel] = useState<string | null>(null);
  const [modelNotice, setModelNotice] = useState<string | null>(null);
  const write = useProviderWrite(storeId, (operation) => {
    if (operation.status !== "committed") return;
    setModelNotice("模型设置已保存。");
    void onUpdated().catch((reason) =>
      setModelNotice(`模型设置已保存，列表暂未刷新：${message(reason)}`),
    );
  });
  const savingModel =
    write.busy || write.unresolved ? (activeModel ?? "_pending") : null;
  const modelError = write.state.kind === "failed" ? write.state.message : null;
  const startWrite = write.start;
  const canWrite = providerOperationId(storeId);
  const vendor = providerCatalog[providerVendor(provider)];
  const verification = status?.verifications.find(
    (v) =>
      v.providerId === provider.id &&
      (v.model ?? provider.model) === provider.model,
  );
  const models = useMemo(() => providerModelIds(provider), [provider]);
  const available = useMemo(
    () => new Set(availableProviderModelIds(provider)),
    [provider],
  );
  const saveModels = useCallback(
    async (id: string, changes: Partial<Provider>) => {
      if (savingModel) return;
      setActiveModel(id);
      setModelNotice(null);
      await startWrite("save", { ...provider, ...changes });
    },
    [provider, savingModel, startWrite],
  );
  const query = search.trim().toLowerCase();
  const matching = useMemo(
    () => models.filter((id) => id.toLowerCase().includes(query)),
    [models, query],
  );
  const [pageSize, setPageSize] = useState(50);
  const pageCount = Math.max(1, Math.ceil(matching.length / pageSize));
  const currentPage = Math.min(page, pageCount - 1);
  const modelRows = useMemo(
    () =>
      matching.map((id) => ({
        id,
        provider,
        available,
        savingModel,
        writeUnavailable: !canWrite,
        saveModels,
      })),
    [matching, provider, available, savingModel, saveModels, canWrite],
  );
  const { table } = useDataTable({
    data: modelRows,
    columns: modelColumns,
    getRowId: (row) => row.id,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: { pageIndex: currentPage, pageSize } },
    onPaginationChange: (updater) => {
      const next =
        typeof updater === "function"
          ? updater({ pageIndex: currentPage, pageSize })
          : updater;
      setPage(next.pageIndex);
      setPageSize(next.pageSize);
    },
  });
  const catalog = provider.modelCatalog;
  const source =
    catalog &&
    catalog.endpoint === provider.endpoint &&
    catalog.protocol === provider.protocol &&
    catalog.credentialRef === provider.credentialRef
      ? catalog
      : null;
  return (
    <div className="space-y-6">
      <Button variant="tertiary" asChild>
        <a href="#/providers">返回供应商列表</a>
      </Button>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <p className="text-label text-fg-subtle">
            {vendor?.name ?? "自定义供应商"} · 连接详情
          </p>
          <h2 className="break-all text-title font-medium">{provider.name}</h2>
          <Badge
            status={
              verification?.state === "verified"
                ? "success"
                : verification?.state === "failed"
                  ? "warning"
                  : "neutral"
            }
          >
            {!provider.configured
              ? "未配置"
              : status
                ? verificationName(verification)
                : "验证状态未读取"}
          </Badge>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            disabled={!!savingModel}
            onClick={() => onEdit(provider)}
          >
            配置连接
          </Button>
          <Button
            variant="neutral"
            disabled={
              !provider.configured ||
              !available.has(provider.model) ||
              !!savingModel
            }
            onClick={() => onVerify(provider)}
          >
            验证默认模型
          </Button>
        </div>
      </div>
      <Tabs
        value={detailTab}
        onValueChange={setDetailTab}
        variant="underline"
        color="default"
      >
        <TabsList aria-label="供应商详情">
          <TabItem value="overview" label="概览" />
          <TabItem value="history" label="调用与用量" />
          <TabItem value="models" label="模型" />
          <TabItem value="connection" label="连接与额度" />
        </TabsList>
        <TabPanel value="overview">
          <ProviderOverview providers={[provider]} providerId={provider.id} />
        </TabPanel>
        <TabPanel value="history">
          <ProviderHistory
            key={provider.id}
            providerId={provider.id}
            filters={historyFilters}
            onFiltersChange={onHistoryFiltersChange}
          />
        </TabPanel>
        <TabPanel value="connection">
          <DetailList aria-label="供应商连接信息">
            <DetailListItem>
              <DetailListLabel>Endpoint</DetailListLabel>
              <DetailListValue className="break-all">
                {provider.endpoint || "未设置"}
              </DetailListValue>
            </DetailListItem>
            <DetailListItem>
              <DetailListLabel>接口协议</DetailListLabel>
              <DetailListValue>
                {protocolName(provider.protocol)}
              </DetailListValue>
            </DetailListItem>
            <DetailListItem>
              <DetailListLabel>默认模型</DetailListLabel>
              <DetailListValue className="break-all">
                {provider.model || "未设置"}
              </DetailListValue>
            </DetailListItem>
            <DetailListItem>
              <DetailListLabel>凭据</DetailListLabel>
              <DetailListValue>
                {provider.credentialLabel ?? "未设置凭据"}
              </DetailListValue>
            </DetailListItem>
            <DetailListItem>
              <DetailListLabel>配置版本</DetailListLabel>
              <DetailListValue>{provider.revision}</DetailListValue>
            </DetailListItem>
          </DetailList>
          {verification && (
            <p className="text-label text-fg-muted">
              默认模型验证：{date(verification.checkedAt)} ·{" "}
              {verification.message}
            </p>
          )}
          <AccountQuota providerId={provider.id} />
          {vendor?.documents && (
            <a
              className="text-body text-fg-default underline"
              href={vendor.documents}
              target="_blank"
              rel="noreferrer"
            >
              官方接口文档
            </a>
          )}
        </TabPanel>
        <TabPanel value="models">
          <section
            className="space-y-4"
            aria-labelledby="provider-models-title"
          >
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <div className="flex items-center gap-2">
                  <h3
                    id="provider-models-title"
                    className="text-title font-medium"
                  >
                    模型
                  </h3>
                  <Badge>{models.length} 个</Badge>
                  <Badge>{available.size} 个已启用</Badge>
                </div>
                <p className="mt-1 text-body text-fg-muted">
                  已启用模型可加入模型服务。停用不改变已发布服务或现有兼容配置。
                </p>
              </div>
              <Button
                variant="secondary"
                disabled={!!savingModel}
                onClick={() => onEdit(provider)}
              >
                获取与补充模型
              </Button>
            </div>
            <p className="text-label text-fg-subtle" title={source?.sourceUrl}>
              {source
                ? `最近获取 ${date(source.fetchedAt)}`
                : "手动添加 / 本机导入"}
            </p>
            <ProviderWriteStatus write={write} />
            {!canWrite && (
              <Alert status="warning">
                <AlertTitle>模型配置暂不可修改</AlertTitle>
                <AlertDescription>
                  当前后台未提供保存结果核查，请升级或刷新原后台；模型列表仍可查看。
                </AlertDescription>
              </Alert>
            )}
            {modelError && (
              <Alert status="danger">
                <AlertTitle>模型设置未完成</AlertTitle>
                <AlertDescription>
                  {modelError}。请刷新连接后重试。
                </AlertDescription>
              </Alert>
            )}
            {modelNotice && (
              <p role="status" className="text-body text-fg-muted">
                {modelNotice}
              </p>
            )}
            {models.length > 0 && !available.size && (
              <Alert status="warning">
                <AlertTitle>没有已启用模型</AlertTitle>
                <AlertDescription>
                  启用至少一个模型后，才能将此来源加入模型服务。
                </AlertDescription>
              </Alert>
            )}
            {available.size > 0 &&
              provider.model &&
              !available.has(provider.model) && (
                <p className="text-body text-fg-muted">
                  当前来源默认模型已停用。可启用它，或选择其他已启用模型并验证；模型服务的默认目标单独维护。
                </p>
              )}
            {models.length ? (
              <>
                <Field className="max-w-md">
                  <FieldLabel>搜索模型 ID</FieldLabel>
                  <Input
                    value={search}
                    placeholder="输入模型 ID"
                    onChange={(e) => {
                      setSearch(e.target.value);
                      setPage(0);
                    }}
                  />
                </Field>
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <p className="text-label text-fg-muted" role="status">
                    {query
                      ? `找到 ${matching.length} 个模型，共 ${models.length} 个`
                      : `共 ${models.length} 个模型`}
                  </p>
                  {search && (
                    <Button
                      variant="tertiary"
                      size="sm"
                      onClick={() => {
                        setSearch("");
                        setPage(0);
                      }}
                    >
                      清除搜索
                    </Button>
                  )}
                </div>
                {matching.length ? (
                  <DataTable
                    table={table}
                    role="region"
                    aria-label="供应商模型目录"
                    paginationProps={{
                      labels: dataTablePagination,
                      pageSizeOptions: [20, 50, 100],
                    }}
                  />
                ) : (
                  <ConnectionEmpty
                    compact
                    title="没有匹配的模型"
                    description="尝试其他关键词，或清除搜索查看全部模型。"
                    action="清除搜索"
                    onAction={() => {
                      setSearch("");
                      setPage(0);
                    }}
                  />
                )}
              </>
            ) : (
              <ConnectionEmpty
                compact
                title="尚未登记模型"
                description="在配置中获取服务商模型目录，也可以手动添加模型 ID。"
                action="获取与管理模型"
                onAction={() => onEdit(provider)}
              />
            )}
          </section>
        </TabPanel>
      </Tabs>
      <PluginDetailSlots target="provider" objectId={provider.id} />
      <ObjectContributionSlots
        object={{ target: "provider", id: provider.id }}
      />
    </div>
  );
}
