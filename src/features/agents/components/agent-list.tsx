import { useCallback, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type { Snapshot } from "@/app/types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { PageActions, PageHeader } from "@/components/ui/page-layout";
import { useIcon } from "@/lib/icon-context";
import { connectionView } from "@/features/agents/connection-display";
import { activationLabel } from "@/features/providers/components/provider-activation-status";
import { ProviderConfigControl } from "@/features/providers/components/provider-config-control";
import { AgentServiceControl } from "@/features/model-services/components/agent-service-control";
import type { ModelServiceCatalog } from "@/features/model-services/types";
import type { NativeConfig, ProviderStatus } from "@/features/providers/types";
import { sourceLabel } from "@/features/sessions/display";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { providerStatusResource } from "@/features/providers/queries";
import { modelServicesResource } from "@/features/model-services/queries";
import { fullPluginDirectoryResource } from "@/features/plugins/queries";
import type { AgentDiscovery } from "@/shared/types/agent";
import type { Status as SourcePackage } from "../../../../contracts/plugins/sources/types";
import { sourceStatus } from "../source-status";
import { PluginAgentConnection } from "./plugin-agent-connection";
import { agentDetailHref } from "./agent-detail-layout";

const agentLogos: Record<string, string> = {
  claude: "/agent-logos/claude.png",
  codex: "/agent-logos/codex.png",
  opencode: "/agent-logos/opencode.png",
};
type AgentRow = {
  agent: AgentDiscovery;
  sourcePackage?: SourcePackage;
  data: Snapshot;
  status: ProviderStatus | null;
  checking: boolean;
  unavailable: boolean;
  services: ModelServiceCatalog | null;
  servicesUnavailable: boolean;
  reloadServices: () => void;
  onSelect: (
    agent: AgentDiscovery,
    config: NativeConfig,
    providerId: string,
  ) => void;
};
const columns: ColumnDef<AgentRow>[] = [
  {
    id: "agent",
    header: "Agent",
    size: 384,
    cell: function AgentCell({ row: { original: item } }) {
      const { agent, data, status } = item;
      const AgentIcon = useIcon("brain");
      const source = data.sessionSources.find(
        (s) => s.agentId === agent.family,
      );
      const connection = connectionView({
        agentId: agent.configId,
        productId: agent.id,
        connections: data.agentConnections,
      });
      const activation = status
        ? activationLabel(status, agent.configId)
        : null;
      const version =
        agent.id === "opencode-desktop"
          ? undefined
          : status?.runtimes.find((r) => r.agentId === agent.configId)?.version;
      return (
        <div className="flex min-w-64 items-center gap-3 py-2 whitespace-normal">
          {agentLogos[agent.family] ? (
            <img
              src={agentLogos[agent.family]}
              alt=""
              width={32}
              height={32}
              draggable={false}
              className="size-8 shrink-0 object-contain"
            />
          ) : (
            <span
              aria-hidden
              className="grid size-8 shrink-0 place-items-center text-fg-muted"
            >
              <AgentIcon size={20} />
            </span>
          )}
          <div className="min-w-0 space-y-1">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <Button
                asChild
                variant="link"
                size="sm"
                className="max-w-56 justify-start px-0"
              >
                <a
                  href={agentDetailHref(agent.id)}
                  aria-label={`详情 ${agent.name}`}
                  title={agent.name}
                >
                  <span className="truncate">{agent.name}</span>
                </a>
              </Button>
              {version && (
                <span className="text-label text-fg-subtle">{version}</span>
              )}
            </div>
            <p
              className="text-label text-fg-muted"
              title={
                !agent.sourceId
                  ? [connection.description, source?.error]
                      .filter(Boolean)
                      .join(" · ")
                  : undefined
              }
            >
              {agent.readonly || agent.sourceId ? (
                sourceStatus(agent, item.sourcePackage)
              ) : (
                <>
                  {connection.label} · {connection.title} ·{" "}
                  {agent.family === "opencode" ? "默认模型配置共享 · " : ""}
                  {sourceLabel(source)}
                </>
              )}
            </p>
            {activation && (
              <Badge size="sm" status="warning">
                {activation}
              </Badge>
            )}
          </div>
        </div>
      );
    },
  },
  {
    id: "provider",
    size: 264,
    header: () => <span className="block text-right">模型连接</span>,
    cell: ({ row: { original: item } }) =>
      item.agent.sourceId &&
      !item.agent.found &&
      !item.agent.connectionAdapter ? (
        <div className="flex justify-end">
          <Button asChild variant="tertiary" size="sm">
            <a href={agentDetailHref(item.agent.id, "connection")}>
              连接与观测<span className="sr-only"> {item.agent.name}</span>
            </a>
          </Button>
        </div>
      ) : (item.agent.readonly || item.agent.sourceId) &&
        !item.agent.connectionAdapter ? (
        <span className="block text-right text-label text-fg-muted">
          仅观测
        </span>
      ) : (
        <div
          className="flex justify-end"
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => event.stopPropagation()}
        >
          {item.agent.connectionAdapter ? (
            <PluginAgentConnection
              compact
              agent={item.agent}
              providers={item.data.providers}
            />
          ) : (
            <AgentServiceControl
              compact
              agentId={item.agent.configId}
              name={item.agent.name}
              providers={item.data.providers}
              status={item.status}
              catalog={item.services}
              checking={item.checking}
              unavailable={item.unavailable}
              servicesUnavailable={item.servicesUnavailable}
              onProviderSelect={(config, providerId) =>
                item.onSelect(item.agent, config, providerId)
              }
              reload={item.reloadServices}
            />
          )}
        </div>
      ),
  },
];

