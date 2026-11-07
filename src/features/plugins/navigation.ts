import type { PluginList } from "./types";

export const pluginViewHref = (pluginId: string, viewId: string) =>
  `#/extensions/${encodeURIComponent(pluginId)}/${encodeURIComponent(viewId)}`;

const contributionFeatures = [
  "host.ui-navigation@1",
  "host.domain-services@1",
  "host.optional-dependencies@1",
  "host.detail-slots@1",
];

export function resolvePluginView(
  directory: PluginList | undefined,
  pluginId: string,
  viewId: string,
) {
  const status = directory?.items.find(
    (item) => item.manifest.id === pluginId && item.enabled,
  );
  if (!status) {
    const installed = directory?.items.some(
      (item) => item.manifest.id === pluginId,
    );
    return { reason: installed ? "disabled" : "uninstalled" } as const;
  }
  if (status.runtimeStatus !== "active")
    return { reason: "unavailable" } as const;
  const view = status.manifest.views?.find((item) => item.id === viewId);
  if (!view) return { reason: "undeclared" } as const;
  const contribution = directory?.contributions?.items.find(
    (item) => item.pluginId === pluginId && item.digest === status.digest,
  );
  if (
    contribution?.errorCode ||
    (status.manifest.requires?.some((feature) =>
      contributionFeatures.includes(feature),
    ) &&
      !contribution)
  )
    return { reason: "unavailable" } as const;
  if (
    contribution?.dependencies.some(
      (dep) => !dep.available && dep.views.includes(viewId),
    )
  )
    return { reason: "dependency" } as const;
  return { reason: "ready", status, view } as const;
}

export function pluginNavigation(directory: PluginList | undefined) {
  return (directory?.contributions?.items ?? []).flatMap((item) =>
    (item.navigation ?? []).flatMap((nav) => {
      const route = resolvePluginView(directory, item.pluginId, nav.viewId);
      if (route.reason !== "ready" || route.status.digest !== item.digest)
        return [];
      return [
        {
          id: `extension:${item.pluginId}:${nav.viewId}`,
          pluginId: item.pluginId,
          viewId: nav.viewId,
          label: nav.title,
          icon: "presentation" as const,
          href: pluginViewHref(item.pluginId, nav.viewId),
        },
      ];
    }),
  );
}
