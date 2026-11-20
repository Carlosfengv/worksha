import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import { useWorkspaceNavigation } from "@/shared/hooks/use-workspace-navigation";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { modelServicesResource } from "@/features/model-services/queries";
import { gatewayStatusResource } from "@/features/settings/queries";
import { ExtensionSlot } from "@/features/plugins/extension-slot";
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
import { PageActions, PageHeader } from "@/components/ui/page-layout";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabPanel, TabsList, TabItem } from "@/components/ui/tabs";
import { AccountQuota } from "@/features/accounts/components/account-quota";
import { providerSourceApi } from "../api";
import { catalogAuthority } from "../catalog-authority";
import { providerCatalogSource } from "../model-inventory";
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
import { useCallback, useEffect, useMemo, useState } from "react";

import { message, protocolName, verificationName } from "../display";
import { ProviderHistory } from "./provider-history";
import type { UsageFilters } from "./provider-usage-filters";
import { ProviderOverview } from "./provider-overview";
import { UpstreamAccounts } from "./upstream-accounts";
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
  initialTab = "overview",
  status,
  onEdit,
  onVerify,
  onUpdated,
  onRefresh,
  refreshing,
  historyFilters,
  onHistoryFiltersChange,
  storeId,
}: {
  provider: Provider;
  initialTab?: string;
  status: ProviderStatus | null;
  onEdit: (p: Provider) => void;
  onVerify: (p: Provider) => void;
  onUpdated: () => Promise<void>;
  onRefresh?: () => void;
  refreshing?: boolean;
  historyFilters?: UsageFilters;
  onHistoryFiltersChange?: (filters: UsageFilters) => void;
  storeId?: string;
}) {
  const [search, setSearch] = useState("");
  const entryTab = ["history", "models", "connection", "accounts"].includes(
    initialTab,
  )
    ? initialTab
    : "overview";
  const [tabSelection, setTabSelection] = useState({
    entry: entryTab,
    value: entryTab,
  });
  const detailTab =
    tabSelection.entry === entryTab ? tabSelection.value : entryTab;
  const setDetailTab = (value: string) =>
    setTabSelection({ entry: entryTab, value });
  const [page, setPage] = useState(0);
  const [activeModel, setActiveModel] = useState<string | null>(null);
  const [modelNotice, setModelNotice] = useState<string | null>(null);
  const [customModels, setCustomModels] = useState("");
  const [impact, setImpact] = useState<string[]>([]);
  const [removeOpen, setRemoveOpen] = useState(false);
  const workspace = useWorkspaceNavigation();
  const { setTitle } = workspace;
  useEffect(() => setTitle(provider.name), [provider.name, setTitle]);
  const services = useSharedRead(modelServicesResource);
  const gateway = useSharedRead(gatewayStatusResource, removeOpen);
  const providerServices = useMemo(
    () =>
      services.data?.services.filter((service) =>
        service.models.some(
          (model) => !model.child && model.provider.id === provider.id,
        ),
      ) ?? [],
    [provider.id, services.data],
  );
  const clients =
    gateway.data?.clients.filter(
      (client) => client.enabled && client.providerId === provider.id,
    ) ?? [];
  const references = [
    ...providerServices.map((service) => service.name),
    ...clients.map((client) => client.name),
  ];
  const remove = useProviderWrite(storeId, (receipt) => {
    if (receipt.status !== "committed") return;
    void onUpdated();
    workspace.close();
  });

  const write = useProviderWrite(storeId, (operation) => {
    if (operation.status !== "committed") return;
    if (activeModel === "_custom") setCustomModels("");
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
      if (savingModel || !storeId) return;
      setActiveModel(id);
      setModelNotice(null);
      const disabled =
        changes.disabledModels?.filter(
          (model) => !(provider.disabledModels ?? []).includes(model),
        ) ?? [];
      setImpact(
        providerServices
          .filter((service) =>
            service.models.some(
              (model) =>
                !model.child &&
                model.provider.id === provider.id &&
                disabled.includes(model.provider.model),
            ),
          )
          .map((service) => service.name),
      );
      const query = {
        storeId,
        providerId: provider.id,
        expectedRevision: String(provider.revision),
      };
      let readAuthority: string | null = null;
      await startWrite(
        "save",
        { ...provider, ...changes },
        id === "_catalog"
          ? async () => {
              const state = await providerSourceApi.state(query);
              readAuthority = catalogAuthority(state);
              const catalog = await providerSourceApi.read(
                query,
                state.sourceRevision ?? null,
              );
              if (!catalog.models.length)
                throw Error("上游目录为空，已登记模型保留。");
              return {
                ...provider,
                models: [
                  ...new Set([
                    ...models,
                    ...catalog.models.map((model) => model.id),
                  ]),
                ],
                modelCatalog: providerCatalogSource(provider, catalog),
              };
            }
          : undefined,
        async () => {
          const state = await providerSourceApi.state(query);
          if (readAuthority && catalogAuthority(state) !== readAuthority)
            throw Error("目录来源已变化，请重新同步。");
          if (state.kind === "builtin")
            return {
              source: { kind: "builtin" },
              expectedSourceRevision: state.sourceRevision ?? null,
            };
          if (state.kind === "external" && state.authorized && state.selection)
            return {
              source: { kind: "external", catalog: state.selection },
              expectedSourceRevision: state.sourceRevision ?? null,
            };
          throw Error("请先在配置连接中核对目录来源与授权。");
        },
      );
    },
    [models, provider, providerServices, savingModel, startWrite, storeId],
  );
  const additions = customModels
    .split(/[,，\n]/)
    .map((id) => id.trim())
    .filter(Boolean);
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
    <div className="min-w-0 space-y-4">
      <PageHeader className="px-0">
        <div className="min-w-0 space-y-1">
          <h2 className="sr-only">{provider.name}</h2>
          <p className="text-label text-fg-subtle">
            {vendor?.name ?? "自定义供应商"} · 连接详情
          </p>
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
        <PageActions>
          <ExtensionSlot
            points={["provider.detail.actions"]}
            context={{ target: "provider", objectId: provider.id }}
          />
          <Button
            variant="tertiary"
            size="sm"
            disabled={!provider.configured || !canWrite || !!savingModel}
            onClick={() => setRemoveOpen(true)}
          >
            删除供应商
          </Button>
          {onRefresh && (
            <Button
              variant="tertiary"
              loading={refreshing}
              disabled={refreshing}
              onClick={onRefresh}
            >
              刷新配置
            </Button>
          )}
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
        </PageActions>
      </PageHeader>
      {impact.length > 0 && (
        <Alert status="warning">
          <AlertTitle>停用模型影响下游服务</AlertTitle>
          <AlertDescription>{impact.join("、")}</AlertDescription>
        </Alert>
      )}
      <Dialog
        open={removeOpen}
        onOpenChange={(open) => {
          if (!remove.busy) setRemoveOpen(open);
        }}
      >
        <DialogContent>
          <DialogTitle>删除供应商</DialogTitle>
          <DialogDescription>
            移除连接及当前钥匙串引用，历史请求归属和账单记录仍可查询。
          </DialogDescription>
          {references.length > 0 && (
            <Alert status="warning">
              <AlertTitle>供应商仍被下游引用</AlertTitle>
              <AlertDescription>{references.join("、")}</AlertDescription>
            </Alert>
          )}
          <ProviderWriteStatus write={remove} />
          <DialogFooter>
            <Button
              variant="secondary"
              disabled={remove.busy}
              onClick={() => setRemoveOpen(false)}
            >
              取消
            </Button>
            <Button
              variant="destructive"
              disabled={
                references.length > 0 ||
                remove.unresolved ||
                gateway.loading ||
                services.loading
              }
              loading={remove.busy}
              onClick={() =>
                void remove.start("delete", {
                  ...provider,
                  credentialRef: null,
                  credentialLabel: null,
                  modelCatalog: null,
                })
              }
            >
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
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
          <TabItem value="accounts" label="上游账号" />
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
        <TabPanel value="accounts">
          <UpstreamAccounts
            key={provider.id}
            provider={provider}
            storeId={storeId}
          />
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
                disabled={!!savingModel || !canWrite}
                loading={savingModel === "_catalog"}
                onClick={() => void saveModels("_catalog", {})}
              >
                从上游同步模型目录
              </Button>
            </div>
            <form
              className="flex flex-wrap items-end gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                if (additions.length)
                  void saveModels("_custom", {
                    models: [...new Set([...models, ...additions])],
                  });
              }}
            >
              <Field className="min-w-48 flex-1">
                <FieldLabel>自定义模型 ID</FieldLabel>
                <Input
                  value={customModels}
                  onChange={(event) => setCustomModels(event.target.value)}
                  placeholder="多个模型 ID 用逗号分隔"
                  maxLength={400000}
                  disabled={!!savingModel || !canWrite}
                />
              </Field>
              <Button
                type="submit"
                disabled={!additions.length || !!savingModel || !canWrite}
                loading={savingModel === "_custom"}
              >
                手动补充模型
              </Button>
            </form>
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
                description="可从上游同步目录，或输入已知的模型 ID。"
              />
            )}
          </section>
        </TabPanel>
      </Tabs>
      <ExtensionSlot
        points={["provider.detail.card", "provider.detail.tab"]}
        legacyDetail
        context={{ target: "provider", objectId: provider.id }}
        object={{ target: "provider", id: provider.id }}
      />
    </div>
  );
}
