import { useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { TabPanel } from "@/components/ui/tabs";
import {
  PublishedSources,
  sourceProvides,
  type SourceView,
} from "@/features/domain/published-sources";
import { domainDirectoryResource } from "@/features/domain/queries";
import { fullPluginsApi } from "@/features/plugins/api";
import { fullPluginDirectoryResource } from "@/features/plugins/queries";
import { SourceAccessControls } from "@/features/plugins/source-access-controls";
import { PluginUpload } from "@/features/plugins/upload";
import type { Provider } from "@/features/providers/types";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import type { AgentDiscovery } from "@/shared/types/agent";
import { bundledDshAgent } from "../bundled-source";
import { sourceStatus } from "../source-status";
import {
  AgentDetailLayout,
  agentDetailHref,
  agentDetailTabs,
  Stat,
} from "./agent-detail-layout";
import { PluginAgentConnection } from "./plugin-agent-connection";

const views = [
  { ...agentDetailTabs.calls, mode: "requests" },
  { ...agentDetailTabs.sessions, mode: "sessions" },
  { id: "activity", label: "活动", mode: "activity" },
  { id: "projects", label: "项目", mode: "projects" },
  { id: "mcp", label: "MCP", mode: "mcp" },
] satisfies { id: string; label: string; mode: SourceView }[];

const displayCount = (value: string | null | undefined) =>
  value == null ? "未知" : BigInt(value).toLocaleString("zh-CN");

export function SourceAgentDetail({
  agent,
  tab,
  providers,
  refresh,
}: {
  agent: AgentDiscovery;
  tab?: string;
  providers: Provider[];
  refresh: () => Promise<void>;
}) {
  const available = views.filter(({ mode }) => sourceProvides(agent, mode));
  const tabs = [
    agentDetailTabs.overview,
    ...available,
    agentDetailTabs.connection,
  ];
  const value =
    tabs.find(({ id }) => id === (tab === "requests" ? "calls" : tab))?.id ??
    "overview";
  const connectionHref = agentDetailHref(agent.id, "connection");
  const bundled = agent.pluginId === bundledDshAgent.pluginId;
  const catalog = useSharedRead(fullPluginDirectoryResource, !!agent.pluginId);
  const packages = catalog.error
    ? []
    : (catalog.data?.items ?? []).filter(
        (item) => item.manifest.id === agent.pluginId,
      );
  const current =
    packages.find((item) => item.enabled) ??
    (packages.length === 1 ? packages[0] : undefined);
  const versionChoiceRequired = !current && packages.length > 1;
  const summary =
    current?.sourceAuthorized &&
    current.freshness === "current" &&
    current.manifest.source?.products.length === 1 &&
    current.factSummary?.sourceId === agent.sourceId
      ? current.factSummary
      : null;
  const description = versionChoiceRequired
    ? "多个已安装版本待选择"
    : !agent.found && current
      ? current.runtimeActive
        ? "接入状态待更新"
        : "来源待启用"
      : sourceStatus(agent, current);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const gate = useRef(false);
  const reload = async () => {
    await Promise.all([
      catalog.invalidate(),
      domainDirectoryResource.invalidate(),
      refresh(),
    ]);
  };
  const run = async (operation: () => Promise<unknown>) => {
    if (gate.current) return;
    gate.current = true;
    setBusy(true);
    setError(null);
    try {
      await operation();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      try {
        await reload();
      } finally {
        gate.current = false;
        setBusy(false);
      }
    }
  };
  const canConfigure = !!agent.connectionAdapter;
  return (
    <AgentDetailLayout
      agent={agent}
      description={description}
      tabs={tabs}
      value={value}
      canConfigure={canConfigure}
      refresh={reload}
      status={
        <div className="flex flex-wrap gap-2" aria-label="连接方式与采集范围">
          <Badge
            size="sm"
            status={
              (current?.sourceAuthorized ?? agent.connected)
                ? "success"
                : "neutral"
            }
          >
            {description}
          </Badge>
          {agent.found && (
            <Badge size="sm" variant="plain">
              来源报告
            </Badge>
          )}
        </div>
      }
    >
      <TabPanel value="overview" className="space-y-4 pt-4">
        {summary && summary.coverage !== "unavailable" ? (
          <>
            <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
              <Stat
                label="已读取会话"
                value={displayCount(summary.sessions.records)}
              />
              <Stat
                label="已报告 Token"
                value={displayCount(summary.sessions.usage.totalTokens)}
              />
              {sourceProvides(agent, "requests") && (
                <Stat
                  label="模型响应"
                  value={displayCount(summary.responses.records)}
                />
              )}
              {sourceProvides(agent, "mcp") && (
                <Stat
                  label="MCP 执行"
                  value={displayCount(summary.mcpExecutions)}
                />
              )}
            </dl>
            <p className="text-label text-fg-muted">
              数据由 {agent.name} 报告
              {summary.coverage === "partial" ? "，当前覆盖范围不完整" : ""}。
            </p>
          </>
        ) : (
          <Alert>
            <AlertTitle>
              {catalog.error ? "来源状态暂不可读取" : description}
            </AlertTitle>
            <AlertDescription>
              {catalog.error ??
                ((current?.sourceAuthorized ?? agent.connected)
                  ? "会话摘要尚未就绪。可在连接与观测核对来源或读取数据。"
                  : "在连接与观测完成接入和授权后，可查看会话与调用。")}
            </AlertDescription>
          </Alert>
        )}
      </TabPanel>
      {available.map(({ id, mode }) => (
        <TabPanel key={id} value={id} className="pt-4">
          {value === id &&
            (agent.found ? (
              <PublishedSources
                mode={mode}
                sourceId={agent.sourceId}
                productId={agent.id}
                manageHref={connectionHref}
              />
            ) : (
              <Alert>
                <AlertTitle>来源尚未接入</AlertTitle>
                <AlertDescription>
                  <a className="underline" href={connectionHref}>
                    前往连接与观测
                  </a>
                </AlertDescription>
              </Alert>
            ))}
        </TabPanel>
      ))}
      <TabPanel value="connection" className="space-y-4 pt-4">
        {value === "connection" && (
          <>
            {canConfigure && (
              <PluginAgentConnection agent={agent} providers={providers} />
            )}
            <section className="space-y-3" aria-label="会话来源">
              <h3 className="text-title font-medium">会话来源</h3>
              {bundled && (
                <>
                  <p className="text-label text-fg-muted">
                    在 DeepSeek Harness
                    启用随附的只读桥接，再选择其导出的会话文件。模型连接使用独立桥接和授权。
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <PluginUpload
                      hideFileUpload
                      offerBundledDsh={
                        !!catalog.data &&
                        !catalog.error &&
                        !packages.length &&
                        !agent.found
                      }
                      showDshBridge
                      disabled={busy}
                      onUploaded={reload}
                    />
                  </div>
                </>
              )}
              {(error || catalog.error) && (
                <p role="alert" className="text-label text-fg-muted">
                  {error ?? catalog.error}
                </p>
              )}
              {catalog.loading && !catalog.data && (
                <p role="status" className="text-label text-fg-muted">
                  正在读取来源配置…
                </p>
              )}
              {versionChoiceRequired && (
                <Alert>
                  <AlertTitle>选择要启用的版本</AlertTitle>
                  <AlertDescription>
                    此来源已安装多个版本。请在插件中心选择要启用的版本。
                  </AlertDescription>
                </Alert>
              )}
              {current && (
                <>
                  {current.runtimeActive ? (
                    <p className="break-words text-label text-fg-muted">
                      来源文件：{current.sourceLabel ?? "未授权"}
                    </p>
                  ) : (
                    <Button
                      variant="tertiary"
                      disabled={busy || !current.publisherTrusted}
                      onClick={() =>
                        void run(() =>
                          fullPluginsApi.change({
                            package: {
                              pluginId: current.manifest.id,
                              digest: current.digest,
                            },
                            action: "enable",
                          }),
                        )
                      }
                    >
                      启用会话来源
                    </Button>
                  )}
                  <SourceAccessControls
                    status={current}
                    busy={busy}
                    run={(operation) => void run(operation)}
                    onConsentChanged={() => void reload()}
                    readLabel="读取会话数据"
                  />
                </>
              )}
              <Button asChild variant="tertiary" size="sm">
                <a href="#/plugins?scene=agents">管理来源插件</a>
              </Button>
            </section>
          </>
        )}
      </TabPanel>
    </AgentDetailLayout>
  );
}
