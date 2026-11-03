import { useEffect, useRef, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { formatDateTime } from "@/shared/lib/date-time";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { settingsApi } from "../api";
import {
  backendStates,
  backendPhases,
  backendCategories,
  type BackendEvent,
} from "../backend-result";

const columns: ColumnDef<BackendEvent>[] = [
  {
    accessorKey: "at",
    header: "记录时间",
    cell: ({ row }) => formatDateTime(row.original.at, "full"),
  },
  {
    id: "event",
    header: "事件",
    cell: ({ row }) => (
      <div className="max-w-64 whitespace-normal">
        <p>
          {backendPhases[row.original.phase]} ·{" "}
          {backendCategories[row.original.category]}
        </p>
        {row.original.category === "child_exited" && (
          <p>
            退出码：{row.original.exitCode ?? "未知"}；信号：
            {row.original.signal ?? "未提供"}
          </p>
        )}
      </div>
    ),
  },
  {
    accessorKey: "operationId",
    header: "诊断编号",
    cell: ({ row }) => (
      <span className="inline-block max-w-64 break-all whitespace-normal">
        {row.original.operationId}
      </span>
    ),
  },
];
const noEvents: BackendEvent[] = [];
export function BackendConnection() {
  const resource = usePollingResource(
    "backend-connection",
    settingsApi.backendDiagnostics,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const alive = useRef(true);
  const running = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const rows = resource.data?.events ?? noEvents;
  const pagination = useTablePagination(rows.length, 10);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.id,
    enableSorting: false,
    enableHiding: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  const reconnect = async () => {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError(null);
    setConfirmed(false);
    try {
      await settingsApi.reconnectBackend();
      if (!alive.current) return;
      setConfirmed(true);
    } catch (reason) {
      if (alive.current)
        setError(
          reason instanceof Error
            ? reason.message
            : "连接核对未完成，请检查状态后再试。",
        );
    } finally {
      running.current = false;
      if (alive.current) setBusy(false);
    }
  };
  return (
    <section aria-label="本地服务连接" className="space-y-3">
      <h2 className="text-title font-medium">本地服务连接</h2>
      <p className="text-body text-fg-muted">
        重新连接前核对已运行后台的身份。恢复连接后，未完成操作仍需核查结果再重试；后台未运行时，请先启动常驻服务或正常重新打开应用。
      </p>
      <p className="text-body">
        {resource.data
          ? backendStates[resource.data.state]
          : resource.loading
            ? "正在读取连接诊断…"
            : "连接诊断尚未取得"}
        {resource.data?.verifiedAt != null && (
          <span className="text-fg-muted">
            {" "}
            · 上次确认：{formatDateTime(resource.data.verifiedAt, "full")}
          </span>
        )}
        {resource.data && resource.loading && (
          <span className="text-fg-muted"> · 正在更新诊断，以上为上次记录</span>
        )}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="secondary"
          size="sm"
          loading={busy}
          disabled={!resource.data?.canReconnect}
          onClick={() => void reconnect()}
        >
          核对并重新连接
        </Button>
        <Button
          variant="tertiary"
          size="sm"
          disabled={busy}
          loading={resource.loading}
          onClick={() => void resource.reload()}
        >
          刷新诊断记录
        </Button>
      </div>
      {confirmed && (
        <p role="status" className="text-body">
          后台连接已通过身份核对。数据读取状态以页面提示为准。
        </p>
      )}
      {(error || resource.error) && (
        <Alert status="warning">
          <AlertTitle>{error ? "连接处理提示" : "诊断读取未完成"}</AlertTitle>
          <AlertDescription>
            {error ?? resource.error}
            {!error && resource.data && "；以下保留上次诊断记录。"}
          </AlertDescription>
        </Alert>
      )}
      <Accordion type="multiple" className="w-full">
        <AccordionItem value="events">
          <AccordionTrigger>最近诊断记录</AccordionTrigger>
          <AccordionContent>
            <p className="mb-3 text-body text-fg-muted">
              仅保留本次打开应用内最近 32
              条记录；包含阶段、固定错误类别与退出码，不包含请求正文或凭据。
            </p>
            <DataTable
              table={table}
              aria-label="最近后台连接诊断"
              isLoading={!resource.data && resource.loading}
              emptyMessage="本次打开尚无诊断记录"
              loadingMessage="正在读取诊断记录"
              paginationProps={{ labels: dataTablePagination }}
            />
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </section>
  );
}
