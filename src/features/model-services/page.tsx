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
import { ServiceEditor } from "./components/service-editor";
import { commonCapabilities, type ModelService } from "./types";

type Row = {
  service: ModelService;
  connections: number;
  cooling: number;
  healthUnknown: boolean;
  configurationUnknown: boolean;
  disabled: boolean;
  edit: () => void;
  remove: () => void;
};
const columns: ColumnDef<Row>[] = [
  {
    id: "service",
    header: "模型服务",
    cell: ({ row }) => (
      <div className="space-y-1">
        <Button
          variant="link"
          size="sm"
          onClick={row.original.edit}
          disabled={row.original.disabled}
        >
          {row.original.service.name}
        </Button>
        <p className="text-label text-fg-muted">
          版本 {row.original.service.revision} ·{" "}
          {new Set(row.original.service.models.map((m) => m.provider.id)).size}{" "}
          个 Provider
        </p>
      </div>
    ),
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
            文本{c.tools ? " / 标准工具" : ""} · 关闭思考
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
        <Badge status={row.original.cooling ? "warning" : "neutral"}>
          {row.original.service.routingPolicy?.enabled
            ? "优先级与回退"
            : "仅首选目标"}
        </Badge>
        <p className="text-fg-muted">
          {row.original.healthUnknown
            ? "冷却状态未知"
            : row.original.cooling
              ? `${row.original.cooling} 个模型来源冷却中`
              : "来源无冷却记录"}
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
          onClick={row.original.edit}
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
        cooling:
          catalog.data?.routingHealth?.filter(
            (h) => h.serviceId === service.id && h.status.state === "cooling",
          ).length ?? 0,
        healthUnknown:
          !catalog.data?.routingHealth ||
          catalog.data.routingHealth.some(
            (h) => h.serviceId === service.id && h.status.state === "unknown",
          ),
        connections:
          catalog.data?.attachments.filter(
            (a) => a.serviceId === service.id && a.referenced,
          ).length ?? 0,
        configurationUnknown:
          catalog.data?.attachments.some(
            (a) => a.serviceId === service.id && a.configurationUnknown,
          ) ?? false,
        edit: () => setEditor(service),
        remove: () => {
          resetDeletion();
          removingStore.current = catalog.data?.writeProtocol?.storeId;
          setRemoving(service);
        },
      })),
    [catalog.data, catalog.error, busy, resetDeletion],
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
  return (
    <div className="space-y-6">
      <ModelServiceOperationRecovery />
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-2">
          <h2 className="text-heading-sm font-semibold">模型服务</h2>
          <p className="max-w-2xl text-body text-fg-muted">
            把不同 Provider 的模型组织为一个服务。Agent
            选择服务后，可使用默认目标或指定模型。
          </p>
        </div>
        <div className="flex gap-2">
          <Button
            variant="tertiary"
            loading={catalog.loading}
            onClick={catalog.reload}
          >
            重新读取
          </Button>
          <Button
            onClick={() => setEditor("new")}
            disabled={!!catalog.error || !catalog.data}
          >
            创建模型服务
          </Button>
        </div>
      </div>
      {catalog.error && (
        <Alert status="danger" role="alert">
          <AlertTitle>模型服务读取失败</AlertTitle>
          <AlertDescription>
            {catalog.error}。保留上次数据，重新读取后再修改。
          </AlertDescription>
        </Alert>
      )}
      {!catalog.data && !catalog.error ? (
        <p role="status" className="text-body text-fg-muted">
          正在读取模型服务…
        </p>
      ) : (
        <DataTable
          table={table}
          aria-label="模型服务列表"
          role="region"
          showPagination={false}
          emptyMessage="尚未创建模型服务。先配置 Provider，再创建服务并选择默认模型。"
        />
      )}
      <p className="text-label text-fg-muted">
        Provider 负责模型来源与凭据；模型服务负责对外目录。服务目标调整不改
        Agent 文件，目录或能力变化需要重新同步。
      </p>
      {catalog.data?.writeProtocol?.storeId && !catalog.error && (
        <ExternalRoutingPolicy
          key={catalog.data.writeProtocol.storeId}
          storeId={catalog.data.writeProtocol.storeId}
          services={catalog.data.services}
        />
      )}
      {editor && (
        <ServiceEditor
          key={editor === "new" ? "new" : editor.versionId}
          service={editor === "new" ? null : editor}
          providers={providers}
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
