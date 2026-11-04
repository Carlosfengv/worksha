import { PluginDetailSlots } from "@/features/plugins/detail-slots";
import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { AssociatedCostsView } from "@/shared/ui/associated-costs";
import type { Snapshot } from "@/app/types";
import { Button } from "@/components/ui/button";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import { TabItem, TabPanel, Tabs, TabsList } from "@/components/ui/tabs";
import { AccountQuota } from "@/features/accounts/components/account-quota";
import { AgentConnectionStatus } from "@/features/agents/components/agent-connection-status";
import { AgentModelUsage } from "@/features/agents/components/agent-model-usage";
import { ObservationConnection } from "@/features/observation/components/observation-connection";
import { AgentProviderSettings } from "@/features/providers/components/provider-config-control";
import { ModelServiceSettings } from "@/features/model-services/components/agent-service-control";
import { ProviderActivationStatus } from "@/features/providers/components/provider-activation-status";
import { sessionsApi } from "@/features/sessions/api";
import { SessionDateFilter } from "@/features/sessions/components/session-date-filter";
import { SessionList } from "@/features/sessions/components/session-list";
import { SessionPagination } from "@/features/sessions/components/session-pagination";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { SessionSourceControl } from "@/features/sessions/components/session-source";
import {
  date,
  number,
  sessionLink,
  sourceLabel,
} from "@/features/sessions/display";
import { useNativePage } from "@/features/sessions/hooks/use-native-page";
import { groupSessionsByProject } from "@/features/sessions/session-collections";
import type { NativeSession, SessionSource } from "@/features/sessions/types";
import type { AgentDiscovery } from "@/shared/types/agent";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useCallback, useEffect, useMemo, useState } from "react";
import { AgentCalls } from "./agent-calls";
import { AgentMemory } from "./agent-memory";

