import { agentProducts } from "@/shared/types/agent";

/** Summaries stay compact; callers retain exact values in details or titles. */
export function compactNumber(value: number | null | undefined) {
  if (value == null || !Number.isFinite(value)) return "未知";
  return value.toLocaleString("zh-CN", {
    notation: Math.abs(value) >= 10_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  });
}

export function chartColor(id: string) {
  // Stable categories use slots 1–5; neutral and danger remain status colors.
  const agent = agentProducts.findIndex((product) => product.id === id);
  if (agent >= 0) return `var(--chart-${(agent % 5) + 1})`;
  let hash = 2166136261;
  for (const character of id)
    hash = Math.imul(hash ^ character.charCodeAt(0), 16777619) >>> 0;
  return `var(--chart-${(hash % 5) + 1})`;
}

export function chartDash(index: number) {
  return index === 0
    ? undefined
    : ["6,4", "2,3", "10,3,2,3", "3,3,1,3", "10,4"][(index - 1) % 5];
}
