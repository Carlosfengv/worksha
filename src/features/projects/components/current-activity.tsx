import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { recordAgentKey } from "@/shared/types/agent";
import { phaseNames, speed } from "@/features/observation/display";
import type { ProjectDetail } from "@/features/projects/types";
import { date } from "@/features/sessions/display";

export function CurrentActivity({ detail }: { detail: ProjectDetail }) {
  const { agentNames } = useAgentDirectory();
  return (
    <section className="space-y-3">
      <h3 className="text-title font-medium">当前 Agent 活动</h3>
      {detail.activities.native.map((item) => (
        <div
          key={item.id}
          className="space-y-1 rounded-lg border border-border p-4"
        >
          <p className="text-body">
            {agentNames[recordAgentKey(item)]} · {item.title}
          </p>
          <p className="text-label text-fg-muted">
            {phaseNames[item.phase] ?? item.phase} · {item.step} ·{" "}
            {date(item.eventAt)}
            {item.stale ? " · 最后快照" : ""}
          </p>
        </div>
      ))}
      {detail.activities.gateway.map((item) => (
        <div
          key={item.request.id}
          className="rounded-lg border border-border p-4 text-body"
        >
          {agentNames[recordAgentKey(item.request)]} ·{" "}
          {phaseNames[item.phase] ?? item.phase} · 当前文本速度{" "}
          {speed(item.textTokensPerSecond)} Token/s（估算）
        </div>
      ))}
      {!detail.activities.native.length &&
        !detail.activities.gateway.length && (
          <p className="text-body text-fg-muted">暂无已确认活动</p>
        )}
    </section>
  );
}
