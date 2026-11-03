import type { NativeCall, SessionSource } from "@/features/sessions/types";
import type { AgentId } from "@/shared/types/agent";
import type { Usage } from "@/shared/types/usage";

export interface McpQuery {
  agent?: string;
  days?: number;
  service?: string;
  model?: string;
  project?: string;
  call?: string;
  cursor?: string;
}

export interface McpService {
  id: string;
  name: string;
  agentId: AgentId;
  applicableAgentIds: string[];
  projectPath: string | null;
  sources: { path: string; scope: string; modifiedAt: string | null }[];
  transport: string | null;
  configuredEnabled: boolean | null;
  configurationStatus: "present" | "unknown";
  connectionStatus: "unknown";
  observationStatus: "native_history" | "not_enabled" | "unknown";
  callCount: number | null;
  returnedCount: number;
  errorCount: number;
  unknownCount: number;
  durationSamples: number;
  p95Ms: number | null;
  errorRate: number | null;
  recordedToolCount: number;
  toolCatalogCount: number | null;
  sessionCount: number;
  lastCallAt: string | null;
}

export interface McpSnapshot {
  services: McpService[];
  groups: McpGroup[];
  usageSummary: McpUsage;
  facets: {
    models: { id: string; label: string }[];
    projects: { id: string; label: string }[];
  };
  calls: {
    serviceId: string;
    groupId: string;
    relatedUsage: Usage | null;
    relatedAgentProductId: string | null;
    relatedUsageState: "recorded" | "unknown" | "outside_agent_scope";
    call: NativeCall;
    projectPath: string | null;
    association: "current_configuration" | "unknown";
  }[];
  totalCalls: number;
  nextCursor: string | null;
  selectedServiceId: string | null;
  selectedGroupId: string | null;
  sources: SessionSource[];
  asOf: string;
  historyAsOf: string;
  coverage: "partial";
  warnings: string[];
  appliedFilter: McpQuery;
}

export interface McpUsage {
  relatedTokens: number | null;
  relatedResponses: number;
  tokenCoveredCalls: number;
  tokenMissingCalls: number;
}

export interface McpGroup extends McpUsage {
  id: string;
  name: string;
  identity: "server_name" | "unattributed";
  serviceIds: string[];
  agentIds: string[];
  configuredAgentIds: string[];
  projectPaths: string[];
  configurationCount: number;
  callCount: number | null;
  lastCallAt: string | null;
  returnedCount: number;
  errorCount: number;
  unknownCount: number;
  errorRate: number | null;
  durationSamples: number;
  p95Ms: number | null;
  sessionCount: number;
  relatedTokens: number | null;
  relatedResponses: number;
  tokenCoveredCalls: number;
  tokenMissingCalls: number;
}
