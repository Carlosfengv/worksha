import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { recordAgentKey } from "@/shared/types/agent";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  McpCallDetail,
  mcpElapsed,
} from "@/features/mcp/components/mcp-call-detail";
import type { McpGroup, McpSnapshot } from "@/features/mcp/types";
import { date, sessionLink, status } from "@/features/sessions/display";
import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";

export function McpCallsTable({
  items,
  selected,
  scope,
  asOf,
  expandedCall,
  onExpandedCallChange,
}: {
  items: McpSnapshot["calls"];
  selected: McpGroup;
  scope: string;
  asOf: string;
  expandedCall: string;
  onExpandedCallChange: (next: string | ((old: string) => string)) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const columns = useMemo<ColumnDef<McpSnapshot["calls"][number]>[]>(
    () => [
      {
        id: "tool",
        header: () => <>工具</>,
        enableSorting: false,
        cell: ({ row }) => {
          const record = row.original;
          const { call } = record;
          return (
            <>
              <p className="max-w-72 break-all font-medium">
                {call.toolName ?? call.name}
              </p>
              <p className="text-label text-fg-muted">
                {record.association === "current_configuration"
                  ? "当前配置关联"
                  : "配置来源未知"}
              </p>
            </>
          );
        },
      },
      {
        id: "agent-model",
        header: () => <>Agent / 模型</>,
        enableSorting: false,
        cell: ({ row }) => {
          const record = row.original;
          const { call } = record;
          return (
            <>
              <p>{agentNames[recordAgentKey(call)]}</p>
              <p className="max-w-56 break-all text-label text-fg-muted">
                {call.model ?? "模型未知"}
              </p>
            </>
          );
        },
      },
      {
        id: "project-session",
        header: () => <>项目 / Session</>,
        enableSorting: false,
        cell: ({ row }) => {
          const record = row.original;
          const { call } = record;
          return (
            <>
              <p
                className="max-w-64 break-all"
                title={record.projectPath ?? undefined}
              >
                {record.projectPath?.split("/").filter(Boolean).at(-1) ??
                  "项目未知"}
              </p>
              <p className="max-w-64 break-words text-label text-fg-muted">
                {call.sessionTitle}
              </p>
            </>
          );
        },
      },
      {
        id: "time",
        header: () => <>调用时间</>,
        enableSorting: false,
        cell: ({ row }) => {
          const record = row.original;
          const { call } = record;
          return <>{date(call.timestamp)}</>;
        },
      },
      {
        id: "outcome",
        header: () => <>结果 / 延迟</>,
        enableSorting: false,
        cell: ({ row }) => {
          const record = row.original;
          const { call } = record;
          return (
            <>
              <p>
                {call.status === "error"
                  ? "原生错误（细分未知）"
                  : status(call.status)}
              </p>
              <p className="text-label text-fg-muted">
                {mcpElapsed(call.durationMs)}
              </p>
            </>
          );
        },
      },
      {
        id: "actions",
        header: () => <>详情 / 对话</>,
        enableSorting: false,
        cell: ({ row }) => {
          const record = row.original;
          const { call } = record;
          return (
            <>
              <Button
                size="sm"
                variant="ghost"
                aria-expanded={expandedCall === call.id}
                aria-controls={`mcp-detail-${call.id}`}
                onClick={() =>
                  onExpandedCallChange((old) =>
                    old === call.id ? "" : call.id,
                  )
                }
              >
                {expandedCall === call.id ? "收起详情" : "调用详情"}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  window.location.hash = sessionLink(call.sessionId, {
                    agent: call.agentProductId ?? "unknown",
                    anchor: call.id,
                    returnTo: window.location.hash.replace(/^#/, ""),
                  });
                }}
              >
                查看 Session
              </Button>
            </>
          );
        },
      },
    ],
    [expandedCall, onExpandedCallChange, agentNames],
  );

  const { table } = useDataTable({
    data: items,
    columns,
    getRowId: (row) => row.call.id,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    manualPagination: true,
    manualSorting: true,
  });
  const expanded = items.find((row) => row.call.id === expandedCall);
  return (
    <div className="space-y-4">
      <DataTable
        table={table}
        role="region"
        aria-label="MCP 调用列表"
        showPagination={false}
        activeRowId={expandedCall}
        onRowActivate={(row) =>
          onExpandedCallChange((old) => (old === row.id ? "" : row.id))
        }
      />
      {expanded && (
        <div id={`mcp-detail-${expanded.call.id}`}>
          <McpCallDetail
            key={`${expanded.call.id}:${asOf}`}
            row={expanded}
            service={selected}
            scope={scope}
            asOf={asOf}
          />
        </div>
      )}
    </div>
  );
}
