import { useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type {
  DraftHint,
  DraftRecoveryEntry,
  DraftRecoveryState,
  DraftRecoveryOwnership,
} from "../draft-recovery";
import {
  useDraftRecovery,
  type DraftInspection,
} from "../hooks/use-draft-recovery";

const stateLabels: Record<DraftRecoveryState, string> = {
  unchecked: "原结果尚未核查",
  unattached: "未关联原操作",
  pending: "原操作仍待核查",
  committed: "原操作已保存",
  failed: "原操作已失败",
  cancelled: "原操作已取消",
  "not-found": "未找到原结果",
  unavailable: "原结果暂不可核查",
};
const ownershipLabels: Record<DraftRecoveryOwnership, string> = {
  prepared: "尚未申请授权",
  managed: "由应用管理的引用",
  shared: "共享或借用的引用",
  missing: "归属信息缺失",
};
interface HostRow {
  entry: DraftRecoveryEntry;
  disabled: boolean;
  inspect: (entry: DraftRecoveryEntry) => void;
}
const hostColumns: ColumnDef<HostRow>[] = [
  {
    id: "scope",
    header: "草稿范围",
    cell: ({ row }) => (
      <div className="min-w-0 space-y-1 whitespace-normal">
        <p className="break-all text-body font-medium">
          {row.original.entry.label}
        </p>
        <p className="break-all text-label text-fg-muted">
          {row.original.entry.endpointOrigin ?? "服务地址待核查"}
        </p>
        <p className="break-all text-label text-fg-muted">
          {row.original.entry.pluginId ?? "目录来源待核查"}
        </p>
      </div>
    ),
  },
  {
    id: "state",
    header: "核查状态",
    cell: ({ row }) => (
      <div className="space-y-1 whitespace-normal">
        <p className="text-body">{stateLabels[row.original.entry.state]}</p>
        <p className="text-label text-fg-muted">
          {ownershipLabels[row.original.entry.ownership]}
        </p>
      </div>
    ),
  },
  {
    id: "action",
    header: "操作",
    cell: ({ row }) => (
      <Button
        type="button"
        variant="tertiary"
        size="sm"
        disabled={row.original.disabled}
        onClick={() => row.original.inspect(row.original.entry)}
      >
        核查此归属
      </Button>
    ),
  },
];
interface HintRow {
  hint: DraftHint;
  disabled: boolean;
  review: (hint: DraftHint) => void;
}
const hintColumns: ColumnDef<HintRow>[] = [
  {
    id: "identity",
    header: "本机授权提示",
    cell: ({ row }) => (
      <div className="min-w-0 space-y-1 whitespace-normal">
        <p className="break-all text-label">
          {row.original.hint.ownerId ?? "无法识别原归属"}
        </p>
        <p className="break-all text-label text-fg-muted">
          {row.original.hint.storeId
            ? `原数据库 ${row.original.hint.storeId}`
            : "原数据库尚不可识别"}
        </p>
        {row.original.hint.oversized && (
          <p className="text-label text-fg-muted">
            内容超过通常容量，已保持原样。
          </p>
        )}
      </div>
    ),
  },
  {
    id: "action",
    header: "操作",
    cell: ({ row }) => (
      <Button
        type="button"
        variant="link"
        size="sm"
        disabled={row.original.disabled}
        onClick={() => row.original.review(row.original.hint)}
      >
        人工核对后移除授权提示
      </Button>
    ),
  },
];
const emptyEntries: DraftRecoveryEntry[] = [];
const emptyHints: DraftHint[] = [];
type Confirmation =
  | { kind: "finish"; detail: DraftInspection }
  | { kind: "hint"; hint: DraftHint };

export function ProviderDraftRecovery({ storeId }: { storeId?: string }) {
  const [open, setOpen] = useState(false);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const recovery = useDraftRecovery(storeId, open);
  const entries = recovery.entries ?? emptyEntries,
    hints = recovery.hints?.rows ?? emptyHints;
  const { busy, fresh, inspect } = recovery;
  const hostRows = useMemo(
    () =>
      entries.map((entry) => ({
        entry,
        disabled: !!busy || !fresh,
        inspect: (value: DraftRecoveryEntry) => void inspect(value),
      })),
    [entries, busy, fresh, inspect],
  );
  const hintRows = useMemo(
    () =>
      hints.map((hint) => ({
        hint,
        disabled: !!recovery.busy || !storeId,
        review: (value: DraftHint) =>
          setConfirmation({ kind: "hint", hint: value }),
      })),
    [hints, recovery.busy, storeId],
  );
  const hostPagination = useTablePagination(entries.length, 10);
  const hintPagination = useTablePagination(hints.length, 10);
  const hostTable = useDataTable({
    data: hostRows,
    columns: hostColumns,
    getRowId: (r) => r.entry.owner.ownerId,
    enableSorting: false,
    enableHiding: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: hostPagination.value },
    onPaginationChange: hostPagination.onChange,
  }).table;
  const hintTable = useDataTable({
    data: hintRows,
    columns: hintColumns,
    getRowId: (r) => r.hint.key,
    enableSorting: false,
    enableHiding: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: hintPagination.value },
    onPaginationChange: hintPagination.onChange,
  }).table;
  // A new database, reconnect, fresh inspection, or refreshed hint invalidates
  // the confirmation. An old dialog can never become valid again by ID alone.
  const confirmed =
    confirmation?.kind === "finish"
      ? recovery.detail === confirmation.detail
        ? confirmation
        : null
      : confirmation?.kind === "hint" && hints.includes(confirmation.hint)
        ? confirmation
        : null;
  const detail = recovery.detail;
  return (
    <>
      <Button
        type="button"
        variant="tertiary"
        size="sm"
        onClick={() => setOpen(true)}
      >
        草稿目录授权恢复
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setConfirmation(null);
        }}
      >
        <DialogContent
          size="lg"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogHeader>
            <DialogTitle>草稿目录授权恢复</DialogTitle>
            <DialogDescription>
              界面提示丢失后，仍可从宿主核查原归属。查询不读取模型目录或发送模型请求；此处不重新提交连接。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4" aria-busy={!!recovery.busy}>
            {!storeId && (
              <Alert status="warning">
                <AlertTitle>原后台尚未确认</AlertTitle>
                <AlertDescription>
                  连接原后台后再核查，当前没有结束任何归属。
                </AlertDescription>
              </Alert>
            )}
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="tertiary"
                disabled={!!recovery.busy || !storeId}
                loading={recovery.busy === "list"}
                onClick={() => void recovery.read()}
              >
                读取宿主归属
              </Button>
              <Button
                type="button"
                variant="link"
                disabled={!!recovery.busy}
                onClick={recovery.loadHints}
              >
                检查本机授权提示
              </Button>
            </div>
            {recovery.error && (
              <Alert status="warning">
                <AlertTitle>恢复结果暂不可核查</AlertTitle>
                <AlertDescription>
                  {recovery.error} 原记录保持，请重新核查。
                </AlertDescription>
              </Alert>
            )}
            {recovery.notice && (
              <p role="status" className="text-body text-fg-muted">
                {recovery.notice}
              </p>
            )}
            <section className="space-y-2" aria-label="宿主草稿授权归属">
              <h3 className="text-body font-medium">宿主归属</h3>
              {recovery.entries === null ? (
                <p className="text-body text-fg-muted">尚未读取宿主记录。</p>
              ) : entries.length ? (
                <>
                  <p className="text-label text-fg-muted">
                    {entries.length} 条归属；逐条核查原结果后再操作。
                    {!recovery.fresh && " 上次结果已过期，操作暂不可用。"}
                  </p>
                  <DataTable table={hostTable} />
                </>
              ) : (
                <p className="text-body text-fg-muted">
                  {recovery.fresh
                    ? "当前数据库没有可列出的草稿归属或请求副本。"
                    : "上次列表为空，当前仍需重新核查。"}
                </p>
              )}
            </section>
            {detail && (
              <section className="space-y-3" aria-label="所选草稿归属核查">
                <h3 className="break-all text-body font-medium">
                  {detail.entry.label}
                </h3>
                <p className="text-body">
                  {stateLabels[detail.entry.state]} ·{" "}
                  {ownershipLabels[detail.entry.ownership]}
                </p>
                <p className="break-all text-label text-fg-muted">
                  归属 {detail.entry.owner.ownerId} · 原数据库{" "}
                  {detail.entry.owner.storeId}
                </p>
                {detail.entry.operationId && (
                  <p className="break-all text-label text-fg-muted">
                    原操作 {detail.entry.operationId}
                  </p>
                )}
                {detail.entry.state === "unattached" && (
                  <p className="text-body text-fg-muted">
                    此引用可能仍被其他编辑器使用。结束后，相关编辑器需要重新核查或授权。
                  </p>
                )}
                {detail.entry.ownership === "missing" && (
                  <p className="text-body text-fg-muted">
                    归属信息缺失时，本入口不会删除可能仍在使用的目录权限。请另到插件页核查原目录绑定。
                  </p>
                )}
                {detail.request && (
                  <p className="text-body text-fg-muted">
                    原请求：{detail.request.provider.name || "未命名连接"} ·{" "}
                    {detail.request.kind === "connect"
                      ? "连接并保存"
                      : "保存配置"}{" "}
                    · {(detail.request.provider.models ?? []).length}{" "}
                    个模型。取消阻止后续保存，不能撤回已经发出的远端请求。
                  </p>
                )}
                {detail.entry.state === "not-found" && !detail.request && (
                  <p className="text-body text-fg-muted">
                    旧记录未保留可恢复的原请求内容，未补造输入或结束归属。可在原服务商恢复列表继续核查。
                  </p>
                )}
                <div className="flex flex-wrap gap-2">
                  <Button
                    type="button"
                    variant="tertiary"
                    disabled={!!recovery.busy || !recovery.fresh}
                    onClick={() => void recovery.inspect(detail.entry)}
                  >
                    重新核查此归属
                  </Button>
                  {detail.request &&
                    ["pending", "not-found"].includes(detail.entry.state) && (
                      <Button
                        type="button"
                        variant="tertiary"
                        loading={recovery.busy === "cancel"}
                        disabled={!!recovery.busy || !recovery.fresh}
                        onClick={() => void recovery.cancel(detail)}
                      >
                        取消此原操作
                      </Button>
                    )}
                  <Button
                    type="button"
                    variant="neutral"
                    disabled={
                      !!recovery.busy ||
                      !recovery.fresh ||
                      !detail.entry.canFinish
                    }
                    onClick={() => setConfirmation({ kind: "finish", detail })}
                  >
                    结束此归属
                  </Button>
                </div>
              </section>
            )}
            {recovery.hintError && (
              <Alert status="warning">
                <AlertTitle>本机提示暂不可读取</AlertTitle>
                <AlertDescription>
                  {recovery.hintError} 宿主核查可继续。
                </AlertDescription>
              </Alert>
            )}
            {recovery.hints && (
              <section className="space-y-2" aria-label="本机草稿授权提示">
                <h3 className="text-body font-medium">本机授权提示</h3>
                <p className="text-label text-fg-muted">
                  提示不等于当前权限。人工移除提示不会取消原操作、删除宿主记录或解除授权。
                </p>
                {recovery.hints.overflow && (
                  <p className="text-body text-fg-muted">
                    仅展示本次容量内的提示，其余保持；处理后可继续检查。
                  </p>
                )}
                {hints.length ? (
                  <DataTable table={hintTable} />
                ) : (
                  <p className="text-body text-fg-muted">
                    未发现本机草稿授权提示。
                  </p>
                )}
              </section>
            )}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="tertiary"
              onClick={() => {
                setOpen(false);
                setConfirmation(null);
              }}
            >
              关闭恢复核查
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={open && !!confirmed}
        onOpenChange={(next) => {
          if (!next && !recovery.busy) setConfirmation(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {confirmed?.kind === "finish"
                ? "结束草稿目录归属"
                : "移除本机授权提示"}
            </DialogTitle>
            <DialogDescription>
              {confirmed?.kind === "finish"
                ? "宿主会再次核查原结果和精确授权引用。共享或借用的授权保持；连接、凭据和原操作回执不删除。同时只移除未变化的对应本机授权提示。"
                : "请先人工核对原后台和宿主归属。此操作仅移除这一条本机授权提示，不结束宿主归属、解除权限或取消原操作；其他记录保持。"}
            </DialogDescription>
          </DialogHeader>
          {recovery.error && (
            <Alert status="warning">
              <AlertTitle>原记录已保留</AlertTitle>
              <AlertDescription>{recovery.error}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="tertiary"
              disabled={!!recovery.busy}
              onClick={() => setConfirmation(null)}
            >
              返回核查
            </Button>
            <Button
              type="button"
              variant="neutral"
              loading={!!recovery.busy}
              disabled={
                !!recovery.busy ||
                (confirmed?.kind === "finish" && !recovery.fresh)
              }
              onClick={async () => {
                if (!confirmed) return;
                if (confirmed.kind === "finish")
                  await recovery.finish(confirmed.detail);
                else await recovery.removeHint(confirmed.hint);
              }}
            >
              {confirmed?.kind === "finish"
                ? "确认结束此归属"
                : "已核对，仅移除此提示"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
