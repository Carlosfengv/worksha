import {
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogTrigger,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import {
  backendReadEpoch,
  subscribeBackendRecovery,
} from "@/shared/lib/backend-recovery";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { performRoutingPolicyRecovery } from "../api";
import { DesktopError } from "@/shared/lib/desktop-command";
import type {
  PolicyRecoveryEntry as Entry,
  PolicyRecoveryInspection as Inspection,
  PolicyRecoveryMode as Mode,
} from "../../../../contracts/domain/routing-policy-recovery/types";

type Row = { entry: Entry; disabled: boolean; inspect: () => void };
const columns: ColumnDef<Row>[] = [
  {
    id: "service",
    header: "策略范围",
    cell: ({ row }) => (
      <div className="max-w-64 break-all">
        <p>
          {row.original.entry.serviceName ??
            (row.original.entry.serviceExists === false
              ? "服务当前不存在"
              : "名称待核查")}
        </p>
        <p className="text-label text-fg-muted">
          {row.original.entry.scope.serviceId}
        </p>
      </div>
    ),
  },
  {
    id: "inspect",
    header: "核查",
    cell: ({ row }) => (
      <Button
        size="sm"
        variant="tertiary"
        disabled={row.original.disabled}
        onClick={row.original.inspect}
      >
        核查此范围
      </Button>
    ),
  },
];
const problemLabels = {
  "missing-control": "控制记录丢失，保留的历史仍可核对。",
  "marker-mismatch": "两份范围记录不一致。",
  "pending-publication": "仍有原发布待核查，暂不能切回内置策略。",
  "incomplete-evidence": "证据不完整，暂不能恢复。",
  unavailable: "记录损坏或暂不可读取，原文件已保留。",
  capacity: "核查容量不足，原记录已保留。",
} as const;

export function RoutingPolicyRecovery({ storeId }: { storeId: string }) {
  const [open, setOpen] = useState(false);
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [limited, setLimited] = useState(false);
  const [detail, setDetail] = useState<Inspection | null>(null);
  const [confirmation, setConfirmation] = useState<{
    detail: Inspection;
    mode: Mode;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [fresh, setFresh] = useState(false);
  const [retryUnknown, setRetryUnknown] = useState(false);
  const epoch = useSyncExternalStore(
    subscribeBackendRecovery,
    backendReadEpoch,
  );
  const generation = useRef(0),
    active = useRef(false),
    inspectedAt = useRef(0);
  useLayoutEffect(() => {
    const view = ++generation.current;
    active.current = false;
    setEntries(null);
    setDetail(null);
    setConfirmation(null);
    setBusy(false);
    setError(null);
    setNotice(null);
    setFresh(false);
    setLimited(false);
    setRetryUnknown(false);
    return () => {
      generation.current = view + 1;
      active.current = false;
    };
  }, [storeId, open, epoch]);
  const run = useCallback(
    async (work: (current: () => boolean) => Promise<void>) => {
      if (active.current || !open) return;
      active.current = true;
      setBusy(true);
      setError(null);
      setNotice(null);
      const view = generation.current,
        backend = backendReadEpoch();
      const current = () =>
        generation.current === view && backendReadEpoch() === backend;
      try {
        await work(current);
      } catch (reason) {
        if (current()) {
          setError(reason instanceof Error ? reason.message : String(reason));
          setFresh(false);
        }
      } finally {
        if (current()) {
          active.current = false;
          setBusy(false);
        }
      }
    },
    [open],
  );
  const read = () =>
    run(async (current) => {
      setFresh(false);
      setDetail(null);
      setConfirmation(null);
      const reply = await performRoutingPolicyRecovery({
        kind: "list",
        storeId,
      });
      if (current() && reply.kind === "listed") {
        setEntries(reply.entries);
        setLimited(reply.limited);
      }
    });
  const inspect = useCallback(
    (entry: Entry) =>
      run(async (current) => {
        setFresh(false);
        setDetail(null);
        setConfirmation(null);
        const reply = await performRoutingPolicyRecovery({
          kind: "inspect",
          scope: entry.scope,
        });
        if (current() && reply.kind === "inspected") {
          inspectedAt.current = performance.now();
          setDetail(reply.inspection);
          setFresh(true);
        }
      }),
    [run],
  );
  const rows = useMemo<Row[]>(
    () =>
      (entries ?? []).map((entry) => ({
        entry,
        disabled: busy,
        inspect: () => void inspect(entry),
      })),
    [entries, busy, inspect],
  );
  const pagination = useTablePagination(rows.length, 5);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (r) => r.entry.scope.serviceId,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
    autoResetPageIndex: false,
  });
  const choose = (mode: Mode) => {
    if (
      !detail?.reference ||
      !fresh ||
      performance.now() - inspectedAt.current >= 60_000
    ) {
      setFresh(false);
      setError("核查已过期，请重新核查此范围。");
      return;
    }
    setError(null);
    setRetryUnknown(false);
    setConfirmation({ detail, mode });
  };
  const confirm = () =>
    run(async (current) => {
      if (
        !confirmation?.detail.reference ||
        confirmation.detail !== detail ||
        (!retryUnknown &&
          (!fresh || performance.now() - inspectedAt.current >= 60_000))
      )
        throw Error("核查已变化，请关闭确认窗口后重新核查。");
      let reply;
      try {
        reply = await performRoutingPolicyRecovery({
          kind: "resolve",
          reference: confirmation.detail.reference,
          mode: confirmation.mode,
        });
      } catch (reason) {
        if (current())
          setRetryUnknown(
            reason instanceof DesktopError &&
              reason.code === "scope_recovery_unknown" &&
              reason.outcomeUnknown,
          );
        throw reason;
      }
      if (current() && reply.kind === "resolved") {
        setConfirmation(null);
        setFresh(false);
        setRetryUnknown(false);
        setNotice(
          reply.mode.kind === "native"
            ? confirmation.detail.entry.serviceExists === false
              ? "已保存遗留范围的解除状态，原历史保留。"
              : "已明确切回内置策略。服务配置、绑定历史和原发布记录保持。"
            : "已恢复原编号核查入口。请关闭窗口，在发布记录中核查原结果；插件授权没有恢复。",
        );
        window.dispatchEvent(new Event("routing-policy-changed"));
      }
    });
  return (
    <>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!busy) setOpen(next);
        }}
      >
        <DialogTrigger render={<Button variant="tertiary" />}>
          核查策略范围与恢复
        </DialogTrigger>
        <DialogContent size="lg" className="flex max-h-[85dvh] flex-col">
          <DialogHeader>
            <DialogTitle>策略范围核查与恢复</DialogTitle>
            <DialogDescription>
              核对丢失或不一致的范围记录。读取不会重新授权插件，也不会提交发布或模型请求。
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 space-y-4 overflow-y-auto break-words">
            <Button
              variant="tertiary"
              disabled={busy}
              loading={busy}
              onClick={() => void read()}
            >
              读取恢复范围
            </Button>
            {limited && (
              <p role="status">
                当前仅有部分范围可列出，未列出或证据不足的记录仍保持未知。
              </p>
            )}
            {error && (
              <p role="alert" className="text-body text-fg-danger">
                {error}
              </p>
            )}
            {notice && (
              <p role="status" className="text-body">
                {notice}
              </p>
            )}
            <DataTable
              table={table}
              aria-label="策略恢复范围"
              paginationProps={{ labels: dataTablePagination }}
              emptyMessage={
                entries === null
                  ? "尚未读取恢复范围。"
                  : "未发现可列出的策略范围记录。"
              }
            />
            {detail && (
              <section
                className="space-y-3 text-body"
                aria-label="所选恢复范围"
              >
                <p className="break-all">
                  服务：
                  {detail.entry.serviceName ?? detail.entry.scope.serviceId}
                </p>
                <p>
                  {detail.entry.problem
                    ? problemLabels[detail.entry.problem]
                    : detail.entry.state === "consistent"
                      ? detail.entry.serviceExists === false
                        ? "遗留策略要求已解除，原记录保留。"
                        : "范围记录一致，可通过原入口管理绑定。"
                      : detail.entry.serviceExists === false &&
                          detail.canRestoreNative
                        ? "服务当前不存在，可明确解除遗留策略要求。"
                        : "范围状态待核查。"}
                </p>
                {detail.entry.pendingOperationIds.map((id) => (
                  <p className="break-all" key={id}>
                    原发布编号：{id}
                  </p>
                ))}
                {!fresh && <p role="status">核查已失效，请重新核查后操作。</p>}
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="tertiary"
                    disabled={busy}
                    onClick={() => void inspect(detail.entry)}
                  >
                    重新核查此范围
                  </Button>
                  {(["native", "pending-original"] as const).map((kind) => (
                    <RecoveryConfirmation
                      key={kind}
                      mode={{ kind }}
                      missingService={detail.entry.serviceExists === false}
                      confirmation={confirmation}
                      disabled={
                        busy ||
                        !fresh ||
                        !(kind === "native"
                          ? detail.canRestoreNative
                          : detail.canRestorePending)
                      }
                      busy={busy}
                      fresh={fresh || retryUnknown}
                      error={error}
                      retryUnknown={retryUnknown}
                      onChoose={() => choose({ kind })}
                      onClose={() => {
                        setConfirmation(null);
                        setRetryUnknown(false);
                      }}
                      onConfirm={() => void confirm()}
                    />
                  ))}
                </div>
              </section>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setOpen(false)}
            >
              关闭
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

function RecoveryConfirmation({
  mode,
  missingService,
  confirmation,
  disabled,
  busy,
  fresh,
  error,
  retryUnknown,
  onChoose,
  onClose,
  onConfirm,
}: {
  mode: Mode;
  missingService: boolean;
  confirmation: { detail: Inspection; mode: Mode } | null;
  disabled: boolean;
  busy: boolean;
  fresh: boolean;
  error: string | null;
  retryUnknown: boolean;
  onChoose: () => void;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog
      open={confirmation?.mode.kind === mode.kind}
      onOpenChange={(next) => {
        if (!busy) {
          if (next) onChoose();
          else onClose();
        }
      }}
    >
      <DialogTrigger render={<Button disabled={disabled} />}>
        {mode.kind === "native"
          ? missingService
            ? "解除遗留策略要求"
            : "确认使用内置策略"
          : "恢复原编号核查入口"}
      </DialogTrigger>
      <DialogContent className="flex max-h-[85dvh] flex-col">
        <DialogHeader>
          <DialogTitle>
            {confirmation?.mode.kind === "native"
              ? confirmation.detail.entry.serviceExists === false
                ? "确认解除遗留策略要求"
                : "确认切回内置策略"
              : "确认恢复原编号核查入口"}
          </DialogTitle>
          <DialogDescription>
            再次核对同一份证据后才会改写范围。原控制记录会先保留备份，绑定与发布历史继续保留。
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 space-y-3 overflow-y-auto break-all text-body">
          <p>
            服务：
            {confirmation?.detail.entry.serviceName ??
              confirmation?.detail.entry.scope.serviceId}
          </p>
          <p>核查编号：{confirmation?.detail.reference?.referenceId}</p>
          <p>
            {confirmation?.mode.kind === "native"
              ? confirmation.detail.entry.serviceExists === false
                ? "仅保存此遗留范围的解除状态，保留绑定与发布历史，不会创建服务或提交原发布。"
                : "此选择用于后续新请求，原服务配置不变。在途请求仍保留原执行归属。"
              : "此操作只恢复保存的原编号核查通道。后续继续或取消仍需通过原发布记录明确操作。"}
          </p>
          {error && (
            <p role="alert" className="text-fg-danger">
              {error}
            </p>
          )}
        </div>
        <DialogFooter>
          <Button variant="tertiary" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button disabled={busy || !fresh} loading={busy} onClick={onConfirm}>
            {retryUnknown ? "核查并完成本次恢复" : "确认恢复此范围"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
