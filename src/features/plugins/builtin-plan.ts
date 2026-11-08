import type {
  CapabilityAddress,
  Composition,
  PluginInfo,
  Profile,
} from "../../../contracts/plugins/builtin/types";
const address = (key: CapabilityAddress) =>
  `${key.name}@${key.version}:${key.instance}`;
export function profileAllows(plugin: PluginInfo, profile: Profile) {
  return (
    profile !== "recovery" &&
    (profile === "desktop" ||
      !plugin.capabilities.some(
        (capability) =>
          capability.address.name === "collector" ||
          capability.metadata.kind === "ui-contribution",
      ))
  );
}
export function compositionSelection(
  plugins: PluginInfo[],
  composition: Composition,
) {
  const disabled = new Set(composition.disabledPluginIds);
  const optedIn = new Set(composition.optedInPluginIds ?? []);
  const removed = new Set(composition.removedPluginIds ?? []);
  const selected = new Set(
    plugins
      .filter(
        (plugin) =>
          profileAllows(plugin, composition.profile) &&
          (!plugin.optIn || optedIn.has(plugin.id)) &&
          !disabled.has(plugin.id) &&
          !removed.has(plugin.id),
      )
      .map((plugin) => plugin.id),
  );
  const providers = new Map(
    plugins.flatMap((p) =>
      p.capabilities.map((c) => [address(c.address), p.id] as const),
    ),
  );
  let changed = true;
  while (changed) {
    changed = false;
    for (const plugin of plugins)
      if (
        selected.has(plugin.id) &&
        plugin.requires.some(
          (key) => !selected.has(providers.get(address(key)) ?? ""),
        )
      ) {
        selected.delete(plugin.id);
        changed = true;
      }
  }
  return selected;
}
export function editableBuiltinSelection(
  plugins: PluginInfo[],
  composition: Composition,
): Composition {
  const selected = compositionSelection(plugins, composition);
  return {
    ...composition,
    profile: "desktop",
    disabledPluginIds: plugins
      .filter((plugin) => {
        const owner = plugin.requires.find(
          (key) => key.name === "business.feature",
        )?.instance;
        return !plugin.id.startsWith("builtin.") &&
          owner &&
          !selected.has(owner)
          ? composition.disabledPluginIds.includes(plugin.id)
          : !selected.has(plugin.id);
      })
      .map((plugin) => plugin.id)
      .sort(),
  };
}
/** Draft-only dependency expansion; the service validates the complete graph again. */
export function toggleBuiltin(
  plugins: PluginInfo[],
  composition: Composition,
  id: string,
  enabled: boolean,
): Composition {
  const providers = new Map(
    plugins.flatMap((plugin) =>
      plugin.capabilities.map(
        (capability) => [address(capability.address), plugin.id] as const,
      ),
    ),
  );
  const selected = compositionSelection(plugins, composition);
  const removed = new Set(composition.removedPluginIds ?? []);
  const optedIn = new Set(composition.optedInPluginIds ?? []);
  if (enabled) selected.add(id);
  else selected.delete(id);
  let changed = true;
  while (changed) {
    changed = false;
    for (const plugin of plugins) {
      if (!selected.has(plugin.id)) continue;
      for (const dependency of plugin.requires) {
        const provider = providers.get(address(dependency));
        if (!provider) throw Error("必需依赖未提供，请重新读取插件目录。");
        if (!selected.has(provider)) {
          if (enabled) {
            const owner = plugins.find((plugin) => plugin.id === provider);
            if (!owner || !profileAllows(owner, composition.profile))
              throw Error("当前功能未提供此必需依赖，请先恢复相关功能。");
            selected.add(provider);
            removed.delete(provider);
          } else selected.delete(plugin.id);
          changed = true;
        }
      }
    }
  }
  if (enabled)
    for (const plugin of plugins)
      if (plugin.optIn && selected.has(plugin.id)) optedIn.add(plugin.id);
  return {
    ...composition,
    ...(optedIn.size ? { optedInPluginIds: [...optedIn].sort() } : {}),
    ...(enabled && removed.size !== (composition.removedPluginIds?.length ?? 0)
      ? { removedPluginIds: [...removed].sort() }
      : {}),
    disabledPluginIds: plugins
      .filter((plugin) =>
        id.startsWith("builtin.") && !plugin.id.startsWith("builtin.")
          ? composition.disabledPluginIds.includes(plugin.id)
          : profileAllows(plugin, composition.profile)
            ? !selected.has(plugin.id)
            : composition.disabledPluginIds.includes(plugin.id),
      )
      .map((plugin) => plugin.id)
      .sort(),
  };
}
export function removeBuiltin(
  plugins: PluginInfo[],
  composition: Composition,
  id: string,
): Composition {
  const disabled = toggleBuiltin(plugins, composition, id, false);
  return {
    ...disabled,
    removedPluginIds: [
      ...new Set([...(composition.removedPluginIds ?? []), id]),
    ].sort(),
  };
}
export function installBuiltin(
  plugins: PluginInfo[],
  composition: Composition,
  id: string,
): Composition {
  return toggleBuiltin(
    plugins,
    {
      ...composition,
      removedPluginIds: (composition.removedPluginIds ?? []).filter(
        (x) => x !== id,
      ),
    },
    id,
    true,
  );
}
