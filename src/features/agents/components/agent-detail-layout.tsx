import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { PageActions, PageHeader } from "@/components/ui/page-layout";
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
  refresh,
  children,
}: {
  agent: AgentDiscovery;
  description: string;
  tabs: readonly { id: string; label: string }[];
  value: string;
  canConfigure: boolean;
  activation?: ReactNode;
  status?: ReactNode;
  refresh: () => Promise<void>;
  children: ReactNode;
}) {
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState("");
  const refreshDetail = async () => {
    if (refreshing) return;
    setRefreshing(true);
    setRefreshError("");
    try {
      await refresh();
    } catch (reason) {
      setRefreshError(
        reason instanceof Error ? reason.message : String(reason),
      );
    } finally {
      setRefreshing(false);
    }
  };
  return (
    <div className="min-w-0 space-y-4">
      <PageHeader className="px-0">
        <div className="min-w-0 space-y-1">
          <h2 className="break-words text-title font-medium">{agent.name}</h2>
          <p className="text-label text-fg-muted">{description}</p>
        </div>
        <PageActions>
          <Button asChild variant="tertiary">
            <a href="#/agents">返回 Agents</a>
          </Button>
          <Button
            variant="tertiary"
            loading={refreshing}
            disabled={refreshing}
            onClick={() => void refreshDetail()}
          >
            刷新
          </Button>
          {value !== "connection" && (
            <Button asChild variant="neutral">
              <a href={agentDetailHref(agent.id, "connection")}>
                {canConfigure ? "模型连接" : "连接与观测"}
              </a>
            </Button>
          )}
        </PageActions>
      </PageHeader>
      {refreshError && (
        <p role="alert" className="wrap-anywhere text-label text-fg-danger">
          刷新未完成。{refreshError}
        </p>
      )}
      {activation}
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
