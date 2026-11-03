import type { Snapshot } from "@/app/types";
import type { Provider } from "@/features/providers/types";
import type { HiddenProject } from "@/features/projects/types";
import { providerValue } from "@/features/providers/status-response";
declare global {
  interface Window {
    savedProviders: Provider[];
    scopeQueries: { feature: string; query: Record<string, unknown> }[];
    projectVisibilityChanges: { id: string; hidden: boolean }[];
    failProjectVisibility: boolean;
    failProviderUsage: boolean;
  }
}

export interface TableFixtureContext {
  mode: string | null;
  sourceState: string | null;
  params: URLSearchParams;
  hiddenProjects: HiddenProject[];
}
export function createTableFixtureContext(): TableFixtureContext {
  window.savedProviders = [];
  window.scopeQueries = [];
  window.projectVisibilityChanges = [];
  window.failProviderUsage = false;
  window.failProjectVisibility = false;
  const params = new URLSearchParams(location.search);
  return {
    params,
    mode: params.get("mode"),
    sourceState: params.get("sourceState"),
    hiddenProjects: [],
  };
}
export function checkedTableSnapshot(snapshot: Snapshot): Snapshot {
  if (
    !snapshot ||
    !Array.isArray(snapshot.agents) ||
    !Array.isArray(snapshot.providers) ||
    !Array.isArray(snapshot.sessionSources) ||
    !Array.isArray(snapshot.observationBindings) ||
    !snapshot.settings ||
    typeof snapshot.settings.captureContent !== "boolean" ||
    ![7, 30, 90].includes(snapshot.settings.retentionDays)
  )
    throw Error("Invalid table fixture snapshot");
  const unique = (ids: string[]) =>
    ids.every((id) => typeof id === "string" && !!id.trim()) &&
    new Set(ids).size === ids.length;
  if (
    !unique(snapshot.agents.map((a) => a.id)) ||
    !unique(snapshot.providers.map((p) => p.id))
  )
    throw Error("Duplicate or missing table fixture identity");
  for (const product of snapshot.agents)
    if (
      !product.family ||
      !product.configId ||
      !product.name ||
      typeof product.connected !== "boolean" ||
      typeof product.found !== "boolean"
    )
      throw Error("Invalid table fixture Agent");
  snapshot.providers.forEach(providerValue);
  return snapshot;
}