export function AgentDetail({
  agent,
  source,
  tab,
  refresh,
  data,
}: {
  agent: AgentDiscovery;
  source?: SessionSource;
  tab?: string;
  refresh: () => Promise<void>;
  data: Snapshot;
}) {
  const [days, setDays] = useState("all");
  const [grouped, setGrouped] = useState(false);
  const epoch = useBackendReadEpoch();
  const queryKey = `${agent.id}:${days}:${source?.enabled}`;
  const [recent, setRecent] = useState<{
    key: string;
    epoch: symbol;
    items: NativeSession[];
  } | null>(null);
  const canConfigure = [
    "claude",
    "claude-desktop",
    "codex",
    "opencode",
  ].includes(agent.configId);
  const loader = useCallback(
    (cursor?: string) =>
      sessionsApi.sessions({
        agent: agent.id,
        days: days === "all" ? undefined : Number(days),
        cursor,
      }),
    [agent.id, days],
  );
  const sessions = useNativePage(queryKey, loader, "page");
  useEffect(() => {
    if (sessions.scopeInvalid) setRecent(null);
    else if (sessions.data && sessions.cursor === undefined)
      setRecent({
        key: queryKey,
        epoch,
        items: sessions.data.items.slice(0, 5),
      });
  }, [sessions.data, sessions.cursor, sessions.scopeInvalid, queryKey, epoch]);
  const recentItems = sessions.scopeInvalid
    ? []
    : sessions.cursor === undefined
      ? (sessions.data?.items.slice(0, 5) ?? [])
      : recent?.key === queryKey && recent.epoch === epoch
        ? recent.items
        : [];
  const value =
    tab &&
    ["overview", "calls", "sessions", "memory", "quota", "connection"].includes(
      tab,
    )
      ? tab
      : "overview";
  const openSession = (session: NativeSession) => {
    window.location.hash = sessionLink(session.id, {
      agent: agent.id,
      from: agent.id,
      tab: "sessions",
    });
  };
  const groups = useMemo(
    () =>
      grouped
        ? groupSessionsByProject(sessions.data?.items ?? [])
        : new Map<string, NativeSession[]>(),
    [grouped, sessions.data?.items],
  );
  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-heading font-medium">{agent.name}</h2>
          <p className="mt-1 text-body text-fg-muted">{sourceLabel(source)}</p>
        </div>
        {canConfigure && <ProviderActivationStatus agentId={agent.configId} />}
        {value !== "connection" && (
          <Button
            variant="neutral"
            size="sm"
            onClick={() => {
              window.location.hash = `/agents/${agent.id}?tab=connection`;
            }}
          >
            {canConfigure ? "模型连接" : "连接与观测"}
          </Button>
        )}
        <Button
          variant="tertiary"
          size="sm"
          onClick={() => {
            window.location.hash = "/agents";
          }}
        >
          返回 Agents
        </Button>
      </div>
      <AgentConnectionStatus agentId={agent.id} data={data} />
      <Tabs
        value={value}
        onValueChange={(next) => {
          window.location.hash = `/agents/${agent.id}?tab=${next}`;
        }}
        variant="underline"
        color="default"
      >
        <TabsList aria-label="Agent 详情">
          <TabItem value="overview" label="概览" />
          <TabItem value="calls" label="调用" />
          <TabItem value="sessions" label="Sessions" />
          <TabItem value="memory" label="记忆" />
          <TabItem value="quota" label="用量与额度" />
          <TabItem value="connection" label="连接与观测" />
        </TabsList>
        <TabPanel value="overview" className="pt-4">
          {source?.enabled ? (
            <div className="space-y-4">
              <SessionDateFilter value={days} onChange={setDays} />
              <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
                <Stat
                  label="已读取会话"
                  value={number(
                    sessions.scopeInvalid ? undefined : sessions.data?.total,
                  )}
                />
                <Stat
                  label="已报告 Token"
                  value={number(sessions.data?.summary?.usage.totalTokens)}
                />
                <Stat
                  label="模型响应"
                  value={number(sessions.data?.summary?.modelCalls)}
                />
                <Stat
                  label="工具 / MCP / Skill"
                  value={
                    sessions.data?.summary
                      ? number(
                          sessions.data.summary.toolCalls +
                            sessions.data.summary.mcpCalls +
                            sessions.data.summary.skillCalls,
                        )
                      : "未知"
                  }
                />
              </dl>
              {Boolean(sessions.data?.summary?.usageUnknownSessions) && (
                <p className="text-label text-fg-muted">
                  {number(sessions.data?.summary?.usageUnknownSessions)}{" "}
                  个会话用量不完整
                </p>
              )}
              <AssociatedCostsView value={sessions.data?.associatedCosts} />
              <h3 className="text-title font-medium">最近会话</h3>
              <SessionList items={recentItems} onSelect={openSession} />
            </div>
          ) : (
            <ConnectionEmpty
              title="读取本机已有会话"
              description="在连接与观测中确认只读来源，查看完整对话、工具和原生用量。"
              action="连接与观测"
              onAction={() => {
                window.location.hash = `/agents/${agent.id}?tab=connection`;
              }}
            />
          )}
        </TabPanel>
        <TabPanel value="calls">
          {value === "calls" && (
            <AgentCalls agentId={agent.id} enabled={Boolean(source?.enabled)} />
          )}
        </TabPanel>
        <TabPanel value="sessions">
          <div className="space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <SessionDateFilter value={days} onChange={setDays} />
              <Button
                variant="tertiary"
                size="sm"
                active={grouped}
                onClick={() => setGrouped(!grouped)}
              >
                {grouped ? "会话列表" : "按项目分组"}
              </Button>
            </div>
            <p className="text-label text-fg-muted">
              共 {number(sessions.data?.total)} 个会话，本页{" "}
              {sessions.data?.items.length ?? 0} 个。
            </p>
            {grouped ? (
              [...groups].map(([path, items]) => (
                <section key={path} className="space-y-2">
                  <h3 className="break-all text-body font-medium">
                    {path} · 本页 {items.length} 个
                  </h3>
                  {items[0]?.projectId && !items[0].projectHidden && (
                    <Button
                      variant="tertiary"
                      size="sm"
                      onClick={() => {
                        window.location.hash = `/projects/${encodeURIComponent(items[0].projectId!)}`;
                      }}
                    >
                      查看项目全部 Agent
                    </Button>
                  )}
                  <SessionList items={items} onSelect={openSession} />
                </section>
              ))
            ) : (
              <SessionList
                items={sessions.data?.items ?? []}
                onSelect={openSession}
              />
            )}
            {!sessions.loading &&
              !sessions.scopeInvalid &&
              sessions.data?.total === 0 && (
                <ConnectionEmpty
                  compact
                  title="暂无匹配会话"
                  description={
                    source?.enabled
                      ? "可调整日期范围，或等待后台读取完成。"
                      : "先在连接与观测中读取原生会话。"
                  }
                />
              )}
            <SessionPagination
              loading={sessions.loading}
              previous={sessions.previousCursors.length > 0}
              next={Boolean(sessions.data?.nextCursor)}
              first={sessions.cursor === undefined}
              count={sessions.data?.items.length ?? 0}
              total={sessions.scopeInvalid ? undefined : sessions.data?.total}
              partial={sessions.data?.coverage === "partial"}
              onNext={() => void sessions.loadMore()}
              onPrevious={() => void sessions.loadPrevious()}
              onFirst={() => void sessions.reload()}
            />
            {sessions.scopeInvalid && !sessions.error && (
              <ConnectionEmpty
                compact
                title="会话来源暂不可读"
                description="请在连接与观测中重新核对原生读取范围。"
              />
            )}
          </div>
        </TabPanel>
        <TabPanel value="memory">
          {value === "memory" && (
            <AgentMemory
              key={agent.id}
              agentId={agent.id}
              enabled={Boolean(source?.enabled)}
            />
          )}
        </TabPanel>
        <TabPanel value="quota">
          {value === "quota" && (
            <div className="mb-6 rounded-xl border border-border p-4">
              <AgentModelUsage
                agentId={agent.id}
                nativeEnabled={Boolean(source?.enabled)}
              />
            </div>
          )}
          {!canConfigure ? (
            <ConnectionEmpty
              compact
              title="账户额度读取尚未接入"
              description={
                agent.id === "antigravity"
                  ? "请在 Antigravity 的用量页面查看套餐与剩余额度；原生 Token 不代表订阅额度或费用。"
                  : "此只读来源尚未提供账户额度；原生 Token 不代表订阅额度或费用。"
              }
            />
          ) : (
            <AccountQuota agentId={agent.configId} />
          )}
          {canConfigure && (
            <p className="mt-3 text-label text-fg-muted">
              账户额度由所有客户端共享。
            </p>
          )}
        </TabPanel>
        <TabPanel value="connection">
          {canConfigure || agent.id === "antigravity" ? (
            <div className="mb-6">
              {canConfigure ? (
                <ModelServiceSettings
                  agentId={agent.configId}
                  agentProductId={agent.id}
                  name={agent.name}
                  providers={data.providers}
                  refresh={refresh}
                />
              ) : (
                <AgentProviderSettings
                  refresh={refresh}
                  agentId={agent.configId}
                  agentProductId={agent.id}
                  providers={data.providers}
                />
              )}
            </div>
          ) : (
            <ConnectionEmpty
              compact
              title="只读来源"
              description="此来源提供历史与观测读取，尚未提供模型连接切换。"
            />
          )}
          <div className="space-y-4">
            {agent.id.endsWith("cli") &&
              ["claude", "codex"].includes(agent.family) && (
                <ObservationConnection
                  agentId={agent.family}
                  data={data}
                  onSaved={() => void refresh()}
                />
              )}
            {source && (
              <SessionSourceControl source={source} refresh={refresh} />
            )}
            <DetailList>
              <DetailListItem>
                <DetailListLabel>默认数据目录</DetailListLabel>
                <DetailListValue>{agent.location ?? "未发现"}</DetailListValue>
              </DetailListItem>
              <DetailListItem>
                <DetailListLabel>观测模式</DetailListLabel>
                <DetailListValue>
                  原生历史与活动；API 网关请求另行观测
                </DetailListValue>
              </DetailListItem>
              <DetailListItem>
                <DetailListLabel>记录版本</DetailListLabel>
                <DetailListValue>
                  {source?.versions.join("、") || "尚未读取"}
                </DetailListValue>
              </DetailListItem>
              <DetailListItem>
                <DetailListLabel>最近检查</DetailListLabel>
                <DetailListValue>{date(source?.lastScan)}</DetailListValue>
              </DetailListItem>
            </DetailList>
            {source?.capabilities.map((capability) => (
              <div key={capability.name} className="space-y-1">
                <h3 className="text-body font-medium">{capability.name}</h3>
                <p className="text-label text-fg-muted">{capability.reason}</p>
              </div>
            ))}
          </div>
        </TabPanel>
      </Tabs>
      <PluginDetailSlots target="agent" objectId={agent.id} />
      <ObjectContributionSlots object={{ target: "agent", id: agent.id }} />
      {sessions.error && (
        <div role="alert" className="space-y-2 text-body text-fg-muted">
          <p>{sessions.error}</p>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => void sessions.reload()}
          >
            重新读取会话列表
          </Button>
        </div>
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
