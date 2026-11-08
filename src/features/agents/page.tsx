import type { Snapshot } from "@/app/types";
import { AgentList } from "@/features/agents/components/agent-list";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { bundledDshAgent } from "./bundled-source";
import { AgentDetail } from "./components/agent-detail";
import { SourceAgentDetail } from "./components/source-agent-detail";

export function AgentsPage({
  data,
  selectedId,
  selectedTab,
  refresh,
}: {
  data: Snapshot | null;
  selectedId?: string;
  selectedTab?: string;
  refresh: () => Promise<void>;
}) {
  if (!data)
    return (
      <ConnectionEmpty
        title="尚未连接本机数据"
        description="在桌面应用中读取 Agent 的会话与调用。"
        icon="brain"
      />
    );
  const agents = data.agents.some((agent) => agent.id === bundledDshAgent.id)
    ? data.agents
    : [...data.agents, bundledDshAgent];
  const selected =
    agents.find((agent) => agent.id === selectedId) ??
    agents.find((agent) => agent.configId === selectedId) ??
    agents.find((agent) => agent.family === selectedId);
  if (selectedId && !selected)
    return (
      <ConnectionEmpty
        title="找不到此 Agent"
        description="返回列表查看可用 Agent。"
        action="返回列表"
        onAction={() => {
          window.location.hash = "/agents";
        }}
      />
    );
  if (selected?.readonly || selected?.sourceId)
    return (
      <SourceAgentDetail
        key={selected.id}
        agent={selected}
        tab={selectedTab}
        providers={data.providers}
        refresh={refresh}
      />
    );
  if (selected)
    return (
      <AgentDetail
        key={`${selected.id}:${JSON.stringify(data.hiddenProjects ?? [])}`}
        agent={selected}
        source={data.sessionSources.find((s) => s.agentId === selected.family)}
        tab={selectedTab}
        refresh={refresh}
        data={data}
      />
    );
  return <AgentList data={data} agents={agents} />;
}
