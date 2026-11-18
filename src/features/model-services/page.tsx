import { ExternalRoutingPolicy } from "./components/external-routing-policy";
import { useEffect, useMemo, useRef, useState } from "react";
import { useServiceWrite } from "./hooks/use-service-write";
import { ServiceWriteFeedback } from "./components/service-write-feedback";
import { ModelServiceOperationRecovery } from "./components/operation-recovery";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { PageActions, PageHeader } from "@/components/ui/page-layout";
import { TabItem, TabPanel, Tabs, TabsList } from "@/components/ui/tabs";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Provider } from "@/features/providers/types";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { modelServicesResource } from "./queries";
import { useWorkspaceNavigation } from "@/shared/hooks/use-workspace-navigation";
import { useIam } from "@/features/iam/context";
import { IamResourceDetail } from "@/features/iam/resource-detail";
import {
  ServiceEditor,
  type ServiceEditorDraftState,
} from "./components/service-editor";
import {
  commonCapabilities,
  serviceDraft,
  defaultRoutingPolicy,
  type ModelServiceDraft,
  type ModelService,
  type ModelServiceCatalog,
} from "./types";

type Row = {
  service: ModelService;
  connections: number;
  routingStatus: string[];
  configurationUnknown: boolean;
};

function routingStatus(
  service: ModelService,
  health: ModelServiceCatalog["routingHealth"],
) {
  const counts = {
    ready: 0,
    cooling: 0,
    unknown: 0,
    waitingForProbe: 0,
    probing: 0,
  };
  const entries =
    health?.filter((entry) => entry.serviceId === service.id) ?? [];
  for (const entry of entries) counts[entry.status.state]++;
  return [
    counts.cooling ? `${counts.cooling} 个模型来源冷却中` : "",
    counts.waitingForProbe
      ? `${counts.waitingForProbe} 个模型来源待恢复探测`
      : "",
    counts.probing ? `${counts.probing} 个模型来源恢复探测中` : "",
    counts.unknown || entries.length < service.models.length
      ? "冷却状态未知"
      : "",
  ].filter(Boolean);
}

