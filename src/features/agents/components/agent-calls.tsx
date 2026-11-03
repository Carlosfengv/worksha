import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { sessionsApi } from "@/features/sessions/api";
import { kindNames, number } from "@/features/sessions/display";
import { useNativePage } from "@/features/sessions/hooks/use-native-page";
import type { CallKind } from "@/features/sessions/types";
import { useCallback, useState } from "react";
import { AgentCallsTable } from "./agent-calls-table";

export function AgentCalls({
  agentId,
  enabled,
}: {
  agentId: string;
  enabled: boolean;
}) {
  const [kind, setKind] = useState<CallKind | "all">("all");
  const loader = useCallback(
    (cursor?: string) =>
      sessionsApi.calls({
        agent: agentId,
        kind: kind === "all" ? undefined : kind,
        cursor,
      }),
    [agentId, kind],
  );
  const calls = useNativePage(`${agentId}:${kind}:${enabled}`, loader);
  return (
    <div className="space-y-4">
      <Select
        value={kind}
        onValueChange={(value) => setKind(value as CallKind | "all")}
      >
        <SelectTrigger aria-label="调用类型" />
        <SelectContent>
          <SelectItem value="all">全部调用</SelectItem>
          {Object.entries(kindNames).map(([id, name]) => (
            <SelectItem key={id} value={id}>
              {name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <p className="text-label text-fg-muted">
        共 {number(calls.data?.total)}{" "}
        条原生调用与异常记录，点击进入对应对话。异常日志不计作额外模型请求；缺少请求
        ID 的记录不补造模型调用。
      </p>
      <AgentCallsTable
        items={calls.data?.items ?? []}
        agentId={agentId}
        loading={calls.loading && !calls.data}
        enabled={enabled}
        error={calls.error}
      />
      {calls.error && (
        <p role="alert" className="text-body text-fg-muted">
          {calls.error}
        </p>
      )}
      <div className="flex gap-2">
        <Button
          variant="ghost"
          size="sm"
          loading={calls.loading}
          onClick={() => void calls.reload()}
        >
          刷新调用
        </Button>
        {calls.data?.nextCursor && (
          <Button
            variant="tertiary"
            loading={calls.loading}
            size="sm"
            onClick={() => void calls.loadMore()}
          >
            加载更多调用
          </Button>
        )}
      </div>
    </div>
  );
}
