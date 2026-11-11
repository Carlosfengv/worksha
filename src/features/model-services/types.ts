import type {
  UpstreamRecovery,
  UpstreamRecoveryState,
} from "@/features/providers/types";

export const accountStrategies = [
  "priority",
  "round-robin",
  "weighted",
  "quota-reset",
  "quota-remaining",
  "quota-pace",
] as const;

export interface AccountPool {
  modelId: string;
  accounts: UpstreamAccountRef[];
  strategy?: (typeof accountStrategies)[number];
  affinity?: "off" | "prefer" | "strict";
  weights?: Record<string, number>;
}

export const routingAgents = [
  "claude",
  "opencode",
  "opencode-cli",
  "opencode-desktop",
  "codex",
] as const;

export interface RouteRule {
  id: string;
  name: string;
  modelId: string;
  agent?: (typeof routingAgents)[number];
  projectId?: string;
  tools?: boolean;
  stream?: boolean;
  minInputTokens?: number;
  intent?: string;
}

export interface IntentClassifier {
  modelId: string;
  account?: UpstreamAccountRef;
}

export interface RoutingPolicy {
  enabled: boolean;
  order: string[];
  disabledModels: string[];
  fallbackOnFailure: boolean;
  fallbackOnRateLimit: boolean;
  fallbackOnAccessDenied?: boolean;
  maxAttempts: number;
  deadlineSeconds: number;
  cooldownSeconds: number;
  accountPools?: AccountPool[];
  fallbackAcrossModels?: boolean;
  rules?: RouteRule[];
  intentClassifier?: IntentClassifier;
  limits: {
    modelId: string;
    maxConcurrent: number;
    requestsPerMinute: number;
  }[];
}
export const defaultRoutingPolicy = (): RoutingPolicy => ({
  enabled: false,
  order: [],
  disabledModels: [],
  fallbackOnFailure: true,
  fallbackOnRateLimit: true,
  maxAttempts: 3,
  deadlineSeconds: 120,
  cooldownSeconds: 30,
  limits: [],
});
import type { Provider, UpstreamAccountRef } from "@/features/providers/types";
export interface ModelCapabilities {
  contextWindow: number;
  maxOutputTokens: number;
  tools: boolean;
  thinking: boolean;
  inputModalities: string[];
}
export interface ChildServiceRef {
  serviceId: string;
  versionId: string;
}
export interface ServiceModel {
  id: string;
  name: string;
  provider: Provider;
  child?: ChildServiceRef;
  capabilities: ModelCapabilities;
}
export interface ModelService {
  id: string;
  versionId: string;
  revision: number;
  name: string;
  defaultModelId: string;
  models: ServiceModel[];
  publishedAt: string;
  routingPolicy?: RoutingPolicy;
}
export interface ModelServiceDraft {
  routingPolicy?: RoutingPolicy;
  id: string;
  revision: number;
  name: string;
  defaultModelId: string;
  models: {
    id: string;
    name: string;
    providerId: string;
    model: string;
    child?: ChildServiceRef;
    capabilities: ModelCapabilities;
  }[];
}
export interface ServiceAttachment {
  bindingId: string;
  serviceId: string;
  installed: ModelService;
  agentId: string;
  localEndpoint: string;
  attached: boolean;
  referenced: boolean;
  configurationUnknown: boolean;
  directorySyncRequired: boolean;
}
export type ServicePreviewEntry =
  | { kind: "default" }
  | { kind: "specified"; modelId: string }
  | { kind: "rule"; ruleId: string };

export interface ServicePreviewRequest {
  storeId: string;
  draft: ModelServiceDraft;
  entry: ServicePreviewEntry;
}

export interface ServicePreview {
  storeId: string;
  checkedAt: string;
  entryState: string;
  models: PreviewModel[];
}

export interface PreviewModel {
  id: string;
  name: string;
  scope: string;
  source: string | null;
  model: string | null;
  state: string;
  accounts: PreviewAccount[];
  child: {
    name: string;
    versionId: string;
    revision: number;
    models: PreviewModel[];
  } | null;
}

export interface PreviewAccount {
  label: string | null;
  position: number | null;
  state: string;
  verification:
    | import("@/features/providers/types").UpstreamModelVerification["state"]
    | "unverified";
  verifiedAt: string | null;
  verifyOnPublish: boolean;
  cooldownKnown: boolean;
  cooldown: { untilMs: number; reason: string } | null;
  recovery?: Omit<UpstreamRecovery, "model"> | null;
  quota: string | null;
}
export interface ModelServiceCatalog {
  connectionAgents?: string[];
  writeProtocol?: { version: number; storeId: string; receiptLimit: number };
  routingHealth?: {
    serviceId: string;
    modelId: string;
    status: {
      state: "ready" | "cooling" | "unknown" | UpstreamRecoveryState;
      untilMs: number | null;
      reason?: string;
    };
  }[];
  services: ModelService[];
  attachments: ServiceAttachment[];
}
export const defaultCapabilities = (): ModelCapabilities => ({
  contextWindow: 32000,
  maxOutputTokens: 8192,
  tools: true,
  thinking: false,
  inputModalities: ["text"],
});
export const serviceDraft = (service: ModelService): ModelServiceDraft => ({
  id: service.id,
  revision: service.revision,
  name: service.name,
  defaultModelId: service.defaultModelId,
  routingPolicy: service.routingPolicy ?? defaultRoutingPolicy(),
  models: service.models.map((m) => ({
    id: m.id,
    name: m.name,
    providerId: m.child ? "" : m.provider.id,
    model: m.child ? "" : m.provider.model,
    ...(m.child ? { child: { ...m.child } } : {}),
    capabilities: { ...m.capabilities },
  })),
});
export const commonCapabilities = (
  service: ModelService,
): ModelCapabilities => ({
  contextWindow: Math.min(
    ...service.models.map((m) => m.capabilities.contextWindow),
  ),
  maxOutputTokens: Math.min(
    ...service.models.map((m) => m.capabilities.maxOutputTokens),
  ),
  tools: service.models.every((m) => m.capabilities.tools),
  thinking: false,
  inputModalities: ["text"],
});
