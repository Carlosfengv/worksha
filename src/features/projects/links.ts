export function projectLink(
  id: string,
  tab = "overview",
  anchor?: string,
  agent?: string,
  scope?: string,
) {
  const params = new URLSearchParams({ tab });
  if (anchor) params.set("anchor", anchor);
  if (agent && agent !== "all") params.set("agent", agent);
  if (scope && scope !== "workspace") params.set("scope", scope);
  return `/projects/${encodeURIComponent(id)}?${params}`;
}
