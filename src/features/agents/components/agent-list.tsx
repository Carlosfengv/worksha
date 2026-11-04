import { useCallback, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import type { Snapshot } from "@/app/types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
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
import type { AgentDiscovery } from "@/shared/types/agent";

const agentLogos: Record<string, string> = {
  claude: "/agent-logos/claude.png",
  codex: "/agent-logos/codex.png",
  opencode: "/agent-logos/opencode.png",
  antigravity: "/agent-logos/antigravity.png",
};
type AgentRow = {
  agent: AgentDiscovery;
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
    size: 640,
    cell: ({ row: { original: item } }) => {
      const { agent, data, status } = item;
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
      const version = status?.runtimes.find(
        (r) => r.agentId === agent.configId,
      )?.version;
      return (
        <div className="flex min-w-64 items-center gap-3 py-2 whitespace-normal">
          {agentLogos[agent.family] && (
            <img
              src={agentLogos[agent.family]}
              alt=""
              width={32}
              height={32}
              draggable={false}
              className="size-8 shrink-0 object-contain"
            />
          )}
          <div className="min-w-0 space-y-1">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <h2 className="text-body font-medium">{agent.name}</h2>
              {version && (
                <span className="text-label text-fg-subtle">{version}</span>
              )}
            </div>
            <p
              className="text-label text-fg-muted"
              title={[connection.description, source?.error]
                .filter(Boolean)
                .join(" · ")}
            >
              {connection.label} · {connection.title} ·{" "}
              <span>{sourceLabel(source)}</span>
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
    cell: ({ row: { original: item } }) => (
      <div
        className="flex justify-end"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => event.stopPropagation()}
      >
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
      </div>
    ),
  },
  {
    id: "actions",
    size: 72,
    header: () => <span className="sr-only">操作</span>,
    cell: ({
      row: {
        original: { agent },
      },
    }) => (
      <Button variant="tertiary" size="sm" asChild>
        <a href={`#/agents/${encodeURIComponent(agent.id)}`}>
          详情<span className="sr-only"> {agent.name}</span>
        </a>
      </Button>
    ),
  },
];

function AgentTable({
  data,
  status,
  checking,
  unavailable,
  services,
  servicesUnavailable,
  reloadServices,
  onSelect,
}: Omit<AgentRow, "agent">) {
  const rows = useMemo(
    () =>
      data.agents.map((agent) => ({
        agent,
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
      showPagination={false}
      aria-label="Agent 列表"
      emptyMessage="尚未发现 Agent"
      onRowActivate={(row) => {
        window.location.hash = `/agents/${encodeURIComponent(row.original.agent.id)}`;
      }}
    />
  );
}

export function AgentList({ data }: { data: Snapshot }) {
  const status = useSharedRead(providerStatusResource);
  const services = useSharedRead(modelServicesResource);
  const reloadProviderStatus = status.invalidate;
  const reloadServiceCatalog = services.invalidate;
  const reloadServices = useCallback(() => {
    reloadProviderStatus();
    reloadServiceCatalog();
  }, [reloadProviderStatus, reloadServiceCatalog]);
  const [active, setActive] = useState<AgentDiscovery | null>(null);
  const renderTable = (
    openSwitch: (config: NativeConfig, providerId: string) => void,
    busy = false,
  ) => (
    <AgentTable
      data={data}
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
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-body text-fg-muted">
          {data.agents.length} 个 Agent · 选择 Provider 或模型服务
        </p>
        <Button
          variant="tertiary"
          size="sm"
          loading={status.loading}
          onClick={status.reload}
        >
          重新读取配置
        </Button>
      </div>
      {status.error && (
        <Alert status="warning" role="alert">
          <AlertTitle>Provider 配置读取未完成</AlertTitle>
          <AlertDescription>
            {status.error}。重新读取后再切换。
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