const columns: ColumnDef<Row>[] = [
  {
    id: "service",
    header: "模型服务",
    cell: ({ row }) => {
      const { service } = row.original;
      const providers = new Set(
        service.models.filter((m) => !m.child).map((m) => m.provider.id),
      ).size;
      const children = service.models.filter((m) => m.child).length;
      return (
        <div className="space-y-1">
          <a
            href={serviceHref(service.id)}
            data-workspace-resource
            data-workspace-title={service.name}
            className="block max-w-56 truncate font-medium hover:underline"
            title={service.name}
          >
            {service.name}
          </a>
          <p className="text-label text-fg-muted">
            {[
              `版本 ${service.revision}`,
              providers ? `${providers} 个 Provider` : "",
              children ? `${children} 个子服务` : "",
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </div>
      );
    },
  },
  {
    id: "models",
    header: "可选模型",
    cell: ({ row }) => (
      <span className="block max-w-64 whitespace-normal break-words text-label">
        默认 · {row.original.service.models.map((m) => m.name).join(" · ")}
      </span>
    ),
  },
  {
    id: "default",
    header: "默认目标",
    cell: ({ row }) => (
      <span className="text-body">
        {row.original.service.models.find(
          (m) => m.id === row.original.service.defaultModelId,
        )?.name ?? "未知"}
      </span>
    ),
  },
  {
    id: "capabilities",
    header: "默认入口能力",
    cell: ({ row }) => {
      const c = commonCapabilities(row.original.service);
      return (
        <div className="space-y-1 text-label">
          <p>{c.contextWindow.toLocaleString()} Token 上下文</p>
          <p className="text-fg-muted">
            文本{c.tools ? " / 标准工具" : ""} · 思考控制关闭
          </p>
        </div>
      );
    },
  },
  {
    id: "policy",
    header: "路由策略",
    cell: ({ row }) => (
      <div className="space-y-1 text-label">
        <Badge
          status={row.original.routingStatus.length ? "warning" : "neutral"}
        >
          {row.original.service.routingPolicy?.enabled
            ? "优先级与回退"
            : "仅首选目标"}
        </Badge>
        <p className="text-fg-muted">
          {row.original.routingStatus.join(" · ") || "来源无冷却记录"}
        </p>
      </div>
    ),
  },
  {
    id: "connections",
    header: "Agent 目录",
    cell: ({ row }) => (
      <Badge status={row.original.configurationUnknown ? "warning" : "neutral"}>
        {row.original.connections} 个目录引用
        {row.original.configurationUnknown ? " · 有配置未知" : ""}
      </Badge>
    ),
  },
];

const serviceHref = (id: string) =>
  `#/model-services/${encodeURIComponent(id)}`;

export function ModelServicesPage({
  providers,
  selectedId,
  cloneFrom,
}: {
  providers: Provider[];
  selectedId?: string;
  cloneFrom?: string;
}) {
  const catalog = useSharedRead(modelServicesResource);
  const member = useIam().status?.account?.role === "member";
  const workspace = useWorkspaceNavigation();
  const draftState = useRef<ServiceEditorDraftState | null>(null);
  const [view, setView] = useState("services");
  const services = catalog.data?.services;
  const selected = services?.find((service) => service.id === selectedId);
  const draftId = selectedId?.startsWith("draft-")
    ? selectedId.slice(6)
    : undefined;
  const clone = services?.find((service) => service.id === cloneFrom);
  const initialDraft: ModelServiceDraft | undefined = draftId
    ? {
        ...(clone
          ? serviceDraft(clone)
          : {
              models: [],
              defaultModelId: "",
              routingPolicy: defaultRoutingPolicy(),
            }),
        id: draftId,
        revision: 0,
        name: clone ? `${clone.name} 副本` : "",
      }
    : undefined;
  const title = selected?.name ?? (draftId ? "新建模型服务" : undefined);
  const { setTitle } = workspace;
  useEffect(() => {
    if (title) setTitle(title);
  }, [setTitle, title]);
  const [removing, setRemoving] = useState<ModelService | null>(null);
  const removingStore = useRef<string | undefined>(undefined);
  const deletion = useServiceWrite(() => {
    setRemoving(null);
    workspace.close();
  });
  const { busy, reset: resetDeletion } = deletion;
  const rows = useMemo(
    () =>
      (catalog.data?.services ?? []).map((service) => ({
        service,
        routingStatus: routingStatus(service, catalog.data?.routingHealth),
        connections:
          catalog.data?.attachments.filter(
            (a) => a.serviceId === service.id && a.referenced,
          ).length ?? 0,
        configurationUnknown:
          catalog.data?.attachments.some(
            (a) => a.serviceId === service.id && a.configurationUnknown,
          ) ?? false,
      })),
    [catalog.data],
  );
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.service.id,
    manualPagination: true,
    enableRowSelection: false,
    enableSorting: false,
    enableHiding: false,
  });
  const attachments =
    catalog.data?.attachments.filter(
      (item) => item.serviceId === selectedId && item.referenced,
    ) ?? [];
  const parents =
    services?.filter((service) =>
      service.models.some((model) => model.child?.serviceId === selectedId),
    ) ?? [];
  const referenced = attachments.length > 0 || parents.length > 0;
  const remove = async () => {
    if (!removing || referenced || busy || deletion.uncertain) return;
    await deletion.start(
      {
        kind: "delete",
        serviceId: removing.id,
        expectedRevision: removing.revision,
      },
      removingStore.current,
    );
  };
  const policyStore = catalog.error
    ? undefined
    : catalog.data?.writeProtocol?.storeId;
  const list =
    !catalog.data && !catalog.error ? (
      <p role="status" className="text-body text-fg-muted">
        正在读取模型服务…
      </p>
    ) : catalog.data && (!catalog.error || rows.length > 0) ? (
      <div className="min-w-0 space-y-3">
        {rows.length > 0 && (
          <p className="text-label text-fg-muted">{rows.length} 个模型服务</p>
        )}
        <DataTable
          table={table}
          aria-label="模型服务列表"
          role="region"
          showPagination={false}
          emptyMessage="尚未创建模型服务。先配置 Provider，再创建服务并选择默认模型。"
        />
      </div>
    ) : null;
  const openDraft = (from?: string) =>
    workspace.open(
      `${serviceHref(`draft-${crypto.randomUUID()}`)}${from ? `?from=${encodeURIComponent(from)}` : ""}`,
      { pin: true, title: "新建模型服务" },
    );

  return (
    <div className="min-w-0 space-y-4">
      {!selectedId && (
        <PageHeader className="px-0">
          <h2 className="text-title font-medium">模型服务与路由</h2>
          <PageActions>
            <Button
              variant="tertiary"
              loading={catalog.loading}
              onClick={catalog.reload}
            >
              重新读取
            </Button>
            <Button
              onClick={() => openDraft()}
              disabled={!!catalog.error || !catalog.data}
            >
              + 创建模型服务
            </Button>
          </PageActions>
        </PageHeader>
      )}
      <ModelServiceOperationRecovery />
      {catalog.error && (
        <Alert status="danger" role="alert">
          <AlertTitle>模型服务读取失败</AlertTitle>
          <AlertDescription>
            {catalog.error}
            {catalog.data && " 正显示上次读取的服务，刷新后可修改。"}
          </AlertDescription>
        </Alert>
      )}
      {!selectedId &&
        (policyStore && catalog.data ? (
          <Tabs
            value={view}
            onValueChange={setView}
            variant="underline"
            color="default"
          >
            <TabsList aria-label="模型服务工作区">
              <TabItem value="services" label="服务列表" />
              <TabItem value="routing" label="外部路由策略" />
            </TabsList>
            <TabPanel value="services" className="pt-3">
              {list}
            </TabPanel>
            <TabPanel value="routing" keepMounted className="pt-3">
              <ExternalRoutingPolicy
                key={policyStore}
                storeId={policyStore}
                services={catalog.data.services}
              />
            </TabPanel>
          </Tabs>
        ) : (
          list
        ))}
      {selectedId &&
        (member ? (
          <IamResourceDetail id={selectedId} onClose={workspace.dismiss} />
        ) : (selected || initialDraft) && services && catalog.data ? (
          <>
            {selected && (
              <div className="flex flex-wrap items-center justify-between gap-2">
                <Badge>rev#{selected.revision}</Badge>
                <span className="text-label text-fg-muted">
                  {routingStatus(selected, catalog.data?.routingHealth).join(
                    " · ",
                  )}
                </span>
                <div className="flex gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => openDraft(selected.id)}
                  >
                    克隆为新服务
                  </Button>
                  <Button
                    size="sm"
                    variant="tertiary"
                    onClick={() => {
                      resetDeletion();
                      removingStore.current = policyStore;
                      setRemoving(selected);
                    }}
                  >
                    删除服务
                  </Button>
                </div>
              </div>
            )}
            <ServiceEditor
              key={selectedId}
              tabId={selectedId}
              draftState={draftState}
              service={selected ?? null}
              initialDraft={initialDraft}
              providers={providers}
              services={services}
              originStoreId={policyStore}
              onClose={workspace.dismiss}
              onDirtyChange={workspace.setDirty}
              onSaved={(receipt) => {
                if (draftId) {
                  workspace.open(serviceHref(receipt.serviceId), {
                    pin: true,
                    title: receipt.service?.name,
                  });
                  workspace.close(false);
                }
              }}
            />
          </>
        ) : (
          <p role="status">
            {catalog.loading ? "正在读取模型服务…" : "找不到此模型服务"}
          </p>
        ))}
      <Dialog
        open={!!removing}
        onOpenChange={(open) => {
          if (!open && !busy) setRemoving(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>删除模型服务</DialogTitle>
            <DialogDescription>
              删除“{removing?.name}
              ”后不再接受新接入。已有调用版本与加密备份保留；正在被 Agent
              使用的服务会拒绝删除。
            </DialogDescription>
          </DialogHeader>
          {referenced && (
            <Alert status="warning">
              <AlertTitle>服务仍被引用</AlertTitle>
              <AlertDescription>
                <ul className="space-y-1">
                  {attachments.map((item) => (
                    <li key={item.bindingId}>
                      <a
                        className="underline"
                        href={`#/agents/${encodeURIComponent(item.agentId)}?tab=connection`}
                      >
                        {item.agentId} · 前往解绑
                      </a>
                    </li>
                  ))}
                  {parents.map((parent) => (
                    <li key={parent.id}>
                      <a className="underline" href={serviceHref(parent.id)}>
                        {parent.name} · 父服务
                      </a>
                    </li>
                  ))}
                </ul>
              </AlertDescription>
            </Alert>
          )}
          <ServiceWriteFeedback operation={deletion} />
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setRemoving(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              disabled={deletion.uncertain || referenced}
              onClick={() => void remove()}
            >
              删除服务
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
