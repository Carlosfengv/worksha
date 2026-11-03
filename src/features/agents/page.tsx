import type { Snapshot } from "@/app/types";
import { AgentList } from "@/features/agents/components/agent-list";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { AgentDetail } from "./components/agent-detail";
import { PluginEntries } from "@/features/plugins/entries";

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
  const selected =
    data.agents.find((agent) => agent.id === selectedId) ??
    data.agents.find((agent) => agent.configId === selectedId) ??
    data.agents.find((agent) => agent.family === selectedId);
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
  return (
    <div className="space-y-6">
      <AgentList data={data} />
      <PluginEntries kind="agents" />
    </div>
  );
}
