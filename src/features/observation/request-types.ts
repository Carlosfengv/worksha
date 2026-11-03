import type { ModelObservation } from "./types";
import type { Usage } from "@/shared/types/usage";
import type {
  ProviderCost,
  ProviderCostTotals,
} from "@/features/providers/types";
export interface RequestQuery {
  source?: string;
  start?: string;
  end?: string;
  agent?: string;
  result?: string;
  provider?: string;
  model?: string;
  session?: string;
  project?: string;
  search?: string;
  issue?: string;
  sort?: string;
  descending?: boolean;
  metric?: string;
  dimension?: string;
  currency?: string;
  snapshot?: string;
  cursor?: string;
}
export interface RequestAttempt extends ModelObservation {
  requestedModel?: string | null;
  routedModel?: string | null;
  reportedModel?: string | null;
  firstOutputKind?: string | null;
  sentMs?: number | null;
  upstreamRequestId?: string | null;
  responseId?: string | null;
  cost: ProviderCost;
  issue: string | null;
  providerKey: string;
  backoffMs: number | null;
}
export interface NativeLink {
  eventId: string;
  sessionId: string;
  responseId: string;
  attemptId: string;
  usageConflict: boolean;
}
export interface NativeRecord {
  failure?: import("@/features/sessions/types").ModelFailure | null;
  eventId: string;
  turnId: string;
  responseId: string | null;
  reportedStatus: string;
  association: string;
  gatewayRequestId: string | null;
  matchedAttemptId: string | null;
  usageConflict: boolean;
}
export interface RequestRecord {
  client?: { id: string; name: string; peer: string } | null;
  source?: string;
  native?: NativeRecord | null;
  nativeLinks?: NativeLink[];
  id: string;
  agentId: string;
  agentProductId: string | null;
  sessionId: string | null;
  sessionTitle: string | null;
  projectId: string | null;
  projectName: string | null;
  startedAt: string;
  endedAt: string | null;
  status: string;
  model: string | null;
  provider: string;
  providerKey: string;
  usage: Usage;
  usageMissing: number;
  cost: ProviderCostTotals;
  durationMs: number | null;
  ttftMs: number | null;
  throughput: number | null;
  timingNote: string | null;
  attempts: RequestAttempt[];
}
export interface Distribution {
  samples: number;
  p50: number | null;
  p95: number | null;
}
export interface RequestSummary {
  requests: number;
  attempts: number;
  statuses: Record<string, number>;
  issues: Record<string, number>;
  successRate: number | null;
  usage: Usage;
  usageMissing: number;
  provisional: number;
  cost: ProviderCostTotals;
  cacheRatio: number | null;
  cacheSamples: number;
  ttft: Distribution;
  duration: Distribution;
  throughput: Distribution;
}
export interface SourceReport {
  gateway: RequestSummary;
  native: RequestSummary;
  linked: RequestSummary;
  matchedNative: number;
  unlinkedNative: number;
  ambiguousNative: number;
  usageConflicts: number;
  nativeMissingTime: number;
  sourceAsOf: string | null;
  collectors: {
    agentId: string;
    enabled: boolean;
    status: string;
    asOf: string | null;
    error: string | null;
    skippedFiles: number;
  }[];
  scope: string;
}
export interface HeatmapDay {
  date: string;
  start: string;
  end: string;
  state: string;
  records: number;
  tokens: number | null;
  cost: ProviderCostTotals;
  usageMissing: number;
}
export interface RequestDashboard {
  missingTime?: number;
  sourceReport?: SourceReport | null;
  heatmap?: { days: HeatmapDay[]; missingTime: number; outsideWindow: number };
  query: RequestQuery;
  asOf: string;
  snapshot: string;
  summary: RequestSummary;
  buckets: {
    groups: {
      id: string;
      label: string;
      value: number | null;
      samples: number;
      missing: number;
    }[];
    start: string;
    end: string;
    value: number | null;
    p95: number | null;
    samples: number;
    missing: number;
  }[];
  groups: {
    id: string;
    label: string;
    value: number | null;
    samples: number;
    missing: number;
  }[];
  items: RequestRecord[];
  total: number;
  nextCursor: string | null;
  models: string[];
  providers: { id: string; label: string }[];
  sourceError: string | null;
  coverage: string;
  recordedStart: string | null;
  recordedEnd: string | null;
}
export interface RequestDetailResponse {
  request: RequestRecord | null;
  asOf: string;
  sourceError: string | null;
}
