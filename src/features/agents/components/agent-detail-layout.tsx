import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { TabItem, Tabs, TabsList } from "@/components/ui/tabs";
import { PluginDetailSlots } from "@/features/plugins/detail-slots";
import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import type { AgentDiscovery } from "@/shared/types/agent";

export const agentDetailTabs = {
  overview: { id: "overview", label: "概览" },
  calls: { id: "calls", label: "调用" },
  sessions: { id: "sessions", label: "Sessions" },
  connection: { id: "connection", label: "连接与观测" },
};

export function agentDetailHref(agentId: string, tab?: string) {
  return `#/agents/${encodeURIComponent(agentId)}${tab ? `?tab=${tab}` : ""}`;
}

export function AgentDetailLayout({
  agent,
  description,
  tabs,
  value,
  canConfigure,
  activation,
  status,
  children,
}: {
  agent: AgentDiscovery;
  description: string;
  tabs: readonly { id: string; label: string }[];
  value: string;
  canConfigure: boolean;
  activation?: ReactNode;
  status?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-heading font-medium">{agent.name}</h2>
          <p className="mt-1 text-body text-fg-muted">{description}</p>
        </div>
        {activation}
        {value !== "connection" && (
          <Button asChild variant="neutral" size="sm">
            <a href={agentDetailHref(agent.id, "connection")}>
              {canConfigure ? "模型连接" : "连接与观测"}
            </a>
          </Button>
        )}
        <Button asChild variant="tertiary" size="sm">
          <a href="#/agents">返回 Agents</a>
        </Button>
      </div>
      {status}
      <Tabs
        value={value}
        onValueChange={(next) => {
          window.location.hash = agentDetailHref(agent.id, next);
        }}
        variant="underline"
        color="default"
      >
        <TabsList aria-label="Agent 详情">
          {tabs.map(({ id, label }) => (
            <TabItem key={id} value={id} label={label} />
          ))}
        </TabsList>
        {children}
      </Tabs>
      {(!agent.sourceId || agent.found) && (
        <>
          <PluginDetailSlots target="agent" objectId={agent.id} />
          <ObjectContributionSlots object={{ target: "agent", id: agent.id }} />
        </>
      )}
    </div>
  );
}

export function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-xl border border-border p-4">
      <dt className="text-label text-fg-muted">{label}</dt>
      <dd className="mt-2 text-heading font-medium">{value}</dd>
    </div>
  );
}
