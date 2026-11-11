import type { Provider } from "@/features/providers/types";
import type { AgentId } from "@/shared/types/agent";
import type { Usage } from "@/shared/types/usage";
import type { SessionContext } from "@/features/sessions/types";

export interface SessionContextSample {
  sessionId: string;
  context: Pick<
    SessionContext,
    | "inputTokens"
    | "windowTokens"
    | "usedPercentage"
    | "sampleAt"
    | "model"
    | "source"
    | "sampleAgentProductId"
  > | null;
  state: "ready" | "partial" | "pending" | "unavailable" | "unknown";
  checkedAt: string | null;
}

export interface ObservationBinding {
  agentId: AgentId;
  provider: Provider;
  revision: number;
  enabled: boolean;
  launcherPath: string;
  activeRequests: number;
}

export interface ModelObservation {
  lastRateSample?: RateSample | null;
  routing?: {
    bindingId: string;
    routeVersionId: string;
    providerRevision: number;
    boundary: string;
    service?: {
      serviceId: string;
      serviceVersionId: string;
      serviceName: string;
      modelId: string;
      modelName: string;
      requestedAlias: string;
      preferredModelId?: string | null;
      preferredModelName?: string | null;
      attempt?: number;
      fallbackReason?: string | null;
      upstreamAccountLabel?: string | null;
      accountSelectionReason?: string | null;
      routingRule?: { id: string; name: string; modelId: string } | null;
      routingIntent?: "unknown" | "unavailable" | { matched: string } | null;
      nestedRoute?: {
        serviceId: string;
        versionId: string;
        serviceName: string;
        modelId: string;
        modelName: string;
        routingRule?: { id: string; name: string; modelId: string };
        routingIntent?: "unknown" | "unavailable" | { matched: string };
      }[];
      budget?: {
        inputTokens: number;
        outputTokens: number;
        contextWindow: number;
        method: string;
        estimated: boolean;
      } | null;
      mode: string;
      capabilities: import("@/features/model-services/types").ModelCapabilities;
    } | null;
  } | null;
  activityMessage?: { id: string; startedAt: string } | null;
  client?: { id: string; name: string; peer: string } | null;
  reportedCost?: { amount: number; currency: string; source: string } | null;
  providerId?: string | null;
  agentProductId?: string | null;
  clientProtocol?: string;
  id: string;
  logicalId: string | null;
  agentId: AgentId;
  sessionId: string | null;
  model: string | null;
  provider: string;
  endpoint: string;
  bindingRevision: number;
  startedAt: string;
  endedAt: string | null;
  status: string;
  httpStatus: number | null;
  ttftMs: number | null;
  firstTextMs: number | null;
  lastTextMs?: number | null;
  estimatedTextTokens?: number | null;
  durationMs: number | null;
  usage: Usage;
  protocol: string;
  timingIssue: string | null;
}

export interface ActivityInput {
  text: string;
  at: string;
}

export interface RateSample {
  at: number;
  windowMs: number;
  tokensPerSecond: number;
}
export interface MessageUsage {
  outputTokens: number;
  reasoningTokens: number | null;
  at: string;
}
export interface ActivitySampling {
  at: number;
  windowMs: number;
  tokensPerSecond: number | null;
  recent: RateSample | null;
  reported: MessageUsage | null;
}

export interface NativeActivity {
  messageUsage?: MessageUsage | null;
  realtime?: {
    sampling?: ActivitySampling | null;
    state: string;
    firstTokenMs?: number | null;
    firstTokenMissed?: boolean;
    firstTextMs: number | null;
    firstOutputMs: number | null;
    textTokensPerSecond: number | null;
    outputTokensPerSecond?: number | null;
    averageTextTokensPerSecond: number | null;
  } | null;
  messageId?: string | null;
  messageStartedAt?: string | null;
  userInput?: ActivityInput | null;
  toolNames?: string[];
  agentAttribution?: string | null;
  agentProductId?: string | null;
  id: string;
  agentId: AgentId;
  sessionId: string;
  title: string;
  project: string | null;
  model: string | null;
  phase: string;
  step: string;
  eventAt: string;
  turnTtftMs: number | null;
  stale: boolean;
}

export interface GatewayActivity {
  sampling?: ActivitySampling | null;
  userInput?: ActivityInput | null;
  toolNames?: string[];
  request: ModelObservation;
  phase: string;
  textTokensPerSecond: number | null;
  outputTokensPerSecond?: number | null;
  averageTextTokensPerSecond?: number | null;
  throughput: number | null;
  elapsedMs: number;
  stale: boolean;
}

export interface OverviewQuery {
  agent?: string | null;
  range: number;
  metric: "calls" | "tokens" | "cost";
  dimension: "agent" | "model";
  source: "native" | "gateway";
}

export interface AnalysisValue {
  id: string;
  value: number | null;
  missing: number;
  samples: number;
}

export interface CallAnalysis {
  cost?: {
    actualUsd: number | null;
    estimatedUsd: number | null;
    unknownAttempts: number;
  };
  asOf: string;
  windowStart: string;
  query: OverviewQuery;
  total: number | null;
  samples: number;
  reported: number;
  missing: number;
  missingTime: number;
  provisional: number;
  agents: AnalysisValue[];
  models: AnalysisValue[];
  buckets: {
    start: string;
    end: string;
    groups: AnalysisValue[];
    value: number | null;
    missing: number;
  }[];
  coverage: "partial";
  unit: string;
}

export interface Overview {
  sessionContexts?: SessionContextSample[];
  asOf: string;
  analysis: CallAnalysis;
  nativeActivities: NativeActivity[];
  gatewayActivities: GatewayActivity[];
  timings: {
    success: number;
    ttftSamples: number;
    ttftP50Ms: number | null;
    ttftP95Ms: number | null;
    throughputSamples: number;
    throughputP50: number | null;
    retryGroupsUnmeasurable: number;
  };
  collector: {
    checkedAt: string | null;
    error: string | null;
    persistenceError: string | null;
  };
  bindings: ObservationBinding[];
}

export interface RequestPage {
  items: ModelObservation[];
  total: number;
  nextOffset: number | null;
  asOf: string;
}
