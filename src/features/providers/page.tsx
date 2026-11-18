import type { Snapshot } from "@/app/types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { PageActions, PageHeader } from "@/components/ui/page-layout";
import { Tabs, TabsList, TabItem, TabPanel } from "@/components/ui/tabs";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { providersApi } from "@/features/providers/api";
import { bundledDshAgent } from "@/features/agents/bundled-source";
import { NativeProviderImport } from "@/features/providers/components/native-provider-import";
import type { Provider } from "@/features/providers/types";
import { useSharedRead, useSharedQuery } from "@/shared/hooks/use-shared-read";
import { providerStatusResource, providerUsageResource } from "./queries";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useEffect, useRef, useState } from "react";
import { useWorkspaceNavigation } from "@/shared/hooks/use-workspace-navigation";
import { agentDetailHref } from "@/features/agents/components/agent-detail-layout";
import { useServiceWrite } from "@/features/model-services/hooks/use-service-write";
import { modelServicesApi } from "@/features/model-services/api";
import { defaultCapabilities } from "@/features/model-services/types";
import { ServiceWriteFeedback } from "@/features/model-services/components/service-write-feedback";
import { GatewayCostComparison } from "./components/gateway-cost-comparison";
import { AccountQuota } from "@/features/accounts/components/account-quota";