function AgentTable({
  data,
  agents,
  sourcePackages,
  status,
  checking,
  unavailable,
  services,
  servicesUnavailable,
  reloadServices,
  onSelect,
}: Omit<AgentRow, "agent" | "sourcePackage"> & {
  agents: AgentDiscovery[];
  sourcePackages?: SourcePackage[];
}) {
  const rows = useMemo(
    () =>
      agents.map((agent) => ({
        agent,
        sourcePackage: sourcePackages?.find(
          (item) => item.enabled && item.manifest.id === agent.pluginId,
        ),
        data,
        status,
        checking,
        unavailable,
        services,
        servicesUnavailable,
        reloadServices,
        onSelect,
      })),
    [
      agents,
      sourcePackages,
      data,
      status,
      checking,
      unavailable,
      onSelect,
      services,
      servicesUnavailable,
      reloadServices,
    ],
  );
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.agent.id,
    manualPagination: true,
    enableRowSelection: false,
    enableSorting: false,
    enableHiding: false,
  });
  return (
    <DataTable
      table={table}
      role="region"
      showPagination={false}
      aria-label="Agent 列表"
      emptyMessage="尚未发现 Agent"
      onRowActivate={(row) => {
        window.location.hash = agentDetailHref(row.original.agent.id);
      }}
    />
  );
}

export function AgentList({
  data,
  agents = data.agents,
  refresh,
}: {
  data: Snapshot;
  agents?: AgentDiscovery[];
  refresh?: () => Promise<void>;
}) {
  const status = useSharedRead(providerStatusResource);
  const services = useSharedRead(modelServicesResource);
  const sources = useSharedRead(
    fullPluginDirectoryResource,
    agents.some((agent) => agent.found && !!agent.sourceId),
  );
  const reloadProviderStatus = status.invalidate;
  const reloadServiceCatalog = services.invalidate;
  const reloadServices = useCallback(() => {
    reloadProviderStatus();
    reloadServiceCatalog();
  }, [reloadProviderStatus, reloadServiceCatalog]);
  const [active, setActive] = useState<AgentDiscovery | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState("");
  const refreshList = async () => {
    if (refreshing) return;
    setRefreshing(true);
    setRefreshError("");
    try {
      await Promise.all([
        refresh?.(),
        status.reload(),
        services.reload(),
        sources.reload(),
      ]);
    } catch (reason) {
      setRefreshError(
        reason instanceof Error ? reason.message : String(reason),
      );
    } finally {
      setRefreshing(false);
    }
  };
  const renderTable = (
    openSwitch: (config: NativeConfig, providerId: string) => void,
    busy = false,
  ) => (
    <AgentTable
      data={data}
      agents={agents}
      sourcePackages={sources.error ? undefined : sources.data?.items}
      status={status.data}
      checking={status.loading || busy}
      unavailable={!!status.error}
      services={services.data}
      servicesUnavailable={!!services.error}
      reloadServices={reloadServices}
      onSelect={(agent, config, providerId) => {
        setActive(agent);
        openSwitch(config, providerId);
      }}
    />
  );
  return (
    <div className="min-w-0 space-y-4">
      <PageHeader className="px-0">
        <h2 className="text-title font-medium">Agents</h2>
        <PageActions>
          <Button
            variant="tertiary"
            loading={refreshing}
            disabled={refreshing}
            onClick={() => void refreshList()}
          >
            重新读取配置
          </Button>
        </PageActions>
      </PageHeader>
      <p className="text-label text-fg-muted">{agents.length} 个 Agent</p>
      {(refreshError || status.error) && (
        <Alert status="warning" role="alert">
          <AlertTitle>
            {refreshError ? "列表更新未完成" : "Provider 配置读取未完成"}
          </AlertTitle>
          <AlertDescription>
            {refreshError || `${status.error}。重新读取后再切换。`}
          </AlertDescription>
        </Alert>
      )}
      {status.data ? (
        <ProviderConfigControl
          status={status.data}
          providers={data.providers}
          reload={status.invalidate}
          checking={status.loading || !!status.error}
          onlyAgent={active?.configId}
          agentProductId={active?.id ?? ""}
          renderPicker={renderTable}
        />
      ) : (
        renderTable(() => {})
      )}
    </div>
  );
}
