import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import type { ColumnDef } from "@tanstack/react-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { providersApi } from "../api";
import { message } from "../display";
import type { ProviderWriteRoute } from "../catalog-operation";
import { useProviderWrite } from "../hooks/use-provider-write";
import type {
  ProviderOperation,
  ProviderWriteRequest,
} from "../operation-receipt";
import {
  providerRecoveryRecords,
  discardReviewedProviderRecord,
  PROVIDER_OPERATIONS_CHANGED,
  type ProviderRecoveryRecord,
} from "../pending-operations";
import { ProviderWriteStatus } from "./provider-write-status";
import { ProviderDraftRecovery } from "./provider-draft-recovery";
function RecoveryRow({
  request,
  storeId,
  onResolved,
  catalog = null,
}: {
  request: ProviderWriteRequest;
  storeId?: string;
  onResolved: (operation: ProviderOperation) => void;
  catalog?: ProviderWriteRoute | null;
}) {
  const write = useProviderWrite(storeId, onResolved);
  return (
    <div className="min-w-0 space-y-2 whitespace-normal">
      <p className="text-body font-medium">
        {request.provider.name || "未命名连接"} ·{" "}
        {request.kind === "connect" ? "连接并保存" : "保存配置"}
      </p>
      <p className="break-all text-label text-fg-muted">
        操作 {request.operationId} · 原版本 {request.provider.revision} · 数据库{" "}
        {request.storeId}
      </p>
      {request.storeId !== storeId && (
        <p className="text-body text-fg-muted">
          请连接此操作原来的后台后核查。
        </p>
      )}
      {catalog && (
        <p className="text-body text-fg-muted">
          此原请求保存了明确的模型目录选择；核查与重试保留原选择。
        </p>
      )}
      {!write.unresolved && (
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            variant="tertiary"
            disabled={write.busy}
            onClick={() => void write.resume("check", request, catalog)}
          >
            核查原结果
          </Button>
          <Button
            type="button"
            variant="tertiary"
            disabled={write.busy}
            onClick={() => void write.resume("retry", request, catalog)}
          >
            重试原请求
          </Button>
          <Button
            type="button"
            variant="tertiary"
            disabled={write.busy}
            onClick={() => void write.resume("cancel", request, catalog)}
          >
            取消原操作
          </Button>
        </div>
      )}
      {write.state.kind === "failed" && (
        <Alert status="danger">
          <AlertTitle>原操作未保存</AlertTitle>
          <AlertDescription>{write.state.message}</AlertDescription>
        </Alert>
      )}
      <ProviderWriteStatus write={write} />
    </div>
  );
}
interface RecoveryData {
  id: string;
  request: ProviderWriteRequest | null;
  record?: ProviderRecoveryRecord;
  storeId?: string;
  onResolved: (operation: ProviderOperation) => void;
  onReview: (record: ProviderRecoveryRecord) => void;
}
const recoveryColumns: ColumnDef<RecoveryData>[] = [
  {
    id: "operation",
    header: "原操作记录",
    enableSorting: false,
    cell: ({ row }) => {
      const data = row.original;
      return (
        <div className="space-y-2">
          {data.request ? (
            <RecoveryRow
              request={data.request}
              storeId={data.storeId}
              onResolved={data.onResolved}
              catalog={data.record?.catalog}
            />
          ) : (
            <p className="text-body">有一项无法读取的本地恢复记录</p>
          )}
          {data.record && (
            <Button
              type="button"
              variant="link"
              size="sm"
              disabled={
                !!data.record.request &&
                data.record.request.storeId !== data.storeId
              }
              onClick={() => data.onReview(data.record!)}
            >
              手动核对后移除提示
            </Button>
          )}
        </div>
      );
    },
  },
];
export function ProviderOperationRecovery({
  storeId,
  onUpdated,
}: {
  storeId?: string;
  onUpdated: () => Promise<void>;
}) {
  const [local, setLocal] = useState<
    ReturnType<typeof providerRecoveryRecords>
  >({ records: [], overflow: false, characters: 0 });
  const [storageError, setStorageError] = useState<string | null>(null);
  const [server, setServer] = useState<{
    storeId: string;
    requests: ProviderWriteRequest[];
  } | null>(null);
  const [readOwner, setReadOwner] = useState<{
    storeId?: string;
    id: string;
  } | null>(null);
  const reading = !!readOwner && readOwner.storeId === storeId;
  const [noticeState, setNoticeState] = useState<{
    storeId?: string;
    message: string | null;
  } | null>(null);
  const notice = noticeState?.storeId === storeId ? noticeState?.message : null;
  const setNotice = useCallback(
    (message: string | null) => setNoticeState({ storeId, message }),
    [storeId],
  );
  const onUpdatedRef = useRef(onUpdated);
  useLayoutEffect(() => {
    onUpdatedRef.current = onUpdated;
  }, [onUpdated]);
  const [review, setReview] = useState<ProviderRecoveryRecord | null>(null);
  const [reviewError, setReviewError] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const openReview = useCallback((record: ProviderRecoveryRecord) => {
    setReviewError(null);
    setReview(record);
  }, []);
  useEffect(() => {
    const sync = () => {
      try {
        setLocal(providerRecoveryRecords());
        setStorageError(null);
      } catch (reason) {
        setStorageError(message(reason));
      }
    };
    sync();
    window.addEventListener(PROVIDER_OPERATIONS_CHANGED, sync);
    window.addEventListener("storage", sync);
    return () => {
      window.removeEventListener(PROVIDER_OPERATIONS_CHANGED, sync);
      window.removeEventListener("storage", sync);
    };
  }, []);
  const readServer = async () => {
    if (reading) return;
    const readId = crypto.randomUUID();
    setReadOwner({ storeId, id: readId });
    setNotice(null);
    try {
      const protocol = await providersApi.writeProtocol(storeId);
      const results = await Promise.allSettled(
        protocol.pendingOperationIds.map((id) =>
          providersApi.pendingOperation(id, protocol.storeId),
        ),
      );
      const requests: ProviderWriteRequest[] = [];
      let failed = 0;
      for (const result of results) {
        if (result.status === "fulfilled") requests.push(result.value.request);
        else failed++;
      }
      setServer({ storeId: protocol.storeId, requests });
      setNotice(
        failed
          ? `${failed} 项暂不可读取，已读取的原操作仍可单独核查。`
          : requests.length
            ? "已读取后台原操作；未重新验证或保存。"
            : "当前后台没有待处理操作；未查到历史回执不代表从未执行。",
      );
    } catch (reason) {
      setNotice(message(reason));
    } finally {
      setReadOwner((current) => (current?.id === readId ? null : current));
    }
  };
  const resolved = useCallback(
    (operation: ProviderOperation) => {
      setServer((current) =>
        current
          ? {
              ...current,
              requests: current.requests.filter(
                (r) =>
                  r.operationId !== operation.request.operationId ||
                  r.storeId !== operation.request.storeId,
              ),
            }
          : null,
      );
      setNotice(
        operation.status === "committed"
          ? "原操作已确认保存。"
          : operation.status === "cancelled"
            ? "原操作已确认取消。"
            : (operation.error?.message ?? "原操作未保存。"),
      );
      if (operation.status === "committed")
        void onUpdatedRef
          .current()
          .catch(() => setNotice("原操作已确认保存，列表暂未刷新。"));
    },
    [setNotice],
  );
  const rows = useMemo(() => {
    const requests = local.records.flatMap((row) =>
      row.request ? [row.request] : [],
    );
    for (const request of server && server.storeId === storeId
      ? server.requests
      : [])
      if (
        !requests.some(
          (r) =>
            r.storeId === request.storeId &&
            r.operationId === request.operationId,
        )
      )
        requests.push(request);
    const rows: RecoveryData[] = requests.map((request) => ({
      id: `${request.storeId}:${request.operationId}`,
      request,
      record: local.records.find(
        (r) =>
          r.request?.storeId === request.storeId &&
          r.request.operationId === request.operationId,
      ),
      storeId,
      onResolved: resolved,
      onReview: openReview,
    }));
    for (const record of local.records)
      if (!record.request)
        rows.push({
          id: record.key,
          request: null,
          record,
          storeId,
          onResolved: resolved,
          onReview: openReview,
        });
    return rows;
  }, [local.records, server, storeId, resolved, openReview]);
  const { table } = useDataTable({
    data: rows,
    columns: recoveryColumns,
    getRowId: (row) => row.id,
    enableRowSelection: false,
    enableHiding: false,
  });
  return (
    <section className="space-y-3" aria-label="服务商操作恢复">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-body font-medium">服务商操作恢复</h2>
        <ProviderDraftRecovery storeId={storeId} />
        <Button
          type="button"
          variant="tertiary"
          size="sm"
          loading={reading}
          disabled={!storeId}
          onClick={() => void readServer()}
        >
          读取后台待核查操作
        </Button>
      </div>
      <p className="text-label text-fg-muted">
        查询只读取原结果；重试保留原编号和输入，尚未开始的连接验证可能产生费用。取消阻止后续保存，不能撤回已经发出的远端请求。
      </p>
      {storageError && (
        <Alert status="warning">
          <AlertTitle>恢复记录暂不可读取</AlertTitle>
          <AlertDescription>{storageError}</AlertDescription>
        </Alert>
      )}
      {notice && (
        <p role="status" className="text-body text-fg-muted">
          {notice}
        </p>
      )}
      {local.overflow && (
        <p className="text-body text-fg-muted">
          仅展示前四项；处理后继续读取其余记录，未自动丢弃。
        </p>
      )}
      {rows.length > 0 && <DataTable table={table} />}
      <Dialog
        open={!!review}
        onOpenChange={(open) => {
          if (!open && !removing) setReview(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>移除本地恢复提示</DialogTitle>
            <DialogDescription>
              先手动核对当前服务商配置和原操作结果。移除提示不会取消后台操作，也不证明原操作从未执行；仍在处理的操作可从后台列表重新读取。
            </DialogDescription>
          </DialogHeader>
          {reviewError && (
            <Alert status="danger">
              <AlertTitle>恢复提示未移除</AlertTitle>
              <AlertDescription>{reviewError}</AlertDescription>
            </Alert>
          )}
          {review?.request && (
            <p className="break-all text-label text-fg-muted">
              原操作 {review.request.operationId} · 原数据库{" "}
              {review.request.storeId}
            </p>
          )}
          <p className="text-body text-fg-muted">
            记录内容保持原样，只有你确认核对后才移除本地提示。
          </p>
          <DialogFooter>
            <Button
              type="button"
              variant="tertiary"
              disabled={removing}
              onClick={() => setReview(null)}
            >
              返回核查
            </Button>
            <Button
              type="button"
              variant="neutral"
              loading={removing}
              onClick={async () => {
                if (!review || removing) return;
                setRemoving(true);
                setReviewError(null);
                try {
                  await discardReviewedProviderRecord(review);
                  setReview(null);
                  setNotice("已移除经手动核对的本地提示；没有取消后台操作。");
                } catch (reason) {
                  setReviewError(message(reason));
                  setNotice(message(reason));
                } finally {
                  setRemoving(false);
                }
              }}
            >
              已核对，移除本地提示
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
