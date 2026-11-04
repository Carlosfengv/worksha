export interface RoutingPolicy {
  enabled: boolean;
  order: string[];
  disabledModels: string[];
  fallbackOnFailure: boolean;
  fallbackOnRateLimit: boolean;
  maxAttempts: number;
  deadlineSeconds: number;
  cooldownSeconds: number;
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
import type { Provider } from "@/features/providers/types";
export interface ModelCapabilities {
  contextWindow: number;
  maxOutputTokens: number;
  tools: boolean;
  thinking: boolean;
  inputModalities: string[];
}
export interface ServiceModel {
  id: string;
  name: string;
  provider: Provider;
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
export interface ModelServiceCatalog {
  connectionAgents?: string[];
  writeProtocol?: { version: number; storeId: string; receiptLimit: number };
  routingHealth?: {
    serviceId: string;
    modelId: string;
    status: {
      state: "ready" | "cooling" | "unknown";
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
    providerId: m.provider.id,
    model: m.provider.model,
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
