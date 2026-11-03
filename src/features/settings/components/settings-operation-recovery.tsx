import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import type { ColumnDef } from "@tanstack/react-table";
import type { ReactNode } from "react";
import type { SettingsRecovery } from "../pending-settings";
import type { SettingsUpdate } from "../types";

type RecoveryRow = { id: string; summary: ReactNode; actions: ReactNode };
const columns: ColumnDef<RecoveryRow>[] = [
  {
    id: "summary",
    header: "原设置请求",
    cell: ({ row }) => (
      <div className="max-w-96 whitespace-normal">{row.original.summary}</div>
    ),
  },
  {
    id: "actions",
    header: "核查与重试",
    cell: ({ row }) => row.original.actions,
  },
];

export function SettingsRequestSummary({
  request,
  storeId,
}: {
  request: SettingsUpdate;
  storeId?: string;
}) {
  return (
    <div className="mt-2 space-y-1">
      <p className="break-all">操作编号：{request.operationId}</p>
      <p>
        {request.storeId === storeId
          ? "属于当前后台。"
          : "属于其他或尚未确认的后台。"}
        原偏好：额外保存内容{request.value.captureContent ? "开启" : "关闭"}
        ，保留
        {request.value.retentionDays} 天；基础版本 {request.expectedRevision}。
      </p>
      <p>这里只保存选择，内容归档与按天自动到期尚未启用。</p>
    </div>
  );
}

export function SettingsOperationRecovery({
  pending,
  activeId,
  storeId,
  busy,
  check,
  retry,
  notice,
}: {
  pending: SettingsRecovery;
  activeId?: string;
  storeId?: string;
  busy: boolean;
  check: (request: SettingsUpdate) => void;
  retry: (request: SettingsUpdate) => void;
  notice?: string;
}) {
  const requests = pending.requests.filter((r) => r.operationId !== activeId);
  const rows: RecoveryRow[] = requests.map((request) => ({
    id: request.operationId,
    summary: <SettingsRequestSummary request={request} storeId={storeId} />,
    actions: (
      <div className="flex max-w-64 flex-wrap gap-2">
        <Button
          variant="tertiary"
          size="sm"
          disabled={busy}
          onClick={() => check(request)}
        >
          核查保留的保存
        </Button>
        <Button
          variant="tertiary"
          size="sm"
          disabled={busy || request.storeId !== storeId}
          onClick={() => retry(request)}
        >
          重试保留的保存
        </Button>
      </div>
    ),
  }));
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.id,
    enableRowSelection: false,
    enableSorting: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: { pageIndex: 0, pageSize: Math.max(1, rows.length) } },
  });
  if (pending.available && !pending.invalid && !requests.length && !notice)
    return null;
  return (
    <section className="space-y-3" aria-label="保留的设置操作">
      {(!pending.available || pending.invalid > 0) && (
        <Alert status="warning">
          <AlertTitle>设置恢复记录暂不可用</AlertTitle>
          <AlertDescription>
            {!pending.available
              ? "无法读取本机恢复记录，新的设置保存已停用。请恢复本机存储后重新打开设置。"
              : "存在不可读取或超出容量的记录；原记录已保留，新的保存已停用。请先核查原操作。"}
          </AlertDescription>
        </Alert>
      )}
      {pending.atCapacity && (
        <Alert status="warning">
          <AlertTitle>待核查设置操作已达上限</AlertTitle>
          <AlertDescription>
            请先核对已有记录；没有自动删除旧编号以开始新的保存。
          </AlertDescription>
        </Alert>
      )}
      {requests.length > 0 && (
        <Alert status="warning">
          <AlertTitle>保留的设置保存待核查</AlertTitle>
          <AlertDescription>
            <p>
              重开页面不会自动提交。查询未找到回执也不能证明从未保存；同编号重试只使用保留的原参数。
            </p>
            <DataTable table={table} showPagination={false} />
          </AlertDescription>
        </Alert>
      )}
      {notice && (
        <p role="status" className="text-body text-fg-muted">
          {notice}
        </p>
      )}
    </section>
  );
}
