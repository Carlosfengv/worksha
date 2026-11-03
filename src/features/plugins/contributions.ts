import schema from "../../../contracts/plugins/contributions/contribution-catalog.schema.json";
import type { ContributionCatalog } from "../../../contracts/plugins/contributions/types";
import { parseSchema } from "./contract";
import type { PluginStatus } from "./types";

export function parseContributions(
  value: unknown,
  statuses: PluginStatus[],
): ContributionCatalog {
  parseSchema(schema, value);
  const data = value as ContributionCatalog;
  if (data.schemaVersion !== 1 || data.items.length > statuses.length)
    throw new Error("插件详情声明版本或数量无效。");
  const seen = new Set<string>();
  for (const item of data.items) {
    const status = statuses.find(
      (s) => s.manifest.id === item.pluginId && s.digest === item.digest,
    );
    const slots = new Set<string>();
    const deps = new Set<string>();
    if (
      !status ||
      seen.has(item.digest) ||
      item.slots.length > 16 ||
      item.dependencies.length > 16
    )
      throw new Error("插件详情声明身份无效。");
    seen.add(item.digest);
    for (const slot of item.slots) {
      if (
        !/^[a-z0-9_-]{1,64}$/.test(slot.id) ||
        slots.has(slot.id) ||
        !status.manifest.views?.some((v) => v.id === slot.viewId) ||
        !status.manifest.requires?.includes("host.detail-slots@1") ||
        !slot.title ||
        slot.title.length > 100 ||
        !slot.objectIds.length ||
        slot.objectIds.length > 32 ||
        slot.objectIds.some(
          (id) => !/^(\*|[a-zA-Z0-9_.:-]{1,160})$/.test(id),
        ) ||
        (slot.objectIds.includes("*") && slot.objectIds.length !== 1)
      )
        throw new Error("插件详情插槽声明无效。");
      slots.add(slot.id);
    }
    for (const dep of item.dependencies) {
      if (
        deps.has(dep.pluginId) ||
        dep.pluginId === item.pluginId ||
        !status.manifest.requires?.includes("host.optional-dependencies@1") ||
        !dep.views.length ||
        dep.views.some(
          (id) => !status.manifest.views?.some((v) => v.id === id),
        ) ||
        (dep.available &&
          (dep.reason != null || typeof dep.selectedVersion !== "string"))
      )
        throw new Error("插件可选依赖状态无效。");
      deps.add(dep.pluginId);
    }
  }
  return data;
}
