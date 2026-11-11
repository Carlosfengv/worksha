import { ExternalRoutingPolicy } from "./components/external-routing-policy";
import { useMemo, useRef, useState } from "react";
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
import { useIam } from "@/features/iam/context";
import { IamResourceDetail } from "@/features/iam/resource-detail";
import { ServiceEditor } from "./components/service-editor";
import {
  commonCapabilities,
  type ModelService,
  type ModelServiceCatalog,
} from "./types";

type Row = {
  service: ModelService;
  connections: number;
  routingStatus: string[];
  configurationUnknown: boolean;
  disabled: boolean;
  edit: () => void;
  detail?: () => void;
  remove: () => void;
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
      const { service, detail, edit, disabled } = row.original;
      const providers = new Set(
        service.models.filter((m) => !m.child).map((m) => m.provider.id),
      ).size;
      const children = service.models.filter((m) => m.child).length;
      return (
        <div className="space-y-1">
          <Button
            variant="link"
            size="sm"
            className="max-w-56 justify-start px-0"
            title={service.name}
            onClick={detail ?? edit}
            disabled={disabled}
          >
            <span className="truncate">{service.name}</span>
          </Button>
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
  {
    id: "actions",
    header: "操作",
    cell: ({ row }) => (
      <div className="flex gap-1">
        <Button
          variant="tertiary"
          size="sm"
          onClick={row.original.detail ?? row.original.edit}
          disabled={row.original.disabled}
        >
          编辑<span className="sr-only"> {row.original.service.name}</span>
        </Button>
        <Button
          variant="tertiary"
          size="sm"
          onClick={row.original.remove}
          disabled={row.original.disabled}
        >
          删除<span className="sr-only"> {row.original.service.name}</span>
        </Button>
      </div>
    ),
  },
];

export function ModelServicesPage({ providers }: { providers: Provider[] }) {
  const catalog = useSharedRead(modelServicesResource);
  const showDetails = !!useIam().status?.enabled;
  const [view, setView] = useState("services");
  const [detail, setDetail] = useState("");
  const [editor, setEditor] = useState<ModelService | "new" | null>(null);
  const [removing, setRemoving] = useState<ModelService | null>(null);
  const removingStore = useRef<string | undefined>(undefined);
  const deletion = useServiceWrite(() => setRemoving(null));
  const { busy, reset: resetDeletion } = deletion;
  const rows = useMemo(
    () =>
      (catalog.data?.services ?? []).map((service) => ({
        service,
        disabled: !!catalog.error || busy,
        routingStatus: routingStatus(service, catalog.data?.routingHealth),
        connections:
          catalog.data?.attachments.filter(
            (a) => a.serviceId === service.id && a.referenced,
          ).length ?? 0,
        configurationUnknown:
          catalog.data?.attachments.some(
            (a) => a.serviceId === service.id && a.configurationUnknown,
          ) ?? false,
        edit: () => setEditor(service),
        detail: showDetails ? () => setDetail(service.id) : undefined,
        remove: () => {
          resetDeletion();
          removingStore.current = catalog.data?.writeProtocol?.storeId;
          setRemoving(service);
        },
      })),
    [catalog.data, catalog.error, busy, resetDeletion, showDetails],
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
  const remove = async () => {
    if (!removing || busy || deletion.uncertain) return;
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
  return (
    <div className="min-w-0 space-y-4">
      <PageHeader className="px-0">
        <h2 className="text-title font-medium">模型服务</h2>
        <PageActions>
          <Button
            variant="tertiary"
            loading={catalog.loading}
            onClick={catalog.reload}
          >
            重新读取
          </Button>
          <Button
            onClick={() => {
              setView("services");
              setEditor("new");
            }}
            disabled={!!catalog.error || !catalog.data}
          >
            创建模型服务
          </Button>
        </PageActions>
      </PageHeader>
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
      {policyStore && catalog.data ? (
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
      )}
      {detail && (
        <IamResourceDetail id={detail} onClose={() => setDetail("")} />
      )}
      {editor && (
        <ServiceEditor
          key={editor === "new" ? "new" : editor.versionId}
          service={editor === "new" ? null : editor}
          providers={providers}
          services={catalog.data?.services ?? []}
          originStoreId={catalog.data?.writeProtocol?.storeId}
          onClose={() => setEditor(null)}
        />
      )}
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
              disabled={deletion.uncertain}
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
