import type { PluginList } from "./types";
import type { IconName } from "@/lib/icon-context";
import type {
  NavigationGroup,
  PlatformContribution,
} from "../../../contracts/plugins/platform/types";

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

const navigationIcons = [
  "presentation",
  "folder",
  "inbox",
  "list",
  "list-checks",
  "file-archive",
  "file-text",
  "message-circle",
  "monitor",
  "globe",
  "rocket",
] as const satisfies readonly IconName[];

function entry(
  pluginId: string,
  viewId: string,
  label: string,
  group?: NavigationGroup | null,
  icon?: string | null,
) {
  return {
    id: `extension:${pluginId}:${viewId}`,
    pluginId,
    viewId,
    label,
    group: group ?? "observation",
    icon: navigationIcons.find((name) => name === icon) ?? "presentation",
    href: pluginViewHref(pluginId, viewId),
  };
}

export function pluginNavigation(
  directory: PluginList | undefined,
  platform: readonly PlatformContribution[] = [],
) {
  const legacy = (directory?.contributions?.items ?? []).flatMap((item) =>
    (item.navigation ?? []).flatMap((nav) => {
      const route = resolvePluginView(directory, item.pluginId, nav.viewId);
      if (route.reason !== "ready" || route.status.digest !== item.digest)
        return [];
      return [entry(item.pluginId, nav.viewId, nav.title, nav.group, nav.icon)];
    }),
  );
  const current = platform.flatMap((item) => {
    const definition = item.definition;
    if (!item.enabled || item.diagnostic || !definition) return [];
    return (definition.slots ?? []).flatMap((slot) => {
      const view = definition.views?.[slot.viewId];
      return slot.point === "nav.home-sidebar" && view
        ? [entry(item.pluginId, slot.viewId, view.title, slot.group, slot.icon)]
        : [];
    });
  });
  const entries = new Map(
    [...legacy, ...current].map((item) => [item.id, item]),
  );
  return [...entries.values()];
}
