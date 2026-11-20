import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { PageActions, PageHeader } from "@/components/ui/page-layout";
import { TabItem, Tabs, TabsList } from "@/components/ui/tabs";
import { ExtensionSlot } from "@/features/plugins/extension-slot";
import type { AgentDiscovery } from "@/shared/types/agent";

const directoryProblems: Record<string, string> = {
  invalid_record: "记录格式无效",
  resource_limit: "超过读取限制",
  permission_denied: "需要目录授权",
  grant_revoked: "目录身份已变化",
  unavailable: "读取暂不可用",
  unmapped_record: "部分记录缺少可识别的会话编号",
  incomplete_directory: "本次扫描仅覆盖部分目录",
};

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
  showTabs = true,
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
  showTabs?: boolean;
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
          <h2 className="sr-only">{agent.name}</h2>
          <p className="text-label text-fg-muted">{description}</p>
          {agent.directoryRead && (
            <p className="text-label text-fg-muted">
              {agent.directoryRead.lastSuccess &&
                `最近读取成功 ${new Date(agent.directoryRead.lastSuccess).toLocaleString()} · ${agent.directoryRead.sessions ?? 0} 个会话`}
              {agent.directoryRead.problem &&
                ` ${directoryProblems[agent.directoryRead.problem] ?? agent.directoryRead.problem}`}
              {agent.directoryRead.problem &&
                agent.directoryRead.lastAttempt &&
                ` · 最近尝试 ${new Date(agent.directoryRead.lastAttempt).toLocaleString()}`}
              {!agent.directoryRead.problem &&
                !agent.directoryRead.lastAttempt &&
                "等待首次读取"}
            </p>
          )}
        </div>
        <PageActions>
          <ExtensionSlot
            points={["agent.detail.actions"]}
            context={{ target: "agent", objectId: agent.id }}
          />
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
        {showTabs && (
          <TabsList aria-label="Agent 详情">
            {tabs.map(({ id, label }) => (
              <TabItem key={id} value={id} label={label} />
            ))}
          </TabsList>
        )}
        {children}
      </Tabs>
      {(!agent.sourceId || agent.found) && (
        <>
          <ExtensionSlot
            points={["agent.detail.card", "agent.detail.tab"]}
            legacyDetail
            context={{ target: "agent", objectId: agent.id }}
            object={{ target: "agent", id: agent.id }}
          />
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
