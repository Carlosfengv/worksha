import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import type { Snapshot } from "@/app/types";
import { Badge } from "@/components/ui/badge";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { connectionView } from "@/features/agents/connection-display";
import { type AgentProductId } from "@/shared/types/agent";

export function AgentConnectionStatus({
  agentId,
  data,
  live,
  compact = false,
}: {
  agentId: AgentProductId;
  data: Snapshot;
  live?: { count: number; stale: boolean };
  compact?: boolean;
}) {
  const { configAgent } = useAgentDirectory();
  const view = connectionView({
    agentId: configAgent(agentId),
    productId: agentId,
    connections: data.agentConnections,
    live,
  });
  const launcher = data.observationBindings.some(
    (binding) =>
      binding.agentId === configAgent(agentId) &&
      binding.enabled &&
      !agentId.endsWith("desktop"),
  );
  return (
    <div className="min-w-0 space-y-2" aria-label="连接方式与采集范围">
      <div className="flex flex-wrap items-center gap-2">
        <Badge size="sm" status={view.status}>
          {view.label}
        </Badge>
        {view.fresh &&
          view.config &&
          view.mode !== "unknown" &&
          view.mode !== "official_login" && (
            <span
              className="min-w-0 break-words text-label text-fg-muted"
              title={view.config.endpoint ?? undefined}
            >
              {view.config.providerName}
            </span>
          )}
        <Badge
          size="sm"
          variant="plain"
          status={
            view.confirmed || view.gatewayConfigured ? "success" : "warning"
          }
          title={view.description}
        >
          {view.title}
        </Badge>
      </div>
      {view.reason && (
        <p role="status" className="text-label text-fg-muted">
          {view.reason}
        </p>
      )}
      {!compact && (
        <Accordion className="w-full">
          <AccordionItem value="connection">
            <AccordionTrigger>
              {agentId.startsWith("codex-")
                ? "连接详情 · CLI 与桌面共用 Codex 配置"
                : "连接详情"}
            </AccordionTrigger>
            <AccordionContent>
              <div className="space-y-2 text-label text-fg-muted">
                <p>{view.description}</p>
                {agentId.startsWith("codex-") && (
                  <p>切换或恢复配置会同时影响 Codex CLI 与桌面端。</p>
                )}
                {view.loginNote && <p>{view.loginNote}</p>}
                {launcher && !view.confirmed && view.mode !== "gateway" && (
                  <p>观测入口已就绪，尚未收到请求。</p>
                )}
              </div>
            </AccordionContent>
          </AccordionItem>
        </Accordion>
      )}
    </div>
  );
}
