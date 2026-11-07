import { Button } from "@/components/ui/button";
import { TabItem, TabPanel, Tabs, TabsList } from "@/components/ui/tabs";
import {
  PublishedSources,
  sourceProvides,
  type SourceView,
} from "@/features/domain/published-sources";
import type { AgentDiscovery } from "@/shared/types/agent";
import { sourceStatus } from "../source-status";

const views: { mode: SourceView; label: string }[] = [
  { mode: "sessions", label: "Sessions" },
  { mode: "requests", label: "调用" },
  { mode: "activity", label: "活动" },
  { mode: "projects", label: "项目" },
  { mode: "mcp", label: "MCP" },
];

export function SourceAgentDetail({
  agent,
  tab,
}: {
  agent: AgentDiscovery;
  tab?: string;
}) {
  const available = views.filter(({ mode }) => sourceProvides(agent, mode));
  const value =
    tab && available.some(({ mode }) => mode === tab)
      ? tab
      : tab === "connection" || !available.length
        ? "connection"
        : available[0].mode;
  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-heading font-medium">{agent.name}</h2>
          <p className="mt-1 text-body text-fg-muted">{sourceStatus(agent)}</p>
        </div>
        <Button asChild variant="tertiary" size="sm">
          <a href="#/agents">返回 Agents</a>
        </Button>
      </div>
      <Tabs
        value={value}
        onValueChange={(next) => {
          window.location.hash = `/agents/${encodeURIComponent(agent.id)}?tab=${next}`;
        }}
        variant="underline"
        color="default"
      >
        <TabsList aria-label="Agent 详情">
          {available.map(({ mode, label }) => (
            <TabItem key={mode} value={mode} label={label} />
          ))}
          <TabItem value="connection" label="连接与观测" />
        </TabsList>
        {available.map(({ mode }) => (
          <TabPanel key={mode} value={mode} className="pt-4">
            {value === mode && (
              <PublishedSources
                mode={mode}
                sourceId={agent.sourceId}
                productId={agent.id}
              />
            )}
          </TabPanel>
        ))}
        <TabPanel value="connection" className="space-y-3 pt-4">
          <p className="text-body text-fg-muted">
            此接入提供只读观测。读取权限和可用数据由对应插件管理。
          </p>
          <Button asChild variant="neutral" size="sm">
            <a href="#/plugins?scene=agents">管理 Agent 接入</a>
          </Button>
        </TabPanel>
      </Tabs>
    </div>
  );
}
