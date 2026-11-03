import type { ModelDocumentation, ProviderModelInfo } from "./types";

export const documentationCurrent = (
  source: ModelDocumentation,
  now = Date.now(),
) => Date.parse(source.checkedAt) <= now && now < Date.parse(source.expiresAt);

/** Expired supplements must not outlive their evidence in a long-open catalog. */
export function currentModelMetadata(
  model: ProviderModelInfo,
  now = Date.now(),
): ProviderModelInfo {
  const capabilities = model.capabilities
    ? { ...model.capabilities }
    : undefined;
  for (const source of model.documentation ?? []) {
    if (documentationCurrent(source, now)) continue;
    for (const field of source.fields) {
      if (field !== "contextWindow" && capabilities) capabilities[field] = null;
    }
  }
  return { ...model, capabilities };
}

export function modelMetadata(
  id: string,
  discovered: Map<string, ProviderModelInfo>,
  now = Date.now(),
): { model: ProviderModelInfo; contextNote?: string } {
  const exact = discovered.get(id);
  if (exact) return { model: currentModelMetadata(exact, now) };
  const base = /\[1m\]$/i.test(id)
    ? discovered.get(id.slice(0, -4))
    : undefined;
  const alias = base?.contextAlias;
  if (
    !alias ||
    !alias.documentation.length ||
    !alias.documentation.every((source) => documentationCurrent(source, now))
  )
    return { model: { id, name: id } };
  return {
    model: {
      id,
      name: id,
      contextWindow: alias.contextWindow,
      documentation: alias.documentation,
    },
    contextNote: `Claude Code 窗口；上游 ${alias.upstreamContextWindow.toLocaleString("zh-CN")} Token`,
  };
}
