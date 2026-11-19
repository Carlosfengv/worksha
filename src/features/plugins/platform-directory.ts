import type { ComponentType } from "react";
import { createContext } from "react";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { createReadResource } from "@/shared/lib/read-resource";
import type {
  PlatformContext,
  PlatformContribution,
  SlotPoint,
} from "../../../contracts/plugins/platform/types";
import { platformPluginsApi } from "./api";
import type { PluginList } from "./types";

export type BuiltinSlotContribution = {
  id: string;
  point: SlotPoint;
  component: ComponentType<{ context?: PlatformContext }>;
};

export const ExtensionSlotContext = createContext<{
  builtin: readonly BuiltinSlotContribution[];
  platform: readonly PlatformContribution[];
}>({ builtin: [], platform: [] });

const platformDirectory = createReadResource(() => platformPluginsApi.list(), {
  interval: false,
});

export const usePlatformDirectory = (directory?: PluginList | null) =>
  useSharedRead(
    platformDirectory,
    directory?.items.some((item) =>
      item.manifest.capabilities.includes("ui.platform@1"),
    ) ?? false,
  );

export function platformEntry(
  items: readonly PlatformContribution[],
  pluginId: string,
  viewId: string,
) {
  return items.find(
    (item) =>
      item.pluginId === pluginId &&
      item.enabled &&
      !item.diagnostic &&
      item.definition &&
      Object.hasOwn(item.definition.views ?? {}, viewId),
  );
}
