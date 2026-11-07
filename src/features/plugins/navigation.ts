import type { PluginList } from "./types";

export function pluginNavigation(directory: PluginList | undefined) {
  return (directory?.contributions?.items ?? []).flatMap((item) => {
    const plugin = directory?.items.find(
      (p) => p.manifest.id === item.pluginId && p.digest === item.digest,
    );
    if (!plugin?.enabled || plugin.runtimeStatus !== "active" || item.errorCode)
      return [];
    return (item.navigation ?? [])
      .filter((nav) =>
        item.dependencies.every(
          (dep) => !dep.views.includes(nav.viewId) || dep.available,
        ),
      )
      .map((nav) => ({
        id: `extension:${item.pluginId}:${nav.viewId}`,
        pluginId: item.pluginId,
        viewId: nav.viewId,
        label: nav.title,
        icon: "presentation" as const,
        href: `#/extensions/${encodeURIComponent(item.pluginId)}/${encodeURIComponent(nav.viewId)}`,
      }));
  });
}
