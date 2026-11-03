import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { AssociatedCostsView } from "@/shared/ui/associated-costs";
import { costText } from "@/shared/lib/money";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import type { ColumnDef } from "@tanstack/react-table";
import { useMemo } from "react";
import type { ProjectSummary, ProjectTotals } from "@/features/projects/types";
import { number } from "@/features/sessions/display";
import { compactNumber } from "@/shared/lib/format";

export function Fact({
  label,
  value,
  exact,
}: {
  label: string;
  value: string;
  exact?: string;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-label text-fg-muted">{label}</dt>
      <dd
        className="mt-1 break-words text-title font-medium tabular-nums"
        title={exact}
      >
        {value}
      </dd>
    </div>
  );
}

export function Totals({
  totals,
  listener,
}: {
  totals: ProjectTotals;
  listener: number | null;
}) {
  return (
    <>
      <dl className="grid grid-cols-2 gap-5 xl:grid-cols-4">
        <Fact label="会话" value={String(totals.sessionCount)} />
        <Fact
          label="Token · 已知"
          value={compactNumber(totals.usage.totalTokens)}
          exact={number(totals.usage.totalTokens)}
        />
        <Fact
          label="已确认成功写入"
          value={`${totals.writeCount} 次 · ${totals.distinctFiles} 个文件`}
        />
        <Fact label="监听端口" value={number(listener)} />
      </dl>
      <p className="text-label text-fg-muted">
        {totals.modelCalls} 个模型响应
        {totals.usageMissingCalls > 0
          ? ` · ${totals.usageMissingCalls} 个用量不完整`
          : ""}
      </p>
      <AssociatedCostsView value={totals.associatedCosts} />
    </>
  );
}

export function AgentTotalsView({
  project,
  onFilter,
}: {
  project: ProjectSummary;
  onFilter: (agent: string) => void;
}) {
  const { agentNames } = useAgentDirectory();
  const columns = useMemo<ColumnDef<ProjectSummary["agents"][number]>[]>(
    () => [
      {
        id: "agent",
        header: () => <>Agent</>,
        cell: ({ row }) => (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onFilter(row.original.agentId)}
          >
            {agentNames[row.original.agentId] ?? row.original.agentId}
          </Button>
        ),
      },
      {
        id: "sessions",
        header: () => <>关联会话</>,
        cell: ({ row }) => <>{row.original.totals.sessionCount}</>,
      },
      {
        id: "models",
        header: () => <>模型响应</>,
        cell: ({ row }) => <>{row.original.totals.modelCalls}</>,
      },
      {
        id: "tokens",
        header: () => <>响应 Token · 已知</>,
        cell: ({ row }) => (
          <>
            {number(row.original.totals.usage.totalTokens)}
            {row.original.totals.usageMissingCalls > 0 && (
              <p className="text-label text-fg-muted">
                {row.original.totals.usageMissingCalls} 个响应用量未知 / 部分
              </p>
            )}
          </>
        ),
      },
      {
        id: "cost",
        header: "关联费用",
        cell: ({ row }) => (
          <>{costText(row.original.totals.associatedCosts?.cost)}</>
        ),
      },
      {
        id: "writes",
        header: () => <>成功写入</>,
        cell: ({ row }) => <>{row.original.totals.writeCount}</>,
      },
    ],
    [onFilter, agentNames],
  );
  const { table } = useDataTable({
    data: project.agents,
    columns,
    getRowId: (row) => row.agentId,
    enableRowSelection: false,
    enableHiding: false,
    manualPagination: true,
    manualSorting: true,
  });
  return (
    <section className="space-y-3">
      <h3 className="text-title font-medium">参与 Agent</h3>
      {project.agents.length ? (
        <DataTable
          table={table}
          role="region"
          aria-label="项目 Agent 用量"
          showPagination={false}
        />
      ) : (
        <p className="text-body text-fg-muted">所选日期没有会话。</p>
      )}
      <p className="text-label text-fg-subtle">
        共享会话不可按 Agent 相加；Token 与写入已去重。
      </p>
    </section>
  );
}
