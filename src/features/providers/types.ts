import type { ConfigAgentId } from "@/shared/types/agent";
import type { Usage } from "@/shared/types/usage";

export interface ProviderUsageQuery {
  source: "gateway" | "native";
  agent?: string;
  days?: number;
  providerId?: string;
  model?: string;
  cursor?: string;
}
export interface ProviderCost {
  kind: "actual" | "estimated" | "unknown";
  amount: number | null;
  currency: string | null;
  reason: string | null;
  source: string | null;
  price: {
    version: string;
    checkedAt: string;
    validFrom: string;
    validUntil: string;
    model: string;
    period: string;
    inputPerMillion: number;
    cachedInputPerMillion: number;
    outputPerMillion: number;
  } | null;
  components: {
    kind: string;
    tokens: number;
    perMillion: number;
    amount: number;
  }[];
}
export type { CostTotals as ProviderCostTotals } from "@/shared/types/cost";
import type { CostTotals as ProviderCostTotals } from "@/shared/types/cost";
export interface ProviderUsageTotals {
  cost: ProviderCostTotals;
  unknownCostReasons?: Record<string, number>;
  calls: number;
  errors: number;
  inProgress: number;
  unknownOutcomes: number;
  usage: Usage;
  reported: Partial<Record<keyof Usage, number>>;
  lastSeen: string | null;
  latencyP95Ms: number | null;
  latencySamples: number;
  ttftP95Ms: number | null;
  ttftSamples: number;
}
export interface ProviderUsage {
  query: ProviderUsageQuery;
  asOf: string;
  sourceAsOf: string;
  sourceState: string;
  summary: ProviderUsageTotals;
  providers: { providerId: string; totals: ProviderUsageTotals }[];
  unattributed: ProviderUsageTotals;
  selected: ProviderUsageTotals | null;
  modelOptions: (string | null)[];
  models: { model: string | null; totals: ProviderUsageTotals }[];
  providerModels?: {
    providerId: string;
    model: string | null;
    totals: ProviderUsageTotals;
  }[];
  calls: {
    id: string;
    providerId: string | null;
    provider: string | null;
    attribution: string;
    agentId: string;
    agentProductId: string | null;
    model: string | null;
    time: string | null;
    status: string;
    usage: Usage;
    cost: ProviderCost;
    durationMs: number | null;
    ttftMs: number | null;
    sessionId: string | null;
    requestId: string | null;
  }[];
  totalCalls: number;
  nextCursor: string | null;
  missingTime: number;
}

export interface Provider {
  id: string;
  name: string;
  vendor?: string;
  endpoint: string;
  protocol: "responses" | "messages" | "chat-completions";
  model: string;
  configured: boolean;
  credentialRef: string | null;
  credentialLabel: string | null;
  models: string[];
  disabledModels?: string[];
  modelCatalog?: {
    fetchedAt: string;
    sourceUrl: string;
    endpoint: string;
    protocol: string;
    credentialRef: string | null;
  } | null;
  revision: number;
  category: "cloud" | "custom" | "local";
}

export interface ModelDocumentation {
  fields: ("tools" | "structuredOutput" | "jsonOutput" | "contextWindow")[];
  label: string;
  url: string;
  checkedAt: string;
  expiresAt: string;
}
export interface ProviderModelInfo {
  id: string;
  name: string;
  contextWindow?: number | null;
  maxInputTokens?: number | null;
  maxOutputTokens?: number | null;
  capabilities?: {
    tools: boolean | null;
    vision: boolean | null;
    reasoning: boolean | null;
    structuredOutput: boolean | null;
    jsonOutput?: boolean | null;
  };
  documentation?: ModelDocumentation[];
  contextAlias?: {
    contextWindow: number;
    upstreamContextWindow: number;
    documentation: ModelDocumentation[];
  };
}
export interface ProviderModelCatalog {
  models: ProviderModelInfo[];
  fetchedAt: string;
  sourceUrl: string;
  truncated: boolean;
  observations?: {
    state: "ready" | "unavailable";
    models: ModelObservation[];
  };
}

export type ModelObservationField =
  | "tools"
  | "jsonOutput"
  | "jsonReliability"
  | "contextMarker"
  | "contextCapacity"
  | "maxOutputTokens"
  | "structuredOutput"
  | "vision"
  | "reasoning";
export interface ModelObservation {
  model: string;
  client: string;
  clientVersion: string;
  checkedAt: string;
  checks: {
    field: ModelObservationField;
    status: "passed" | "failed" | "unknown";
    detail: string;
  }[];
}

export interface NativeConfig {
  agentId: ConfigAgentId;
  path: string;
  state: string;
  revision: string | null;
  mode: string;
  endpoint: string | null;
  model: string | null;
  providerName: string;
  credential: string;
  error: string | null;
  warnings: string[];
}

export interface ProviderTransaction {
  localTokenOperation?: import("./local-token-operation").LocalTokenIdentity;
  id: string;
  agentId: ConfigAgentId;
  state: string;
  createdAt: string;
  appliedAt: string | null;
  before: NativeConfig;
  targetName: string;
  provider: Provider | null;
  model?: string | null;
  backupPath: string;
  restoreOf: string | null;
  error: string | null;
  activation?: {
    state: "deferred" | "awaiting_restart" | "launch_requested";
    updatedAt: string;
  } | null;
  routing?: {
    id: string;
    clientProtocol: string;
    upstreamProtocol: string;
    localEndpoint: string;
    models?: string[];
  };
}

export interface ProviderVerification {
  providerId: string;
  model?: string | null;
  fingerprint: string;
  state: string;
  checkedAt: string;
  message: string;
}

export interface ProviderStatus {
  localTokenProtocol?: import("./local-token-operation").LocalTokenProtocol;
  dynamicBindings?: DynamicBinding[];
  dynamicChanges?: DynamicChange[];
  dynamicLeases?: {
    bindingId: string;
    routeVersionId: string;
    turns: number;
  }[];
  configs: NativeConfig[];
  transactions: ProviderTransaction[];
  backupProtection: string;
  verifications: ProviderVerification[];
  runtimes: {
    agentId: ConfigAgentId;
    version: string | null;
    supported: boolean;
    processes: number | null;
    reason: string;
  }[];
}

export interface DynamicVersion {
  id: string;
  bindingId: string;
  provider: Provider;
  createdAt: string;
}
export interface DynamicBinding {
  id: string;
  agentId: string;
  scope: string;
  localEndpoint: string;
  lastObserved?: {
    providerName: string;
    model: string | null;
    status: string;
    routeVersionId: string;
    startedAt: string;
  };
  credentialRef: string;
  generation: string;
  target: DynamicVersion;
  createdAt: string;
}
export interface DynamicChange {
  id: string;
  bindingId: string;
  expectedGeneration: string;
  state: "pending" | "applied" | "cancelled";
  before: DynamicVersion;
  target: Provider;
  published: DynamicVersion | null;
  createdAt: string;
}
export function dynamicBindingFor(
  status: ProviderStatus,
  config: NativeConfig,
) {
  return status.dynamicBindings?.find(
    (binding) =>
      binding.agentId === config.agentId &&
      binding.scope === config.path &&
      config.endpoint === binding.localEndpoint,
  );
}

export interface ProviderImportCandidate {
  id: string;
  agentId: "claude" | "codex";
  sourcePath: string;
  sourceLabel: string;
  revision: string;
  name: string;
  vendor: string;
  endpoint: string | null;
  protocol: string;
  model: string | null;
  active: boolean;
  credentialSource: string;
  canImport: boolean;
  reason: string | null;
  existingProviderId: string | null;
}
