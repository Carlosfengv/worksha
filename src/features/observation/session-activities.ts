export const activityWindowSeconds = 3 * 60 * 60;

interface SessionActivity {
  id: string;
  sessionId: string | null;
  messageAt: string | null;
  requestAt: string | null;
  time: string;
}

const timestamp = (value: string | null) => {
  const parsed = Date.parse(value ?? "");
  return Number.isFinite(parsed) ? parsed : null;
};
const startedAt = (row: SessionActivity) =>
  timestamp(row.messageAt) ??
  timestamp(row.requestAt) ??
  timestamp(row.time) ??
  0;

/** Select a message, not its latest tool step. Unlinked requests stay separate. */
export function latestSessionActivities<T extends SessionActivity>(
  rows: T[],
): T[] {
  const latest = new Map<string, T>();
  for (const row of rows) {
    const key = row.sessionId ? `session:${row.sessionId}` : `row:${row.id}`;
    const previous = latest.get(key);
    if (
      !previous ||
      startedAt(row) > startedAt(previous) ||
      (startedAt(row) === startedAt(previous) &&
        row.id.localeCompare(previous.id) > 0)
    )
      latest.set(key, row);
  }
  return [...latest.values()];
}