import { ProviderDetail } from "./components/provider-detail";
import {
  ProviderEditor,
  providerWizardFormId,
} from "./components/provider-editor";
import { ProviderOperationRecovery } from "./components/provider-operation-recovery";
import { ProviderTable } from "./components/provider-table";
import { ProviderOverview } from "./components/provider-overview";
import { ProviderModelInventory } from "./components/provider-model-inventory";
import { PluginEntries } from "@/features/plugins/entries";
import { message, protocolName } from "./display";
import {
  ProviderHistory,
  ProviderUsageNotes,
  ProviderUsageStatus,
  ProviderUsageSummary,
} from "./components/provider-history";
import {
  ProviderUsageFilters,
  providerHistoryHref,
  type UsageFilters,
} from "./components/provider-usage-filters";
export function ProvidersPage({
  data,
  refresh,
  selectedId,
  selectedTab,
  routeSearch = "",
  canCreateDefaultService = false,
}: {
  data: Snapshot | null;
  refresh: () => Promise<void>;
  selectedId?: string;
  selectedTab?: string;
  routeSearch?: string;
  canCreateDefaultService?: boolean;
}) {
  const status = useSharedRead(providerStatusResource, !!data);
  const fallbackListTab =
    selectedTab === "quotas" || selectedTab === "models"
      ? selectedTab
      : "connections";
  const [listSelection, setListSelection] = useState({
    entry: selectedTab,
    value: fallbackListTab,
  });
  const listTab =
    listSelection.entry === selectedTab ? listSelection.value : fallbackListTab;
  const setListTab = (value: string) =>
    setListSelection({ entry: selectedTab, value });
  const [usageFilters, setUsageFilters] = useState<UsageFilters>(() => {
    const params = new URLSearchParams(routeSearch);
    const days = Number(params.get("days"));
    return {
      source: params.get("source") === "native" ? "native" : "gateway",
      agent: params.get("agent") ?? undefined,
      days: [1, 7, 30].includes(days) ? days : undefined,
    };
  });
  const usage = useSharedQuery(
    providerUsageResource,
    usageFilters,
    !!data && !selectedId && listTab === "connections",
  );
  const [draft, setDraft] = useState<Provider | null>(null);
  const [verify, setVerify] = useState<Provider | null>(null);
  const [busy, setBusy] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [bindingHref, setBindingHref] = useState<string | null>(null);
  const pendingBinding = useRef<{ serviceId: string; href: string } | null>(
    null,
  );
  const workspace = useWorkspaceNavigation();
  const active = useRef(false);
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  const openBinding = (href: string, notice: string) => {
    setBindingHref(href);
    setNotice(notice);
    if (active.current) workspace.open(href, { pin: true });
  };
  const createService = useServiceWrite((receipt) => {
    const pending = pendingBinding.current;
    if (pending?.serviceId !== receipt.serviceId) return;
    pendingBinding.current = null;
    openBinding(pending.href, "供应商与同名默认模型服务已就绪。");
  });
  const createDefault = async (provider: Provider) => {
    if (createService.busy || createService.uncertain)
      throw Error("请先核查上一条模型服务保存结果，再创建新服务。");
    const catalog = await modelServicesApi.catalog();
    if (!active.current) return;
    const clients =
      data?.agents.filter(
        (agent) =>
          agent.found &&
          !agent.readonly &&
          !agent.sourceId &&
          ["claude", "codex", "opencode", "claude-desktop"].includes(
            agent.configId,
          ),
      ) ?? [];
    const preferredId = {
      messages: "claude",
      responses: "codex",
      "chat-completions": "opencode",
    }[provider.protocol];
    const target =
      clients.find((agent) => agent.configId === preferredId) ??
      (clients.length === 1 ? clients[0] : null);
    const href = target ? agentDetailHref(target.id, "connection") : "#/agents";
    const existing = catalog.services.find(
      (service) => service.name === provider.name,
    );
    if (existing) {
      if (
        existing.models.some(
          (model) =>
            model.id === existing.defaultModelId &&
            !model.child &&
            !existing.routingPolicy?.disabledModels.includes(model.id) &&
            model.provider.id === provider.id &&
            model.provider.model === provider.model,
        )
      ) {
        openBinding(href, "已复用同名默认模型服务。");
      } else
        setError(
          "供应商已保存，已有同名服务使用不同的模型来源，请在模型服务中检查。",
        );
      return;
    }
    const id = crypto.randomUUID();
    const serviceId = crypto.randomUUID();
    pendingBinding.current = { serviceId, href };
    await createService.start(
      {
        kind: "save",
        draft: {
          id: serviceId,
          revision: 0,
          name: provider.name,
          defaultModelId: id,
          models: [
            {
              id,
              name: provider.model,
              providerId: provider.id,
              model: provider.model,
              capabilities: defaultCapabilities(),
            },
          ],
        },
      },
      catalog.writeProtocol?.storeId,
      async (_progress, isCurrent, storeId) => {
        const result = await modelServicesApi.verifyMember({
          storeId,
          providerId: provider.id,
          expectedRevision: provider.revision,
          model: provider.model,
        });
        if (isCurrent() && result.state !== "verified")
          throw Error(result.message);
      },
    );
  };

  const updated = async () => {
    setNotice(null);
    await refresh();
    void status.invalidate();
  };
  const refreshConfiguration = async () => {
    if (refreshing) return;
    setRefreshing(true);
    setError(null);
    try {
      await Promise.all([refresh(), status.reload()]);
    } catch (reason) {
      setError(message(reason));
    } finally {
      setRefreshing(false);
    }
  };
  const verifyConnection = async () => {
    if (!verify || busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const result = await providersApi.verifyProvider(
        verify.id,
        verify.revision,
      );
      if (result.state !== "verified") {
        setError(result.message);
        return;
      }
      setNotice(result.message);
      setVerify(null);
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
      void status.invalidate();
    }
  };
  if (!data)
    return (
      <div className="min-w-0 space-y-4">
        <PageHeader className="px-0">
          <h2 className="text-title font-medium">模型与额度</h2>
        </PageHeader>
        <ConnectionEmpty title="尚未连接服务商配置" icon="globe" />
      </div>
    );
  const freshStatus = status.error ? null : status.data;
  const selected = data.providers.find((p) => p.id === selectedId);
  const configured = data.providers.filter((p) => p.configured);
  const adding =
    !!draft &&
    !selectedId &&
    !draft.configured &&
    draft.revision === 0 &&
    draft.id.startsWith("custom-");
  const edit = (p: Provider) => {
    setDraft(p);
    setError(null);
    setNotice(null);
    setBindingHref(null);
  };
  const requestVerification = (p: Provider) => {
    setVerify(p);
    setError(null);
  };
  return (
    <div className="min-w-0 space-y-4">
      {!selectedId && (
        <PageHeader className="px-0">
          <h2 className="text-title font-medium">模型与额度</h2>
          <PageActions>
            {!adding && <NativeProviderImport onImported={updated} />}
            {adding ? (
              <Button
                key="return"
                type="reset"
                form={providerWizardFormId}
                variant="secondary"
              >
                返回列表
              </Button>
            ) : (
              <Button
                key="add"
                type="button"
                onClick={(event) => {
                  event.preventDefault();
                  edit({
                    id: `custom-${crypto.randomUUID()}`,
                    name: "",
                    vendor: "custom",
                    endpoint: "",
                    protocol: "responses",
                    model: "",
                    models: [],
                    credentialRef: null,
                    credentialLabel: null,
                    category: "custom",
                    configured: false,
                    revision: 0,
                  });
                }}
              >
                添加供应商
              </Button>
            )}
          </PageActions>
        </PageHeader>
      )}
      {notice && (
        <Alert>
          <AlertTitle>服务商配置</AlertTitle>
          <AlertDescription>{notice}</AlertDescription>
        </Alert>
      )}
      {error && !verify && (
        <Alert status="danger">
          <AlertTitle>操作未完成</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {selectedId ? (
        selectedId === "_unknown" ? (
          <>
            <PageHeader className="px-0">
              <h2 className="text-title font-medium">未归属调用</h2>
              <PageActions>
                <Button variant="tertiary" asChild>
                  <a href="#/providers">返回供应商列表</a>
                </Button>
              </PageActions>
            </PageHeader>
            <ProviderHistory
              key="_unknown"
              providerId="_unknown"
              filters={usageFilters}
              onFiltersChange={setUsageFilters}
            />
          </>
        ) : selected ? (
          <ProviderDetail
            key={`${data.providerWriteProtocol?.storeId ?? "unknown"}:${selected.id}`}
            provider={selected}
            initialTab={selectedTab}
            storeId={data.providerWriteProtocol?.storeId}
            status={freshStatus}
            onEdit={edit}
            onVerify={requestVerification}
            onUpdated={updated}
            onRefresh={() => void refreshConfiguration()}
            refreshing={refreshing}
            historyFilters={usageFilters}
            onHistoryFiltersChange={setUsageFilters}
          />
        ) : (
          <ConnectionEmpty
            title="未找到供应商连接"
            description="该连接可能已移除，请返回供应商列表重新选择。"
            icon="globe"
            action="返回供应商列表"
            onAction={() => {
              window.location.hash = "/providers";
            }}
          />
        )
      ) : !adding ? (
        <>
          <Tabs
            value={listTab}
            onValueChange={setListTab}
            variant="segment"
            color="default"
          >
            <TabsList aria-label="Provider 页面">
              <TabItem value="connections" label="供应商连接" />
              <TabItem value="models" label="全部模型" />
              <TabItem value="quotas" label="额度与账单" />
            </TabsList>
            <TabPanel value="quotas" className="space-y-3 pt-3">
              <AccountQuota />
              <GatewayCostComparison />
              <ProviderOverview
                providers={data.providers}
                dshConnectionHref={`#/agents/${encodeURIComponent(bundledDshAgent.id)}?tab=connection`}
              />
            </TabPanel>
            <TabPanel value="models" keepMounted className="pt-3">
              <ProviderModelInventory
                storeId={data.providerWriteProtocol?.storeId}
                providers={data.providers}
                active={listTab === "models"}
              />
            </TabPanel>
            <TabPanel value="connections" className="pt-3">
              <section
                className="min-w-0 space-y-3"
                aria-label="服务商连接列表"
              >
                <p className="text-label text-fg-muted">
                  已添加 · {configured.length}
                </p>
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <ProviderUsageFilters
                    value={usageFilters}
                    onChange={setUsageFilters}
                  />
                  <Button
                    variant="tertiary"
                    size="sm"
                    loading={usage.loading}
                    onClick={usage.reload}
                  >
                    刷新调用统计
                  </Button>
                </div>
                <ProviderUsageStatus
                  data={usage.data}
                  error={usage.error}
                  reload={usage.reload}
                />
                <ProviderTable
                  providers={configured}
                  usageFilters={usageFilters}
                  status={freshStatus}
                  onEdit={edit}
                  onVerify={requestVerification}
                  usage={
                    usage.data?.sourceState === "disabled" ? null : usage.data
                  }
                />
                <ProviderUsageSummary
                  totals={
                    usage.data?.sourceState === "disabled"
                      ? null
                      : usage.data?.summary
                  }
                />
                {usage.data && usage.data.unattributed.calls > 0 && (
                  <Button variant="link" size="sm" asChild>
                    <a href={providerHistoryHref("_unknown", usageFilters)}>
                      未归属调用 ·{" "}
                      {usage.data.unattributed.calls.toLocaleString("zh-CN")} 次
                    </a>
                  </Button>
                )}
                <ProviderUsageNotes />
              </section>
            </TabPanel>
          </Tabs>
          <PluginEntries kind="providers" />
        </>
      ) : null}
      <ServiceWriteFeedback operation={createService} />
      {bindingHref && (
        <Button variant="secondary" asChild>
          <a href={bindingHref}>前往绑定本机 Agent</a>
        </Button>
      )}
      <ProviderOperationRecovery
        storeId={data.providerWriteProtocol?.storeId}
        compact
        hasPending={!!data.providerWriteProtocol?.pendingOperationIds.length}
        onUpdated={updated}
      />
      {status.error && (
        <Alert status="warning">
          <AlertTitle>暂时无法读取验证状态</AlertTitle>
          <AlertDescription>
            {message(status.error)}
            <Button variant="tertiary" size="sm" onClick={status.reload}>
              重新读取
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {draft && (
        <ProviderEditor
          key={draft.id}
          provider={draft}
          storeId={data.providerWriteProtocol?.storeId}
          onClose={() => setDraft(null)}
          guidedCreation={adding}
          canCreateDefaultService={canCreateDefaultService}
          onSaved={async (saved, makeService) => {
            await updated();
            if (!active.current) return;
            setDraft(null);
            setNotice("已保存模型来源。");
            if (makeService && canCreateDefaultService) {
              try {
                await createDefault(saved);
              } catch (reason) {
                setError(`供应商已保存，默认服务创建失败：${message(reason)}`);
              }
            } else if (adding) {
              workspace.open(
                `#/providers/${encodeURIComponent(saved.id)}?tab=models`,
                { pin: true },
              );
            }
          }}
        />
      )}
      <Dialog
        open={!!verify}
        onOpenChange={(open) => {
          if (!open && !busy) setVerify(null);
        }}
      >
        <DialogContent
          size="lg"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogHeader>
            <DialogTitle>验证 {verify?.name}</DialogTitle>
            <DialogDescription>
              将向以下地址发送 1 次模型请求，最多生成 16 个输出 Token，可能产生
              API 费用。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3">
            <p className="break-all text-body">{verify?.endpoint}</p>
            <p className="text-body text-fg-muted">
              {verify && protocolName(verify.protocol)} ·{" "}
              {verify?.model || "未填写模型"} · 凭据：
              {verify?.credentialLabel ?? "本地无鉴权"}
            </p>
            <p className="text-label text-fg-subtle">
              验证仅确认当前协议和模型可返回结果；完整工具循环及 MCP 能力需在
              Agent 中确认。
            </p>
            {error && (
              <Alert status="danger">
                <AlertTitle>未完成验证</AlertTitle>
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setVerify(null)}
            >
              取消
            </Button>
            <Button
              variant="neutral"
              loading={busy}
              disabled={busy || !verify?.model}
              onClick={() => void verifyConnection()}
            >
              验证协议与模型
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
