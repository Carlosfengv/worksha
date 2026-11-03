import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import type { ColumnDef } from "@tanstack/react-table";
import type { useCredentialOperations } from "../hooks/use-credential-operations";
import type { RecoverableCredentialReceipt } from "../credential-receipt";
import { message } from "../display";
import { useState, type ReactNode } from "react";
import { LegacyCredentialRecovery } from "./legacy-credential-recovery";

interface RecoveryRow {
  id: string;
  label: string;
  status: string;
  detail: string | null;
  actions: ReactNode;
}
const columns: ColumnDef<RecoveryRow>[] = [
  {
    accessorKey: "label",
    header: "凭据名称",
    cell: ({ row }) => (
      <div className="max-w-64 break-words whitespace-normal">
        {row.original.label}
      </div>
    ),
  },
  {
    accessorKey: "status",
    header: "保存状态",
    cell: ({ row }) => (
      <div className="max-w-64 space-y-2 whitespace-normal">
        <p>{row.original.status}</p>
        {row.original.detail && <p>{row.original.detail}</p>}
      </div>
    ),
  },
  {
    id: "actions",
    header: "操作",
    cell: ({ row }) => row.original.actions,
  },
];

export function CredentialRecovery({
  operations,
  currentReference,
  readSecret,
  onAdopt,
  disabled,
}: {
  operations: ReturnType<typeof useCredentialOperations>;
  currentReference: string | null;
  readSecret: () => string;
  onAdopt: (receipt: RecoverableCredentialReceipt) => void;
  disabled: boolean;
}) {
  const [error, setError] = useState<string | null>(null);
  const [discard, setDiscard] = useState<
    (typeof operations.records)[number] | null
  >(null);
  const act = async (work: () => Promise<void>) => {
    if (disabled) return;
    setError(null);
    try {
      await work();
    } catch (reason) {
      setError(message(reason));
    }
  };
  const rows: RecoveryRow[] = operations.records.map((record) => ({
    id: record.storageKey,
    label: record.label,
    status: record.issue
      ? "记录无法核查"
      : !record.storeId
        ? "旧版记录：来源未绑定"
        : record.storeId !== operations.storeId
          ? "属于其他后台，未采用"
          : record.version === 3 &&
              operations.namespace &&
              record.namespaceId !== operations.namespace.namespaceId
            ? "属于之前的恢复空间，未采用"
            : record.legacySourceDigest
              ? "旧凭据迁移尚待核查"
              : !operations.canUse(record)
                ? "恢复状态尚未核查"
                : record.receipt
                  ? record.receipt.credentialRef === currentReference
                    ? "当前草稿已采用，连接尚待保存"
                    : "已保存，尚待采用"
                  : record.cancelledPending
                    ? "已停止后续保存，已发出写入仍待核查"
                    : "结果尚未确认",
    detail:
      record.issue ??
      (!record.storeId
        ? "原记录没有数据库身份，已保留；不能自动认作当前后台的操作。"
        : record.legacySourceDigest && operations.canRecoverLegacy(record)
          ? "已连接原恢复空间。请继续核对旧记录来源并明确确认，再核查凭据保存结果。"
          : !operations.canUse(record)
            ? record.version === 3
              ? "请连接原恢复空间或恢复原数据库后核查，原请求不会自动转入新空间。"
              : "请连接原后台并核查原恢复材料后操作。"
            : record.cancelledPending && !record.receipt
              ? "移除提醒只清除本机查看记录；后台仍保留核查记录，不删除凭据。"
              : null),
    actions: (
      <div className="flex flex-wrap gap-2">
        {!record.issue &&
          (record.version === 1 || !!record.legacySourceDigest) && (
            <LegacyCredentialRecovery
              record={record}
              operations={operations}
              disabled={disabled}
            />
          )}
        {operations.canUse(record) && !record.receipt && (
          <>
            <Button
              type="button"
              size="sm"
              variant="secondary"
              disabled={disabled || operations.busy}
              onClick={() =>
                void act(async () => {
                  const result = await operations.check(record, false);
                  if (result.status === "pending")
                    setError(
                      "尚未确认写入，可稍后核查；也可输入同一 API Key 后重试本次保存，或取消尚未完成的操作。",
                    );
                })
              }
            >
              核查保存结果
            </Button>
            {!record.cancelledPending && (
              <>
                <Button
                  type="button"
                  size="sm"
                  variant="secondary"
                  disabled={disabled || operations.busy}
                  onClick={() =>
                    void act(async () => {
                      onAdopt(
                        await operations.create(
                          record.label,
                          readSecret(),
                          record,
                        ),
                      );
                    })
                  }
                >
                  用同一密钥重试
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={disabled || operations.busy}
                  onClick={() =>
                    void act(async () => {
                      const result = await operations.check(record, true);
                      if (result.status === "ready")
                        setError(
                          "凭据已经保存，未删除。可核对名称后采用到当前连接。",
                        );
                    })
                  }
                >
                  取消未完成保存
                </Button>
              </>
            )}
            {record.cancelledPending && (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                disabled={disabled || operations.busy}
                onClick={() =>
                  void act(async () => {
                    const result = await operations.dismissCancelled(record);
                    if (result.status === "ready")
                      setError("凭据已经保存，未移除提醒；可采用到当前连接。");
                  })
                }
              >
                移除本机提醒
              </Button>
            )}
          </>
        )}
        {operations.canUse(record) &&
          record.receipt &&
          record.receipt.storeId === operations.storeId &&
          record.receipt.credentialRef !== currentReference && (
            <Button
              type="button"
              size="sm"
              variant="secondary"
              disabled={disabled || operations.busy}
              onClick={() => {
                setError(null);
                onAdopt(record.receipt!);
              }}
            >
              采用到当前连接
            </Button>
          )}
        <Button
          type="button"
          size="sm"
          variant="ghost"
          disabled={operations.busy}
          onClick={() => setDiscard(record)}
        >
          人工核对后移除提示
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
  if (!rows.length) return null;
  return (
    <>
      <Alert>
        <AlertTitle>凭据保存结果与待绑定凭据</AlertTitle>
        <AlertDescription className="space-y-3">
          <p>
            核查只确认凭据，不自动重发连接或模型请求。已保存的凭据会保留，成功保存使用它的连接后移出此列表。
          </p>
          <DataTable table={table} showPagination={false} />
          {error && <p role="status">{error}</p>}
        </AlertDescription>
      </Alert>
      <Dialog
        open={!!discard}
        onOpenChange={(open) => {
          if (!open) setDiscard(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>移除这条本机凭据提示</DialogTitle>
            <DialogDescription>
              请先在原后台人工核对保存结果。此操作只移除本机提示，不查询或取消后台操作，不删除钥匙串中的凭据。旧记录和无法核查的记录不会被自动迁移到当前后台。
            </DialogDescription>
          </DialogHeader>
          <p className="break-all">
            {discard?.label} · {discard?.operationId || "编号无法核查"}
          </p>
          <DialogFooter>
            <Button variant="secondary" onClick={() => setDiscard(null)}>
              保留提示
            </Button>
            <Button
              onClick={() => {
                if (!discard) return;
                try {
                  operations.discardLocal(discard);
                  setDiscard(null);
                } catch (reason) {
                  setError(message(reason));
                  setDiscard(null);
                }
              }}
            >
              已人工核对，移除提示
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
