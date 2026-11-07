import { useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { DomainConsentControls } from "@/features/domain/consent-controls";
import { domainApi } from "@/features/domain/api";
import { domainDirectoryResource } from "@/features/domain/queries";
import { fullPluginsApi } from "./api";
import {
  fullPluginDirectoryResource,
  pluginDirectoryResource,
} from "./queries";
import type {
  Review,
  Status,
  StateAction,
} from "../../../contracts/plugins/sources/types";

const actions: Record<StateAction, string> = {
  enable: "启用此版本",
  disable: "停用",
  remove: "卸载此版本",
  rollback: "回退上一版本",
  "revoke-grants": "撤销资源授权",
  "revoke-publisher": "撤销发布者信任",
};
const columns: ColumnDef<Status>[] = [
  {
    id: "name",
    header: "完整只读来源",
    cell: ({ row }) => (
      <div className="max-w-64 whitespace-normal break-words">
        <p>{row.original.manifest.displayName}</p>
        <p className="text-label text-fg-muted">
          {row.original.manifest.version}
        </p>
      </div>
    ),
  },
  {
    id: "state",
    header: "状态",
    cell: ({ row }) => (
      <span>
        {row.original.runtimeActive
          ? "已启用"
          : row.original.enabled
            ? "待重新启用"
            : "已停用"}{" "}
        · {row.original.sourceAuthorized ? "来源已授权" : "来源未授权"}
      </span>
    ),
  },
];
export function FullPluginManager({
  selectedDigest,
  detailsOnly = false,
}: { selectedDigest?: string; detailsOnly?: boolean } = {}) {
  const resource = useSharedRead(fullPluginDirectoryResource);
  const [selected, setSelected] = useState<string | null>(
    selectedDigest ?? null,
  );
  const [review, setReview] = useState<Review | null>(null);
  const [approve, setApprove] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<{
    digest: string;
    pluginId: string;
    action: StateAction;
  } | null>(null);
  const items = useMemo(
    () => (resource.error ? [] : (resource.data?.items ?? [])),
    [resource.error, resource.data],
  );
  const current = items.find((p) => p.digest === selected);
  const pagination = useTablePagination(items.length, 10);
  const { table } = useDataTable({
    data: items,
    columns,
    getRowId: (p) => p.digest,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
    autoResetPageIndex: false,
  });
  const run = async (operation: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await operation();
      await Promise.all([
        resource.invalidate(),
        pluginDirectoryResource.invalidate(),
        domainDirectoryResource.invalidate(),
      ]);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="space-y-4" aria-label="插件数据管理">
      {!detailsOnly && (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="max-w-xl text-label text-fg-muted">
            安装后分别启用、选择来源文件、手动读取并授权统一展示。插件报告的项目路径仅作为证据展示，不获得本机项目访问权限。
          </p>
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                setApprove(false);
                setReview(await fullPluginsApi.inspect());
              })
            }
          >
            选择插件包
          </Button>
        </div>
      )}
      {(error || resource.error) && (
        <p role="alert" className="break-words text-label text-fg-muted">
          {error ?? resource.error}
        </p>
      )}
      {!detailsOnly && (
        <DataTable
          table={table}
          isLoading={resource.loading && !resource.data}
          emptyMessage={resource.error ? "插件目录暂不可用" : "尚未安装插件"}
          aria-label="数据插件"
          onRowActivate={(row) => setSelected(row.original.digest)}
          activeRowId={selected}
          paginationProps={{ labels: dataTablePagination }}
        />
      )}
      {!detailsOnly && (
        <Button
          variant="tertiary"
          disabled={busy || resource.loading}
          onClick={() => void resource.invalidate()}
        >
          重新读取插件目录
        </Button>
      )}
      {current && (
        <article
          className="space-y-3 rounded-xl border border-border p-4"
          aria-label="插件数据详情"
        >
          <h3 className="text-title font-medium">
            {current.manifest.displayName} · {current.manifest.version}
          </h3>
          <p className="break-all text-label text-fg-muted">
            发布者：{current.manifest.publisher}
            <br />
            公钥：{current.signingKey}
            <br />
            包摘要：{current.digest}
            <br />
            来源文件：{current.sourceLabel ?? "未授权"}
          </p>
          <p className="text-label text-fg-muted">
            {current.freshness === "current"
              ? current.manifest.source?.refreshSeconds
                ? `已读取 · 每 ${current.manifest.source.refreshSeconds} 秒更新`
                : "已手动读取"
              : current.freshness === "failed"
                ? "读取失败，保留上一完整代次"
                : current.freshness === "stale"
                  ? "元数据已过期"
                  : "尚未读取"}
            。重开后需要重新启用并选择资源。
          </p>
          {current.problem && (
            <p role="alert" className="text-label text-fg-muted">
              {current.problem === "source_stalled"
                ? "来源暂未发布新数据，已有记录保留。"
                : "当前能力不可用，请核查授权、插件包或重新启用。"}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            {(
              [
                current.runtimeActive ? "disable" : "enable",
                "revoke-grants",
                "rollback",
                "remove",
                "revoke-publisher",
              ] as StateAction[]
            ).map((action) => (
              <Button
                key={action}
                variant="tertiary"
                disabled={busy}
                onClick={() =>
                  setConfirmation({
                    pluginId: current.manifest.id,
                    digest: current.digest,
                    action,
                  })
                }
              >
                {actions[action]}
              </Button>
            ))}
            <Button
              variant="tertiary"
              disabled={busy || !current.runtimeActive}
              onClick={() =>
                void run(() =>
                  domainApi.authorizeSource({
                    pluginId: current.manifest.id,
                    digest: current.digest,
                  }),
                )
              }
            >
              选择数据文件
            </Button>
            <Button
              variant="tertiary"
              disabled={busy || !current.sourceAuthorized}
              onClick={() =>
                void run(() =>
                  domainApi.refreshSource({
                    pluginId: current.manifest.id,
                    digest: current.digest,
                  }),
                )
              }
            >
              读取插件数据
            </Button>
          </div>
          {current.runtimeActive && current.manifest.source && (
            <DomainConsentControls
              key={`${current.digest}:${current.grantRevision}`}
              pluginId={current.manifest.id}
              digest={current.digest}
              sourceId={`${current.manifest.id}:${current.manifest.source.id}`}
              fullFacts
              onChanged={() => void resource.invalidate()}
            />
          )}
          {current.manifest.capabilities.includes("agent.connection@1") && (
            <p className="text-label text-fg-muted">
              模型连接需要另行授权。可在 Agents
              的「连接与观测」中预览和确认变更。
            </p>
          )}
        </article>
      )}
      <Dialog
        open={review !== null}
        onOpenChange={(open) => !open && !busy && setReview(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>检查数据插件</DialogTitle>
            <DialogDescription>
              公钥签名证明包由对应密钥签署；本机信任不代表已核实组织身份。安装不会启用或读取资源。
            </DialogDescription>
          </DialogHeader>
          {review && (
            <div className="max-h-96 space-y-3 overflow-y-auto">
              <p>
                {review.manifest.displayName} · {review.manifest.version}
              </p>
              <p className="break-all text-label text-fg-muted">
                发布者：{review.manifest.publisher}
                <br />
                公钥：{review.signingKey}
                <br />
                摘要：{review.digest}
              </p>
              <p className="text-label">
                授权后读取所选单个 JSONL，包括全部正文。项目、会话、响应、活动和
                MCP 事实按声明能力接受核查；不访问其他目录或网络。
              </p>
              {review.manifest.capabilities.includes("agent.connection@1") && (
                <p className="text-label">
                  此包还声明了模型连接能力。启用连接桥接并单独授权后，可在 Agent
                  详情中确认默认模型变更。
                </p>
              )}
              {!review.trustedPublisher && (
                <label className="flex items-start gap-2 text-label">
                  <Checkbox
                    checked={approve}
                    disabled={busy}
                    onCheckedChange={(v) => setApprove(v === true)}
                  />
                  我已核对并信任此发布者公钥
                </label>
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
              disabled={
                busy || !review || (!review.trustedPublisher && !approve)
              }
              onClick={() =>
                void run(async () => {
                  if (!review) return;
                  const installed = await fullPluginsApi.install({
                    packageRef: review.packageRef,
                    digest: review.digest,
                    approvePublisher: approve,
                    expectedSigningKey: review.signingKey,
                  });
                  setSelected(installed.digest);
                  setReview(null);
                })
              }
            >
              安装数据插件
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={confirmation !== null}
        onOpenChange={(open) => !open && !busy && setConfirmation(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {confirmation ? actions[confirmation.action] : "确认操作"}
            </DialogTitle>
            <DialogDescription>
              启用只进行无资源预检。停用、回退或撤权会使统一查询失效；卸载仅删除受管插件副本，保留所选原文件。撤销发布者信任同时影响该发布者的其他插件。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setConfirmation(null)}
            >
              取消
            </Button>
            <Button
              disabled={busy || !confirmation}
              onClick={() =>
                void run(async () => {
                  if (!confirmation) return;
                  await fullPluginsApi.change({
                    package: {
                      pluginId: confirmation.pluginId,
                      digest: confirmation.digest,
                    },
                    action: confirmation.action,
                  });
                  setConfirmation(null);
                })
              }
            >
              确认操作
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
