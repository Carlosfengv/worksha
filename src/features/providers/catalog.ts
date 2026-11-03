import builtinPresets from "../../../resources/provider-catalog.json" with { type: "json" };
import type { Provider } from "@/features/providers/types";

export function providerModelIds(provider: Provider): string[] {
  return [
    ...new Set([provider.model, ...provider.models].filter(Boolean)),
  ].sort((a, b) =>
    a === b
      ? 0
      : a === provider.model
        ? -1
        : b === provider.model
          ? 1
          : a.localeCompare(b),
  );
}

export function availableProviderModelIds(provider: Provider): string[] {
  const disabled = new Set(provider.disabledModels ?? []);
  return providerModelIds(provider).filter((id) => !disabled.has(id));
}

export interface ProviderPreset {
  name: string;
  protocol: Provider["protocol"];
  endpoints: Partial<Record<Provider["protocol"], string>>;
  documents?: string;
}

// Compiled-in contributions share the same metadata as the Rust host.
export function registerProviderPresets(
  contributions: readonly (readonly [string, ProviderPreset])[],
): Readonly<Record<string, ProviderPreset>> {
  const presets: Record<string, ProviderPreset> = Object.create(null);
  for (const [id, preset] of contributions) {
    if (
      !id ||
      presets[id] ||
      !preset.name ||
      !["messages", "responses", "chat-completions"].includes(preset.protocol)
    ) {
      throw new Error("供应商目录贡献无效或重复");
    }
    presets[id] = Object.freeze({
      ...preset,
      endpoints: Object.freeze({ ...preset.endpoints }),
    });
  }
  return Object.freeze(presets);
}
export const providerCatalog = registerProviderPresets(
  builtinPresets.map(
    ({ id, ...preset }) => [id, preset as ProviderPreset] as const,
  ),
);

export function providerVendor(provider: Provider): string {
  return (
    provider.vendor || (providerCatalog[provider.id] ? provider.id : "custom")
  );
}

export function defaultEndpoint(provider: Provider): string | undefined {
  return providerCatalog[providerVendor(provider)]?.endpoints[
    provider.protocol
  ];
}

export function usesDefaultEndpoint(provider: Provider): boolean {
  const endpoint = defaultEndpoint(provider);
  return (
    !!endpoint &&
    provider.endpoint.replace(/\/+$/, "") === endpoint.replace(/\/+$/, "")
  );
}

export function isLoopbackEndpoint(endpoint: string): boolean {
  try {
    const { hostname } = new URL(endpoint);
    return (
      hostname === "localhost" ||
      hostname === "[::1]" ||
      /^127(?:\.\d{1,3}){3}$/.test(hostname)
    );
  } catch {
    return false;
  }
}
