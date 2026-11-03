import type { Manifest } from "./types";

export function isExtension(manifest: Manifest) {
  return manifest.capabilities.some((capability) =>
    [
      "gateway.protocol@1",
      "config.plan-provider@1",
      "integration.connector@1",
    ].includes(capability),
  );
}
