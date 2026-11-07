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
    const navigationIds = new Set<string>();
    const navigationViews = new Set<string>();
    if ((item.navigation?.length ?? 0) > 8)
      throw new Error("插件导航数量超出范围。");
    for (const nav of item.navigation ?? []) {
      if (
        !status.manifest.requires?.includes("host.ui-navigation@1") ||
        !/^[a-z0-9_-]{1,64}$/.test(nav.id) ||
        navigationIds.has(nav.id) ||
        !nav.title ||
        nav.title.length > 100 ||
        /[\u0000-\u001f\u007f]/.test(nav.title) ||
        navigationViews.has(nav.viewId) ||
        !status.manifest.views?.some((view) => view.id === nav.viewId)
      )
        throw new Error("插件导航声明无效。");
      navigationIds.add(nav.id);
      navigationViews.add(nav.viewId);
    }
    if ((item.dataQueries?.length ?? 0) > 8)
      throw new Error("插件数据绑定数量超出范围。");
    const queryViews = new Set<string>();
    for (const query of item.dataQueries ?? []) {
      if (
        !status.manifest.requires?.includes("host.domain-services@1") ||
        queryViews.has(query.viewId) ||
        query.capability !== "domain.session-source@1" ||
        !/^[a-zA-Z0-9_.:-]{1,160}$/.test(query.sourceId) ||
        !status.manifest.views?.some(
          (view) => view.id === query.viewId && view.kind === "report",
        )
      )
        throw new Error("插件数据绑定无效。");
      queryViews.add(query.viewId);
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
