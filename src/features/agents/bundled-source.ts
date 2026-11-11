import { bundledDshManifest as manifest } from "@/features/plugins/bundled-dsh";
import type { AgentDiscovery } from "@/shared/types/agent";

const product = manifest.source.products[0];
const productId = `${manifest.id}:${product.id}`;

// The bundled model bridge is available before a session source is installed.
export const bundledDshAgent: AgentDiscovery = {
  id: productId,
  configId: productId,
  family: manifest.id,
  name: product.displayName,
  pluginId: manifest.id,
  sourceId: `${manifest.id}:${manifest.source.id}`,
  capabilities: manifest.capabilities,
  connectionAdapter: "agent.connection@1",
  readonly: true,
  found: false,
  connected: false,
  location: null,
};
