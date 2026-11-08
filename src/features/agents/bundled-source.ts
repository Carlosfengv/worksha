import { bundledDshManifest as manifest } from "@/features/plugins/bundled-dsh";
import type { AgentDiscovery } from "@/shared/types/agent";

const product = manifest.source.products[0];
const productId = `${manifest.id}:${product.id}`;

// This listing advertises a setup route. Only the service publishes enabled
// products to the data and model connection directories.
export const bundledDshAgent: AgentDiscovery = {
  id: productId,
  configId: productId,
  family: manifest.id,
  name: product.displayName,
  pluginId: manifest.id,
  sourceId: `${manifest.id}:${manifest.source.id}`,
  capabilities: manifest.capabilities,
  readonly: true,
  found: false,
  connected: false,
  location: null,
};
