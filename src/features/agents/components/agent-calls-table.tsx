import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { failureLabels } from "@/features/sessions/components/model-failure-detail";
import {
  date,
  kindNames,
  number,
  sessionLink,
  status,
} from "@/features/sessions/display";
import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";

import type { NativeCall } from "@/features/sessions/types";
import { recordAgentKey } from "@/shared/types/agent";

export function AgentCallsTable({
  items,
  agentId,
  loading,
  enabled = true,
  error = null,
}: {
  items: NativeCall[];
  agentId?: string;
  loading: boolean;
  enabled?: boolean;
  error?: string | null;
}) {
  const { agentNames } = useAgentDirectory();
  const columns = useMemo<ColumnDef<NativeCall>[]>(
    () => [
      {
        id: "call",
        header: () => <>调用</>,
        enableSorting: false,
        cell: ({ row }) => {
          const call = row.original;
          return (
            <>
              <div className="max-w-64 truncate" title={call.name}>
                {call.name}
              </div>
              <p className="text-label text-fg-muted">
                {kindNames[call.kind]} · {date(call.timestamp)}
              </p>
              {!agentId && (
                <p className="text-label text-fg-muted">
                  {agentNames[recordAgentKey(call)]}
                </p>
              )}
              {call.agentAttribution === "desktop_tool_execution" && (
                <p className="text-label text-fg-muted">桌面本机执行证据</p>
              )}
            </>
          );
        },
      },
      {
        id: "session",
        header: () => <>会话</>,
        enableSorting: false,
        cell: ({ row }) => {
          const call = row.original;
          return (
            <>
              <p className="max-w-64 truncate" title={call.sessionTitle}>
                {call.sessionTitle}
              </p>
              <p className="text-label text-fg-muted">
                {call.model ?? "模型未知"}
              </p>
            </>
          );
        },
      },
      {
        id: "outcome",
        header: () => <>状态 / Token</>,
        enableSorting: false,
        cell: ({ row }) => {
          const call = row.original;
          return (
            <>
              {status(call.status)}
              {call.failure && (
                <p className="text-label text-fg-muted">
                  {failureLabels[call.failure.category] ??
                    failureLabels.unknown}
                </p>
              )}
              <p className="text-label text-fg-muted">
                {call.kind === "diagnostic"
                  ? "请求数 / Token 未知"
                  : call.kind === "model"
                    ? `${number(call.usage?.totalTokens)} Token`
                    : "Token 不适用"}
              </p>
            </>
          );
        },
      },
      {
        id: "actions",
        header: () => (
          <>
            <span className="sr-only">操作</span>
          </>
        ),
        enableSorting: false,
        cell: ({ row }) => {
          const call = row.original;
          return (
            <>
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => {
                  window.location.hash = sessionLink(call.sessionId, {
                    agent: agentId ?? recordAgentKey(call),
                    anchor: call.id,
                    from: agentId,
                    tab: agentId ? "calls" : undefined,
                  });
                }}
              >
                查看对话
              </Button>
            </>
          );
        },
      },
    ],
    [agentId, agentNames],
  );

  const { table } = useDataTable({
    data: items,
    columns,
    getRowId: (row) => row.id,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    manualPagination: true,
    manualSorting: true,
  });
  return (
    <DataTable
      table={table}
      role="region"
      aria-label="Agent 调用记录"
      showPagination={false}
      emptyState={
        <ConnectionEmpty
          compact
          title={
            error
              ? "调用数据暂不可用"
              : enabled
                ? "暂无匹配调用"
                : "尚未读取调用来源"
          }
          description={
            error
              ? "可重新读取，当前没有可展示的调用快照。"
              : enabled
                ? "可切换类型，或等待来源读取完成。"
                : "请先读取本机原生会话。"
          }
        />
      }
      isLoading={loading}
      loadingMessage="正在读取调用记录"
      loadingRowCount={5}
    />
  );
}
