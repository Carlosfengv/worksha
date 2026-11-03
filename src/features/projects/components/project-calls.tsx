import { useCallback, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { AgentCallsTable } from "@/features/agents/components/agent-calls-table";
import { sessionsApi } from "@/features/sessions/api";
import { kindNames } from "@/features/sessions/display";
import type { CallKind } from "@/features/sessions/types";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";

export function ProjectCalls({
  projectId,
  agent,
  days,
  sourceKey,
}: {
  projectId: string;
  agent: string;
  days: string;
  sourceKey: string;
}) {
  const [kind, setKind] = useState("all");
  const [limit, setLimit] = useState(40);
  const loader = useCallback(async () => {
    const query = {
      project: projectId,
      agent: agent === "all" ? undefined : agent,
      days: days === "all" ? undefined : Number(days),
      kind: kind === "all" ? undefined : (kind as CallKind),
    };
    let result = await sessionsApi.calls(query);
    while (result.nextCursor && result.items.length < limit) {
      const next = await sessionsApi.calls({
        ...query,
        cursor: result.nextCursor,
      });
      result = { ...next, items: [...result.items, ...next.items] };
    }
    return result;
  }, [projectId, agent, days, kind, limit]);
  const resource = usePollingResource(
    `project-calls:${projectId}:${agent}:${days}:${kind}:${limit}:${sourceKey}`,
    loader,
    true,
  );
  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h3 className="text-title font-medium">
          项目调用记录 · {resource.data?.total ?? "未知"}
        </h3>
        <Select
          value={kind}
          onValueChange={(value) => {
            setKind(value);
            setLimit(40);
          }}
        >
          <SelectTrigger aria-label="项目调用类型" />
          <SelectContent>
            <SelectItem value="all">全部调用类型</SelectItem>
            {(["model", "tool", "mcp", "skill", "diagnostic"] as const).map(
              (value) => (
                <SelectItem key={value} value={value}>
                  {kindNames[value]}
                </SelectItem>
              ),
            )}
          </SelectContent>
        </Select>
      </div>
      {resource.error && (
        <p role="alert" className="text-label text-fg-muted">
          {resource.error} · 已有快照可能过期。
        </p>
      )}
      <AgentCallsTable
        items={resource.data?.items ?? []}
        loading={resource.loading}
        error={resource.error}
      />
      {resource.data?.nextCursor && (
        <Button
          variant="tertiary"
          loading={resource.loading}
          onClick={() => setLimit((n) => n + 40)}
        >
          加载更多项目调用
        </Button>
      )}
      <p className="text-label text-fg-subtle">
        与项目的 Agent 和会话开始日期筛选一致；Token
        仅统计模型响应，工具关联不重复计量。诊断记录的请求数和用量保持未知。
      </p>
    </section>
  );
}
