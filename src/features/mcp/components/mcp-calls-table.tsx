import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { recordAgentKey } from "@/shared/types/agent";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { DialogClose } from "@/components/ui/dialog";
import { MobileDrawer } from "@/components/ui/mobile-drawer";
import {
  McpCallDetail,
  mcpElapsed,
} from "@/features/mcp/components/mcp-call-detail";
import type { McpGroup, McpSnapshot } from "@/features/mcp/types";
import { date, sessionLink, status } from "@/features/sessions/display";
import type { ColumnDef } from "@tanstack/react-table";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

export function McpCallsTable({
  items,
  selected,
  scope,
  asOf,
}: {
  items: McpSnapshot["calls"];
  selected: McpGroup;
  scope: string;
  asOf: string;
}) {
  const { agentNames } = useAgentDirectory();
  const [selectedCallId, setSelectedCallId] = useState<string | null>(null);
  const tableRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLElement>(null);
  const pendingFocus = useRef<HTMLElement>(null);
  const openCall = useCallback((id: string, trigger?: HTMLElement) => {
    const button = tableRef.current?.querySelector<HTMLButtonElement>(
      `[data-mcp-call="${CSS.escape(id)}"]`,
    );
    const active = document.activeElement;
    triggerRef.current =
      trigger ??
      (active instanceof HTMLElement && active === button?.closest("tr")
        ? active
        : (button ?? null));
    triggerRef.current?.focus({ preventScroll: true });
    pendingFocus.current = null;
    setSelectedCallId(id);
  }, []);
  function closeCall() {
    pendingFocus.current = triggerRef.current;
    setSelectedCallId(null);
  }
  useEffect(() => {
    const target = pendingFocus.current;
    if (selectedCallId || !target) return;
    // The modal has been removed, so WebKit can focus the table again.
    const frame = requestAnimationFrame(() => {
      pendingFocus.current = null;
      const active = document.activeElement;
      if (
        target.isConnected &&
        (active === document.body || active === document.documentElement)
      ) {
        target.focus({ preventScroll: true });
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [selectedCallId]);
  const columns = useMemo<ColumnDef<McpSnapshot["calls"][number]>[]>(
    () => [
      {
        id: "tool",
        header: () => <>工具</>,
        enableSorting: false,
        cell: ({ row }) => {
          const record = row.original;
          const { call } = record;
          const tool = call.toolName ?? call.name;
          return (
            <>
              <Button
                size="sm"
                variant="link"
                className="max-w-56 justify-start px-0"
                aria-label={`调用详情 · ${tool}`}
                aria-haspopup="dialog"
                title={tool}
                tabIndex={0}
                data-mcp-call={call.id}
                onClick={(event) => openCall(call.id, event.currentTarget)}
              >
                <span className="truncate">{tool}</span>
              </Button>
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
        header: () => <>会话</>,
        enableSorting: false,
        cell: ({ row }) => {
          const record = row.original;
          const { call } = record;
          return (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                window.location.hash = sessionLink(call.sessionId, {
                  agent: recordAgentKey(call),
                  anchor: call.id,
                  returnTo: window.location.hash.replace(/^#/, ""),
                });
              }}
            >
              查看会话
            </Button>
          );
        },
      },
    ],
    [openCall, agentNames],
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
  const expanded = items.find((row) => row.call.id === selectedCallId);
  return (
    <div ref={tableRef}>
      <DataTable
        table={table}
        role="region"
        aria-label="MCP 调用列表"
        showPagination={false}
        activeRowId={expanded?.call.id}
        onRowActivate={(row) => openCall(row.id)}
      />
      {expanded && (
        <MobileDrawer
          open
          onClose={closeCall}
          side="end"
          triggerRef={triggerRef}
          ariaLabel="MCP 调用详情"
          panelClassName="flex w-full max-w-xl flex-col overflow-hidden p-0"
        >
          <header className="flex shrink-0 items-start justify-between gap-3 border-b border-border p-4">
            <h3 className="min-w-0 break-words text-title font-medium">
              调用详情 · {expanded.call.toolName ?? expanded.call.name}
            </h3>
            <DialogClose render={<Button variant="tertiary" size="sm" />}>
              关闭详情
            </DialogClose>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto">
            <McpCallDetail
              key={`${expanded.call.id}:${asOf}`}
              row={expanded}
              service={selected}
              scope={scope}
              asOf={asOf}
            />
          </div>
        </MobileDrawer>
      )}
    </div>
  );
}
