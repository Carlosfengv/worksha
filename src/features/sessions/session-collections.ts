import type { NativeSession } from "./types";

export function groupSessionsByProject<
  T extends Pick<NativeSession, "projectHidden" | "projectPath">,
>(items: readonly T[]): Map<string, T[]> {
  const groups = new Map<string, T[]>();
  for (const item of items) {
    const path = item.projectHidden
      ? "已隐藏项目的会话"
      : (item.projectPath ?? "项目未知");
    const group = groups.get(path);
    if (group) group.push(item);
    else groups.set(path, [item]);
  }
  return groups;
}

/** Keep previous items and append each new identity once in source order. */
export function appendUniqueById<T extends { id: string }>(
  previous: readonly T[],
  incoming: readonly T[],
): T[] {
  const ids = new Set(previous.map((item) => item.id));
  const result = [...previous];
  for (const item of incoming) {
    if (ids.has(item.id)) continue;
    ids.add(item.id);
    result.push(item);
  }
  return result;
}
