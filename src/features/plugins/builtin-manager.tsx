import { useEffect, useRef, useState, type ReactNode } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { DesktopError } from "@/shared/lib/desktop-command";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import type {
  Composition,
  CompositionCatalog,
  CompositionChange,
  PluginInfo,
} from "../../../contracts/plugins/builtin/types";
import { builtinPluginsApi } from "./api";
import { builtinChange, builtinReply } from "./builtin-contract";
import {
  BUILTIN_OPERATIONS_CHANGED,
  builtinPending,
  forgetBuiltin,
  rememberBuiltin,
  requireBuiltinPending,
} from "./builtin-pending";
import {
  compositionSelection,
  editableBuiltinSelection,
  toggleBuiltin,
  removeBuiltin,
  installBuiltin,
} from "./builtin-plan";
import { builtinPluginResource } from "./queries";
import { systemPluginPresentation } from "./scenarios";
import { businessDisableEffects } from "./system-features";

export type BuiltinPluginRow = {
  plugin: PluginInfo;
  disabled: boolean;
  toggle: (enabled: boolean) => void;
  installed: boolean;
  remove: () => void;
  install: () => void;
};
const columns: ColumnDef<BuiltinPluginRow>[] = [
  {
    id: "name",
    header: "插件",
    cell: ({ row }) => (
      <>
        <p className="font-medium">{row.original.plugin.displayName}</p>
        <p className="text-label text-fg-muted">
          随应用分发 · {row.original.plugin.version}
        </p>
      </>
    ),
  },
  {
    id: "state",
    header: "当前运行",
    cell: ({ row }) => (
      <Badge
        status={
          row.original.plugin.runtimeActive
            ? "success"
            : row.original.plugin.desiredEnabled
              ? "warning"
              : "neutral"
        }
      >
        {row.original.plugin.runtimeActive
          ? "运行中"
          : row.original.plugin.desiredEnabled
            ? "运行异常"
            : "已停用"}
      </Badge>
    ),
  },
  {
    id: "scope",
    header: "能力与资源",
    cell: ({ row }) => (
      <details className="max-w-80 text-label text-fg-muted">
        <summary className="cursor-pointer rounded-lg outline-none focus-visible:ring-1 focus-visible:ring-focus-ring">
          {row.original.plugin.capabilities.length} 项能力 ·{" "}
          {row.original.plugin.resources.reduce(
            (sum, resource) => sum + resource.count,
            0,
          )}{" "}
          项资源
        </summary>
        <div className="mt-2 space-y-1 break-all">
          <p>{row.original.plugin.id}</p>
          {row.original.plugin.capabilities.map((capability) => (
            <p
              key={`${capability.address.name}:${capability.address.instance}`}
            >
              {capability.address.name} · v{capability.address.version} ·{" "}
              {capability.address.instance}
            </p>
          ))}
          <p>
            必需依赖 {row.original.plugin.requires.length} 项 · 可选依赖{" "}
            {row.original.plugin.optionalRequires.length} 项
          </p>
        </div>
      </details>
    ),
  },
  {
    id: "enabled",
    header: "启用",
    cell: ({ row }) => (
      <div className="flex justify-end">
        <Switch
          label={`启用 ${row.original.plugin.displayName}`}
          labelVisibility="sr-only"
          checked={row.original.plugin.runtimeActive}
          disabled={row.original.disabled}
          onCheckedChange={row.original.toggle}
        />
      </div>
    ),
  },
];
export function BuiltinSupportTable({ rows }: { rows: BuiltinPluginRow[] }) {
  const pagination = useTablePagination(rows.length, 10);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.plugin.id,
    enableSorting: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  return (
    <DataTable
      table={table}
      aria-label="内部支撑组件"
      emptyMessage="此业务的完整后台生命周期尚未接入统一宿主"
      showPagination={rows.length > 10}
      paginationProps={{ labels: dataTablePagination }}
    />
  );
}
const noWriteCodes = new Set([
  "invalid_input",
  "operation_store_changed",
  "stale_snapshot",
  "dependency_missing",
  "dependency_cycle",
  "capability_conflict",
  "operation_conflict",
  "resource_limit",
  "restart_required",
  "busy",
]);
function sameBase(request: CompositionChange, catalog: CompositionCatalog) {
  return (
    request.storeId === catalog.storeId &&
    request.expectedGeneration === catalog.host.generation &&
    request.expectedRevision === catalog.revision
  );
}
export function BuiltinPluginManager({
  refreshSnapshot,
  renderCatalog,
  externalBusy = false,
}: {
  refreshSnapshot?: () => Promise<void>;
  renderCatalog?: (rows: BuiltinPluginRow[], loading: boolean) => ReactNode;
  externalBusy?: boolean;
}) {
  const resource = useSharedRead(builtinPluginResource);
  const [review, setReview] = useState<CompositionChange | null>(null);
  const [pending, setPending] = useState(builtinPending);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const inFlight = useRef(false);
  useEffect(() => {
    const changed = () => setPending(builtinPending());
    window.addEventListener(BUILTIN_OPERATIONS_CHANGED, changed);
    window.addEventListener("storage", changed);
    return () => {
      window.removeEventListener(BUILTIN_OPERATIONS_CHANGED, changed);
      window.removeEventListener("storage", changed);
    };
  }, []);
  const catalog = resource.data;
  const restorableCollectors = (catalog?.host.plugins ?? []).filter(
    (plugin) =>
      plugin.capabilities.some((cap) => cap.address.name === "collector") &&
      !catalog?.composition.removedPluginIds?.includes(plugin.id) &&
      plugin.requires
        .filter((key) => key.name === "business.feature")
        .every((key) =>
          catalog?.host.plugins.some(
            (owner) => owner.id === key.instance && owner.runtimeActive,
          ),
        ),
  );
  const unavailable = !!resource.error || resource.loading || !catalog;
  const blocked =
    busy ||
    externalBusy ||
    pending.invalid > 0 ||
    !pending.available ||
    !!pending.requests.find((request) => request.storeId === catalog?.storeId);
  const editingDisabled = blocked || unavailable || !!review;
  const preview = (composition: Composition) => {
    if (!catalog || editingDisabled) return;
    setError(null);
    setNotice(null);
    setReview(
      builtinChange({
        operationId: crypto.randomUUID(),
        storeId: catalog.storeId,
        expectedRevision: catalog.revision,
        expectedGeneration: catalog.host.generation,
        composition,
        ...(catalog.impact ? { expectedImpact: catalog.impact } : {}),
      }),
    );
  };
  const rows: BuiltinPluginRow[] = (catalog?.host.plugins ?? []).map(
    (plugin) => ({
      plugin,
      installed: !catalog?.composition.removedPluginIds?.includes(plugin.id),
      remove: () => {
        if (catalog)
          preview(
            removeBuiltin(
              catalog.host.plugins,
              editableBuiltinSelection(
                catalog.host.plugins,
                catalog.composition,
              ),
              plugin.id,
            ),
          );
      },
      install: () => {
        if (catalog)
          preview(
            installBuiltin(
              catalog.host.plugins,
              editableBuiltinSelection(
                catalog.host.plugins,
                catalog.composition,
              ),
              plugin.id,
            ),
          );
      },
      disabled:
        editingDisabled ||
        !!catalog?.problem ||
        plugin.desiredEnabled !== plugin.runtimeActive,
      toggle: (enabled) => {
        if (!catalog || editingDisabled) return;
        try {
          const editable = editableBuiltinSelection(
            catalog.host.plugins,
            catalog.composition,
          );
          preview(
            toggleBuiltin(catalog.host.plugins, editable, plugin.id, enabled),
          );
        } catch (reason) {
          setError(reason instanceof Error ? reason.message : String(reason));
        }
      },
    }),
  );
  const pagination = useTablePagination(rows.length, 10);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.plugin.id,
    enableSorting: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  const finish = async (request: CompositionChange) => {
    forgetBuiltin(request);
    setReview(null);
    setNotice("插件变更已保存，当前状态见下表。来源读取授权仍单独管理。");
    await resource.invalidate();
    await refreshSnapshot?.();
  };
  const submit = async (request: CompositionChange, retry = false) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    setNotice(null);
    let remembered = retry;
    try {
      if (!retry) {
        if (!catalog || unavailable || !sameBase(request, catalog))
          throw Error("功能状态已变化，请重新读取后核对。");
        rememberBuiltin(request);
        remembered = true;
      }
      requireBuiltinPending(request);
      await builtinPluginsApi.change(request);
      await finish(request);
    } catch (reason) {
      setReview(null);
      if (
        remembered &&
        !retry &&
        reason instanceof DesktopError &&
        !reason.outcomeUnknown &&
        noWriteCodes.has(reason.code)
      ) {
        try {
          forgetBuiltin(request);
        } catch (storage) {
          setError(
            storage instanceof Error ? storage.message : String(storage),
          );
          return;
        }
      }
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setPending(builtinPending());
      setBusy(false);
      inFlight.current = false;
    }
  };
  const inspect = async (request: CompositionChange) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      requireBuiltinPending(request);
      const reply = builtinReply(
        await builtinPluginsApi.operation({
          storeId: request.storeId,
          operationId: request.operationId,
        }),
        request,
      );
      if (reply.receipt) await finish(request);
      else
        setNotice(
          "尚未查到已提交回执，已保留原编号。可继续查询，或按原编号与原参数重试；没有自动重放。",
        );
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setPending(builtinPending());
      setBusy(false);
      inFlight.current = false;
    }
  };
  const reviewSelection =
    review && catalog
      ? compositionSelection(catalog.host.plugins, review.composition)
      : new Set<string>();
  const affected =
    review && catalog
      ? catalog.host.plugins.filter(
          (plugin) =>
            reviewSelection.has(plugin.id) !== plugin.desiredEnabled ||
            !!review.composition.removedPluginIds?.includes(plugin.id) !==
              !!catalog.composition.removedPluginIds?.includes(plugin.id),
        )
      : [];
  return (
    <section
      aria-label={renderCatalog ? "插件运行管理" : "官方只读插件"}
      className="space-y-4"
    >
      {!renderCatalog && (
        <div className="space-y-1">
          <h3 className="text-body font-medium">官方只读插件</h3>
          <p className="text-label text-fg-muted">
            管理会话来源、项目读取、账户来源与后台采集。启停仅影响下列只读能力；任务工作台由应用单独管理。
          </p>
        </div>
      )}
      {(resource.error || error) && (
        <Alert>
          <AlertTitle>功能状态待核查</AlertTitle>
          <AlertDescription>{error ?? resource.error}</AlertDescription>
        </Alert>
      )}
      {notice && (
        <p role="status" className="text-body text-fg-muted">
          {notice}
        </p>
      )}
      {catalog &&
        (catalog.problem || catalog.effectiveProfile === "recovery") && (
          <Alert>
            <AlertTitle>应用功能需要恢复</AlertTitle>
            <AlertDescription>
              恢复后可重新访问已有功能；不会重新发送任务或恢复已撤销的数据授权。如果资源回收失败，需要正常重启应用。
            </AlertDescription>
            <Button
              variant="secondary"
              disabled={editingDisabled}
              onClick={() =>
                preview({
                  profile: "desktop",
                  disabledPluginIds: [],
                  ...(catalog.composition.removedPluginIds?.length
                    ? { removedPluginIds: catalog.composition.removedPluginIds }
                    : {}),
                })
              }
            >
              恢复应用功能
            </Button>
          </Alert>
        )}
      {catalog &&
        !catalog.problem &&
        catalog.effectiveProfile !== "recovery" &&
        restorableCollectors.some((plugin) => !plugin.runtimeActive) && (
          <Alert>
            <AlertTitle>部分数据尚未自动更新</AlertTitle>
            <AlertDescription>
              可恢复会话、活动和本地服务的数据更新。将先核对所需组件；不会自动读取未授权来源。
            </AlertDescription>
            <Button
              variant="secondary"
              disabled={editingDisabled}
              onClick={() => {
                try {
                  let target = editableBuiltinSelection(
                    catalog.host.plugins,
                    catalog.composition,
                  );
                  for (const plugin of restorableCollectors.filter(
                    (plugin) => !plugin.runtimeActive,
                  ))
                    target = toggleBuiltin(
                      catalog.host.plugins,
                      target,
                      plugin.id,
                      true,
                    );
                  preview(target);
                } catch (reason) {
                  setError(
                    reason instanceof Error ? reason.message : String(reason),
                  );
                }
              }}
            >
              恢复自动更新
            </Button>
          </Alert>
        )}
      {!renderCatalog && (
        <Button
          variant="tertiary"
          disabled={busy}
          onClick={() => void resource.reload()}
        >
          重新读取功能状态
        </Button>
      )}
      <p className="text-label text-fg-muted">
        开关显示最近读取的实际运行状态，点击后核对影响范围，确认成功才会切换。插件启用、来源授权和模型连接分别管理；停用本机来源会撤销读取授权，重新启用后需再次确认。
      </p>
      {renderCatalog ? (
        renderCatalog(rows, !catalog && resource.loading)
      ) : (
        <DataTable
          table={table}
          aria-label="官方插件列表"
          isLoading={!catalog && resource.loading}
          showPagination={rows.length > 10}
          paginationProps={{
            labels: dataTablePagination,
            pageSizeOptions: [10, 20],
          }}
        />
      )}
      {(!pending.available || pending.invalid > 0) && (
        <Alert>
          <AlertTitle>恢复记录不可用</AlertTitle>
          <AlertDescription>
            已暂停新的插件变更，原记录未删除。请恢复本机存储或核查已有记录。
          </AlertDescription>
        </Alert>
      )}
      {pending.requests.map((request) => (
        <section
          key={request.operationId}
          className="space-y-2 rounded-xl border border-border p-4"
          aria-label="待核查的插件操作"
        >
          <p className="text-body font-medium">插件变更结果待核查</p>
          <p className="break-all text-label text-fg-muted">
            操作编号 {request.operationId}
          </p>
          <p className="text-label text-fg-muted">
            {request.storeId === catalog?.storeId
              ? "保留原请求，查询不会重新保存。"
              : "此记录属于其他后台，需连接原后台核查；未转移到当前数据库。"}
          </p>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="tertiary"
              disabled={busy || request.storeId !== catalog?.storeId}
              onClick={() => void inspect(request)}
            >
              查询原结果
            </Button>
            <Button
              variant="ghost"
              disabled={busy || request.storeId !== catalog?.storeId}
              onClick={() => void submit(request, true)}
            >
              按原编号重试
            </Button>
          </div>
        </section>
      ))}
      <Dialog
        open={!!review}
        onOpenChange={(open) => {
          if (!open && !busy) setReview(null);
        }}
      >
        <DialogContent
          size="lg"
          className="flex max-h-[calc(100dvh-2rem)] flex-col"
        >
          <DialogHeader className="shrink-0">
            <DialogTitle>确认插件变更</DialogTitle>
            <DialogDescription>
              将调整以下功能。应用中仍有任务运行时，需要先处理任务再修改；不会自动停止
              Agent。
            </DialogDescription>
          </DialogHeader>
          {review && (
            <div className="min-h-0 space-y-3 overflow-y-auto text-body">
              <ul className="list-disc space-y-1 pl-5">
                {affected.map((plugin) => (
                  <li key={plugin.id}>
                    {renderCatalog
                      ? systemPluginPresentation(plugin).title
                      : plugin.displayName}{" "}
                    ·{" "}
                    {compositionSelection(
                      catalog?.host.plugins ?? [],
                      review.composition,
                    ).has(plugin.id)
                      ? catalog?.composition.removedPluginIds?.includes(
                          plugin.id,
                        )
                        ? "安装并启用"
                        : "启用"
                      : review.composition.removedPluginIds?.includes(plugin.id)
                        ? "卸载"
                        : "停用"}
                  </li>
                ))}
              </ul>
              {affected.some(
                (p) =>
                  p.id.startsWith("builtin.") && !reviewSelection.has(p.id),
              ) && (
                <div className="space-y-2">
                  <p className="font-medium">停用后不可使用的功能与服务</p>
                  <ul className="list-disc space-y-1 pl-5">
                    {affected
                      .filter(
                        (p) =>
                          p.id.startsWith("builtin.") &&
                          !reviewSelection.has(p.id),
                      )
                      .map((p) => (
                        <li key={p.id}>
                          {p.displayName}：
                          {businessDisableEffects[p.id] ?? "相关功能将不可用。"}
                        </li>
                      ))}
                  </ul>
                  {affected.some(
                    (p) =>
                      ["builtin.providers", "builtin.model-services"].includes(
                        p.id,
                      ) && !reviewSelection.has(p.id),
                  ) && (
                    <p>
                      {catalog?.impact?.modelServices.length
                        ? `受影响的模型服务：${catalog.impact.modelServices.join("、")}。已有 Agent 配置不会自动改写，其本机入口可能连接失败。`
                        : "当前没有已发布的模型服务。"}
                    </p>
                  )}
                </div>
              )}
              {catalog?.impact?.activeTasks && (
                <p role="alert">
                  仍有运行中或等待审批的任务，请先返回任务页处理，当前不能提交插件变更。
                </p>
              )}
              <p className="text-label text-fg-muted">
                卸载后该功能将不可用，保留历史数据与配置；系统自带插件可重新安装。不会改动原始文件或终止外部
                Agent。若仍有请求或流式响应，操作将拒绝，等待结束后再核对。
              </p>
              {affected.some(
                (plugin) =>
                  !reviewSelection.has(plugin.id) &&
                  plugin.capabilities.some(
                    (cap) => cap.address.name === "session.source",
                  ),
              ) && (
                <p className="text-label text-fg-muted">
                  停用本机来源会同时影响共享该来源的 CLI
                  与桌面端，并撤销读取授权。不会修改原生会话文件、登录凭据或模型配置。
                </p>
              )}
              {catalog && !sameBase(review, catalog) && (
                <p role="alert">功能状态已变化，请关闭弹层并重新核对。</p>
              )}
            </div>
          )}
          <DialogFooter className="shrink-0">
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => setReview(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              disabled={
                !review ||
                !catalog ||
                !!catalog.impact?.activeTasks ||
                unavailable ||
                !sameBase(review, catalog) ||
                blocked
              }
              onClick={() => {
                if (review) void submit(review);
              }}
            >
              确认变更
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
