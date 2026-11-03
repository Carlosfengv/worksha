import type { CostTotals } from "@/shared/types/cost";
import type { Usage } from "@/shared/types/usage";

export interface MemoryFile {
  id: string;
  name: string;
  path: string;
  scope: string;
  project: string | null;
  sizeBytes: number;
  modifiedAt: string | null;
}
export interface MemorySnapshot {
  agentId: string;
  sharedAgents: string[];
  files: MemoryFile[];
  selected: { id: string; text: string; truncated: boolean } | null;
  asOf: string;
  limited: boolean;
  warnings: string[];
}

export interface AgentUsageQuery {
  agent: string;
  source: "native" | "gateway";
  days?: number;
  provider?: string;
  model?: string;
  cursor?: string;
}

export interface ModelUsageTotals {
  cost?: CostTotals;
  attempts: number;
  missingUsage: number;
  usage: Usage;
  reported: Record<keyof Usage, number>;
}

export interface AgentUsage {
  query: AgentUsageQuery;
  asOf: string;
  sourceAsOf: string;
  sourceState: string;
  summary: ModelUsageTotals;
  providers: {
    id: string;
    name: string | null;
    endpoint: string | null;
    routeRevision: number | null;
  }[];
  items: {
    providerId: string;
    routeRevision: number | null;
    provider: string | null;
    endpoint: string | null;
    model: string | null;
    totals: ModelUsageTotals;
    lastSeen: string | null;
    sessionId: string | null;
    requestId: string | null;
  }[];
  total: number;
  nextCursor: string | null;
  missingTime: number;
}
