import catalog from "../../../resources/agent-catalog.json" with { type: "json" };
import type { ProviderStatus as SourceProviderStatus } from "../../../contracts/domain/types";

export type AgentId = string;
export type AgentProductId = string;
export type ConfigAgentId = string;
export interface AgentProduct {
  id: AgentProductId;
  name: string;
  family: AgentId;
  configId: ConfigAgentId;
  readonly?: boolean;
  sourceId?: string;
  pluginId?: string;
  capabilities?: readonly string[];
  connectionAdapter?: "agent.connection@1" | null;
}
// The compiled fallback and Rust use the same composition metadata. Connected UI
// consumes Snapshot.agents through AgentDirectoryProvider, including new products.
export const agentProducts: readonly AgentProduct[] = catalog.products;
export function configAgentName(id: string, products = agentProducts): string {
  const exact = products.find((product) => product.id === id);
  if (exact) return exact.name;
  const matches = products.filter((product) => product.configId === id);
  if (matches.length === 1) return matches[0].name;
  return matches.length > 1 &&
    matches.every((product) => product.family === matches[0].family)
    ? sourceName(matches[0].family, products)
    : id;
}
export function agentFamily(
  id: string,
  products = agentProducts,
): AgentId | undefined {
  const source = id.endsWith("-unknown") ? id.slice(0, -8) : id;
  return (
    products.find((p) => p.id === id || p.configId === id)?.family ??
    products.find((p) => p.family === source)?.family
  );
}
export function configAgent(
  id: string,
  products = agentProducts,
): ConfigAgentId {
  return products.find((p) => p.id === id)?.configId ?? id;
}
export function isSharedSource(family: string, products = agentProducts) {
  return (
    catalog.sources.find((source) => source.id === family)?.shared ??
    products.filter((product) => product.family === family).length > 1
  );
}
export function sourceName(family: string, products = agentProducts) {
  return (
    catalog.sources.find((source) => source.id === family)?.name ??
    products.find((product) => product.family === family)?.name ??
    family
  );
}
export function recordProduct(
  record: { agentId: string; agentProductId?: string | null },
  products = agentProducts,
) {
  const candidates = products.filter((p) => p.family === record.agentId);
  const shared = catalog.sources.find(
    (source) => source.id === record.agentId,
  )?.shared;
  return (
    record.agentProductId ??
    (shared !== true && candidates.length === 1 ? candidates[0].id : null)
  );
}
export function recordAgentKey(
  record: { agentId: string; agentProductId?: string | null },
  products = agentProducts,
) {
  return recordProduct(record, products) ?? `${record.agentId}-unknown`;
}
export function createAgentNames(
  products = agentProducts,
): Record<string, string> {
  const names: Record<string, string> = Object.fromEntries(
    products.map((p) => [p.id, p.name]),
  );
  names.unknown = "客户端来源未知";
  for (const family of new Set(products.map((p) => p.family))) {
    const source = catalog.sources.find((s) => s.id === family);
    const label =
      source?.name ?? products.find((p) => p.id === family)?.name ?? family;
    const shared =
      source?.shared ?? products.filter((p) => p.family === family).length > 1;
    if (shared) names[family] = `${label} · 客户端来源未知`;
    names[`${family}-unknown`] = `${label} · 客户端来源未知`;
  }
  return names;
}
export function createFamilyNames(
  products = agentProducts,
): Record<string, string> {
  return Object.fromEntries(
    [...new Set(products.map((p) => p.family))].map((family) => {
      const source = catalog.sources.find((s) => s.id === family);
      const name =
        source?.name ?? products.find((p) => p.id === family)?.name ?? family;
      return [
        family,
        isSharedSource(family, products) ? `${name} · 全部客户端` : name,
      ];
    }),
  );
}

export interface AgentDiscovery extends AgentProduct {
  found: boolean;
  location: string | null;
  connected: boolean;
  sourceState?: SourceProviderStatus;
}

export interface AgentConnection {
  agentId: ConfigAgentId;
  mode: "gateway" | "official_login" | "official_api" | "direct" | "unknown";
  providerName: string;
  model: string | null;
  endpoint: string | null;
  reason: string | null;
  activeRequests: number;
  activeByProduct?: Record<string, number>;
}

export interface AgentConnections {
  checkedAt: string;
  error: string | null;
  items: AgentConnection[];
}
