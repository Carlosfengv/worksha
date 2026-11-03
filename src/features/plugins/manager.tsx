import { PluginOptionalDependencies } from "./optional-dependencies";
import { useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Tabs, TabsList, TabItem, TabPanel } from "@/components/ui/tabs";
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
import { pluginDirectoryResource } from "./queries";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { pluginsApi } from "./api";
import { PermissionSummary, PluginControls } from "./controls";
import { PluginOperationHistory } from "./advanced-controls";
import type { PackageReview, PluginStatus } from "./types";
import { capabilityNames, PluginDiagnosticsPanel } from "./diagnostics-panel";
import { FullPluginManager } from "./full-manager";

const stateNames: Record<string, string> = {
  installed: "已安装 · 待启用",
  awaiting_permission: "已安装 · 待启用",
  active: "已启用",
  disabled: "已停用",
  degraded: "部分能力失败",
  failed: "运行失败",
  removed: "已卸载",
};
const columns: ColumnDef<PluginStatus>[] = [
  {
    id: "name",
    header: "插件 / 版本",
    cell: ({ row }) => (
      <>
        <p>{row.original.manifest.displayName}</p>
        <p className="text-label text-fg-muted">
          {row.original.manifest.version}
        </p>
      </>
    ),
  },
  {
    id: "capabilities",
    header: "能力",
    cell: ({ row }) => (
      <p className="max-w-80 whitespace-normal break-words">
        {capabilityNames(row.original)}
      </p>
    ),
  },
  {
    id: "publisher",
    header: "发布者",
    cell: ({ row }) => (
      <>
        <p>{row.original.manifest.publisher}</p>
        <p className="text-label text-fg-muted">
          {row.original.signingKey ? "公钥签名" : "未签名开发包"}
        </p>
      </>
    ),
  },
  {
    id: "status",
    header: "状态",
    cell: ({ row }) => (
      <Badge status={row.original.errorCode ? "warning" : "neutral"}>
        {stateNames[row.original.runtimeStatus] ?? "状态未知"}
      </Badge>
    ),
  },
];
export function PluginManager({ providers }: { providers: Provider[] }) {
  return (
    <section className="space-y-4" aria-label="插件管理">
      <h2 className="text-title font-medium">插件管理</h2>
      <Tabs defaultValue="installed" color="default">
        <TabsList aria-label="插件管理">
          <TabItem value="installed" label="已安装" />
          <TabItem value="full" label="完整只读来源" />
        </TabsList>
        <TabPanel value="installed" keepMounted>
          <InstalledPlugins providers={providers} />
        </TabPanel>
        <TabPanel value="full">
          <FullPluginManager />
        </TabPanel>
      </Tabs>
    </section>
  );
}
function InstalledPlugins({ providers }: { providers: Provider[] }) {
  const resource = useSharedRead(pluginDirectoryResource);
  const [selected, setSelected] = useState<string | null>(null);
  const [review, setReview] = useState<PackageReview | null>(null);
  const [approve, setApprove] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const items = useMemo(() => resource.data?.items ?? [], [resource.data]);
  const pagination = useTablePagination(items.length, 10);
  const { table } = useDataTable({
    data: items,
    columns,
    getRowId: (s) => s.digest,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
    autoResetPageIndex: false,
  });
  const current = items.find((item) => item.digest === selected);
  const inspect = async () => {
    setBusy(true);
    setError(null);
    setApprove(false);
    try {
      setReview(await pluginsApi.inspect());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const install = async () => {
    if (!review || busy) return;
    setBusy(true);
    setError(null);
    try {
      const installed = await pluginsApi.install({
        packageRef: review.packageRef,
        digest: review.digest,
        approvePublisher: approve,
      });
      setReview(null);
      setSelected(installed.digest);
      void resource.invalidate();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="space-y-4" aria-label="本地已安装插件管理">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h3 className="text-title font-medium">本地已安装</h3>
          <p className="mt-1 text-label text-fg-muted">
            安装本地包后，分别启用插件并授权具体资源。
          </p>
        </div>
        <Button
          variant="tertiary"
          loading={busy}
          onClick={() => void inspect()}
        >
          选择插件包
        </Button>
      </div>
      {resource.data?.developmentMode && (
        <p className="text-label text-warning-fg">
          当前为显式插件开发模式，允许未签名开发包。
        </p>
      )}
      {(error || resource.error) && (
        <Alert status="danger">
          <AlertTitle>插件操作未完成</AlertTitle>
          <AlertDescription>{error ?? resource.error}</AlertDescription>
        </Alert>
      )}
      <DataTable
        table={table}
        isLoading={resource.loading && !resource.data}
        loadingMessage="正在读取插件目录"
        emptyMessage={resource.error ? "插件目录暂不可用" : "尚未安装外部插件"}
        aria-label="已安装插件"
        onRowActivate={(row) => setSelected(row.original.digest)}
        activeRowId={selected}
        paginationProps={{ labels: dataTablePagination }}
      />
      {current && (
        <article className="space-y-4 rounded-xl border border-border p-4">
          <h3 className="text-title font-medium">
            {current.manifest.displayName} · {current.manifest.version}
          </h3>
          <p className="break-all text-label text-fg-muted">
            {current.manifest.id}
            <br />
            包摘要：{current.digest}
            <br />
            发布者公钥：{current.signingKey ?? "未签名"}
          </p>
          <PluginControls
            key={current.digest}
            status={current}
            providers={providers}
            onChanged={resource.invalidate}
          />
          <PluginOptionalDependencies
            status={current}
            catalog={resource.data?.contributions}
            error={resource.data?.contributionsError}
          />
          <PluginDiagnosticsPanel
            status={current}
            diagnostics={resource.data?.diagnostics}
            unavailable={!!resource.error || !!resource.data?.diagnosticsError}
          />
          <div className="flex flex-wrap gap-3">
            {current.manifest.views?.map((view) => (
              <Button key={view.id} variant="link" asChild>
                <a
                  href={`#/extensions/${encodeURIComponent(current.manifest.id)}/${encodeURIComponent(view.id)}`}
                >
                  打开{view.title}
                </a>
              </Button>
            ))}
          </div>
        </article>
      )}
      <Dialog
        open={review !== null}
        onOpenChange={(open) => !busy && !open && setReview(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>检查插件来源与权限</DialogTitle>
            <DialogDescription>
              公钥签名证明包由对应密钥签署；本机信任不等于已核实组织身份。安装不会自动启用插件。
            </DialogDescription>
          </DialogHeader>
          {review && (
            <div className="max-h-96 space-y-4 overflow-y-auto">
              <p>
                {review.manifest.displayName} · {review.manifest.version}
              </p>
              <p className="break-all text-label text-fg-muted">
                发布者：{review.manifest.publisher}
                <br />
                公钥：{review.signingKey ?? "未签名开发包"}
                <br />
                摘要：{review.digest}
              </p>
              <PermissionSummary manifest={review.manifest} />
              {review.signingKey && !review.trustedPublisher && (
                <label className="flex items-start gap-2 text-label">
                  <Checkbox
                    checked={approve}
                    onCheckedChange={(checked) => setApprove(checked === true)}
                  />
                  我已核对并信任此发布者公钥
                </label>
              )}
              {review.trustedPublisher && (
                <p className="text-label text-fg-muted">
                  此公钥已在当前数据目录受信任。
                </p>
              )}
            </div>
          )}
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setReview(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              disabled={
                !!review?.signingKey && !review.trustedPublisher && !approve
              }
              onClick={() => void install()}
            >
              安装此包
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <PluginOperationHistory />
    </section>
  );
}
