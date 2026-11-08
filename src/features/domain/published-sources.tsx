import { useMemo } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { domainDirectoryResource } from "./queries";
import { RequestWorkspace } from "./request-workspace";
import { ActivityWorkspace } from "./activity-workspace";
import { FactWorkspace } from "./fact-workspace";
import { UnifiedWorkspace } from "./session-workspace";
import type { QueryScope } from "./types";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import type { AgentProduct } from "@/shared/types/agent";

const capabilities = {
  sessions: "domain.session-source@1",
  requests: "domain.call-source@1",
  activity: "domain.activity-source@1",
  projects: "domain.project-source@1",
  mcp: "domain.mcp-source@1",
} as const;
export type SourceView = keyof typeof capabilities;

export function sourceProvides(agent: AgentProduct, mode: SourceView) {
  return Boolean(
    agent.sourceId && agent.capabilities?.includes(capabilities[mode]),
  );
}

export function PublishedSources({
  mode,
  sourceId,
  productId,
  manageHref = "#/plugins?scene=agents",
}: {
  mode: SourceView;
  sourceId?: string;
  productId?: string;
  manageHref?: string;
}) {
  const { agentProducts } = useAgentDirectory();
  const eligible = useMemo(
    () =>
      new Set(
        agentProducts
          .filter(
            (agent) =>
              sourceProvides(agent, mode) &&
              (!sourceId || agent.sourceId === sourceId) &&
              (!productId || agent.id === productId),
          )
          .map((agent) => agent.sourceId),
      ),
    [agentProducts, mode, sourceId, productId],
  );
  const enabled = eligible.size > 0;
  const directory = useSharedRead(domainDirectoryResource, enabled);
  const sources = useMemo(() => {
    return (directory.data?.catalog.sources ?? []).filter((s) =>
      eligible.has(s.id),
    );
  }, [directory.data, eligible]);
  if (!enabled) return null;
  if (directory.error)
    return (
      <Alert status="warning">
        <AlertTitle>Agent 来源暂不可读取</AlertTitle>
        <AlertDescription>{directory.error}</AlertDescription>
      </Alert>
    );
  if (!sources.length)
    return sourceId || productId ? (
      <p role="status" className="text-body text-fg-muted">
        {directory.loading
          ? "正在读取来源…"
          : "暂无可用数据，请检查插件状态与读取授权。"}
      </p>
    ) : null;
  const names = Object.fromEntries(
    sources.map((s) => [
      s.id,
      directory.data?.catalog.products
        .filter((p) => p.sourceId === s.id)
        .map((p) => p.displayName)
        .join(" / ") || s.id,
    ]),
  );
  const scope: QueryScope = {
    kind: "external",
    sourceIds: sources.slice(0, 16).map((s) => s.id),
  };
  const title = {
    sessions: "已连接 Agent 的会话",
    requests: "已连接 Agent 的响应",
    activity: "Agent 的活动记录",
    projects: "已连接 Agent 的项目",
    mcp: "已连接 Agent 的工具记录",
  }[mode];
  const close = () => {
    window.location.hash = manageHref;
  };
  return (
    <section aria-label={title} className="space-y-3">
      <h2 className="text-title font-medium">{title}</h2>
      <p className="text-label text-fg-muted">
        {Object.values(names).join("、")} ·
        数据与活动由来源报告，未提供的指标保持未知。
      </p>
      {sources.length > 16 && (
        <Alert status="warning">
          <AlertTitle>当前展示前 16 个来源</AlertTitle>
          <AlertDescription>
            请在 Agent 详情中选择其他来源查看。
          </AlertDescription>
        </Alert>
      )}
      <div className="flex min-h-0 flex-col">
        {mode === "sessions" ? (
          <UnifiedWorkspace
            initialSource={sourceId}
            initialProduct={productId}
            manageHref={manageHref}
          />
        ) : mode === "requests" ? (
          <RequestWorkspace
            scope={scope}
            names={names}
            productId={productId}
            onClose={close}
            closeLabel="管理来源"
          />
        ) : mode === "activity" ? (
          <ActivityWorkspace
            scope={scope}
            names={names}
            productId={productId}
            onClose={close}
            closeLabel="管理来源"
          />
        ) : (
          <FactWorkspace
            scope={scope}
            names={names}
            initialKind={mode === "mcp" ? "mcp-execution" : "project"}
            returnLabel="管理来源"
            onClose={close}
          />
        )}
      </div>
    </section>
  );
}
