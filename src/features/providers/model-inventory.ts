import { providerModelIds } from "./catalog.ts";
import { modelMetadata } from "./model-metadata.ts";
import { observationModelKey } from "./model-observations.ts";
import type {
  Provider,
  ProviderModelCatalog,
  ProviderModelInfo,
  ProviderUsage,
  ProviderUsageTotals,
  ModelObservation,
} from "./types";

export type CatalogState = {
  data?: ProviderModelCatalog;
  error?: string;
  loading?: boolean;
};
export type InventoryRow = {
  key: string;
  provider: Provider;
  model: ProviderModelInfo;
  contextNote?: string;
  state: string;
  catalog: CatalogState | undefined;
  totals: ProviderUsageTotals | undefined;
  usageState: "ready" | "unknown" | "partial";
  observations: ModelObservation[];
};
export const catalogKey = (provider: Provider) =>
  JSON.stringify([
    provider.id,
    provider.endpoint,
    provider.protocol,
    provider.vendor,
    provider.credentialRef,
    provider.revision,
  ]);
export function providerCatalogSource(
  provider: Provider,
  catalog?: ProviderModelCatalog | null,
): Provider["modelCatalog"] {
  let endpoint: string;
  try {
    endpoint = new URL(provider.endpoint.trim()).toString().replace(/\/$/, "");
  } catch {
    return null;
  }
  if (catalog)
    return {
      fetchedAt: catalog.fetchedAt,
      sourceUrl: catalog.sourceUrl,
      endpoint,
      protocol: provider.protocol,
      credentialRef: provider.credentialRef,
    };
  const source = provider.modelCatalog;
  return source &&
    source.endpoint === endpoint &&
    source.protocol === provider.protocol &&
    source.credentialRef === provider.credentialRef
    ? source
    : null;
}
export const modelKey = (providerId: string, model: string) =>
  JSON.stringify([providerId, model]);

export function modelInventory(
  providers: Provider[],
  catalogs: Record<string, CatalogState>,
  usage: ProviderUsage | null,
): InventoryRow[] {
  const totals = new Map(
    usage?.providerModels?.map((row) => [
      modelKey(row.providerId, row.model ?? ""),
      row.totals,
    ]),
  );
  const usageState =
    !usage?.providerModels || usage.sourceState === "disabled"
      ? "unknown"
      : usage.sourceState === "ready"
        ? "ready"
        : "partial";
  return providers
    .filter((p) => p.configured)
    .flatMap((provider) => {
      const catalog = catalogs[catalogKey(provider)];
      const registered = new Set(providerModelIds(provider));
      const disabled = new Set(provider.disabledModels ?? []);
      const discovered = new Map(
        catalog?.data?.models.map((model) => [model.id, model]),
      );
      const ids = new Set([...registered, ...discovered.keys()]);
      return [...ids].map((id) => ({
        key: modelKey(provider.id, id),
        provider,
        ...modelMetadata(id, discovered),
        observations: (catalog?.data?.observations?.models ?? []).filter(
          (o) => observationModelKey(o.model) === observationModelKey(id),
        ),
        state: disabled.has(id)
          ? "已停用"
          : registered.has(id)
            ? "已启用"
            : "目录可选",
        catalog,
        totals:
          usageState === "unknown"
            ? undefined
            : totals.get(modelKey(provider.id, id)),
        usageState,
      }));
    });
}

export const modelLimit = (value: number | null | undefined) =>
  value == null ? "未知" : `${value.toLocaleString("zh-CN")} Token`;
export const modelSupport = (value: boolean | null | undefined) =>
  value == null ? "未知" : value ? "支持" : "不支持";
export function inventoryCalls(row: InventoryRow) {
  if (row.usageState === "unknown") return "未知";
  if (row.totals)
    return `${row.totals.calls.toLocaleString("zh-CN")}${row.usageState === "partial" ? " · 部分" : ""}`;
  return row.usageState === "ready" ? "0" : "未知";
}
