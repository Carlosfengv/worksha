import { useState } from "react";
import { Alert, AlertTitle, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import type { ColumnDef } from "@tanstack/react-table";
import type { ReactNode } from "react";
import type { useGatewayOperations } from "../use-gateway-operations";
import type { PendingGatewayOperation } from "../types";
import { sameGatewayRequest } from "../gateway-operation";
type Row = { id: string; summary: ReactNode; actions: ReactNode };
const columns: ColumnDef<Row>[] = [
  {
    id: "summary",
    header: "原客户端请求",
    cell: ({ row }) => (
      <div className="max-w-96 whitespace-normal break-all">
        {row.original.summary}
      </div>
    ),
  },
  {
    id: "actions",
    header: "核查与恢复",
    cell: ({ row }) => row.original.actions,
  },
];
export function GatewayOperationRecovery({
  operations,
  storeId,
}: {
  operations: ReturnType<typeof useGatewayOperations>;
  storeId?: string;
}) {
  const [confirm, setConfirm] = useState<{
    kind: "file" | "revoke";
    row: PendingGatewayOperation;
  } | null>(null);
  const { pending, busy, confirmed, settled } = operations;
  const rows: Row[] = pending.requests.map((row) => {
    const { request } = row;
    const result = confirmed[request.operationId];
    const known = settled[request.operationId];
    const done = known && JSON.stringify(known.pending) === JSON.stringify(row);
    const issued =
      request.action.kind === "create" &&
      !done &&
      result &&
      sameGatewayRequest(result.request, request);
    return {
      id: request.operationId,
      summary: (
        <div className="space-y-1">
          <p className="break-all">操作编号：{request.operationId}</p>
          <p>
            {request.storeId === storeId
              ? "属于当前后台。"
              : "属于其他或尚未确认的后台。"}
            {request.action.kind === "create"
              ? `原客户端：${request.action.draft.name}；${request.action.draft.agent}；模型 ${request.action.draft.models.join("、")}。`
              : `原撤销客户端：${request.action.clientId}。`}
          </p>
          <p>
            {done
              ? known.label
              : issued
                ? "签发已确认，原连接文件待核对。"
                : "原操作结果待核查。"}
          </p>
        </div>
      ),
      actions: (
        <div className="flex max-w-72 flex-wrap gap-2">
          <Button
            variant="tertiary"
            size="sm"
            disabled={busy || !!done}
            onClick={() => void operations.check(row)}
          >
            核查原客户端操作
          </Button>
          <Button
            variant="tertiary"
            size="sm"
            disabled={busy || request.storeId !== storeId || !!issued || !!done}
            onClick={() => void operations.retry(row)}
          >
            重试原客户端操作
          </Button>
          {done && (
            <Button
              variant="tertiary"
              size="sm"
              disabled={busy}
              onClick={() => void operations.removeKnown(row)}
            >
              移除已确认的恢复记录
            </Button>
          )}
          {issued && (
            <>
              <Button
                variant="tertiary"
                size="sm"
                disabled={busy}
                onClick={() => setConfirm({ kind: "file", row })}
              >
                连接文件已核对
              </Button>
              <Button
                variant="tertiary"
                size="sm"
                disabled={busy || request.storeId !== storeId}
                onClick={() => setConfirm({ kind: "revoke", row })}
              >
                撤销本次签发
              </Button>
            </>
          )}
        </div>
      ),
    };
  });
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.id,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: { pageIndex: 0, pageSize: Math.max(1, rows.length) } },
  });
  return (
    <section className="space-y-3" aria-label="保留的客户端操作">
      {(!pending.available || pending.invalid > 0 || pending.atCapacity) && (
        <Alert status="warning">
          <AlertTitle>客户端恢复记录暂不可用或已达上限</AlertTitle>
          <AlertDescription>
            新的客户端操作已停用。原有记录保留，请恢复本机存储并核查原编号；没有自动清理或改用新编号。
          </AlertDescription>
        </Alert>
      )}
      {rows.length > 0 && (
        <Alert status="warning">
          <AlertTitle>保留的客户端操作待核查</AlertTitle>
          <AlertDescription>
            <p>
              重开页面不会自动提交。未查到不代表从未签发；同编号重试只使用原请求。密钥只在首次签发返回，已签发但文件未知时请核对原文件或撤销原客户端。
            </p>
            <DataTable table={table} showPagination={false} />
          </AlertDescription>
        </Alert>
      )}
      <Dialog
        open={confirm !== null}
        onOpenChange={(open) => {
          if (!open && !busy) setConfirm(null);
        }}
      >
        <DialogContent>
          <DialogTitle>
            {confirm?.kind === "file"
              ? "确认原连接文件已核对"
              : "撤销原后台本次签发"}
          </DialogTitle>
          <DialogDescription>
            {confirm?.kind === "file"
              ? "请确认原连接文件已保存于预期位置。此操作仅移除本机恢复记录，不更改客户端授权，也不会生成新密钥。"
              : "将使用保留的原补偿编号撤销原后台本次客户端。其他客户端和已有请求继续运行，请同时核对目标文件。"}
          </DialogDescription>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setConfirm(null)}
            >
              取消
            </Button>
            <Button
              disabled={busy}
              onClick={() => {
                const selected = confirm;
                if (!selected) return;
                void (
                  selected.kind === "file"
                    ? operations.acknowledge(selected.row)
                    : operations.revokeIssued(selected.row)
                ).then(() => setConfirm(null));
              }}
            >
              {confirm?.kind === "file" ? "确认文件已核对" : "确认撤销本次签发"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
