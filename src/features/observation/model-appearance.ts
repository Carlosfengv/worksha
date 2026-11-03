import { chartCategoricalColors } from "@/components/ui/chart-primitives";
export { modelLogo } from "@/shared/lib/model-logo";

const palette = [0, 2, 4, 3, 1].map((index) => chartCategoricalColors[index]);
const storageKey = "local-agent:model-flow-colors:v1";

/** Keep assigned identities through restarts, filtering and temporary idle states. */
export function assignModelColors(
  previous: ReadonlyMap<string, string>,
  ids: readonly string[],
): ReadonlyMap<string, string> {
  const missing = [...new Set(ids)].filter((id) => !previous.has(id)).sort();
  if (!missing.length) return previous;
  const colors = new Map(previous);
  let index = [...previous.keys()].filter((id) => id !== "unknown").length;
  for (const id of missing) {
    if (id === "unknown") {
      colors.set(id, "var(--fg-subtle)");
      continue;
    }
    const base = palette[index % palette.length];
    const generation = Math.floor(index / palette.length);
    colors.set(
      id,
      generation === 0
        ? base
        : `color-mix(in srgb, ${base} ${100 / (generation + 1)}%, var(--fg-default))`,
    );
    index++;
  }
  return colors;
}

/** Store identities only; reconstruct semantic colors in their assignment order. */
export function loadModelColors(): ReadonlyMap<string, string> {
  try {
    const ids: unknown = JSON.parse(
      localStorage.getItem(storageKey) ??
        sessionStorage.getItem(storageKey) ??
        "null",
    );
    if (
      Array.isArray(ids) &&
      ids.length <= 1024 &&
      ids.every(
        (id) => typeof id === "string" && id.length > 0 && id.length < 512,
      )
    ) {
      return ids.reduce<ReadonlyMap<string, string>>(
        (colors, id) => assignModelColors(colors, [id]),
        new Map(),
      );
    }
  } catch {
    // A corrupt or unavailable store must not prevent displaying live activity.
  }
  return new Map();
}

export function saveModelColors(colors: ReadonlyMap<string, string>): void {
  try {
    localStorage.setItem(storageKey, JSON.stringify([...colors.keys()]));
    sessionStorage.removeItem(storageKey);
  } catch {
    // In-memory assignments still survive ordinary activity updates.
  }
}
